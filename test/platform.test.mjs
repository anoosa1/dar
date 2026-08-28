import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import path from "node:path";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";

const repositoryRoot = path.resolve(import.meta.dirname, "..");
const serverEntry = path.join(repositoryRoot, "dist/server/main.js");

async function availablePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", resolve);
  });
  const { port } = probe.address();
  await new Promise((resolve, reject) => probe.close((error) => error ? reject(error) : resolve()));
  return port;
}

function startDar(dataRoot, port) {
  const child = spawn(process.execPath, [serverEntry], {
    cwd: repositoryRoot,
    env: {
      ...process.env,
      DAR_HOST: "127.0.0.1",
      DAR_PORT: String(port),
      DAR_DATA_ROOT: dataRoot,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  return { child, output: () => output };
}

async function waitForHealth(port, child, output) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Dar exited before health was ready:\n${output()}`);
    }
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      if (response.ok) return response.json();
    } catch {
      // The listener may not be bound yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for Dar:\n${output()}`);
}

async function stopDar(child) {
  if (child.exitCode !== null) return child.exitCode;
  const exited = new Promise((resolve, reject) => {
    child.once("exit", resolve);
    child.once("error", reject);
  });
  child.kill("SIGTERM");
  return exited;
}

test("fresh startup creates a private versioned SQLite instance and shuts down", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-process-test-");
  const port = await availablePort();
  const running = startDar(dataRoot, port);
  try {
    assert.deepEqual(await waitForHealth(port, running.child, running.output), {
      status: "ok",
      service: "dar",
      runtime: "node",
      apiVersion: "1",
    });

    const marker = JSON.parse(await readFile(path.join(dataRoot, "instance.json"), "utf8"));
    assert.equal(marker.product, "dar");
    assert.equal(marker.dataVersion, 1);
    assert.equal((await stat(path.join(dataRoot, "instance.json"))).mode & 0o777, 0o600);
    assert.equal((await stat(path.join(dataRoot, "dar.sqlite"))).mode & 0o777, 0o600);
    assert.equal((await stat(path.join(dataRoot, "users"))).mode & 0o777, 0o700);
    assert.equal((await stat(path.join(dataRoot, "plugins"))).mode & 0o777, 0o700);

    assert.equal(await stopDar(running.child), 0);
    assert.match(running.output(), /server closing/);

    const database = new DatabaseSync(path.join(dataRoot, "dar.sqlite"), { readOnly: true });
    assert.equal(database.prepare("SELECT version FROM dar_migrations").get().version, 1);
    database.close();
  } finally {
    await stopDar(running.child);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("startup rejects an unmarked Go-era data root without modifying it", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-legacy-root-test-");
  const legacyDatabase = path.join(dataRoot, "dar.db");
  await writeFile(legacyDatabase, "legacy");
  await mkdir(path.join(dataRoot, "legacy-files"));
  const port = await availablePort();
  const running = startDar(dataRoot, port);
  try {
    const exitCode = await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error(`Dar did not reject the root:\n${running.output()}`)), 10_000);
      running.child.once("exit", (code) => {
        clearTimeout(timeout);
        resolve(code);
      });
      running.child.once("error", reject);
    });
    assert.equal(exitCode, 1);
    assert.equal(await readFile(legacyDatabase, "utf8"), "legacy");
    await assert.rejects(stat(path.join(dataRoot, "instance.json")), { code: "ENOENT" });
  } finally {
    await stopDar(running.child);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
