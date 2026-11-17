import { constants } from "node:fs";
import { link, open, unlink, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pipeline } from "node:stream/promises";
import { spawn } from "node:child_process";
import type { IncomingMessage, ServerResponse } from "node:http";
import path from "node:path";
import { AppError } from "../errors.ts";

export interface BlobFile { id: string; name: string; blobId: string; size: number; mediaType: string; revision: number; /** Inside the Locked folder (set by downloads through Files). */ locked?: boolean }
export function blobPath(root: string, id: string) {
  if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(id)) throw new AppError("invalidBlobId");
  return path.join(root, "blobs", id);
}

/**
 * syncfs(2), which Node does not offer, through coreutils' sync: flushes every file written on the
 * file system holding `target`. Resolves false when it could not run, so the caller flushes each file.
 */
function syncFileSystem(target: string) {
  return new Promise<boolean>(resolve => {
    const child = spawn("sync", ["--file-system", target], { stdio: "ignore", env: { PATH: process.env.PATH ?? "" } });
    child.once("error", () => resolve(false)); child.once("close", code => resolve(code === 0));
  });
}

/** One receive per upload ID. Final blobs are linked into place without overwrite. */
export function blobStore(root: string) {
  // IDs being received (never removed meanwhile), and how many receives run at once.
  const active = new Set<string>();
  let receivers = 0;
  const syncDirectory = async () => {
    const directory = await open(path.join(root, "blobs"), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try { await directory.sync(); } finally { await directory.close(); }
  };
  /** Writes one stream to its temporary file, checking its size; returns the open file and digest. */
  async function stage(source: AsyncIterable<Buffer>, temporary: string, expectedSize: number) {
    const file = await open(temporary, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
    try {
      let size = 0;
      const hash = createHash("sha256");
      for await (const chunk of source) {
        size += chunk.length;
        if (size > expectedSize) throw new AppError("uploadTooLarge", 413);
        hash.update(chunk);
        let offset = 0;
        while (offset < chunk.length) offset += (await file.write(chunk, offset, chunk.length - offset)).bytesWritten;
      }
      if (size !== expectedSize) throw new AppError("incompleteUpload", 400);
      return { file, digest: hash.digest("hex") };
    } catch (error) { await file.close(); throw error; }
  }
  return {
    async recover() {
      for (const name of await readdir(path.join(root, "uploads"))) {
        if (!/^[a-f0-9-]{36}\.tmp$/.test(name)) throw new Error("Unrecognized temporary upload file.");
        await unlink(path.join(root, "uploads", name));
      }
    },
    async existingDigest(id: string, expectedSize: number): Promise<string | undefined> {
      let file;
      try { file = await open(blobPath(root, id), constants.O_RDONLY | constants.O_NOFOLLOW); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; }
      try {
        const stat = await file.stat();
        if (!stat.isFile() || stat.size !== expectedSize) throw new AppError("contentUnavailable", 503);
        const hash = createHash("sha256"), buffer = Buffer.alloc(128 * 1024);
        let count: number;
        do { count = (await file.read(buffer)).bytesRead; hash.update(buffer.subarray(0, count)); } while (count);
        return hash.digest("hex");
      } finally { await file.close(); }
    },
    async remove(id: string): Promise<boolean> {
      if (active.has(id)) return false;
      await unlink(blobPath(root, id)).catch(error => { if (error.code !== "ENOENT") throw error; });
      await syncDirectory();
      return true;
    },
    /** Receives a request body, or a local stream (a staged Photos upload or import). */
    async receive(req: IncomingMessage | AsyncIterable<Buffer>, id: string, expectedSize: number) {
      const destination = blobPath(root, id);
      if (receivers >= 4 || active.has(id)) throw new AppError("uploadBusy", 409);
      active.add(id); receivers++;
      const temporary = path.join(root, "uploads", `${id}.tmp`);
      let file: Awaited<ReturnType<typeof open>> | undefined;
      try {
        if ("setTimeout" in req) req.setTimeout(60_000, () => req.destroy(new Error("Upload stalled.")));
        const staged = await stage(req, temporary, expectedSize);
        file = staged.file;
        await file.sync(); await file.close(); file = undefined;
        await link(temporary, destination);
        await syncDirectory();
        return staged.digest;
      } finally {
        await file?.close();
        await unlink(temporary).catch(error => { if (error.code !== "ENOENT") throw error; });
        active.delete(id); receivers--;
      }
    },
    /**
     * Receives local streams one after another, as a Git push hands over its files, and makes them
     * durable together (one file-system flush and one directory flush) instead of two flushes in turn
     * per file. Returns each ID's SHA-256 once all are stored. On failure no temporary file remains;
     * blobs already linked expire with their uploads, like any unfinished upload.
     */
    async receiveBatch(items: AsyncIterable<{ id: string; size: number; source: AsyncIterable<Buffer> }>) {
      if (receivers >= 4) throw new AppError("uploadBusy", 409);
      receivers++;
      const digests = new Map<string, string>(), staged: { id: string; temporary: string }[] = [];
      try {
        for await (const item of items) {
          if (active.has(item.id) || digests.has(item.id)) throw new AppError("uploadBusy", 409);
          const temporary = path.join(root, "uploads", `${item.id}.tmp`);
          active.add(item.id); staged.push({ id: item.id, temporary });
          const { file, digest } = await stage(item.source, temporary, item.size);
          await file.close(); digests.set(item.id, digest);
        }
        // Many files: one flush of the file system. Few, or when that is unavailable: each file.
        if (staged.length < 16 || !await syncFileSystem(path.join(root, "uploads"))) {
          for (const { temporary } of staged) {
            const file = await open(temporary, constants.O_RDONLY | constants.O_NOFOLLOW);
            try { await file.sync(); } finally { await file.close(); }
          }
        }
        for (const { id, temporary } of staged) await link(temporary, blobPath(root, id));
        if (staged.length) await syncDirectory();
        return digests;
      } finally {
        for (const { id, temporary } of staged) { await unlink(temporary).catch(error => { if (error.code !== "ENOENT") throw error; }); active.delete(id); }
        receivers--;
      }
    },
    async download(req: IncomingMessage, res: ServerResponse, entry: BlobFile, preview = false) {
      const file = await open(blobPath(root, entry.blobId), constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const stat = await file.stat();
        if (!stat.isFile() || stat.size !== entry.size) throw new AppError("contentUnavailable", 503);
        const etag = `"${entry.id}:${entry.revision}:${entry.blobId}"`;
        res.setHeader("etag", etag);
        res.setHeader("cache-control", "private, no-store");
        res.setHeader("accept-ranges", "bytes");
        // Only types browsers render without running the file's own script; SVG and HTML always download.
        const inline = preview && /^(image\/(png|jpeg|gif|webp|avif)|text\/plain|application\/pdf|video\/(mp4|webm|ogg)|audio\/(mpeg|mp4|ogg|wav|webm|flac|aac|x-wav|x-flac))$/.test(entry.mediaType);
        res.setHeader("content-type", inline ? entry.mediaType : "application/octet-stream");
        res.setHeader("content-disposition", `${inline ? "inline" : "attachment"}; filename*=UTF-8''${encodeURIComponent(entry.name).replace(/['()*]/g, c => `%${c.charCodeAt(0).toString(16)}`)}`);
        if (inline) res.setHeader("content-security-policy", "sandbox; default-src 'none'; frame-ancestors 'self'");
        if (req.headers["if-none-match"] === etag) { res.writeHead(304); res.end(); return; }
        let start = 0, end = entry.size - 1, status = 200;
        if (req.headers.range && (!req.headers["if-range"] || req.headers["if-range"] === etag)) {
          const match = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range);
          if (!match || (!match[1] && !match[2]) || entry.size === 0) throw new AppError("invalidRange", 416);
          if (!match[1]) { const suffix = Number(match[2]); if (!Number.isSafeInteger(suffix) || suffix <= 0) throw new AppError("invalidRange", 416); start = Math.max(0, entry.size - suffix); }
          else { start = Number(match[1]); end = match[2] ? Math.min(Number(match[2]), end) : end; }
          if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || start > end || start >= entry.size) { res.setHeader("content-range", `bytes */${entry.size}`); throw new AppError("invalidRange", 416); }
          status = 206; res.setHeader("content-range", `bytes ${start}-${end}/${entry.size}`);
        }
        res.setHeader("content-length", entry.size ? end - start + 1 : 0);
        res.writeHead(status);
        if (req.method === "HEAD" || !entry.size) { res.end(); return; }
        await pipeline(file.createReadStream({ start, end, autoClose: true }), res);
      } finally { await file.close(); }
    },
  };
}
