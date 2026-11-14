import {mkdir,stat} from "node:fs/promises";
import {createReadStream} from "node:fs";
import {createHash} from "node:crypto";
import path from "node:path";
import type {Database} from "../database.ts";
import {blobPath} from "../files/blobs.ts";
import {readFacts,makeThumbnail,makeVideo,perceptualHash,cachePath,videoPath,type ThumbnailSize} from "./media.ts";
import {keyringBox} from "../auth/keyring.ts";
import {hashLinkPassword,verifyPassword} from "../auth/password.ts";

export interface PhotoJob {fileId:string;blobId:string;name:string;mediaType:string;modifiedAt:number;kind:"image"|"video"}
export interface VideoRequest {blobId:string;mode:"remux"|"transcode";audio:string;duration:number}
export interface VideoState {state:"preparing"|"ready"|"failed";progress:number}

/**
 * Owns Photos background work: a bounded scan of Files for new or replaced media,
 * fact extraction, thumbnails, perceptual hashes and browser-playable videos. Derived
 * files live in a regenerable cache keyed by immutable blob ID. Closing stops the loop
 * and kills a running transcode; other tools finish or time out.
 */
export async function createPhotosRuntime(db:Database,root:string){
  const cache=path.join(root,"photo-cache");await mkdir(cache,{recursive:true,mode:0o700});
  // Shared-link tokens are kept encrypted so owners can copy links again; only digests are looked up.
  const links=await keyringBox(path.join(root,"secrets"),"photo-links"),immichKeys=await keyringBox(path.join(root,"secrets"),"immich-api-keys");
  // scrypt uses about 128 MiB per run: link passwords are hashed and checked one at a time.
  let hashing=Promise.resolve();
  const serial=<T,>(work:()=>Promise<T>)=>{const next=hashing.then(work);hashing=next.then(()=>undefined,()=>undefined);return next;};
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
    const facts=await slot(()=>readFacts(blobPath(root,job.blobId),job.kind,job.modifiedAt,job.name,job.mediaType));
    const ok=await thumbnail(job.blobId,job.kind,"s");
    if(ok)facts.phash=await slot(()=>perceptualHash(cachePath(cache,job.blobId,"s")));
    facts.sha1=await sha1Of(blobPath(root,job.blobId)).catch(()=>"");
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

  // Videos are prepared one at a time, on request, newest request last. A failure is
  // remembered for ten minutes so a broken file does not loop.
  const videos=new Map<string,VideoState&{until?:number}>(),queue:VideoRequest[]=[],stopVideo=new AbortController();let converting:Promise<void>|undefined;
  function nextVideo(){
    if(converting||closing)return;const request=queue.shift();if(!request)return;
    const state=videos.get(request.blobId)!;
    converting=makeVideo(cache,blobPath(root,request.blobId),request.blobId,request.mode,request.audio,request.duration,fraction=>{state.progress=fraction;},stopVideo.signal)
      .catch(()=>false).then(ok=>{if(ok)videos.delete(request.blobId);else videos.set(request.blobId,{state:"failed",progress:0,until:Date.now()+600_000});})
      .finally(()=>{converting=undefined;nextVideo();});
  }
  return {
    cache,
    /** Opening Photos rescans soon, at most every 5 seconds, so files added in Files appear quickly. */
    kick(){if(!running&&Date.now()-lastPass>5_000)schedule(0);},
    /** Indexes one file now (after a Photos upload); returns whether it was indexed. */
    async indexNow(job:PhotoJob|null){if(!job)return false;await index(job);return true;},
    /** Path of a ready thumbnail, generating it on demand. */
    async thumbnailFile(blobId:string,kind:"image"|"video",size:ThumbnailSize){return await thumbnail(blobId,kind,size)?cachePath(cache,blobId,size):undefined;},
    /** Starts (or reports) preparing a browser-playable copy. */
    async video(request:VideoRequest):Promise<VideoState&{file?:string}>{
      const file=videoPath(cache,request.blobId);
      if(await ready(file))return {state:"ready",progress:1,file};
      const known=videos.get(request.blobId);
      if(known&&(known.state!=="failed"||(known.until??0)>Date.now()))return {state:known.state,progress:known.progress};
      if(queue.length>=50)return {state:"failed",progress:0};
      videos.set(request.blobId,{state:"preparing",progress:0});queue.push(request);nextVideo();
      return {state:"preparing",progress:0};
    },
    videoFile:(blobId:string)=>videoPath(cache,blobId),
    sealLink:(token:string)=>links.encrypt({token}),
    /** Immich server API keys live in the database only in this sealed form. */
    sealImmichKey:(key:string)=>immichKeys.encrypt({key}),
    openImmichKey:(sealed:string)=>{try{return immichKeys.decrypt<{key:string}>(sealed).key;}catch{return "";}},
    /** The token of a sealed link, or "" if this instance cannot decrypt it (for example after a restore elsewhere). */
    openLink:(sealed:string)=>{try{return links.decrypt<{token:string}>(sealed).token;}catch{return "";}},
    hashLinkPassword:(password:string)=>serial(()=>hashLinkPassword(password)),
    checkLinkPassword:(password:string,verifier:string)=>serial(()=>verifyPassword(password,verifier)),
    async close(){closing=true;if(timer)clearTimeout(timer);stopVideo.abort();await running;await converting;await Promise.allSettled([...inflight.values()]);},
  };
}
/** Base64 SHA-1 of a file, as Immich clients send checksums. */
async function sha1Of(file:string){const hash=createHash("sha1");for await (const chunk of createReadStream(file))hash.update(chunk as Buffer);return hash.digest("base64");}
async function ready(file:string){try{return (await stat(file)).size>0;}catch{return false;}}
export type PhotosRuntime=Awaited<ReturnType<typeof createPhotosRuntime>>;
