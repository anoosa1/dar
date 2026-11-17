import { constants } from "node:fs";
import { link, lstat, mkdir, open, readdir, rename, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import type { Database } from "../database.ts";
import { AppError } from "../errors.ts";
import { blobPath, isLegacyBlobId, legacyBlobPath } from "./blobs.ts";

/** required: versions or files use it; otherwise only Photos, Git blob IDs or pins name it, and it may be gone. */
interface Plan { done: boolean; blobs: { id: string; size: number; digest: string; required: boolean }[] }
/** Derived files keyed by blob ID as `<first two characters>/<blob ID>-<suffix>`. */
const caches = ["photo-cache", "book-cache"];

/**
 * Content-addressed storage (November 17, 2026): content stored under upload IDs gains its
 * digest name. Every step repeats safely after a crash. Hard links give each blob its digest name
 * first, the database then switches every reference in one transaction, and only then do the
 * upload-ID names go. Runs before the runtime serves requests, and does nothing once done.
 */
export async function migrateBlobs(db: Database, root: string): Promise<{ migrated: number; removed: number }> {
  const plan = await db.call<Plan>("service_blob_migration_plan");
  let migrated = 0;
  if (!plan.done) {
    const hashed: Record<string, { digest: string; size: number }> = {}, digests = new Map<string, string>();
    for (const blob of plan.blobs) {
      const source = legacyBlobPath(root, blob.id), stat = await lstat(source).catch(() => undefined);
      if (!stat && !blob.required) continue;
      if (!stat?.isFile()) throw new Error(`Stored content ${blob.id} is missing or not a regular file; restore it from a backup, then start Dar again.`);
      if (blob.size >= 0 && stat.size !== blob.size) throw new Error(`Stored content ${blob.id} is ${stat.size} bytes, not the ${blob.size} its record says.`);
      let digest = blob.digest;
      if (!digest) { digest = await sha256(source); hashed[blob.id] = { digest, size: stat.size }; }
      await linkOnce(source, blobPath(root, digest), stat.size);
      digests.set(blob.id, digest); migrated++;
    }
    await syncDirectory(path.join(root, "blobs"));
    for (const cache of caches) await renameCache(path.join(root, cache), digests);
    // A large database may answer after the request's time limit; the reducer is idempotent.
    for (let attempt = 1; ; attempt++) {
      try { await db.call("service_migrate_blobs", [JSON.stringify(hashed)]); break; }
      catch (error) { if (!(error instanceof AppError) || error.code !== "databaseUnavailable" || attempt >= 40) throw error; await delay(1500); }
    }
  }
  return { migrated, removed: await removeLegacyNames(root) };
}

/** After the database switched, upload-ID names are spare links or content nothing used. */
async function removeLegacyNames(root: string) {
  const directory = path.join(root, "blobs");
  let removed = 0;
  for (const name of await readdir(directory)) if (isLegacyBlobId(name)) { await unlink(path.join(directory, name)); removed++; }
  if (!removed) return 0;
  await syncDirectory(directory);
  for (const cache of caches) await renameCache(path.join(root, cache), new Map());
  return removed;
}

/** Moves cache files to their digest's name; with no digest they are dropped (caches regenerate). */
async function renameCache(cache: string, digests: Map<string, string>) {
  for (const prefix of await readdir(cache).catch(() => [] as string[])) {
    const directory = path.join(cache, prefix);
    if (!/^[a-f0-9]{2}$/.test(prefix) || !(await lstat(directory)).isDirectory()) continue;
    for (const name of await readdir(directory)) {
      const match = /^([a-f0-9-]{36})-(.+)$/.exec(name);
      if (!match || !isLegacyBlobId(match[1])) continue;
      const digest = digests.get(match[1]), source = path.join(directory, name);
      if (!digest) { await unlink(source); continue; }
      const target = path.join(cache, digest.slice(0, 2));
      await mkdir(target, { recursive: true, mode: 0o700 });
      await rename(source, path.join(target, `${digest}-${match[2]}`));
    }
  }
}

async function linkOnce(source: string, destination: string, size: number) {
  try { await link(source, destination); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    const existing = await lstat(destination);
    if (!existing.isFile() || existing.size !== size) throw new Error(`Stored content ${path.basename(destination)} does not match its other copy.`);
  }
}

async function sha256(file: string) {
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW), hash = createHash("sha256");
  try { for await (const chunk of handle.createReadStream({ autoClose: false })) hash.update(chunk as Buffer); }
  finally { await handle.close(); }
  return hash.digest("hex");
}

async function syncDirectory(directory: string) {
  const handle = await open(directory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { await handle.sync(); } finally { await handle.close(); }
}
