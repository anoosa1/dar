import { constants } from "node:fs";
import { link, open, unlink, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pipeline } from "node:stream/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import path from "node:path";
import { AppError } from "../errors.ts";

export interface BlobFile { id: string; name: string; blobId: string; size: number; mediaType: string; revision: number }
export function blobPath(root: string, id: string) {
  if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(id)) throw new AppError("invalidBlobId");
  return path.join(root, "blobs", id);
}

/** One receive per upload ID. Final blobs are linked into place without overwrite. */
export function blobStore(root: string) {
  const active = new Set<string>();
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
      const directory = await open(path.join(root, "blobs"), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
      try { await directory.sync(); } finally { await directory.close(); }
      return true;
    },
    async receive(req: IncomingMessage, id: string, expectedSize: number) {
      const destination = blobPath(root, id);
      if (active.size >= 4 || active.has(id)) throw new AppError("uploadBusy", 409);
      active.add(id);
      const temporary = path.join(root, "uploads", `${id}.tmp`);
      let file: Awaited<ReturnType<typeof open>> | undefined;
      try {
        file = await open(temporary, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
        let size = 0;
        const hash = createHash("sha256");
        req.setTimeout(60_000, () => req.destroy(new Error("Upload stalled.")));
        for await (const chunk of req) {
          size += chunk.length;
          if (size > expectedSize) throw new AppError("uploadTooLarge", 413);
          hash.update(chunk);
          let offset = 0;
          while (offset < chunk.length) offset += (await file.write(chunk, offset, chunk.length - offset)).bytesWritten;
        }
        if (size !== expectedSize) throw new AppError("incompleteUpload", 400);
        await file.sync(); await file.close(); file = undefined;
        await link(temporary, destination);
        const directory = await open(path.join(root, "blobs"), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
        try { await directory.sync(); } finally { await directory.close(); }
        return hash.digest("hex");
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
        const inline = preview && /^(image\/(png|jpeg|gif|webp)|text\/plain|application\/pdf)$/.test(entry.mediaType);
        res.setHeader("content-type", inline ? entry.mediaType : "application/octet-stream");
        res.setHeader("content-disposition", `${inline ? "inline" : "attachment"}; filename*=UTF-8''${encodeURIComponent(entry.name).replace(/['()*]/g, c => `%${c.charCodeAt(0).toString(16)}`)}`);
        if (inline) res.setHeader("content-security-policy", "sandbox; default-src 'none'");
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
