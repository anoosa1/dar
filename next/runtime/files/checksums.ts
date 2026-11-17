import { createHash } from "node:crypto";
import { constants, open } from "node:fs/promises";
import type { Database } from "../database.ts";
import { blobPath } from "./blobs.ts";

/**
 * Records the SHA-1 and MD5 of every stored blob that has none, for WebDAV clients that compare
 * only those (rclone, ownCloud, Nextcloud). One pass reads each such blob once, one at a time; afterwards
 * new content is hashed when `nudge` is called (after a WebDAV upload, and by maintenance).
 * Dar never names or trusts content by either.
 */
export function checksumWorker(db: Database, root: string) {
  let running: Promise<void> | undefined, again = false, stopped = false;
  async function checksumsOf(id: string) {
    let handle;
    try { handle = await open(blobPath(root, id), constants.O_RDONLY | constants.O_NOFOLLOW); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; }
    try {
      if (!(await handle.stat()).isFile()) return undefined;
      const sha1 = createHash("sha1"), md5 = createHash("md5");
      for await (const chunk of handle.createReadStream({ highWaterMark: 1 << 20, autoClose: false })) { if (stopped) return undefined; sha1.update(chunk as Buffer); md5.update(chunk as Buffer); }
      return { id, sha1: sha1.digest("hex"), md5: md5.digest("hex") };
    } finally { await handle.close(); }
  }
  async function pass() {
    // In ID order, so content that cannot be read (a removal in progress) never blocks the rest.
    for (let after = ""; !stopped;) {
      const ids = await db.read<string[]>("service_blobs_without_checksum", [after, 50]);
      if (!ids.length) return;
      const items: { id: string; sha1: string; md5: string }[] = [];
      for (const id of ids) { if (stopped) return; const sums = await checksumsOf(id); if (sums) items.push(sums); }
      if (items.length && !stopped) await db.call("service_record_checksums", [JSON.stringify(items)]);
      after = ids.at(-1)!;
    }
  }
  return {
    nudge() {
      if (stopped) return;
      if (running) { again = true; return; }
      running = (async () => { do { again = false; await pass(); } while (again && !stopped); })()
        .catch(() => { console.warn("Content checksums will retry."); }).finally(() => { running = undefined; });
    },
    async close() { stopped = true; await running; },
  };
}
