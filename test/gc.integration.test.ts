import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID, createHash } from "node:crypto";
import { mkdtemp, mkdir, copyFile, writeFile, access, utimes } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { database } from "./support/database.ts";
import { browserToken } from "./support/auth.ts";
import { reconcileStorage, sweepOrphans } from "./support/operator.ts";
// Content lives under <root>/blobs/<id>; a new root is marked as Dar's before anything is put in it.
const blobPath = (root: string, id: string) => path.join(root, "blobs", id);
const threeHoursAgo = () => new Date(Date.now() - 3 * 3600_000);
import { connect, login, upload } from "./integration-helpers.ts";
import {moduleArguments} from "./instance.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;

test("identical files share one blob, kept until no version or backup pin uses it; unrecorded content is swept", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 30_000 }, async () => {
  const session = await login(origin!), contents = `Shared by two files ${randomUUID()}`, digest = createHash("sha256").update(contents).digest("hex");
  const original = await upload(origin!, session, `GC-${randomUUID()}.txt`, contents), twin = await upload(origin!, session, `GC-twin-${randomUUID()}.txt`, contents);
  const source = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const sourcePin = randomUUID();
  const snapshot = await source.call<{ accounts: { id: string; verifier: string; username: string }[]; blobs: { id: string; digest: string }[]; files: { id: string; blobId: string }[] }>("service_backup", [sourcePin]);
  assert.deepEqual([original.fileId, twin.fileId].map(id => snapshot.files.find(row => row.id === id)?.blobId), [digest, digest]);
  assert.ok(snapshot.blobs.every(blob => blob.id === blob.digest));
  const root = await mkdtemp(path.join(tmpdir(), "dar-gc-proof-"));
  await writeFile(path.join(root, "instance.json"), JSON.stringify({ format: "dar-spacetime-v1" }), { mode: 0o600 });
  await mkdir(path.join(root, "blobs"), { mode: 0o700 });
  try {
    for (const blob of snapshot.blobs) await copyFile(blobPath(process.env.DAR_TEST_ROOT!, blob.id), blobPath(root, blob.id));
    const name = `dar-gc-${randomUUID().slice(0, 8)}`;
    const publish = spawnSync(process.env.DAR_SPACETIME_BIN!, ["--root-dir", process.env.DAR_SPACETIME_CLI_ROOT!, "publish", name, ...moduleArguments(), "--server", process.env.DAR_DATABASE_URL!, "--yes", "--no-config"], { env: { ...process.env, DAR_ISSUER: `${origin}/auth/issuer` }, encoding: "utf8" });
    assert.equal(publish.status, 0, publish.stdout + publish.stderr);
    const target = await database(process.env.DAR_DATABASE_URL!, name, process.env.DAR_DATABASE_TOKEN_FILE!);
    await target.call("service_restore", [JSON.stringify(snapshot)]);
    // Content no upload recorded (a failed upload or a crash) goes; recorded content stays.
    const orphanBytes = `orphan ${randomUUID()}`, orphan = createHash("sha256").update(orphanBytes).digest("hex");
    // Past the grace period that protects content still being written.
    await writeFile(blobPath(root, orphan), orphanBytes); await utimes(blobPath(root, orphan), threeHoursAgo(), threeHoursAgo());
    assert.equal(await sweepOrphans(target, root), 1);
    await assert.rejects(access(blobPath(root, orphan)));
    assert.equal(await reconcileStorage(target, root), 0);
    await access(blobPath(root, digest));
    const pin = randomUUID(); await target.call("service_backup", [pin]);
    const user = snapshot.accounts.find(user => user.username === "admin")!;
    const id = randomUUID(), expiresAt = Date.now() + 60_000;
    await target.call("service_create_session", [id, user.id, `test:${id}`, "", expiresAt, user.verifier]);
    const token = await browserToken(path.join(process.env.DAR_TEST_ROOT!, "secrets"), `${origin}/auth/issuer`, id, expiresAt);
    const db = await connect({ ...session, token, databaseName: name });
    try {
      await db.reducers.trashEntry({ id: original.fileId, expectedRevision: 1 });
      await db.reducers.deleteEntry({ id: original.fileId, expectedRevision: 2 });
      assert.equal(await reconcileStorage(target, root), 0);
      await target.call("service_release_backup", [pin]);
      assert.equal(await reconcileStorage(target, root), 0);
      await access(blobPath(root, digest));
      await db.reducers.trashEntry({ id: twin.fileId, expectedRevision: 1 });
      await db.reducers.deleteEntry({ id: twin.fileId, expectedRevision: 2 });
      // Content linked or used again within the grace period stays claimed until it settles.
      assert.equal(await reconcileStorage(target, root), 0);
      await access(blobPath(root, digest));
      await utimes(blobPath(root, digest), threeHoursAgo(), threeHoursAgo());
      assert.equal(await reconcileStorage(target, root), 1);
      await assert.rejects(access(blobPath(root, digest)));
      assert.equal(await reconcileStorage(target, root), 0);
    } finally { db.disconnect(); }
  } finally { await source.call("service_release_backup", [sourcePin]); }
});
