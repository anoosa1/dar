import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile, readdir, copyFile, unlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { database } from "../runtime/database.ts";
import { backup, restore } from "../runtime/backup.ts";
import {publishModule,startInstance} from "./instance.ts";
import { login, upload } from "./integration-helpers.ts";

const origin = process.env.DAR_INTEGRATION_ORIGIN;
const sha256 = (value: string) => createHash("sha256").update(value).digest("hex");
type Snapshot = Record<string, unknown> & { format: string; blobs: { id: string; size: number; digest: string }[]; versions: { id: string; fileId: string; blobId: string; size: number; mediaType: string }[]; files: { id: string; ownerId: string }[] };

/** Rewrites a v11 backup as Dar wrote backups before content addressing: content named by upload ID. */
async function asLegacyBackup(directory: string) {
  const snapshot = JSON.parse(await readFile(path.join(directory, "snapshot.json"), "utf8")) as Snapshot;
  const byDigest = new Map<string, string>(), uploads: Record<string, unknown>[] = [], blobs: Snapshot["blobs"] = [];
  const owners = new Map(snapshot.files.map(file => [file.id, file.ownerId]));
  // Every uploaded version had its own upload ID, even for identical content.
  for (const version of snapshot.versions) {
    const id = randomUUID(), digest = version.blobId;
    if (!byDigest.has(digest)) byDigest.set(digest, id);
    await copyFile(path.join(directory, "blobs", digest), path.join(directory, "blobs", id));
    uploads.push({ id, sessionId: "", ownerId: owners.get(version.fileId) ?? "", parentId: "", fileId: version.fileId, name: "", size: version.size, mediaType: version.mediaType, expectedRevision: 0, expiresAt: 0, state: "complete", digest, expiresKey: "0000000000000000" });
    blobs.push({ id, size: version.size, digest });
    version.blobId = id;
  }
  for (const digest of byDigest.keys()) await unlink(path.join(directory, "blobs", digest));
  const rename = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(rename);
    if (!value || typeof value !== "object") return value;
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, key === "blobId" && typeof item === "string" && byDigest.has(item) ? byDigest.get(item) : rename(item)]));
  };
  const { versions, ...rest } = snapshot;
  const legacy = { ...(rename(rest) as Snapshot), format: "dar-backup-v10", versions, blobs, uploads };
  const text = JSON.stringify(legacy);
  await writeFile(path.join(directory, "snapshot.json"), text);
  await writeFile(path.join(directory, "complete.json"), JSON.stringify({ format: "dar-backup-v1", snapshotDigest: sha256(text) }));
  return byDigest;
}

test("a backup from before content addressing restores, and the next start names its content by digest", { skip: !origin || !process.env.DAR_TEST_ROOT, timeout: 60_000 }, async () => {
  const session = await login(origin!), shared = `Uploaded twice ${randomUUID()}`, single = `Uploaded once ${randomUUID()}`;
  const first = await upload(origin!, session, `migrate-${randomUUID()}.txt`, shared), second = await upload(origin!, session, `migrate-${randomUUID()}.txt`, shared);
  const third = await upload(origin!, session, `migrate-${randomUUID()}.txt`, single);
  const source = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const temporary = await mkdtemp(path.join(tmpdir(), "dar-blob-migration-")), directory = path.join(temporary, "backup");
  await backup(source, process.env.DAR_TEST_ROOT!, directory);
  const byDigest = await asLegacyBackup(directory);
  assert.ok(byDigest.has(sha256(shared)) && byDigest.has(sha256(single)));
  const listener = createServer(); await new Promise<void>(resolve => listener.listen(0, "127.0.0.1", resolve));
  const port = (listener.address() as import("node:net").AddressInfo).port;
  await new Promise<void>(resolve => listener.close(() => resolve()));
  const newOrigin = `http://127.0.0.1:${port}`, name = `dar-migrate-${randomUUID().slice(0, 8)}`;
  publishModule(name, newOrigin);
  const target = await database(process.env.DAR_DATABASE_URL!, name, process.env.DAR_DATABASE_TOKEN_FILE!), root = path.join(temporary, "restored");
  await restore(target, directory, root);
  assert.ok((await readdir(path.join(root, "blobs"))).every(file => /^[a-f0-9-]{36}$/.test(file)));
  assert.equal((await target.call<{ done: boolean }>("service_blob_migration_plan")).done, false);
  const runtime = await startInstance({ port, origin: newOrigin, root, databaseName: name, development: true });
  try {
    const names = await readdir(path.join(root, "blobs"));
    assert.ok(names.length && names.every(file => /^[a-f0-9]{64}$/.test(file)), names.join(" "));
    assert.equal(names.filter(file => file === sha256(shared)).length, 1);
    const recovered = await login(newOrigin);
    for (const [file, text] of [[first, shared], [second, shared], [third, single]] as const)
      assert.equal(await (await fetch(`${newOrigin}/api/v1/files/${file.fileId}/content`, { headers: recovered.headers })).text(), text);
    const after = await target.call<{ format: string; files: { id: string; blobId: string }[]; blobs: { id: string; digest: string }[] }>("service_backup", [randomUUID()]);
    assert.equal(after.format, "dar-backup-v12");
    assert.deepEqual([first, second, third].map(file => after.files.find(row => row.id === file.fileId)?.blobId), [sha256(shared), sha256(shared), sha256(single)]);
    assert.ok(after.blobs.every(blob => blob.id === blob.digest));
  } finally { await runtime.close(); }
});
