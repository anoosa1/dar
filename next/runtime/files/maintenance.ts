import type { Database } from "../database.ts";
import type { blobStore } from "./blobs.ts";

/** Claim unreferenced expired blobs transactionally, then remove and acknowledge. */
export async function reconcileStorage(db: Database, blobs: ReturnType<typeof blobStore>): Promise<number> {
  const candidates = await db.call<string[]>("service_maintenance");
  let removed = 0;
  for (const id of candidates) if (await blobs.remove(id)) { await db.call("service_acknowledge_gc", [id]); removed++; }
  return removed;
}
