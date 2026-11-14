import {createReadStream,createWriteStream} from "node:fs";
import {mkdtemp,rm,stat} from "node:fs/promises";
import {Readable} from "node:stream";
import {pipeline} from "node:stream/promises";
import {createHash} from "node:crypto";
import path from "node:path";
import {lookup} from "node:dns/promises";
import {BlockList,isIP} from "node:net";
import type {Database} from "../database.ts";
import {blobPath,type blobStore} from "../files/blobs.ts";
import {photoKind} from "../../shared/photos.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";

interface Connection {userId:string;url:string;sealedKey:string;direction:"import"|"twoWay"}
interface RemoteAsset {id:string;checksum:string;originalFileName:string;originalMimeType?:string;fileCreatedAt:string;fileModifiedAt:string;isFavorite:boolean;visibility:string;isTrashed:boolean;type:string;exifInfo?:{description?:string|null}}
interface Local {fileId:string;sha1:string;name:string;mediaType:string;takenAt:number;modifiedAt:number;blobId:string;size:number}
/** Per run, so a large library arrives over several runs without starving the server. */
const budget={files:300,bytes:4*1024**3};

/**
 * Dar's connector to an Immich server (API key, x-api-key). Import copies originals that
 * Dar does not have (by SHA-1) into the owner's Photos folder with favorite, archive and
 * description, then mirrors album membership; two-way also uploads Dar photos Immich does
 * not have. One run at a time; each run is bounded and resumes where it left off.
 */
export function createImmichConnector(db:Database,root:string,blobs:ReturnType<typeof blobStore>,photos:PhotosRuntime,options:{allowLoopback:boolean}){
  let closing=false,running:Promise<void>|undefined,timer:NodeJS.Timeout|undefined;const controller=new AbortController();
  const worker=<T=unknown>(operation:string,value:object)=>db.call<T>("service_immich_worker",[operation,JSON.stringify(value)]);
  async function runOne(c:Connection){
    const key=photos.openImmichKey(c.sealedKey),summary={remote:0,imported:0,albums:0,exported:0,skipped:0};let error="";
    try{
      if(!key)throw new Error("The API key cannot be read on this installation. Enter it again.");
      const client=immichClient(c.url,key,controller.signal,options.allowLoopback);
      await client.json("/users/me");
      // What Dar already has.
      const mapped=new Map<string,string>(),local=new Map<string,Local>();let after="";
      for(let page=0;page<1000;page++){
        const known=await worker<{remote:{remoteId:string;fileId:string}[];local:Local[];next:string}>("known",{userId:c.userId,after,limit:2000});
        for(const r of known.remote)mapped.set(r.remoteId,r.fileId);for(const l of known.local)local.set(l.sha1,l);
        if(!known.next)break;after=known.next;
      }
      const map=(remoteId:string,fileId:string,checksum:string)=>{mapped.set(remoteId,fileId);return worker("map",{userId:c.userId,remoteId,fileId,checksum});};
      // Import: page through Immich's library.
      const remoteChecksums=new Set<string>();let files=0,bytes=0;
      for(let page=1;page<10_000&&!closing;page++){
        const result=await client.json<{assets:{items:RemoteAsset[];nextPage:string|null}}>("/search/metadata",{method:"POST",body:{page,size:500,withExif:true,withDeleted:false}});
        for(const asset of result.assets.items){
          summary.remote++;if(asset.checksum)remoteChecksums.add(asset.checksum);
          if(asset.isTrashed||asset.visibility==="locked"||mapped.has(asset.id))continue;
          const existing=local.get(asset.checksum);if(existing){await map(asset.id,existing.fileId,asset.checksum);continue;}
          if(files>=budget.files||bytes>=budget.bytes){summary.skipped++;continue;}
          const imported=await importAsset(c,client,asset);if(!imported){summary.skipped++;continue;}
          files++;bytes+=imported.size;summary.imported++;await map(asset.id,imported.fileId,asset.checksum);
        }
        if(!result.assets.nextPage)break;
      }
      // Albums: membership of imported photos, by Immich album.
      const albums=await client.json<{id:string;albumName:string;description:string}[]>("/albums");
      for(const album of albums){
        if(closing)break;const fileIds:string[]=[];
        for(let page=1;page<1000;page++){
          const result=await client.json<{assets:{items:RemoteAsset[];nextPage:string|null}}>("/search/metadata",{method:"POST",body:{page,size:1000,albumIds:[album.id]}});
          for(const a of result.assets.items){const fileId=mapped.get(a.id)??local.get(a.checksum)?.fileId;if(fileId)fileIds.push(fileId);}
          if(!result.assets.nextPage)break;
        }
        const {albumId}=await worker<{albumId:string}>("album",{userId:c.userId,remoteId:album.id,name:album.albumName,description:album.description??"",fileIds});if(albumId)summary.albums++;
      }
      // Two-way: Dar photos Immich lacks go up, original bytes and dates.
      if(c.direction==="twoWay")for(const item of local.values()){
        if(closing||files>=budget.files||bytes>=budget.bytes)break;if(remoteChecksums.has(item.sha1)||[...mapped.values()].includes(item.fileId))continue;
        const uploaded=await client.upload(item,blobPath(root,item.blobId));files++;bytes+=item.size;summary.exported++;await map(uploaded,item.fileId,item.sha1);
      }
    }catch(cause){error=(cause as Error).name==="AbortError"?"Stopped.":(cause as Error).message.slice(0,300);}
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
  }
  function schedule(){if(closing)return;timer=setTimeout(()=>{void tick().finally(schedule);},5*60_000);}
  schedule();
  return {
    /** Runs one account's connection now (Sync now). */
    syncNow(userId:string){void tick(userId);},
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
    /** Multipart upload of a Dar original; Immich answers created or duplicate with its asset ID. */
    async upload(item:Local,file:string){
      const boundary=`dar-${crypto.randomUUID()}`,field=(name:string,value:string)=>`--${boundary}\r\ncontent-disposition: form-data; name="${name}"\r\n\r\n${value}\r\n`;
      const head=Buffer.from(field("deviceAssetId",item.fileId)+field("deviceId","dar")+field("fileCreatedAt",new Date(item.takenAt).toISOString())+field("fileModifiedAt",new Date(item.modifiedAt).toISOString())+field("filename",item.name)
        +`--${boundary}\r\ncontent-disposition: form-data; name="assetData"; filename="${item.name.replace(/"/g,"_")}"\r\ncontent-type: ${item.mediaType}\r\n\r\n`),tail=Buffer.from(`\r\n--${boundary}--\r\n`);
      const body=Readable.from((async function*(){yield head;for await (const chunk of createReadStream(file))yield chunk;yield tail;})());
      await allowedHost(base,allowLoopback);
      let response:Response;
      try{
        response=await fetch(base+"/assets",{method:"POST",headers:{"x-api-key":key,"x-immich-checksum":item.sha1,"content-type":`multipart/form-data; boundary=${boundary}`,"content-length":String(head.length+item.size+tail.length)},
          body:Readable.toWeb(body) as unknown as BodyInit,duplex:"half",signal,redirect:"manual"} as RequestInit);
      }catch(error){throw new Error(networkMessage(error,base+"/assets"));}
      if(!response.ok)throw new Error(`Immich answered ${response.status} to an upload.`);
      return (await response.json() as {id:string}).id;
    },
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
  for(const {address,family} of addresses){if(allowLoopback&&(/^127\.|^::1$|^::ffff:127\./.test(address)))continue;const mapped=/^::ffff:(\d+\.\d+\.\d+\.\d+)$/i.exec(address)?.[1];if(mapped?blocked.check(mapped,"ipv4"):blocked.check(address,family===6?"ipv6":"ipv4"))throw new Error("That address points to this server or a reserved network. Use the Immich server's network address.");}
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
