import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID, createHash } from "node:crypto";
import { mkdtemp, copyFile, writeFile, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { database } from "../runtime/database.ts";
import { createIssuer } from "../runtime/auth/issuer.ts";
import { openRoot } from "../runtime/root.ts";
import { lockRoot } from "../runtime/lock.ts";
import { blobStore, blobPath } from "../runtime/files/blobs.ts";
import { reconcileStorage } from "../runtime/files/maintenance.ts";
import { connect, login, upload } from "./integration-helpers.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;

test("expired orphan reclamation respects live versions and in-progress backup pins", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 30_000 }, async () => {
  const session = await login(origin!), original = await upload(origin!, session, `GC-${randomUUID()}.txt`, "Protected until every reference is gone.");
  const source = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const sourcePin = randomUUID();
  const snapshot = await source.call<{ accounts: { id: string; verifier: string; username: string }[]; blobs: { id: string }[];
    uploads: { id: string; fileId: string; expiresAt: number; expiresKey: string; digest: string; size: number }[] }>("service_backup", [sourcePin]);
  const root = await openRoot(await mkdtemp(path.join(tmpdir(), "dar-gc-proof-"))), release = await lockRoot(root);
  try {
    for (const blob of snapshot.blobs) await copyFile(blobPath(process.env.DAR_TEST_ROOT!, blob.id), blobPath(root, blob.id));
    for (const pending of snapshot.uploads) { pending.expiresAt = 0; pending.expiresKey = "0000000000000000"; }
    const orphan = randomUUID(), bytes = Buffer.from("orphan");
    snapshot.uploads.push({ ...snapshot.uploads[0], id: orphan, fileId: randomUUID(), size: bytes.length, digest: createHash("sha256").update(bytes).digest("hex") });
    await writeFile(blobPath(root, orphan), bytes);
    const name = `dar-gc-${randomUUID().slice(0, 8)}`;
    const publish = spawnSync(process.env.DAR_SPACETIME_BIN!, ["--root-dir", process.env.DAR_SPACETIME_CLI_ROOT!, "publish", name, "--js-path", path.resolve(import.meta.dirname, "../module/dist/bundle.js"), "--server", process.env.DAR_DATABASE_URL!, "--yes", "--no-config"], { env: { ...process.env, DAR_ISSUER: `${origin}/auth/issuer` }, encoding: "utf8" });
    assert.equal(publish.status, 0, publish.stdout + publish.stderr);
    const target = await database(process.env.DAR_DATABASE_URL!, name, process.env.DAR_DATABASE_TOKEN_FILE!);
    await target.call("service_restore", [JSON.stringify(snapshot)]);
    const blobs = blobStore(root);
    assert.equal(await reconcileStorage(target, blobs), 1);
    await assert.rejects(access(blobPath(root, orphan)));
    await access(blobPath(root, original.id));
    const pin = randomUUID(); await target.call("service_backup", [pin]);
    const user = snapshot.accounts.find(user => user.username === "admin")!;
    const id = randomUUID(), expiresAt = Date.now() + 60_000;
    await target.call("service_create_session", [id, user.id, `test:${id}`, "", expiresAt, user.verifier]);
    const issuer = await createIssuer(path.join(process.env.DAR_TEST_ROOT!, "secrets"), `${origin}/auth/issuer`);
    const db = await connect({ ...session, token: await issuer.token(id, expiresAt), databaseName: name });
    try {
      await db.reducers.trashEntry({ id: original.fileId, expectedRevision: 1 });
      await db.reducers.deleteEntry({ id: original.fileId, expectedRevision: 2 });
      assert.equal(await reconcileStorage(target, blobs), 0);
      await access(blobPath(root, original.id));
      await target.call("service_release_backup", [pin]);
      assert.equal(await reconcileStorage(target, blobs), 1);
      await assert.rejects(access(blobPath(root, original.id)));
    } finally { db.disconnect(); }
  } finally { await release(); await source.call("service_release_backup", [sourcePin]); }
});
