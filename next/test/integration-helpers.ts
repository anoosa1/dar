import assert from "node:assert/strict";
import { DbConnection } from "../client/module_bindings/index.ts";

export async function login(base: string, username = "admin", password = "a long isolated test password") {
  const response = await fetch(`${base}/api/v1/auth/login`, { method: "POST", headers: { "content-type": "application/json", origin: base }, body: JSON.stringify({ username, password }) });
  assert.equal(response.status, 200, await response.clone().text());
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  const { csrfToken } = await response.json() as { csrfToken: string };
  const headers = { cookie, origin: base, "x-csrf-token": csrfToken };
  const r = await fetch(`${base}/api/v1/auth/token`, { method: "POST", headers });
  assert.equal(r.status, 200);
  return { headers, ...await r.json() as { token: string; databaseUrl: string; databaseName: string } };
}
export async function connect(session: Awaited<ReturnType<typeof login>>) {
  const connection = await new Promise<DbConnection>((resolve, reject) => {
    const db = DbConnection.builder().withUri(session.databaseUrl.replace(/^http/, "ws")).withDatabaseName(session.databaseName).withToken(session.token)
      .onConnect(() => resolve(db)).onConnectError((_ctx, error) => reject(error)).build();
  });
  await new Promise<void>((resolve, reject) => connection.subscriptionBuilder().onApplied(() => resolve()).onError(ctx => reject(ctx.event)).subscribe(["SELECT * FROM me", "SELECT * FROM my_files", "SELECT * FROM my_usage"]));
  return connection;
}
export async function upload(base: string, session: Awaited<ReturnType<typeof login>>, name: string, contents: string | Buffer, parentId = "", fileId?: string, expectedRevision?: number) {
  const response = await fetch(`${base}/api/v1/files/uploads`, { method: "POST", headers: { ...session.headers, "content-type": "application/json" }, body: JSON.stringify({ name, size: Buffer.byteLength(contents), mediaType: "text/plain", parentId, fileId, expectedRevision }) });
  assert.equal(response.status, 201, await response.clone().text());
  const reservation = await response.json() as { id: string; fileId: string };
  const transfer = await fetch(`${base}/api/v1/files/uploads/${reservation.id}`, { method: "PUT", headers: session.headers, body: typeof contents === "string" ? contents : new Uint8Array(contents), signal: AbortSignal.timeout(10_000) });
  assert.equal(transfer.status, 200, await transfer.clone().text());
  return reservation;
}
