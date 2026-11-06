import { constants } from "node:fs";
import { open, link, unlink } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import path from "node:path";

/** Publish a new durable file without replacing an existing file or following it. */
export async function createDurableFile(filename: string, content: string) {
  const temporary = `${filename}.${randomUUID()}.tmp`;
  const file = await open(temporary, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW, 0o600);
  try {
    await file.writeFile(content); await file.sync(); await file.close();
    await link(temporary, filename);
    const parent = await open(path.dirname(filename), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try { await parent.sync(); } finally { await parent.close(); }
  } finally { await file.close(); await unlink(temporary); }
}
