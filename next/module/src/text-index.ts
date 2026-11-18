import {table,t,SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {isBlobId} from "./blobs.ts";
import {isTextFile,textIndexBytes,textIndexChars} from "../../shared/search.ts";

/**
 * The text of stored content, for searching inside notes and text files. Like blobChecksums it
 * belongs to the blob (identical files share it), the runtime fills it (files/text-index.ts) and
 * it goes when the blob does. Not in backups: it is rebuilt from the content. An empty text
 * marks content that is not readable text, so it is not read again.
 */
export const blobTexts=table({}, {id:t.string().primaryKey(),text:t.string()});

/** Text-like content that current files use and that has no text yet, in blob ID order after `after`. */
export function blobsWithoutText(ctx:Ctx,after:string,limit:number){
  if(limit<1||limit>200)throw new SenderError("invalidInput");
  const ids=new Map<string,number>();
  for(const entry of ctx.db.fileEntries.iter()){
    if(entry.kind!=="file"||entry.trashed||!isBlobId(entry.blobId)||entry.blobId<=after||entry.size>textIndexBytes)continue;
    if(!isTextFile(entry.name,entry.mediaType)||ctx.db.blobTexts.id.find(entry.blobId))continue;
    ids.set(entry.blobId,entry.size);
  }
  return [...ids].sort(([a],[b])=>a<b?-1:a>b?1:0).slice(0,limit).map(([id,size])=>({id,size}));
}
export function recordTexts(ctx:Ctx,items:unknown){
  if(!Array.isArray(items)||items.length>200)throw new SenderError("invalidInput");
  for(const item of items as {id?:unknown;text?:unknown}[]){
    if(typeof item?.id!=="string"||!isBlobId(item.id)||typeof item.text!=="string"||item.text.length>textIndexChars)throw new SenderError("invalidInput");
    // Content collected while it was read gets no row.
    if(ctx.db.fileBlobs.id.find(item.id)&&!ctx.db.blobTexts.id.find(item.id))ctx.db.blobTexts.insert({id:item.id,text:item.text});
  }
}
/** A short excerpt around the first match, for the search result's detail line. */
export function excerpt(text:string,q:string){
  const at=text.normalize("NFC").toLowerCase().indexOf(q);if(at<0)return "";
  const start=Math.max(0,at-50),end=Math.min(text.length,at+q.length+70);
  return `${start>0?"…":""}${text.slice(start,end).replace(/\s+/g," ").trim()}${end<text.length?"…":""}`;
}
