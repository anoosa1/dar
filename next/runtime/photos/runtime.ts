import {mkdir} from "node:fs/promises";
import path from "node:path";
import type {Database} from "../database.ts";
import {blobPath} from "../files/blobs.ts";
import {readFacts,makeThumbnail,cachePath,type ThumbnailSize} from "./media.ts";

export interface PhotoJob {fileId:string;blobId:string;name:string;mediaType:string;modifiedAt:number;kind:"image"|"video"}

/**
 * Owns Photos background work: a bounded scan of Files for new or replaced media,
 * fact extraction and thumbnails. Derived images live in a regenerable cache keyed
 * by immutable blob ID. Closing stops the loop; in-flight tools finish or time out.
 */
export async function createPhotosRuntime(db:Database,root:string){
  const cache=path.join(root,"photo-cache");await mkdir(cache,{recursive:true,mode:0o700});
  let closing=false,timer:NodeJS.Timeout|undefined,running:Promise<void>|undefined;
  const inflight=new Map<string,Promise<boolean>>();let active=0;const waiting:(()=>void)[]=[];
  // Media tools are CPU heavy: two at a time across scans and on-demand requests.
  async function slot<T>(work:()=>Promise<T>){
    if(active>=2)await new Promise<void>(resolve=>waiting.push(resolve));active++;
    try{return await work();}finally{active--;waiting.shift()?.();}
  }
  function thumbnail(blobId:string,kind:"image"|"video",size:ThumbnailSize){
    const key=`${blobId}-${size}`;let task=inflight.get(key);
    if(!task){task=slot(()=>makeThumbnail(cache,blobPath(root,blobId),blobId,kind,size)).catch(()=>false).finally(()=>inflight.delete(key));inflight.set(key,task);}
    return task;
  }
  async function index(job:PhotoJob){
    const facts=await slot(()=>readFacts(blobPath(root,job.blobId),job.kind,job.modifiedAt));
    const ok=await thumbnail(job.blobId,job.kind,"s");
    await db.call("service_photos_worker",["store",JSON.stringify({fileId:job.fileId,blobId:job.blobId,facts,thumbnail:ok?"ready":"failed"})]);
  }
  async function pass(){
    let rounds=0;
    while(!closing&&rounds++<50){
      const {jobs,done}=await db.call<{jobs:PhotoJob[];done:boolean}>("service_photos_worker",["scan",JSON.stringify({limit:5000,jobs:8})]);
      for(const job of jobs){if(closing)return;try{await index(job);}catch(error){console.warn("Photos indexing skipped a file:",(error as Error).message);}}
      if(done&&!jobs.length)return;
    }
  }
  let lastPass=0;
  function schedule(delay:number){if(closing)return;clearTimeout(timer);timer=setTimeout(()=>{lastPass=Date.now();running=pass().catch(error=>{if(!closing)console.warn("Photos scan failed:",(error as Error).message);}).finally(()=>{running=undefined;schedule(30_000);});},delay);}
  schedule(2_000);
  return {
    cache,
    /** Opening Photos rescans soon, at most every 5 seconds, so files added in Files appear quickly. */
    kick(){if(!running&&Date.now()-lastPass>5_000)schedule(0);},
    /** Indexes one file now (after a Photos upload); returns whether it was indexed. */
    async indexNow(job:PhotoJob|null){if(!job)return false;await index(job);return true;},
    /** Path of a ready thumbnail, generating it on demand. */
    async thumbnailFile(blobId:string,kind:"image"|"video",size:ThumbnailSize){return await thumbnail(blobId,kind,size)?cachePath(cache,blobId,size):undefined;},
    async close(){closing=true;if(timer)clearTimeout(timer);await running;await Promise.allSettled([...inflight.values()]);},
  };
}
export type PhotosRuntime=Awaited<ReturnType<typeof createPhotosRuntime>>;
