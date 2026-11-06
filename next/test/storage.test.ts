import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, readFile, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { openRoot } from "../runtime/root.ts";
import { lockRoot } from "../runtime/lock.ts";
import { blobStore } from "../runtime/files/blobs.ts";
import { randomUUID } from "node:crypto";

test("managed storage refuses unknown roots and symlink ancestors without modifying their targets", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-proof-"));
  const unknown = path.join(directory, "unknown"); await mkdir(unknown); await writeFile(path.join(unknown, "keep.txt"), "preserve this");
  await assert.rejects(openRoot(unknown), /unknown non-empty/);
  assert.equal(await readFile(path.join(unknown, "keep.txt"), "utf8"), "preserve this");
  const real = path.join(directory, "real"); await mkdir(real);
  await symlink(real, path.join(directory, "alias"));
  await assert.rejects(openRoot(path.join(directory, "alias", "child")), /symlinks/);
});
test("a root has one owner; blob reads fail closed on symlinks", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "dar-storage-lock-"));
  const root = await openRoot(directory), release = await lockRoot(root);
  await assert.rejects(lockRoot(root), /Another runtime/);
  await release(); const again = await lockRoot(root); await again();
  const target = path.join(root, "keep.txt"); await writeFile(target, "do not read through the blob symlink");
  const id = randomUUID(); await symlink(target, path.join(root, "blobs", id));
  await assert.rejects(blobStore(root).existingDigest(id, 36));
  assert.equal(await readFile(target, "utf8"), "do not read through the blob symlink");
});
