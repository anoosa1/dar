import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { DbConnection } from "../client/module_bindings/index.ts";

const origin = process.env.DAR_INTEGRATION_ORIGIN;
test("native login, private views, per-session revocation, and forbidden service calls on real SpacetimeDB", { skip: !origin, timeout: 30_000 }, async () => {
  const base = origin!;
  const status = await (await fetch(`${base}/api/v1/auth/status`)).json() as { initialized: boolean };
  if (!status.initialized) {
    const bootstrapToken = (await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!, "utf8")).trim();
    const responses = await Promise.all([1, 2].map(() => fetch(`${base}/api/v1/auth/setup`, { method: "POST", headers: { "content-type": "application/json", origin: base },
      body: JSON.stringify({ username: "admin", displayName: "Test Administrator", password: "a long isolated test password", bootstrapToken }) })));
    assert.deepEqual(responses.map(response => response.status).sort(), [200, 409]);
  }
  async function login() {
    const response = await fetch(`${base}/api/v1/auth/login`, { method: "POST", headers: { "content-type": "application/json", origin: base }, body: JSON.stringify({ username: "admin", password: "a long isolated test password" }) });
    assert.equal(response.status, 200);
    const cookie = response.headers.get("set-cookie")!.split(";")[0];
    const { csrfToken } = await response.json() as { csrfToken: string };
    const headers = { cookie, origin: base, "x-csrf-token": csrfToken };
    const r = await fetch(`${base}/api/v1/auth/token`, { method: "POST", headers });
    assert.equal(r.status, 200);
    return { headers, ...await r.json() as { token: string; databaseUrl: string; databaseName: string } };
  }
  const first = await login(), second = await login();
  async function connect(session: typeof first) {
    return new Promise<DbConnection>((resolve, reject) => {
      const connection = DbConnection.builder().withUri(session.databaseUrl.replace(/^http/, "ws")).withDatabaseName(session.databaseName).withToken(session.token)
        .onConnect(() => resolve(connection)).onConnectError((_ctx, error) => reject(error)).build();
    });
  }
  const a = await connect(first), b = await connect(second);
  try {
    async function subscribe(connection: DbConnection) {
      await new Promise<void>((resolve, reject) => connection.subscriptionBuilder().onApplied(() => resolve()).onError(ctx => reject(ctx.event)).subscribe(["SELECT * FROM me", "SELECT * FROM my_files"]));
    }
    await Promise.all([subscribe(a), subscribe(b)]);
    assert.equal(a.db.me.count(), 1n);
    assert.equal(b.db.me.count(), 1n);
    await assert.rejects(a.procedures.serviceAccount({ username: "admin" }));
    await a.reducers.createFolder({ id: randomUUID(), parentId: "", name: `Proof ${Date.now()}` });
    assert.ok(a.db.myFiles.count() > 0n);
    const removed = new Promise<void>(resolve => { a.db.me.onDelete(() => resolve()); });
    const response = await fetch(`${base}/api/v1/auth/logout`, { method: "POST", headers: first.headers });
    assert.equal(response.status, 200);
    await removed;
    assert.equal(a.db.me.count(), 0n);
    assert.equal(a.db.myFiles.count(), 0n);
    assert.equal(b.db.me.count(), 1n);
    await assert.rejects(a.reducers.createFolder({ id: randomUUID(), parentId: "", name: "Denied" }));
    assert.equal((await fetch(`${base}/api/v1/auth/token`, { method: "POST", headers: first.headers })).status, 401);
  } finally { a.disconnect(); b.disconnect(); }
});
