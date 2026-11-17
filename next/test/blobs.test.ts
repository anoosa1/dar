import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, readFile, readdir, rm, utimes, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID, createHash } from "node:crypto";
import { openRoot } from "../runtime/root.ts";
import { blobStore, blobPath } from "../runtime/files/blobs.ts";
import { sweepOrphans } from "../runtime/files/maintenance.ts";
import { migrateBlobs } from "../runtime/files/migrate.ts";
import type { Database } from "../runtime/database.ts";

const sha256 = (text: string) => createHash("sha256").update(text).digest("hex");
const stream = (text: string) => (async function* () { yield Buffer.from(text); })();
const hoursAgo = (hours: number) => new Date(Date.now() - hours * 3600_000);
/** Answers the named service calls; any other call fails the test. */
function fakeDatabase(answers: Record<string, (args: unknown[]) => unknown>) {
  const answer = async (name: string, args: unknown[] = []) => { if (!answers[name]) throw new Error(`Unexpected call ${name}`); return answers[name](args); };
  return { call: answer, read: answer } as unknown as Database;
}
async function temporaryRoot(t: { after: (fn: () => Promise<void>) => void }) {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-blobs-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return openRoot(path.join(directory, "dar"));
}

test("identical uploads are stored once under their SHA-256, and nothing is left in uploads", async t => {
  const root = await temporaryRoot(t), blobs = blobStore(root), content = "the same bytes, uploaded twice\n";
  const first = await blobs.receive(stream(content), randomUUID(), Buffer.byteLength(content));
  const second = await blobs.receive(stream(content), randomUUID(), Buffer.byteLength(content));
  assert.equal(first, sha256(content)); assert.equal(second, first);
  assert.deepEqual(await readdir(path.join(root, "blobs")), [first]);
  assert.equal(await readFile(blobPath(root, first), "utf8"), content);
  assert.deepEqual(await readdir(path.join(root, "uploads")), []);
  await assert.rejects(blobs.receive(stream("short"), randomUUID(), 99), /incompleteUpload/);
  assert.deepEqual(await readdir(path.join(root, "blobs")), [first]);
});

test("content linked or used again within the grace period is never removed", async t => {
  const root = await temporaryRoot(t), content = "grace", id = sha256(content);
  const blobs = blobStore(root), eager = blobStore(root, { graceMs: 0 });
  await blobs.receive(stream(content), randomUUID(), content.length);
  assert.equal(await blobs.remove(id), false);
  await utimes(blobPath(root, id), hoursAgo(3), hoursAgo(3));
  // An upload of the same content marks it used now, so a reclaim already under way keeps it.
  await blobs.receive(stream(content), randomUUID(), content.length);
  assert.ok(Date.now() - (await stat(blobPath(root, id))).mtimeMs < 60_000);
  assert.equal(await blobs.remove(id), false);
  assert.equal(await eager.remove(id), true);
  await assert.rejects(stat(blobPath(root, id)), /ENOENT/);
  assert.equal(await eager.remove(id), true);
});

test("the orphan sweep removes only settled content the database does not know", async t => {
  const root = await temporaryRoot(t), blobs = blobStore(root);
  const [known, orphan, recent] = await Promise.all(["known", "orphan", "recent"].map(text => blobs.receive(stream(text), randomUUID(), text.length)));
  for (const id of [known, orphan]) await utimes(blobPath(root, id), hoursAgo(3), hoursAgo(3));
  const asked: string[][] = [];
  const db = fakeDatabase({ service_unknown_blobs: ([ids]) => { const list = JSON.parse(ids as string) as string[]; asked.push(list); return list.filter(id => id !== known); } });
  assert.equal(await sweepOrphans(db, blobs), 1);
  assert.deepEqual(asked.flat().sort(), [known, orphan].sort());
  assert.deepEqual((await readdir(path.join(root, "blobs"))).sort(), [known, recent].sort());
});

test("legacy upload-ID blobs gain digest names, caches follow, and the old names go after the database switches", async t => {
  const root = await temporaryRoot(t), legacy = () => randomUUID();
  const [a, b, c, orphan] = [legacy(), legacy(), legacy(), legacy()];
  const write = (id: string, text: string) => writeFile(path.join(root, "blobs", id), text);
  await write(a, "alpha"); await write(b, "alpha"); await write(c, "gamma"); await write(orphan, "delta");
  const cache = async (name: string, id: string, suffix: string) => { await mkdir(path.join(root, name, id.slice(0, 2)), { recursive: true }); await writeFile(path.join(root, name, id.slice(0, 2), `${id}-${suffix}`), `${name} ${id}`); };
  await cache("photo-cache", a, "s.webp"); await cache("book-cache", c, "meta.json"); await cache("photo-cache", orphan, "s.webp");
  const gone = randomUUID(), plan = { done: false, blobs: [{ id: a, size: 5, digest: sha256("alpha"), required: true }, { id: b, size: 5, digest: sha256("alpha"), required: true }, { id: c, size: 5, digest: "", required: true }, { id: gone, size: -1, digest: "", required: false }] };
  let switched = false, failures = 1;
  const hashed: unknown[] = [];
  const db = fakeDatabase({
    service_blob_migration_plan: () => switched ? { done: true, blobs: [] } : plan,
    service_migrate_blobs: ([value]) => { if (failures-- > 0) throw new Error("database stopped"); hashed.push(JSON.parse(value as string)); switched = true; },
  });
  // A failure before the database switches leaves every upload-ID name, and the next start finishes.
  await assert.rejects(migrateBlobs(db, root), /database stopped/);
  for (const id of [a, b, c, orphan]) await stat(path.join(root, "blobs", id));
  assert.deepEqual(await migrateBlobs(db, root), { migrated: 3, removed: 4 });
  assert.deepEqual(hashed, [{ [c]: { digest: sha256("gamma"), size: 5 } }]);
  assert.deepEqual((await readdir(path.join(root, "blobs"))).sort(), [sha256("alpha"), sha256("gamma")].sort());
  assert.equal(await readFile(blobPath(root, sha256("gamma")), "utf8"), "gamma");
  const alpha = sha256("alpha"), gamma = sha256("gamma");
  assert.equal(await readFile(path.join(root, "photo-cache", alpha.slice(0, 2), `${alpha}-s.webp`), "utf8"), `photo-cache ${a}`);
  assert.equal(await readFile(path.join(root, "book-cache", gamma.slice(0, 2), `${gamma}-meta.json`), "utf8"), `book-cache ${c}`);
  await assert.rejects(stat(path.join(root, "photo-cache", orphan.slice(0, 2), `${orphan}-s.webp`)), /ENOENT/);
  assert.deepEqual(await migrateBlobs(db, root), { migrated: 0, removed: 0 });
});

test("the migration stops, changing nothing in the database, when referenced content is missing", async t => {
  const root = await temporaryRoot(t), missing = randomUUID();
  const db = fakeDatabase({ service_blob_migration_plan: () => ({ done: false, blobs: [{ id: missing, size: 3, digest: "", required: true }] }) });
  await assert.rejects(migrateBlobs(db, root), /is missing/);
});
