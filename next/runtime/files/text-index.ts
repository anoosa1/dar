import { constants, open } from "node:fs/promises";
import type { Database } from "../database.ts";
import { blobPath } from "./blobs.ts";
import { textIndexBytes, textIndexChars } from "../../shared/search.ts";

/**
 * Records the text of notes and other text files for the shared search (module/src/text-index.ts).
 * Each piece of content is read once, one at a time; `nudge` runs a pass after file writes and from
 * maintenance. Content that is not UTF-8 text is recorded as empty so it is not read again.
 */
export function textIndexWorker(db: Database, root: string) {
  let running: Promise<void> | undefined, again = false, stopped = false;
  const decoder = new TextDecoder("utf-8", { fatal: true });
  async function textOf(id: string) {
    let handle;
    try { handle = await open(blobPath(root, id), constants.O_RDONLY | constants.O_NOFOLLOW); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; }
    try {
      const stat = await handle.stat();
      if (!stat.isFile() || stat.size > textIndexBytes) return undefined;
      const bytes = Buffer.alloc(stat.size);
      let read = 0; while (read < stat.size) { const { bytesRead } = await handle.read(bytes, read, stat.size - read, read); if (!bytesRead) break; read += bytesRead; }
      let text: string;
      try { text = decoder.decode(bytes.subarray(0, read)); } catch { return { id, text: "" }; }
      // NUL bytes mean binary content with a text-like name.
      if (text.includes("\0")) return { id, text: "" };
      return { id, text: text.slice(0, textIndexChars) };
    } finally { await handle.close(); }
  }
  async function pass() {
    for (let after = ""; !stopped;) {
      const items = await db.read<{ id: string; size: number }[]>("service_blobs_without_text", [after, 50]);
      if (!items.length) return;
      const texts: { id: string; text: string }[] = [];
      for (const item of items) { if (stopped) return; const value = await textOf(item.id); if (value) texts.push(value); }
      // Keep each call well under the request limit.
      for (let start = 0; start < texts.length && !stopped;) {
        let size = 0, end = start;
        while (end < texts.length && (end === start || size + texts[end].text.length < 4 * 1024 * 1024)) size += texts[end++].text.length;
        await db.call("service_record_texts", [JSON.stringify(texts.slice(start, end))]);
        start = end;
      }
      after = items.at(-1)!.id;
    }
  }
  return {
    nudge() {
      if (stopped) return;
      if (running) { again = true; return; }
      running = (async () => { do { again = false; await pass(); } while (again && !stopped); })()
        .catch(() => { console.warn("Search text indexing will retry."); }).finally(() => { running = undefined; });
    },
    async close() { stopped = true; await running; },
  };
}
