import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, readFile, symlink, realpath, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { openRoot } from "../runtime/root.ts";
import { lockRoot } from "../runtime/lock.ts";
import { blobStore } from "../runtime/files/blobs.ts";
import { randomUUID, createHash } from "node:crypto";

test("managed storage refuses unknown roots and symlinked roots without modifying their targets", async t => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-proof-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const unknown = path.join(directory, "unknown"); await mkdir(unknown); await writeFile(path.join(unknown, "keep.txt"), "preserve this");
  await assert.rejects(openRoot(unknown), /unknown non-empty/);
  assert.equal(await readFile(path.join(unknown, "keep.txt"), "utf8"), "preserve this");
  const real = path.join(directory, "real"); await mkdir(real);
  await symlink(real, path.join(directory, "alias"));
  await assert.rejects(openRoot(path.join(directory, "alias")), /symlink/);
  assert.deepEqual(await readdir(real), []);
});
test("host directory aliases resolve to one persistent canonical storage root", async t => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-alias-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const real = path.join(directory, "data", "lib"); await mkdir(real, { recursive: true });
  const alias = path.join(directory, "lib"); await symlink(real, alias);
  const root = await openRoot(path.join(alias, "dar"));
  assert.equal(root, path.join(await realpath(real), "dar"));
  assert.deepEqual(JSON.parse(await readFile(path.join(root, "instance.json"), "utf8")), { format: "dar-spacetime-v1" });
  await writeFile(path.join(root, "blobs", "keep.txt"), "existing blob");
  assert.equal(await openRoot(path.join(alias, "dar")), root);
  assert.equal(await openRoot(path.join(real, "dar")), root);
  assert.equal(await readFile(path.join(root, "blobs", "keep.txt"), "utf8"), "existing blob");
  assert.equal(await openRoot(path.join(alias, "new", "nested", "dar")), path.join(await realpath(real), "new", "nested", "dar"));
});
test("managed directories reject symlinks beneath a resolved host alias", async t => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-managed-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const real = path.join(directory, "real"); await mkdir(real);
  const alias = path.join(directory, "alias"); await symlink(real, alias);
  const root = await openRoot(path.join(alias, "dar"));
  const target = path.join(directory, "target"); await mkdir(target);
  for (const name of ["secrets", "blobs", "uploads"]) {
    const managed = path.join(root, name);
    await rm(managed, { recursive: true });
    await symlink(target, managed);
    await assert.rejects(openRoot(path.join(alias, "dar")), /Unsafe managed storage directory/);
    assert.deepEqual(await readdir(target), []);
    await rm(managed); await mkdir(managed);
  }
});
test("a root has one owner; blob reads fail closed on symlinks", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-lock-"));
  const root = await openRoot(directory), release = await lockRoot(root);
  await assert.rejects(lockRoot(root), /Another runtime/);
  await release(); const again = await lockRoot(root); await again();
  const target = path.join(root, "keep.txt"); await writeFile(target, "do not read through the blob symlink");
  // A symlink under the digest of incoming content is neither followed nor replaced nor removed.
  const content = "content whose digest names the symlink", id = createHash("sha256").update(content).digest("hex");
  await symlink(target, path.join(root, "blobs", id));
  const blobs = blobStore(root, { graceMs: 0 });
  await assert.rejects(blobs.receive((async function* () { yield Buffer.from(content); })(), randomUUID(), Buffer.byteLength(content)));
  await assert.rejects(blobs.remove(id));
  assert.equal(await readFile(target, "utf8"), "do not read through the blob symlink");
});
test("a batch of blobs is stored with each digest, flushed together or one by one, and leaves nothing on failure", async t => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-batch-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = await openRoot(path.join(directory, "dar")), blobs = blobStore(root);
  const { createHash } = await import("node:crypto");
  const items = (contents: string[]) => contents.map(text => ({ id: randomUUID(), size: Buffer.byteLength(text), text }));
  const feed = (list: ReturnType<typeof items>) => (async function* () { for (const item of list) yield { id: item.id, size: item.size, source: (async function* () { yield Buffer.from(item.text); })() }; })();
  // Fewer than 16 files are flushed one by one; more take one file-system flush.
  for (const count of [3, 40]) {
    const list = items(Array.from({ length: count }, (_, index) => `blob ${count}/${index}\n`.repeat(index + 1)));
    const digests = await blobs.receiveBatch(feed(list));
    for (const item of list) {
      const digest = createHash("sha256").update(item.text).digest("hex");
      assert.equal(digests.get(item.id), digest);
      assert.equal(await readFile(path.join(root, "blobs", digest), "utf8"), item.text);
    }
  }
  assert.deepEqual(await readdir(path.join(root, "uploads")), []);
  // A stream shorter than its size fails the batch before anything is linked.
  const bad = items(["complete and new\n", "short\n"]); bad[1].size += 10;
  await assert.rejects(blobs.receiveBatch(feed(bad)), /incompleteUpload/);
  assert.deepEqual(await readdir(path.join(root, "uploads")), []);
  for (const item of bad) await assert.rejects(readFile(path.join(root, "blobs", createHash("sha256").update(item.text).digest("hex"))), /ENOENT/);
});
