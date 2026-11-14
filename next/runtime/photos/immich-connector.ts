import {createReadStream,createWriteStream} from "node:fs";
import {mkdtemp,rm,stat} from "node:fs/promises";
import {Readable} from "node:stream";
import {pipeline} from "node:stream/promises";
import {createHash} from "node:crypto";
import path from "node:path";
import {lookup} from "node:dns/promises";
import {BlockList,isIP} from "node:net";
import type {Database} from "../database.ts";
import type {blobStore} from "../files/blobs.ts";
import {photoKind,type PhotoImmichProgress} from "../../shared/photos.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";

interface Connection {userId:string;url:string;sealedKey:string}
interface RemoteAsset {id:string;checksum:string;originalFileName:string;originalMimeType?:string;fileCreatedAt:string;fileModifiedAt:string;isFavorite:boolean;visibility:string;isTrashed:boolean;type:string;exifInfo?:{description?:string|null}}
interface Local {fileId:string;sha1:string}
/** Per run, so a large library arrives over several runs without starving the server. */
const budget={files:300,bytes:4*1024**3};
/** While Immich has more to import, the next run starts this soon instead of waiting for the interval. */
const backlogDelay=30_000;

/**
 * Dar's import from an Immich server (API key, x-api-key). Originals Dar does not have
 * (by SHA-1) are copied into the owner's Photos folder with favorite, archive and
 * description, then album membership is mirrored. One run at a time; each is bounded,
 * reports live progress, and the next continues where it stopped.
 */
export function createImmichConnector(db:Database,root:string,blobs:ReturnType<typeof blobStore>,photos:PhotosRuntime,options:{allowLoopback:boolean}){
  let closing=false,running:Promise<void>|undefined,timer:NodeJS.Timeout|undefined;const controller=new AbortController();
  const worker=<T=unknown>(operation:string,value:object)=>db.call<T>("service_immich_worker",[operation,JSON.stringify(value)]);
  const progress=new Map<string,PhotoImmichProgress>(),backlog=new Set<string>();
  async function runOne(c:Connection){
    const key=photos.openImmichKey(c.sealedKey),summary={remote:0,imported:0,albums:0,skipped:0};let error="";
    const state:PhotoImmichProgress={phase:"connecting",done:0,total:0,imported:0,file:"",startedAt:Date.now()};progress.set(c.userId,state);
    const phase=(name:PhotoImmichProgress["phase"],total=0)=>{state.phase=name;state.done=0;state.total=total;state.file="";};
    try{
      if(!key)throw new Error("The API key cannot be read on this installation. Enter it again.");
      const client=immichClient(c.url,key,controller.signal,options.allowLoopback);
      await client.json("/users/me");
      // What Dar already has: imported Immich IDs, and the SHA-1 of the owner's photos.
      phase("comparing");
      const mapped=new Map<string,string>(),local=new Map<string,Local>();let after="";
      for(let page=0;page<1000;page++){
        const known=await worker<{remote:{remoteId:string;fileId:string}[];local:Local[];next:string}>("known",{userId:c.userId,after,limit:2000});
        for(const r of known.remote)mapped.set(r.remoteId,r.fileId);for(const l of known.local)local.set(l.sha1,l);
        if(!known.next)break;after=known.next;
      }
      const map=(remoteId:string,fileId:string,checksum:string)=>{mapped.set(remoteId,fileId);return worker("map",{userId:c.userId,remoteId,fileId,checksum});};
      // Import: page through Immich's library.
      phase("importing");let files=0,bytes=0;
      for(let page=1;page<10_000&&!closing;page++){
        const result=await client.json<{assets:{items:RemoteAsset[];nextPage:string|null;total?:number}}>("/search/metadata",{method:"POST",body:{page,size:500,withExif:true,withDeleted:false}});
        if(result.assets.total)state.total=result.assets.total;
        for(const asset of result.assets.items){
          summary.remote++;state.done++;if(state.total<state.done)state.total=state.done;
          if(asset.isTrashed||asset.visibility==="locked"||mapped.has(asset.id))continue;
          const existing=local.get(asset.checksum);if(existing){await map(asset.id,existing.fileId,asset.checksum);continue;}
          if(files>=budget.files||bytes>=budget.bytes){summary.skipped++;continue;}
          state.file=asset.originalFileName;
          const imported=await importAsset(c,client,asset);if(!imported){summary.skipped++;continue;}
          files++;bytes+=imported.size;summary.imported++;state.imported++;await map(asset.id,imported.fileId,asset.checksum);
        }
        if(!result.assets.nextPage)break;
      }
      // Albums: membership of imported photos, by Immich album.
      const albums=await client.json<{id:string;albumName:string;description:string}[]>("/albums");
      phase("albums",albums.length);
      for(const album of albums){
        if(closing)break;state.done++;state.file=album.albumName;const fileIds:string[]=[];
        for(let page=1;page<1000;page++){
          const result=await client.json<{assets:{items:RemoteAsset[];nextPage:string|null}}>("/search/metadata",{method:"POST",body:{page,size:1000,albumIds:[album.id]}});
          for(const a of result.assets.items){const fileId=mapped.get(a.id)??local.get(a.checksum)?.fileId;if(fileId)fileIds.push(fileId);}
          if(!result.assets.nextPage)break;
        }
        const {albumId}=await worker<{albumId:string}>("album",{userId:c.userId,remoteId:album.id,name:album.albumName,description:album.description??"",fileIds});if(albumId)summary.albums++;
      }
    }catch(cause){error=(cause as Error).name==="AbortError"?"Stopped.":(cause as Error).message.slice(0,300);}
    progress.delete(c.userId);
    if(!error&&summary.skipped>0)backlog.add(c.userId);else backlog.delete(c.userId);
    await worker("report",{userId:c.userId,error,summary}).catch(()=>undefined);
  }
  async function importAsset(c:Connection,client:ReturnType<typeof immichClient>,asset:RemoteAsset){
    const name=(asset.originalFileName||"immich").replace(/[\\/]/g,"_").slice(0,255),mediaType=asset.originalMimeType||"application/octet-stream";
    if(!photoKind(mediaType,name))return undefined;
    const dir=await mkdtemp(path.join(root,"uploads","immich-import-"));
    try{
      const file=path.join(dir,"original"),hash=createHash("sha1"),response=await client.raw(`/assets/${asset.id}/original`);
      await pipeline(Readable.fromWeb(response.body as import("node:stream/web").ReadableStream),async function*(source){for await (const chunk of source){hash.update(chunk as Buffer);yield chunk;}},createWriteStream(file,{mode:0o600}));
      const {size}=await stat(file),sha1=hash.digest("base64"),created=/^(\d{4})-(\d{2})/.exec(asset.fileCreatedAt)??/^(\d{4})-(\d{2})/.exec(new Date().toISOString())!;
      const begun=await worker<{duplicate?:string;id?:string}>("importBegin",{userId:c.userId,sha1,name,size,mediaType,year:Number(created[1]),month:Number(created[2])});
      if(begun.duplicate)return {fileId:begun.duplicate,size:0};
      const digest=await blobs.receive(createReadStream(file) as AsyncIterable<Buffer>,begun.id!,size);
      const time=Date.parse(asset.fileCreatedAt);
      const {fileId,job}=await worker<{fileId:string;job:PhotoJob}>("importFinish",{userId:c.userId,id:begun.id,digest,...Number.isFinite(time)?{fileTime:time}:{}});
      await photos.indexNow(job).catch(()=>false);
      await worker("apply",{userId:c.userId,fileId,favorite:asset.isFavorite,archived:asset.visibility==="archive",description:asset.exifInfo?.description??""});
      return {fileId,size};
    }finally{await rm(dir,{recursive:true,force:true});}
  }
  async function tick(userId=""){
    if(running||closing)return;
    running=(async()=>{const {connections}=await worker<{connections:Connection[]}>("due",{userId});for(const c of connections){if(closing)break;await runOne(c);}})()
      .catch(error=>{if(!closing)console.warn("Immich connector:",(error as Error).message);}).finally(()=>{running=undefined;});
    await running;
    // A library larger than one run keeps going, an account at a time, until it is all in.
    const next=[...backlog][0];if(next&&!closing)setTimeout(()=>{void tick(next);},backlogDelay).unref();
  }
  function schedule(){if(closing)return;timer=setTimeout(()=>{void tick().finally(schedule);},5*60_000);}
  schedule();
  return {
    /** Runs one account's connection now (Sync now). */
    syncNow(userId:string){void tick(userId);},
    /** Where a running import is, for the Photos status bars; undefined when idle. */
    progress(userId:string):PhotoImmichProgress|undefined{const value=progress.get(userId);return value?{...value}:backlog.has(userId)?{phase:"waiting",done:0,total:0,imported:0,file:"",startedAt:0}:undefined;},
    /** Checks an Immich server and key before they are saved. */
    async test(url:string,key:string){const client=immichClient(url,key,AbortSignal.timeout(15_000),options.allowLoopback);await client.json("/server/ping");return await client.json<{name:string;email:string}>("/users/me");},
    async close(){closing=true;clearTimeout(timer);controller.abort();await running;},
  };
}
export type ImmichConnector=ReturnType<typeof createImmichConnector>;

/** Minimal Immich API client: the server URL (with or without /api) and an API key. */
function immichClient(url:string,key:string,signal:AbortSignal,allowLoopback=false){
  // Same-host redirects (http to https, a new port) are followed and remembered.
  let base=url.replace(/\/+$/,"").replace(/\/api$/,"")+"/api";
  async function raw(route:string,init:{method?:string;body?:unknown}={}){
    let target=base+route,response:Response|undefined;
    for(let hop=0;hop<5;hop++){
      await allowedHost(target,allowLoopback);
      try{response=await fetch(target,{method:init.method??"GET",headers:{"x-api-key":key,accept:"application/json",...init.body!==undefined?{"content-type":"application/json"}:{}},body:init.body===undefined?undefined:JSON.stringify(init.body),signal:AbortSignal.any([signal,AbortSignal.timeout(10*60_000)]),redirect:"manual"});}catch(error){throw new Error(networkMessage(error,target));}
      if(![301,302,303,307,308].includes(response.status))break;
      const location=response.headers.get("location"),next=location?new URL(location,target):undefined;await response.body?.cancel().catch(()=>undefined);
      if(!next||next.hostname!==new URL(target).hostname)throw new Error(`The server redirects to ${next?.host??"nowhere"}. Enter that address instead.`);
      if(next.pathname.endsWith(route))base=next.origin+next.pathname.slice(0,next.pathname.length-route.length);
      target=next.toString();response=undefined;
    }
    if(!response)throw new Error("The server redirected too many times.");
    if(!response.ok){await response.body?.cancel().catch(()=>undefined);throw new Error(response.status===401||response.status===403?"Immich rejected the API key.":`Immich answered ${response.status} for ${route}.`);}
    return response;
  }
  return {
    raw,
    async json<T=unknown>(route:string,init:{method?:string;body?:unknown}={}){return await (await raw(route,init)).json() as T;},
  };
}

/**
 * Immich servers usually live on the local network, so private addresses are allowed.
 * Loopback, link-local (cloud metadata), unspecified and multicast addresses are not:
 * the connector must not become a way to probe the server itself.
 */
const blocked=new BlockList();
for(const [net,bits] of [["0.0.0.0",8],["127.0.0.0",8],["169.254.0.0",16],["224.0.0.0",4],["240.0.0.0",4]] as const)blocked.addSubnet(net,bits,"ipv4");
for(const [net,bits] of [["::",128],["::1",128],["fe80::",10],["ff00::",8]] as const)blocked.addSubnet(net,bits,"ipv6");
/** Development installations may reach a local Immich (tests use one). */
async function allowedHost(base:string,allowLoopback:boolean){
  const url=new URL(base);if(!["http:","https:"].includes(url.protocol)||url.username||url.password)throw new Error("Use an http or https address without credentials.");
  const host=url.hostname.replace(/^\[|\]$/g,""),addresses=isIP(host)?[{address:host,family:isIP(host)}]:await lookup(host,{all:true}).catch(()=>{throw new Error("The Immich server's name could not be resolved.");});
  for(const {address,family} of addresses){if(allowLoopback&&(/^127\.|^::1$|^::ffff:127\./.test(address)))continue;const mapped=/^::ffff:(\d+\.\d+\.\d+\.\d+)$/i.exec(address)?.[1];if(mapped?blocked.check(mapped,"ipv4"):blocked.check(address,family===6?"ipv6":"ipv4"))throw new Error("That address points to this server or a reserved network. For Immich on the same machine, the administrator can set services.dar.photos.immichAllowLocalHost and you can use http://localhost:2283 (or http://[::1]:2283 if Immich listens on IPv6 only).");}
}

/**
 * fetch reports every network failure as "fetch failed"; the reason is in its cause.
 * Turn it into something an owner can act on (never including the API key).
 */
export function networkMessage(error:unknown,target:string){
  const cause=(error as {cause?:{code?:string;message?:string;cause?:{code?:string}}}).cause,code=cause?.code??cause?.cause?.code??"",detail=cause?.message??(error as Error).message;
  let host=target;try{const u=new URL(target);host=u.host;}catch{host=target;}
  if((error as Error).name==="AbortError"||(error as Error).name==="TimeoutError")return `${host} did not answer in time.`;
  console.warn(`Immich connector: ${host}: ${code||detail}`);
  if(code==="ECONNREFUSED")return `Nothing accepted the connection at ${host}. Check the address and port (Immich listens on 2283 unless a proxy is in front).`;
  if(code==="ENOTFOUND"||code==="EAI_AGAIN")return `The Dar server cannot resolve ${host}.`;
  if(["ETIMEDOUT","UND_ERR_CONNECT_TIMEOUT","EHOSTUNREACH","ENETUNREACH","ECONNRESET"].includes(code))return `The Dar server cannot reach ${host} (${code}). Check that it is reachable from the Dar server, not just from your browser.`;
  if(/CERT|SELF_SIGNED|UNABLE_TO_VERIFY|ALTNAME/.test(code))return `The HTTPS certificate of ${host} is not trusted by the Dar server (${code}). Use a certificate from a trusted authority, or the server's http:// address on your network.`;
  if(code==="EPROTO"||/wrong version number|packet length too long/i.test(detail))return `${host} does not speak HTTPS on that port. Use http:// for it.`;
  if(code==="UND_ERR_SOCKET"||/other side closed/i.test(detail))return `${host} closed the connection. If it serves HTTPS, use https://.`;
  return `Immich could not be reached at ${host} (${code||detail||"network error"}).`;
}
