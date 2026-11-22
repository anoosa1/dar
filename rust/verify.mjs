// Runs Dar's acceptance suite (test/) against the stack: SpacetimeDB with the Rust module, the
// Rust runtime and the Rust client, on a throwaway root and ports. Isolated instances
// (test/instance.ts) start the same runtime. Every suite runs even after a failure, and a
// summary closes the run. Unit tests live with the crates (cargo test).
//
// Usage (from Dar's root dev shell): node rust/verify.mjs [--only <substring>...] [--skip-build]
// The database and runtime log to database.log and runtime.log in the run's root.
import {spawn, spawnSync} from "node:child_process";
import {mkdtemp, mkdir, readFile, writeFile, chmod} from "node:fs/promises";
import {existsSync, openSync, readFileSync, closeSync, mkdtempSync} from "node:fs";
import {tmpdir} from "node:os";
import net from "node:net";
import path from "node:path";
import {randomBytes} from "node:crypto";

const rust = path.resolve(import.meta.dirname), dar = path.dirname(rust);
const args = process.argv.slice(2), only = args.flatMap((value, index) => args[index - 1] === "--only" ? [value] : []);
const nixShell = (command) => spawnSync("nix", ["develop", `path:${rust}/nix`, "-c", "bash", "-c", command], {stdio: "inherit"});
if (!args.includes("--skip-build")) {
  // The installed-app SDK bundle goes into the client's dist.
  if (spawnSync(process.execPath, ["sdk/build-bundle.mjs"], {cwd: dar, stdio: "inherit"}).status !== 0) throw new Error("The app SDK bundle did not build.");
  const built = nixShell(`set -e
    cd '${rust}/module' && ./patch-macro.sh && cargo build -q --release --target wasm32-unknown-unknown
    cd '${rust}/client' && ./generate.sh >/dev/null && ./build.sh --release >/dev/null
    cd '${rust}' && cargo build -q --release -p dar-runtime`);
  if (built.status !== 0) throw new Error("The Rust stack did not build.");
}
const module = path.join(rust, "module/target/wasm32-unknown-unknown/release/dar_module.wasm"), runtimeBin = path.join(rust, "target/release/dar-runtime"), client = path.join(rust, "client/dist");
// The tests' SpacetimeDB TypeScript bindings, generated from the module they run against.
const bindings = spawnSync(process.env.DAR_SPACETIME_BIN ?? "spacetime", ["--root-dir", mkdtempSync(path.join(tmpdir(), "dar-generate-")), "generate", "--lang", "typescript", "--bin-path", module, "--out-dir", path.join(dar, "test/module_bindings"), "--import-extension", "ts", "--yes", "--no-config"], {encoding: "utf8"});
if (bindings.status !== 0) throw new Error(`The test bindings were not generated.\n${bindings.stdout}${bindings.stderr}`);

const root = await mkdtemp(path.join(tmpdir(), "dar-rust-acceptance-"));
const appBuild = spawnSync(process.execPath, ["sdk/build.mjs", "apps/prayer-times"], {cwd: dar, stdio: "inherit"});
if (appBuild.status !== 0) throw new Error("Example app build failed.");
const installed = spawnSync(runtimeBin, ["apps-install", root, path.join(dar, "apps/prayer-times/dist")], {stdio: "inherit"});
if (installed.status !== 0) throw new Error("The example app was not installed.");
await writeFile(path.join(root, "oidc-secret"), "isolated-configured-secret-1234567890\n", {mode: 0o600});
await writeFile(path.join(root, "oidc.json"), JSON.stringify({clients: [{id: "configured-rp", name: "Configured RP", type: "confidential", redirectUris: ["http://127.0.0.1:49999/callback"], scopes: ["openid", "profile"], secretFile: "oidc-secret"}]}));
async function freePort() {
  const server = net.createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const {port} = server.address(); await new Promise(resolve => server.close(resolve)); return port;
}
const port = await freePort(); let databasePort;
do { databasePort = await freePort(); } while (port === databasePort);
const chromium = process.env.DAR_CHROMIUM ?? process.env.PATH.split(path.delimiter).map(directory => path.join(directory, "chromium")).find(existsSync);
if (!chromium) throw new Error("Chromium is required. Use Dar's root Nix shell or set DAR_CHROMIUM.");
const chatControl = path.join(root, "chat-control"), chatBridge = path.join(chatControl, "bridge.sh");
await mkdir(chatControl, {recursive: true, mode: 0o700});
await writeFile(chatBridge, `#!/bin/sh\nDAR_FAKE_CHAT_CONTROL=${JSON.stringify(chatControl)} exec node ${JSON.stringify(path.join(dar, "test/fixtures/fake-chat-bridge.mjs"))}\n`); await chmod(chatBridge, 0o700);
const origin = `http://127.0.0.1:${port}`, databaseUrl = `http://127.0.0.1:${databasePort}`, cliRoot = path.join(root, "cli");
const environment = {...process.env, DAR_ROOT: root, DAR_CHAT_BRIDGE: chatBridge, DAR_CHAT_CONTROL: chatControl, DAR_TEST_ROOT: root, DAR_PORT: String(port), DAR_DATABASE_PORT: String(databasePort),
  DAR_INTEGRATION_ORIGIN: origin, DAR_ORIGIN: origin, DAR_DATABASE_URL: databaseUrl, DAR_DATABASE_NAME: "dar-acceptance", DAR_ISSUER: `${origin}/auth/issuer`, DAR_DEVELOPMENT: "1",
  DAR_SPACETIME_BIN: process.env.DAR_SPACETIME_BIN ?? "spacetime", DAR_SPACETIME_CLI_ROOT: cliRoot,
  DAR_DATABASE_TOKEN_FILE: path.join(root, "secrets/database-token"), DAR_BOOTSTRAP_TOKEN_FILE: path.join(root, "secrets/bootstrap-token"), DAR_CHROMIUM: chromium,
  DAR_RUST_MODULE: module, DAR_RUST_RUNTIME: runtimeBin, DAR_RUST_CLIENT: client, DAR_CLIENT_DIST: client};
await mkdir(path.join(root, "secrets"), {recursive: true, mode: 0o700}); await mkdir(path.join(root, "database"), {recursive: true, mode: 0o700});
await writeFile(environment.DAR_BOOTSTRAP_TOKEN_FILE, randomBytes(32).toString("base64url"), {mode: 0o600});

const children = [];
// Services write to log files, never to pipes: suites run with spawnSync, which stops this
// process from reading, and a service blocked on a full pipe stalls every later suite.
function start(executable, argv, name) {
  const file = path.join(root, `${name}.log`), fd = openSync(file, "a", 0o600);
  const child = spawn(executable, argv, {cwd: dar, env: environment, stdio: ["ignore", fd, fd]});
  closeSync(fd);
  children.push(child); child.output = () => { try { return readFileSync(file, "utf8").slice(-16_000); } catch { return ""; } }; return child;
}
async function waitFor(url, child) {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`A service stopped during startup.\n${child.output()}`);
    try { if ((await fetch(url, {signal: AbortSignal.timeout(500)})).ok) return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for ${url}.\n${child.output()}`);
}
const results = [];
function run(argv, timeout = 300_000) {
  if (only.length && !only.some(pattern => argv.join(" ").includes(pattern))) return;
  const started = Date.now();
  const result = spawnSync(process.execPath, argv, {cwd: dar, env: environment, stdio: "inherit", timeout});
  const ok = !result.error && result.status === 0;
  results.push({name: argv.filter(value => !value.startsWith("--")).join(" "), ok, seconds: Math.round((Date.now() - started) / 1000), reason: result.error?.message ?? (result.signal ? `signal ${result.signal}` : ok ? "" : `exit ${result.status}`)});
}
try {
  const database = start(environment.DAR_SPACETIME_BIN, ["--root-dir", cliRoot, "start", "--data-dir", path.join(root, "database"), "--listen-addr", `127.0.0.1:${databasePort}`, "--non-interactive"], "database");
  await waitFor(`${databaseUrl}/v1/ping`, database);
  const published = spawnSync(environment.DAR_SPACETIME_BIN, ["--root-dir", cliRoot, "publish", environment.DAR_DATABASE_NAME, "--bin-path", module, "--server", databaseUrl, "--yes", "--no-config"], {env: environment, encoding: "utf8"});
  if (published.status !== 0) throw new Error(`The Rust module was not published.\n${published.stdout}${published.stderr}`);
  const token = (await readFile(path.join(cliRoot, "config/cli.toml"), "utf8")).match(/spacetimedb_token\s*=\s*"([^"]+)"/)?.[1];
  if (!token) throw new Error("The local database service identity was not initialized.");
  await writeFile(environment.DAR_DATABASE_TOKEN_FILE, token, {mode: 0o600});
  const runtime = start(runtimeBin, [], "runtime");
  await writeFile(path.join(root, "service-pids.json"), JSON.stringify({database: database.pid, runtime: runtime.pid}), {mode: 0o600});
  await waitFor(`${origin}/api/v1/auth/status`, runtime);

  // Setup races first; the login-failure budget and restart last.
  const t = (...files) => ["--test", "--test-concurrency=1", ...files.map(file => `test/${file}`)];
  run(t("auth.integration.test.ts"));
  // The chat bridge's own tests (fake Discord servers; never a real account).
  if (!only.length || only.some(pattern => "bridge go test".includes(pattern))) {
    const started = Date.now(), go = spawnSync("go", ["test", "./..."], {cwd: path.join(dar, "bridge"), env: {...process.env, CGO_ENABLED: "0", GOTOOLCHAIN: "local", GOFLAGS: "-mod=readonly"}, stdio: "inherit", timeout: 600_000});
    results.push({name: "bridge go test", ok: go.status === 0, seconds: Math.round((Date.now() - started) / 1000), reason: go.error?.message ?? (go.status === 0 ? "" : `exit ${go.status}`)});
  }
  run(t("news.integration.test.ts"));
  run(t("chat.integration.test.ts", "chat-discord.integration.test.ts")); run(["test/chat-browser.mjs"]);
  run(t("books.integration.test.ts")); run(["test/books-browser.mjs"]); run(["test/git-browser.mjs"]); run(["test/git-tracker-browser.mjs"]);
  run(t("photos.integration.test.ts", "photos-library.integration.test.ts", "photos-sharing.integration.test.ts", "photos-immich.integration.test.ts"), 600_000);
  run(t("agent.integration.test.ts", "codex.integration.test.ts", "claude.integration.test.ts")); run(["test/agent-browser.mjs"]);
  run(t("apps.integration.test.ts")); run(["test/prayer-times-browser.mjs"]);
  run(t("appearance-scopes.integration.test.ts")); run(["test/appearance-scopes-browser.mjs"]);
  run(["test/production-proxy.mjs"]);
  run(t("calendar-subscriptions.integration.test.ts", "davx5.integration.test.ts", "pim.integration.test.ts")); run(t("pim-upgrade.integration.test.ts"));
  run(t("global-search.integration.test.ts"));
  run(["test/refinement-browser.mjs"]); run(["test/themed-controls-browser.mjs"]); run(["test/app-launcher-browser.mjs"]);
  run(t("oidc-config.integration.test.ts", "identity.integration.test.ts", "identity-lifecycle.integration.test.ts")); run(t("identity-upgrade.integration.test.ts")); run(t("identity-proxy.integration.test.ts"));
  run(["test/identity-browser.mjs"]);
  run(t("ui-settings.integration.test.ts", "workspace.integration.test.ts", "workspace-upgrade.integration.test.ts"));
  run(["test/workspace-browser.mjs"]); run(["test/shared-ui-browser.mjs"]);
  run(t("auth-security.integration.test.ts", "files.integration.test.ts", "history.integration.test.ts", "git.integration.test.ts", "git-app.integration.test.ts", "git-interop.integration.test.ts", "git-tracker.integration.test.ts",
    "dav.integration.test.ts", "backup.integration.test.ts", "gc.integration.test.ts", "blob-migration.integration.test.ts", "pagination.integration.test.ts"));
  run(t("upload-timeout.integration.test.ts"), 240_000);
  run(["test/browser.mjs"]); run(["test/history-browser.mjs"]);
  run(["test/pim-browser.mjs"]); run(["test/contacts-layout-browser.mjs"]); run(["test/twilight-browser.mjs"]); run(["test/pim-dav-client.mjs"]);
  run(t("http-security.integration.test.ts"));
  run(["test/restart.mjs"]);
} finally {
  for (const child of children.reverse()) if (child.exitCode === null) await new Promise(resolve => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
  });
}
const failed = results.filter(result => !result.ok);
console.info("\nRust stack acceptance:");
for (const result of results) console.info(`${result.ok ? "PASS" : "FAIL"} ${String(result.seconds).padStart(4)}s ${result.name}${result.reason ? ` (${result.reason})` : ""}`);
console.info(`${results.length - failed.length}/${results.length} suites passed. Isolated artifacts: ${root}`);
process.exitCode = failed.length ? 1 : 0;
