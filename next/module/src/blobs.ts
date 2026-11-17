import {SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {blobGraceMs} from "../../shared/limits.ts";

/**
 * Stored content is named by the SHA-256 of its bytes (lowercase hex), so a file uploaded twice
 * is kept once. fileBlobs records each blob's size and state; versions and backup pins are what
 * keep it. See docs/spacetimedb/architecture.md.
 */
export const isBlobId=(value:string)=>/^[a-f0-9]{64}$/.test(value);
/** Content stored under its upload ID, before content addressing (November 17, 2026). */
export const isLegacyBlobId=(value:string)=>/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value);
/** fileBlobs.checkKey: when maintenance should next see whether anything uses the blob ("" never). */
export const checkKeyAt=(time:number)=>String(Math.max(0,Math.floor(time))).padStart(16,"0");
const nowOf=(ctx:Ctx)=>Number(ctx.timestamp.toMillis());

/**
 * Records that the runtime stored this content and something may use it. New content is checked
 * once its grace has passed, which also reclaims content a transfer stored but never used. Content
 * maintenance is reclaiming is kept again: the runtime removes only blobs nothing linked recently,
 * and maintenance acknowledges only blobs still claimed.
 */
export function storeBlob(ctx:Ctx,id:string,size:number){
  if(!isBlobId(id)||!Number.isSafeInteger(size)||size<0)throw new SenderError("invalidInput");
  const row=ctx.db.fileBlobs.id.find(id),later=checkKeyAt(nowOf(ctx)+blobGraceMs);
  if(!row){ctx.db.fileBlobs.insert({id,size,state:"stored",checkKey:later});return;}
  if(row.size!==size)throw new SenderError("conflict");
  if(row.state!=="stored")ctx.db.fileBlobs.id.update({...row,state:"stored",checkKey:later});
}
/** A version or pin using this content went away; maintenance checks whether anything still does. */
export function releaseBlob(ctx:Ctx,id:string){
  const row=ctx.db.fileBlobs.id.find(id);
  if(row&&row.state==="stored"&&(!row.checkKey||row.checkKey>checkKeyAt(nowOf(ctx))))ctx.db.fileBlobs.id.update({...row,checkKey:checkKeyAt(nowOf(ctx))});
}
