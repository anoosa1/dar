import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import net from "node:net";
import path from "node:path";

const project = path.resolve(import.meta.dirname, "..");
const identityOnly=process.argv.includes("--identity");
const workspaceOnly=process.argv.includes("--workspace");
const pimOnly=process.argv.includes("--pim");
const root = await mkdtemp(path.join(tmpdir(), "dar-acceptance-"));
async function freePort() {
  const server = net.createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address(); await new Promise(resolve => server.close(resolve)); return port;
}
const port = await freePort(); let databasePort;
do { databasePort = await freePort(); } while (port === databasePort);
const chromium = process.env.DAR_CHROMIUM ?? process.env.PATH.split(path.delimiter).map(directory => path.join(directory, "chromium")).find(existsSync);
if (!chromium) throw new Error("Chromium is required. Use the next/ Nix shell or set DAR_CHROMIUM.");
const environment = { ...process.env, DAR_ROOT: root, DAR_TEST_ROOT: root, DAR_PORT: String(port), DAR_DATABASE_PORT: String(databasePort),
  DAR_INTEGRATION_ORIGIN: `http://127.0.0.1:${port}`, DAR_DATABASE_URL: `http://127.0.0.1:${databasePort}`, DAR_DATABASE_NAME: "dar-acceptance",
  DAR_SPACETIME_BIN: process.env.DAR_SPACETIME_BIN ?? "spacetime", DAR_SPACETIME_CLI_ROOT: path.join(root, "cli"),
  DAR_DATABASE_TOKEN_FILE: path.join(root, "secrets/database-token"), DAR_BOOTSTRAP_TOKEN_FILE: path.join(root, "secrets/bootstrap-token"), DAR_CHROMIUM: chromium };
const child = spawn(process.execPath, ["scripts/dev.mjs"], { cwd: project, env: environment, stdio: ["ignore", "pipe", "pipe"] });
let output = "";
for (const stream of [child.stdout, child.stderr]) stream.on("data", chunk => { output = (output + chunk).slice(-16_000); });
function run(args) {
  const result = spawnSync(process.execPath, args, { cwd: project, env: environment, stdio: "inherit", timeout: 120_000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Verification failed: ${args.join(" ")}`);
}
try {
  const deadline = Date.now() + 30_000; let ready = false;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Fixture stopped.\n${output}`);
    try { ready = (await fetch(`${environment.DAR_INTEGRATION_ORIGIN}/api/v1/auth/status`, { signal: AbortSignal.timeout(500) })).ok; } catch {}
    if (ready) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!ready) throw new Error(`Fixture readiness timed out.\n${output}`);
  run(["node_modules/typescript/bin/tsc", "-p", "tsconfig.json"]);
  run(["node_modules/typescript/bin/tsc", "-p", "module/tsconfig.json"]);
  // Setup race verification must run before the other tests create users.
  run(["--test", "test/auth.integration.test.ts"]);
  if(!identityOnly&&!workspaceOnly&&!pimOnly){
    run(["--test","test/appearance-scopes.integration.test.ts"]);
    run(["test/appearance-scopes-browser.mjs"]);
  }
  if(!identityOnly&&!workspaceOnly&&!pimOnly)run(["test/production-proxy.mjs"]);
  if(!identityOnly&&!workspaceOnly){
    run(["--test", "--test-concurrency=1", "test/calendar-navigation.test.ts", "test/davx5.integration.test.ts", "test/pim-standards.test.ts", "test/pim.integration.test.ts"]);
    run(["--test", "test/pim-upgrade.integration.test.ts"]);
  }
  if(!workspaceOnly&&!pimOnly){
  run(["--test", "--test-concurrency=1", "test/scim-model.test.ts", "test/identity.integration.test.ts", "test/identity-lifecycle.integration.test.ts"]);
  run(["--test", "test/identity-upgrade.integration.test.ts"]);
  run(["--test", "test/identity-proxy.integration.test.ts"]);
  run(["test/identity-browser.mjs"]);
  }
  if(!identityOnly&&!pimOnly)run(["--test","--test-concurrency=1","test/ui-settings.integration.test.ts","test/workspace-standards.test.ts","test/workspace.integration.test.ts","test/workspace-upgrade.integration.test.ts"]);
  if(!identityOnly&&!pimOnly)run(["test/workspace-browser.mjs"]);
  if(!identityOnly&&!pimOnly)run(["test/shared-ui-browser.mjs"]);
  if(!identityOnly&&!workspaceOnly){
  if(!pimOnly){run(["--test", "--test-concurrency=1", "test/auth-primitives.test.ts", "test/auth-security.integration.test.ts", "test/files.integration.test.ts", "test/dav.integration.test.ts", "test/backup.integration.test.ts", "test/gc.integration.test.ts", "test/keyring.test.ts", "test/pagination.integration.test.ts", "test/storage.test.ts"]);
  run(["test/browser.mjs"]);}
  run(["test/pim-browser.mjs"]);
  run(["test/pim-dav-client.mjs"]);
  // This deliberately consumes the fixture's login failure budget.
  run(["--test", "test/http-security.integration.test.ts"]);
  run(["test/restart.mjs"]);
  }
  const pids = JSON.parse(await readFile(path.join(root, "service-pids.json"), "utf8"));
  let rss = 0;
  async function memory(pid) {
    const status = await readFile(`/proc/${pid}/status`, "utf8");
    rss += Number(status.match(/^VmRSS:\s+(\d+)/m)?.[1] ?? 0);
    for (const id of (await readFile(`/proc/${pid}/task/${pid}/children`, "utf8")).trim().split(/\s+/).filter(Boolean)) await memory(Number(id));
  }
  await memory(pids.database);
  console.info(`PASS all acceptance checks. Database process-tree RSS for this fixture: ${Math.round(rss / 1024)} MiB; this is not an archive-capacity guarantee.`);
  console.info(`Isolated artifacts: ${root}`);
} finally {
  if (child.exitCode === null && child.signalCode === null) await new Promise(resolve => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
  });
}
