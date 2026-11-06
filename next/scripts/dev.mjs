import { spawn, spawnSync } from "node:child_process";
import { openSync, closeSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import path from "node:path";
import { homedir } from "node:os";
import { openRoot } from "../runtime/root.ts";
import { createDurableFile } from "../runtime/atomic.ts";
import { database as connectDatabase } from "../runtime/database.ts";
import { restore } from "../runtime/backup.ts";

const [major, minor, patch] = process.versions.node.split(".").map(Number);
if (major !== 24 || minor < 22 || minor === 22 && patch < 1) throw new Error("Use Node 24.22.1 or later in the Node 24 series.");
process.umask(0o077);
const project = path.resolve(import.meta.dirname, "..");
const production = process.argv.includes("--production");
const args = process.argv.slice(2).filter(value => value !== "--production");
const restorePath = args[0] === "--restore" ? args[1] : undefined;
if (args.length && (!restorePath || args.length !== 2)) throw new Error("Usage: dev.mjs [--production] [--restore <backup-directory>]");
const root = await openRoot(process.env.DAR_ROOT ?? path.join(process.env.XDG_DATA_HOME ?? path.join(homedir(), ".local/share"), "dar-spacetime"));
const port = Number(process.env.DAR_PORT ?? 4311), databasePort = Number(process.env.DAR_DATABASE_PORT ?? 4310);
const localOrigin = `http://127.0.0.1:${port}`, databaseUrl = `http://127.0.0.1:${databasePort}`;
const origin = production ? process.env.DAR_ORIGIN : localOrigin;
if (!origin) throw new Error("DAR_ORIGIN is required in production.");
const publicOrigin = new URL(origin);
if (production && (publicOrigin.protocol !== "https:" || publicOrigin.origin !== origin)) throw new Error("Production DAR_ORIGIN must be an HTTPS origin without a trailing slash or path.");
const cliRoot = path.join(root, "cli"), cli = process.env.DAR_SPACETIME_BIN ?? "spacetime";
const bootstrapFile = path.join(root, "secrets/bootstrap-token"), serviceFile = path.join(root, "secrets/database-token");
try { await readFile(bootstrapFile); } catch (error) { if (error.code !== "ENOENT") throw error; await createDurableFile(bootstrapFile, randomBytes(32).toString("base64url")); }
await mkdir(path.join(root, "database"), { recursive: true, mode: 0o700 });
const environment = { ...process.env, DAR_ROOT: root, DAR_ORIGIN: origin, DAR_PORT: String(port), DAR_DATABASE_URL: databaseUrl,
  DAR_DATABASE_NAME: process.env.DAR_DATABASE_NAME ?? "dar", DAR_DATABASE_TOKEN_FILE: serviceFile, DAR_BOOTSTRAP_TOKEN_FILE: bootstrapFile,
  DAR_DEVELOPMENT: production ? "0" : "1", DAR_SPACETIME_BIN: cli, DAR_SPACETIME_CLI_ROOT: cliRoot, DAR_ISSUER: `${origin}/auth/issuer` };
const children = new Set();
let stopping = false;
function start(executable, args, logFile) {
  const fd = logFile ? openSync(logFile, "a", 0o600) : undefined;
  const child = spawn(executable, args, { cwd: project, env: environment, stdio: fd === undefined ? "inherit" : ["ignore", fd, fd] });
  if (fd !== undefined) closeSync(fd);
  children.add(child); child.once("exit", () => children.delete(child));
  child.once("error", error => { console.error(error.message); process.exitCode = 1; });
  return child;
}
function run(executable, args) {
  const result = spawnSync(executable, args, { cwd: project, env: environment, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Command failed: ${path.basename(executable)}`);
}
async function waitFor(url, child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) throw new Error("A required service stopped during startup.");
    try { if ((await fetch(url, { signal: AbortSignal.timeout(500) })).ok) return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error("Timed out waiting for service readiness.");
}
async function stop() {
  stopping = true;
  for (const child of [...children].reverse()) await new Promise(resolve => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
    child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
  });
}
try {
  const database = start(cli, ["--root-dir", cliRoot, "start", "--data-dir", path.join(root, "database"), "--listen-addr", `127.0.0.1:${databasePort}`, "--non-interactive"], path.join(root, "database.log"));
  await waitFor(`${databaseUrl}/v1/ping`, database);
  run(process.execPath, ["scripts/build-module.mjs"]);
  run(cli, ["--root-dir", cliRoot, "publish", environment.DAR_DATABASE_NAME, "--js-path", "module/dist/bundle.js", "--server", databaseUrl, "--yes", "--no-config"]);
  const config = await readFile(path.join(cliRoot, "config/cli.toml"), "utf8");
  const token = config.match(/spacetimedb_token\s*=\s*"([^"]+)"/)?.[1];
  if (!token) throw new Error("The local database service identity was not initialized.");
  try { if ((await readFile(serviceFile, "utf8")) !== token) throw new Error("Stored database identity changed; refusing to overwrite it."); }
  catch (error) { if (error.code !== "ENOENT") throw error; await createDurableFile(serviceFile, token); }
  if (restorePath) await restore(await connectDatabase(databaseUrl, environment.DAR_DATABASE_NAME, serviceFile), path.resolve(restorePath), root);
  run(process.execPath, ["scripts/build-client.mjs"]);
  const runtime = start(process.execPath, ["runtime/main.ts"]);
  await writeFile(path.join(root, "service-pids.json"), JSON.stringify({ database: database.pid, runtime: runtime.pid }), { mode: 0o600 });
  await waitFor(`${localOrigin}/api/v1/auth/status`, runtime);
  console.info(`Dar is ready at ${origin}. The operator setup key is in ${bootstrapFile}.`);
  await new Promise(resolve => {
    process.once("SIGINT", resolve); process.once("SIGTERM", resolve);
    for (const child of children) child.once("exit", () => { if (!stopping) process.exitCode = 1; resolve(); });
  });
} finally { await stop(); }
