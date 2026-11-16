import {open} from "node:fs/promises";
import {constants} from "node:fs";
import type {Database} from "../database.ts";
import {blobPath} from "../files/blobs.ts";
import type {NotesFile,NotesFolder,NotesLibrary} from "../../shared/workspace.ts";

interface Row {id:string;parentId:string;name:string;blobId:string;size:number;revision:number;modifiedAt:number;favorite:boolean;path:string}
interface Listing {rootId:string;path:string;folders:NotesFolder[];files:Row[];truncated:boolean;legacy:number}
const head=8192,fullLimit=1024*1024;

async function read(root:string,blobId:string,bytes:number){
  if(!blobId)return "";
  const file=await open(blobPath(root,blobId),constants.O_RDONLY|constants.O_NOFOLLOW);
  try{const buffer=Buffer.alloc(bytes);const {bytesRead}=await file.read(buffer,0,bytes,0);return buffer.subarray(0,bytesRead).toString("utf8").replace(/�$/,"");}
  finally{await file.close();}
}
/** Title from front matter or the first heading, else the file name; excerpt is the first prose as plain text. */
export function summarize(name:string,text:string){
  let body=text.replace(/^﻿/,""),title="";
  const front=/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(body);
  if(front){title=/^title:\s*["']?(.+?)["']?\s*$/m.exec(front[1])?.[1]??"";body=body.slice(front[0].length);}
  const heading=/^#{1,6}\s+(.+?)\s*#*\s*$/m.exec(body);
  if(!title&&heading&&body.slice(0,heading.index).trim()==="")title=heading[1];
  // List markers and checkboxes are dropped per line, before lines are joined.
  const plain=body.split(/\r?\n/).filter(line=>!/^#{1,6}\s/.test(line)&&!/^(```|~~~)/.test(line)).map(line=>line.replace(/^\s*(?:>\s*)?(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/,"")).join(" ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g,"").replace(/\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g,(_m,target,label)=>label??target).replace(/\[([^\]]+)\]\([^)]*\)/g,"$1")
    .replace(/[*_`~>#]+/g,"").replace(/\s+/g," ").trim();
  return {title:(title||name.replace(/\.(md|markdown)$/i,"")).slice(0,200),excerpt:plain.slice(0,240)};
}

/**
 * The Notes library over Files: the module lists the Notes folder, this reads note heads from
 * immutable blobs (cached by blob ID, so a cached summary can never be stale) and filters by query.
 */
export function createNotesLibrary(db:Database,root:string){
  const summaries=new Map<string,{title:string;excerpt:string}>();
  async function summary(row:Row){
    const cached=summaries.get(row.blobId);if(cached)return cached;
    const value=summarize(row.name,await read(root,row.blobId,Math.min(head,row.size)).catch(()=>""));
    if(summaries.size>=20000)summaries.delete(summaries.keys().next().value!);
    summaries.set(row.blobId,value);return value;
  }
  async function library(sessionId:string,query=""):Promise<NotesLibrary>{
    const listing=await db.call<Listing>("service_notes",[sessionId,"library"]);
    const q=query.normalize("NFC").toLowerCase().trim(),notes:NotesFile[]=[];
    let next=0;
    await Promise.all(Array.from({length:8},async()=>{while(next<listing.files.length){
      const row=listing.files[next++],{title,excerpt}=await summary(row);
      if(q&&!`${row.name}\n${title}\n${row.path}`.toLowerCase().includes(q)){
        // Search the whole note only when the name and heading do not match.
        const text=row.size<=fullLimit?await read(root,row.blobId,row.size).catch(()=>""):"";
        if(!text.normalize("NFC").toLowerCase().includes(q))continue;
      }
      notes.push({id:row.id,parentId:row.parentId,name:row.name,title,excerpt,size:row.size,revision:row.revision,modifiedAt:row.modifiedAt,favorite:row.favorite,path:row.path});
    }}));
    notes.sort((a,b)=>b.modifiedAt-a.modifiedAt||a.title.localeCompare(b.title));
    return {rootId:listing.rootId,path:listing.path,folders:listing.folders,notes,truncated:listing.truncated,legacy:listing.legacy};
  }
  return {library};
}
export type NotesLibraryRuntime=ReturnType<typeof createNotesLibrary>;
