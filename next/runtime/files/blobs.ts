import { constants } from "node:fs";
import { link, lstat, open, opendir, unlink, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pipeline } from "node:stream/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import path from "node:path";
import { AppError } from "../errors.ts";
import { blobGraceMs } from "../../shared/limits.ts";

export interface BlobFile { id: string; name: string; blobId: string; size: number; mediaType: string; revision: number; /** Inside the Locked folder (set by downloads through Files). */ locked?: boolean }
/** Stored content is named by the SHA-256 of its bytes, in lowercase hex. */
export const isBlobId = (id: string) => /^[a-f0-9]{64}$/.test(id);
/** Content stored under its upload ID before content addressing (November 17, 2026). */
export const isLegacyBlobId = (id: string) => /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(id);
export function blobPath(root: string, id: string) {
  if (!isBlobId(id)) throw new AppError("invalidBlobId");
  return path.join(root, "blobs", id);
}
/** Only the blob migration (files/migrate.ts) and restores of older backups read upload-ID names. */
export function legacyBlobPath(root: string, id: string) {
  if (!isLegacyBlobId(id)) throw new AppError("invalidBlobId");
  return path.join(root, "blobs", id);
}

// Storing content under a digest and removing that digest never overlap.
const turns = new Map<string, Promise<unknown>>();
function inTurn<T>(key: string, work: () => Promise<T>): Promise<T> {
  const result = (turns.get(key) ?? Promise.resolve()).then(work), settled = result.catch(() => {});
  turns.set(key, settled);
  void settled.then(() => { if (turns.get(key) === settled) turns.delete(key); });
  return result;
}

/**
 * One receive per upload ID. A receive hashes into a temporary file, then links it under its
 * digest, or keeps the identical content already stored and marks it used now. The database
 * records the digest afterwards, so content linked or used within graceMs is never removed;
 * maintenance reclaims what no reference ever reached (files/maintenance.ts).
 */
export function blobStore(root: string, options: { graceMs?: number } = {}) {
  const active = new Set<string>(), graceMs = options.graceMs ?? blobGraceMs, directory = path.join(root, "blobs");
  async function syncDirectory() {
    const handle = await open(directory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try { await handle.sync(); } finally { await handle.close(); }
  }
  /** Stores a complete, flushed temporary file under its digest. */
  function store(temporary: string, digest: string, size: number) {
    const destination = blobPath(root, digest);
    return inTurn(destination, async () => {
      try { await link(temporary, destination); await syncDirectory(); return; }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error; }
      const existing = await open(destination, constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const stat = await existing.stat();
        if (!stat.isFile() || stat.size !== size) throw new AppError("contentUnavailable", 503);
        const at = new Date(); await existing.utimes(at, at);
      } finally { await existing.close(); }
    });
  }
  return {
    async recover() {
      for (const name of await readdir(path.join(root, "uploads"))) {
        if (!/^[a-f0-9-]{36}\.tmp$/.test(name)) throw new Error("Unrecognized temporary upload file.");
        await unlink(path.join(root, "uploads", name));
      }
    },
    /**
     * Removes content nothing linked or used within the grace period. Returns false when it was
     * kept because an upload may be about to record it; maintenance asks again later.
     */
    async remove(id: string): Promise<boolean> {
      const target = blobPath(root, id);
      return inTurn(target, async () => {
        let stat;
        try { stat = await lstat(target); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return true; throw error; }
        if (!stat.isFile()) throw new Error("Unexpected blob file.");
        if (Date.now() - stat.mtimeMs < graceMs) return false;
        await unlink(target); await syncDirectory();
        return true;
      });
    },
    /** Stored content past its grace period, a page of IDs at a time (for the orphan sweep). */
    async *settled(pageSize = 1000): AsyncGenerator<string[]> {
      let page: string[] = [];
      for await (const entry of await opendir(directory)) {
        if (!isBlobId(entry.name)) continue;
        const stat = await lstat(path.join(directory, entry.name)).catch(() => undefined);
        if (!stat?.isFile() || Date.now() - stat.mtimeMs < graceMs) continue;
        page.push(entry.name);
        if (page.length >= pageSize) { yield page; page = []; }
      }
      if (page.length) yield page;
    },
    /** Receives a request body, or a local stream (a staged Photos upload or import); returns its SHA-256. */
    async receive(req: IncomingMessage | AsyncIterable<Buffer>, id: string, expectedSize: number): Promise<string> {
      if (active.size >= 4 || active.has(id)) throw new AppError("uploadBusy", 409);
      active.add(id);
      const temporary = path.join(root, "uploads", `${id}.tmp`);
      let file: Awaited<ReturnType<typeof open>> | undefined;
      try {
        file = await open(temporary, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
        let size = 0;
        const hash = createHash("sha256");
        if ("setTimeout" in req) req.setTimeout(60_000, () => req.destroy(new Error("Upload stalled.")));
        for await (const chunk of req) {
          size += chunk.length;
          if (size > expectedSize) throw new AppError("uploadTooLarge", 413);
          hash.update(chunk);
          let offset = 0;
          while (offset < chunk.length) offset += (await file.write(chunk, offset, chunk.length - offset)).bytesWritten;
        }
        if (size !== expectedSize) throw new AppError("incompleteUpload", 400);
        await file.sync(); await file.close(); file = undefined;
        const digest = hash.digest("hex");
        await store(temporary, digest, expectedSize);
        return digest;
      } finally {
        await file?.close();
        await unlink(temporary).catch(error => { if (error.code !== "ENOENT") throw error; });
        active.delete(id);
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
