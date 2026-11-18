import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, chmod } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import net from "node:net";
import path from "node:path";
import {manageApps} from "../runtime/apps/install.ts";

const project = path.resolve(import.meta.dirname, "..");
const identityOnly=process.argv.includes("--identity");
const workspaceOnly=process.argv.includes("--workspace");
const pimOnly=process.argv.includes("--pim");
const agentOnly=process.argv.includes("--agent");
const newsOnly=process.argv.includes("--news");
const photosOnly=process.argv.includes("--photos");
const historyOnly=process.argv.includes("--history");
const booksOnly=process.argv.includes("--books");
const chatOnly=process.argv.includes("--chat");
const gitOnly=process.argv.includes("--git");
const storageOnly=process.argv.includes("--storage");
const root = await mkdtemp(path.join(tmpdir(), "dar-acceptance-"));
const appBuild=spawnSync(process.execPath,["sdk/build.mjs","../apps/prayer-times"],{cwd:project,stdio:"inherit"});
if(appBuild.status!==0)throw new Error("Example app build failed.");
await manageApps(root,"apps-install",path.resolve(project,"../apps/prayer-times/dist"));
await writeFile(path.join(root, "oidc-secret"), "isolated-configured-secret-1234567890\n", { mode: 0o600 });
await writeFile(path.join(root, "oidc.json"), JSON.stringify({ clients: [{ id: "configured-rp", name: "Configured RP", type: "confidential", redirectUris: ["http://127.0.0.1:49999/callback"], scopes: ["openid", "profile"], secretFile: "oidc-secret" }] }));
async function freePort() {
  const server = net.createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address(); await new Promise(resolve => server.close(resolve)); return port;
}
const port = await freePort(); let databasePort;
do { databasePort = await freePort(); } while (port === databasePort);
const chromium = process.env.DAR_CHROMIUM ?? process.env.PATH.split(path.delimiter).map(directory => path.join(directory, "chromium")).find(existsSync);
if (!chromium) throw new Error("Chromium is required. Use the next/ Nix shell or set DAR_CHROMIUM.");
// Chat runs against a fake bridge (test/fixtures/fake-chat-bridge.mjs) that the browser test drives through control files.
const chatControl = path.join(root, "chat-control"), chatBridge = path.join(chatControl, "bridge.sh");
await mkdir(chatControl, { recursive: true, mode: 0o700 });
await writeFile(chatBridge, `#!/bin/sh\nDAR_FAKE_CHAT_CONTROL=${JSON.stringify(chatControl)} exec node ${JSON.stringify(path.join(project, "test/fixtures/fake-chat-bridge.mjs"))}\n`); await chmod(chatBridge, 0o700);
const environment = { ...process.env, DAR_ROOT: root, DAR_CHAT_BRIDGE: chatBridge, DAR_CHAT_CONTROL: chatControl, DAR_TEST_ROOT: root, DAR_PORT: String(port), DAR_DATABASE_PORT: String(databasePort),
  DAR_INTEGRATION_ORIGIN: `http://127.0.0.1:${port}`, DAR_DATABASE_URL: `http://127.0.0.1:${databasePort}`, DAR_DATABASE_NAME: "dar-acceptance",
  DAR_SPACETIME_BIN: process.env.DAR_SPACETIME_BIN ?? "spacetime", DAR_SPACETIME_CLI_ROOT: path.join(root, "cli"),
  DAR_DATABASE_TOKEN_FILE: path.join(root, "secrets/database-token"), DAR_BOOTSTRAP_TOKEN_FILE: path.join(root, "secrets/bootstrap-token"), DAR_CHROMIUM: chromium };
const child = spawn(process.execPath, ["scripts/dev.mjs"], { cwd: project, env: environment, stdio: ["ignore", "pipe", "pipe"] });
let output = "";
for (const stream of [child.stdout, child.stderr]) stream.on("data", chunk => { output = (output + chunk).slice(-16_000); });
function run(args, timeout = 120_000) {
  const result = spawnSync(process.execPath, args, { cwd: project, env: environment, stdio: "inherit", timeout });
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
  // The Chat bridge (Go): its whatsmeow store and protocol, without WhatsApp.
  if(!agentOnly&&!identityOnly&&!workspaceOnly&&!pimOnly){const go=spawnSync("go",["test","./..."],{cwd:path.join(project,"bridge"),env:{...process.env,CGO_ENABLED:"0",GOTOOLCHAIN:"local",GOFLAGS:"-mod=readonly"},stdio:"inherit",timeout:600_000});if(go.error||go.status!==0)throw new Error("Verification failed: go test (bridge)");}
  // Setup race verification must run before the other tests create users.
  run(["--test", "test/auth.integration.test.ts"]);
  if(storageOnly){run(["--test","--test-concurrency=1","test/blobs.test.ts","test/storage.test.ts","test/files.integration.test.ts","test/dav.integration.test.ts","test/backup.integration.test.ts","test/gc.integration.test.ts","test/blob-migration.integration.test.ts","test/history.integration.test.ts","test/git.integration.test.ts","test/pim-upgrade.integration.test.ts"],600_000);console.info(`PASS Storage acceptance. Isolated artifacts: ${root}`);}
  else if(gitOnly){run(["--test","--test-concurrency=1","test/git.integration.test.ts","test/git-app.integration.test.ts","test/git-interop.integration.test.ts","test/git-tracker.integration.test.ts","test/storage.test.ts","test/upload-pace.test.ts","test/upload-timeout.integration.test.ts"],600_000);if(existsSync(path.join(project,"test/git-browser.mjs")))run(["test/git-browser.mjs"],300_000);run(["test/git-tracker-browser.mjs"],300_000);console.info(`PASS Git acceptance. Isolated artifacts: ${root}`);}
  else if(chatOnly){run(["--test","test/chat.test.ts","test/chat.integration.test.ts","test/chat-discord.integration.test.ts"],300_000);run(["test/chat-browser.mjs"],300_000);console.info(`PASS Chat acceptance. Isolated artifacts: ${root}`);}
  else if(booksOnly){run(["--test","test/books.integration.test.ts"],300_000);run(["test/books-browser.mjs"],300_000);console.info(`PASS Books acceptance. Isolated artifacts: ${root}`);}
  else if(historyOnly){run(["--test","--test-concurrency=1","test/history.integration.test.ts","test/git.integration.test.ts"],300_000);if(existsSync(path.join(project,"test/history-browser.mjs")))run(["test/history-browser.mjs"],300_000);console.info(`PASS history acceptance. Isolated artifacts: ${root}`);}
  else{
  if(!photosOnly&&(newsOnly||!agentOnly&&!identityOnly&&!workspaceOnly&&!pimOnly)){
    run(["--test","--test-concurrency=1","test/news.test.ts","test/news.integration.test.ts"],300_000);
    run(["--test","test/chat.test.ts","test/chat.integration.test.ts","test/chat-discord.integration.test.ts"],300_000);run(["test/chat-browser.mjs"],300_000);
  }
  if(!agentOnly&&!identityOnly&&!workspaceOnly&&!pimOnly&&!newsOnly&&!photosOnly){run(["--test","test/books.integration.test.ts"],300_000);run(["test/books-browser.mjs"],300_000);run(["test/git-browser.mjs"],300_000);run(["test/git-tracker-browser.mjs"],300_000);}
  if(photosOnly||!agentOnly&&!identityOnly&&!workspaceOnly&&!pimOnly&&!newsOnly)run(["--test","--test-concurrency=1","test/photos.integration.test.ts","test/photos-library.integration.test.ts","test/photos-sharing.integration.test.ts","test/photos-immich.integration.test.ts"],600_000);
  if(!newsOnly&&!photosOnly&&(agentOnly||!identityOnly&&!workspaceOnly&&!pimOnly)){
    run(["--test","--test-concurrency=1","test/agent-connection.test.ts","test/agent.integration.test.ts","test/codex.integration.test.ts","test/claude.integration.test.ts"]);
    run(["test/agent-browser.mjs"]);
  }
  if(!agentOnly&&!newsOnly&&!photosOnly){
  run(["--test","../apps/prayer-times/test/calculations.test.ts","test/app-packages.test.ts","test/apps.integration.test.ts"]);
  run(["test/prayer-times-browser.mjs"]);
  if(!identityOnly&&!workspaceOnly&&!pimOnly){
    run(["--test","test/appearance-scopes.integration.test.ts"]);
    run(["test/appearance-scopes-browser.mjs"]);
  }
  if(!identityOnly&&!workspaceOnly&&!pimOnly)run(["test/production-proxy.mjs"]);
  if(!identityOnly&&!workspaceOnly){
    run(["--test", "--test-concurrency=1", "test/calendar-navigation.test.ts", "test/calendar-subscriptions.integration.test.ts", "test/davx5.integration.test.ts", "test/pim-standards.test.ts", "test/pim.integration.test.ts"]);
    run(["--test", "test/pim-upgrade.integration.test.ts"]);
  }
  run(["--test","test/global-search.integration.test.ts"]);
  run(["test/refinement-browser.mjs"]);
  run(["test/themed-controls-browser.mjs"]);
  run(["test/app-launcher-browser.mjs"]);
  if(!workspaceOnly&&!pimOnly){
  run(["--test", "--test-concurrency=1", "test/scim-model.test.ts", "test/oidc-config.integration.test.ts", "test/identity.integration.test.ts", "test/identity-lifecycle.integration.test.ts"]);
  run(["--test", "test/identity-upgrade.integration.test.ts"]);
  run(["--test", "test/identity-proxy.integration.test.ts"]);
  run(["test/identity-browser.mjs"]);
  }
  if(!identityOnly&&!pimOnly)run(["--test","--test-concurrency=1","test/ui-settings.integration.test.ts","test/presentation.test.ts","test/dates.test.ts","test/format-markdown.test.ts","test/workspace-standards.test.ts","test/workspace.integration.test.ts","test/workspace-upgrade.integration.test.ts"]);
  if(!identityOnly&&!pimOnly)run(["test/workspace-browser.mjs"]);
  if(!identityOnly&&!pimOnly)run(["test/shared-ui-browser.mjs"]);
  if(!identityOnly&&!workspaceOnly){
  if(!pimOnly){run(["--test", "--test-concurrency=1", "test/auth-primitives.test.ts", "test/auth-security.integration.test.ts", "test/files.integration.test.ts", "test/history.integration.test.ts", "test/git.integration.test.ts", "test/git-app.integration.test.ts", "test/git-interop.integration.test.ts", "test/git-tracker.integration.test.ts", "test/dav.integration.test.ts", "test/backup.integration.test.ts", "test/gc.integration.test.ts", "test/blob-migration.integration.test.ts", "test/blobs.test.ts", "test/keyring.test.ts", "test/pagination.integration.test.ts", "test/storage.test.ts"]);
    // A 75-second upload proves bodies are timed by progress, not by a total.
    run(["--test","test/upload-pace.test.ts","test/upload-timeout.integration.test.ts"],240_000);
  run(["test/browser.mjs"]);}
  run(["test/pim-browser.mjs"]);
  run(["test/contacts-layout-browser.mjs"]);
  if(!pimOnly)run(["test/twilight-browser.mjs"]);
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
  }else console.info(`PASS ${photosOnly?"Photos":newsOnly?"News":"Agent"} acceptance. Isolated artifacts: ${root}`);
  }
} finally {
  if (child.exitCode === null && child.signalCode === null) await new Promise(resolve => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
  });
}
