import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID } from "node:crypto";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { request } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { login, connect } from "./integration-helpers.ts";

const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("HTTP limits, CSRF/origin rejection, operator recovery, split Unicode input and login failure budget", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 30_000 }, async () => {
  const base = origin!, admin = await login(base), username = `recovery-${randomUUID().slice(0, 8)}`;
  const json = { "content-type": "application/json", origin: base };
  assert.equal((await fetch(`${base}/api/v1/auth/token`, { method: "POST", headers: { cookie: admin.headers.cookie, origin: base } })).status, 403);
  assert.equal((await fetch(`${base}/api/v1/auth/token`, { method: "POST", headers: { ...admin.headers, origin: "https://unrelated.example" } })).status, 403);
  assert.equal((await fetch(`${base}/api/v1/auth/users`, { method: "POST", headers: { ...admin.headers, ...json }, body: JSON.stringify({ username, displayName: "Recovery proof", password: "initial recovery password" }) })).status, 201);
  const old = await login(base, username, "initial recovery password"), connection = await connect(old);
  const temporary = await mkdtemp(path.join(tmpdir(), "dar-recovery-proof-"));
  try {
    const password = "  a recovered Unicode password 🔐  ", filename = path.join(temporary, "password");
    await writeFile(filename, `${password}\n`, { mode: 0o600 });
    const removed = new Promise<void>(resolve => connection.db.me.onDelete(() => resolve()));
    const recovered = spawnSync(process.execPath, ["scripts/operator.ts", "recover-password", username, filename], { cwd: path.resolve(import.meta.dirname, ".."), env: process.env, encoding: "utf8", timeout: 10_000 });
    assert.equal(recovered.status, 0, recovered.stderr); await removed;
    assert.equal((await fetch(`${base}/api/v1/auth/session`, { headers: old.headers })).status, 401);
    // Split a four-byte UTF-8 character across network writes. Decode only once
    // the bounded body is complete, preserving credentials byte-for-byte.
    const bytes = Buffer.from(JSON.stringify({ username, password })), boundary = bytes.indexOf(Buffer.from("🔐")) + 2;
    const status = await new Promise<number>((resolve, reject) => {
      const req = request(`${base}/api/v1/auth/login`, { method: "POST", headers: json }, res => { res.resume(); res.on("end", () => resolve(res.statusCode!)); });
      req.on("error", reject); req.write(bytes.subarray(0, boundary)); setImmediate(() => req.end(bytes.subarray(boundary)));
    });
    assert.equal(status, 200);
    const oversized = await fetch(`${base}/api/v1/auth/login`, { method: "POST", headers: json, body: JSON.stringify({ username, password: "x".repeat(20_000) }) });
    assert.equal(oversized.status, 413);
    const errors = [];
    for (const name of [username, `missing-${randomUUID()}`]) {
      const response = await fetch(`${base}/api/v1/auth/login`, { method: "POST", headers: json, body: JSON.stringify({ username: name, password: "wrong password" }) });
      assert.equal(response.status, 401); errors.push(await response.json());
    }
    assert.deepEqual(errors[0], errors[1]);
    let limited = false;
    for (let i = 0; i < 11; i++) {
      // Give this probe a fresh peer/window, independent of earlier suite logins.
      const status=await new Promise<number>((resolve,reject)=>{const req=request(`${base}/api/v1/auth/login`,{method:"POST",localAddress:"127.0.0.2",headers:json},res=>{res.resume();res.on("end",()=>resolve(res.statusCode!));});req.on("error",reject);req.end(JSON.stringify({username,password:"wrong password"}));});
      if (status === 429) { limited = true; break; }
      assert.equal(status, 401);
    }
    assert.equal(limited, true);
  } finally { connection.disconnect(); await rm(temporary, { recursive: true, force: true }); }
});
