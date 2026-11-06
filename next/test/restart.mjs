import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import http from "node:http";
import net from "node:net";
import { mkdtemp, readFile, stat, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { login, upload, connect } from "./integration-helpers.ts";

if (!process.env.DAR_SPACETIME_BIN) throw new Error("DAR_SPACETIME_BIN is required.");
async function freePort() {
  const server = net.createServer(); await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address(); await new Promise(resolve => server.close(resolve)); return port;
}
const root = await mkdtemp(path.join(tmpdir(), "dar-restart-proof-")), port = await freePort(), databasePort = await freePort();
const origin = `http://127.0.0.1:${port}`, project = path.resolve(import.meta.dirname, "..");
const environment = { ...process.env, DAR_ROOT: root, DAR_PORT: String(port), DAR_DATABASE_PORT: String(databasePort), DAR_DATABASE_NAME: "dar-restart" };
let child;
async function until(probe) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    try { if (await probe()) return; } catch {}
    if (child?.exitCode !== null && child?.exitCode !== undefined) throw new Error("Fixture stopped before readiness.");
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error("Fixture readiness timed out.");
}
async function start() {
  child = spawn(process.execPath, ["scripts/dev.mjs"], { cwd: project, env: environment, stdio: ["ignore", "pipe", "pipe"] });
  let output = ""; child.stdout.on("data", chunk => { output = (output + chunk).slice(-8000); }); child.stderr.on("data", chunk => { output = (output + chunk).slice(-8000); });
  try { await until(async () => (await fetch(`${origin}/api/v1/auth/status`, { signal: AbortSignal.timeout(500) })).ok); }
  catch (error) { throw new Error(`${error.message}\n${output}`); }
}
async function stop() {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  await new Promise(resolve => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 10_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
  });
}
try {
  await start();
  const setup = await fetch(`${origin}/api/v1/auth/setup`, { method: "POST", headers: { origin, "content-type": "application/json" }, body: JSON.stringify({ username: "admin", displayName: "Restart proof", password: "a long isolated test password", bootstrapToken: await readFile(path.join(root, "secrets/bootstrap-token"), "utf8") }) });
  assert.equal(setup.status, 200);
  const session = await login(origin);
  const durable = await upload(origin, session, "Durable.txt", "Committed content survives process failure.");
  const reservationResponse = await fetch(`${origin}/api/v1/files/uploads`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ name: "Interrupted.txt", parentId: "", size: 4096, mediaType: "text/plain" }) });
  assert.equal(reservationResponse.status, 201); const reservation = await reservationResponse.json();
  const request = http.request(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: { ...session.headers, "content-length": 4096 } });
  request.on("error", () => {}); request.write(Buffer.alloc(1024, "x"));
  await until(async () => (await stat(path.join(root, "uploads", `${reservation.id}.tmp`))).size >= 1024);
  const pids = JSON.parse(await readFile(path.join(root, "service-pids.json"), "utf8"));
  const processInfo = await readFile(`/proc/${pids.runtime}/stat`, "utf8");
  assert.equal(Number(processInfo.slice(processInfo.lastIndexOf(")") + 2).split(" ")[1]), child.pid);
  const stopped = new Promise(resolve => child.once("exit", resolve));
  process.kill(pids.runtime, "SIGKILL"); await stopped; request.destroy();
  await start();
  assert.deepEqual(await readdir(path.join(root, "uploads")), []);
  assert.equal(await (await fetch(`${origin}/api/v1/files/${durable.fileId}/content`, { headers: session.headers })).text(), "Committed content survives process failure.");
  const retry = await fetch(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: session.headers, body: Buffer.alloc(4096, "r") });
  assert.equal(retry.status, 200, await retry.clone().text());
  const repeated = await fetch(`${origin}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: session.headers, body: Buffer.alloc(4096, "s") });
  assert.equal(repeated.status, 200);
  assert.equal(await (await fetch(`${origin}/api/v1/files/${reservation.fileId}/content`, { headers: session.headers })).text(), "r".repeat(4096));
  const db = await connect(session);
  try { assert.equal([...db.db.myFiles.iter()].some(entry => entry.id === durable.fileId), true); } finally { db.disconnect(); }
  console.info("PASS forced runtime failure during upload, full process/database restart, stale-lock/temp reconciliation, durable bytes, persisted login, reconnection and idempotent upload completion.");
} finally { await stop(); }
