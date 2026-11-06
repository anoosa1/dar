import { lstat, readFile, unlink } from "node:fs/promises";
import { hostname } from "node:os";
import path from "node:path";
import { createDurableFile } from "./atomic.ts";

/** Exactly one integration runtime may reconcile a local blob root. */
export async function lockRoot(root: string) {
  const filename = path.join(root, "runtime.lock");
  try {
    if (!(await lstat(filename)).isFile()) throw new Error("Unsafe runtime lock.");
    const record = JSON.parse(await readFile(filename, "utf8"));
    if (record.host !== hostname() || !Number.isSafeInteger(record.pid) || record.pid <= 0) throw new Error("Unrecognized runtime lock.");
    let alive = true;
    try { process.kill(record.pid, 0); } catch (error) { if ((error as NodeJS.ErrnoException).code === "ESRCH") alive = false; }
    if (alive) throw new Error("Another runtime owns this storage root.");
    await unlink(filename);
  } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  await createDurableFile(filename, JSON.stringify({ pid: process.pid, host: hostname() }));
  return async () => { await unlink(filename); };
}
