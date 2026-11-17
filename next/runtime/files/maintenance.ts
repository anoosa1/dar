import type { Database } from "../database.ts";
import type { blobStore } from "./blobs.ts";

/** Claim blobs no version or backup pin uses, transactionally, then remove and acknowledge them. */
export async function reconcileStorage(db: Database, blobs: ReturnType<typeof blobStore>): Promise<number> {
  const candidates = await db.call<string[]>("service_maintenance");
  let removed = 0;
  for (const id of candidates) if (await blobs.remove(id)) { await db.call("service_acknowledge_gc", [id]); removed++; }
  return removed;
}

/**
 * Removes stored content the database never recorded: an upload stored it and then failed, or
 * the runtime stopped before recording it. Only content past its grace period is considered, and
 * remove() checks the grace again, so content an upload is recording meanwhile stays.
 */
export async function sweepOrphans(db: Database, blobs: ReturnType<typeof blobStore>): Promise<number> {
  let removed = 0;
  for await (const page of blobs.settled()) {
    for (const id of await db.read<string[]>("service_unknown_blobs", [JSON.stringify(page)])) if (await blobs.remove(id)) removed++;
  }
  return removed;
}
