import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { signingJwk } from "../runtime/auth/keyring.ts";
import { decodeJwt, importJWK, SignJWT } from "jose";
import { database } from "../runtime/database.ts";
import { connect, login } from "./integration-helpers.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("password changes, suspension and scheduled expiry revoke active sessions; wrong-audience tokens are rejected", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 30_000 }, async () => {
  const base = origin!, admin = await login(base), username = `security-${randomUUID().slice(0, 8)}`;
  const created = await fetch(`${base}/api/v1/auth/users`, { method: "POST", headers: { ...admin.headers, "content-type": "application/json" }, body: JSON.stringify({ username, displayName: "Security proof", password: "initial security proof password" }) });
  assert.equal(created.status, 201); const { id } = await created.json() as { id: string };
  const first = await login(base, username, "initial security proof password"), second = await login(base, username, "initial security proof password");
  const a = await connect(first), b = await connect(second);
  const owner = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  try {
    const keyData = await signingJwk(`${process.env.DAR_TEST_ROOT}/secrets`);
    const key = await importJWK(keyData, "ES256"), original = decodeJwt(first.token);
    const invalid = await new SignJWT({ sid: original.sid }).setProtectedHeader({ alg: "ES256", kid: keyData.kid }).setIssuer(original.iss!).setAudience("unrelated-application").setSubject(original.sub!).setIssuedAt().setExpirationTime("5m").sign(key);
    await assert.rejects(connect({ ...first, token: invalid }));
    const removed = Promise.all([a, b].map(c => new Promise<void>(resolve => c.db.me.onDelete(() => resolve()))));
    const changed = await fetch(`${base}/api/v1/auth/password`, { method: "POST", headers: { ...first.headers, "content-type": "application/json" }, body: JSON.stringify({ currentPassword: "initial security proof password", password: "replacement security proof password" }) });
    assert.equal(changed.status, 200, await changed.clone().text()); await removed;
    assert.equal((await fetch(`${base}/api/v1/auth/session`, { headers: second.headers })).status, 401);
    const active = await login(base, username, "replacement security proof password"), c = await connect(active);
    try {
      const revoked = new Promise<void>(resolve => c.db.me.onDelete(() => resolve()));
      await owner.call("service_set_enabled", [id, false]); await revoked;
      assert.equal((await fetch(`${base}/api/v1/auth/session`, { headers: active.headers })).status, 401);
      await owner.call("service_set_enabled", [id, true]);
      assert.equal((await fetch(`${base}/api/v1/auth/session`, { headers: active.headers })).status, 401);
      await login(base, username, "replacement security proof password");
    } finally { c.disconnect(); }
    const account = await owner.call<{ verifier: string }>("service_account", [username]);
    const expiringId = randomUUID(), expiresAt = Date.now() + 5000;
    await owner.call("service_create_session", [expiringId, id, `expiry:${expiringId}`, "", expiresAt, account.verifier]);
    const token = await new SignJWT({ sid: expiringId }).setProtectedHeader({ alg: "ES256", kid: keyData.kid }).setIssuer(original.iss!).setAudience("dar-browser").setSubject(`${expiringId}:${randomUUID()}`).setIssuedAt().setExpirationTime("5m").sign(key);
    const expiring = await connect({ ...first, token });
    try {
      assert.equal(expiring.db.me.count(), 1n);
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([new Promise<void>(resolve => expiring.db.me.onDelete(() => resolve())), new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("Scheduled session expiry did not revoke the live view.")), 8000); })]).finally(() => clearTimeout(timer));
      assert.equal(expiring.db.me.count(), 0n);
      await assert.rejects(expiring.reducers.createFolder({ id: randomUUID(), parentId: "", name: "Expired session must not write" }));
    } finally { expiring.disconnect(); }
  } finally { a.disconnect(); b.disconnect(); }
});
