import type {IncomingMessage,ServerResponse} from "node:http";
import {createReadStream} from "node:fs";
import {stat} from "node:fs/promises";
import {randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import {blobPath,type BlobFile} from "../files/blobs.ts";
import {AppError} from "../errors.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";
import {createOpaqueToken} from "../auth/session.ts";
import type {ImmichConnector} from "./immich-connector.ts";
import {validLinkPassword,type PhotoLink,type PhotoAlbumMember} from "../../shared/photos.ts";
/** A link as the database returns it: the token sealed with the instance keyring. */
type SealedLink=Omit<PhotoLink,"url">&{sealed:string};

const messages:Record<string,Record<string,string>>={
  createAlbum:{quotaExceeded:"You have reached the limit of 500 albums."},
  editAlbum:{conflict:"This album changed elsewhere. Reload it and try again."},
  describe:{conflict:"This description changed elsewhere. Reload it and try again."},
  uploadTarget:{conflict:"A file is in the way of the Photos folder (or a year or month inside it). Rename it in Files or choose another folder in Photos settings."},
  unlock:{conflict:"A file is in the way of the Photos folder (or a year or month inside it). Rename it in Files or choose another folder in Photos settings."},
  restore:{conflict:"A file with the same name is already where this photo was. Rename it in Files and try again."},
  resolveDuplicates:{conflict:"These duplicates changed. Reload the list and try again."},
  shareAlbum:{notFound:"No active account has that username.",quotaExceeded:"An album can be shared with up to 100 people."},
  createLink:{quotaExceeded:"You can have up to 200 shared links. Remove one first."},
  editLink:{conflict:"This link changed elsewhere. Reload it and try again."},
  saveImmichConnection:{conflict:"The Immich connection changed elsewhere. Reload and try again.",invalidInput:"Check the server address (http:// or https://), the direction and the interval."},
  addToAlbum:{quotaExceeded:"An album can hold up to 50,000 items.",forbidden:"You can view this album but not add to it."},
};
const opaque=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
/** Types every browser decodes; used when no thumbnail could be made. */
const renderable=/^image\/(jpeg|png|gif|webp|avif|bmp)$/;
export interface AssetInfo {id:string;revision:number;blobId:string;kind:"image"|"video";mediaType:string;name:string;playback:string;audioCodec:string;duration:number;size:number}

/** Streams a local file with byte ranges (video seeking) and a strong ETag. */
export async function sendFile(req:IncomingMessage,res:ServerResponse,file:string,type:string,tag:string,cache:string){
  const {size}=await stat(file);let start=0,end=size-1,status=200;
  const headers:Record<string,string|number>={"content-type":type,etag:tag,"cache-control":cache,"accept-ranges":"bytes","x-content-type-options":"nosniff","content-security-policy":"default-src 'none'"};
  if(req.headers["if-none-match"]===tag){res.writeHead(304,{etag:tag,"cache-control":cache});res.end();return;}
  if(req.headers.range&&(!req.headers["if-range"]||req.headers["if-range"]===tag)){
    const match=/^bytes=(\d*)-(\d*)$/.exec(req.headers.range);
    if(match&&(match[1]||match[2])&&size>0){
      if(!match[1])start=Math.max(0,size-Number(match[2]));else{start=Number(match[1]);end=match[2]?Math.min(Number(match[2]),end):end;}
    }else start=-1;
    if(!Number.isSafeInteger(start)||start<0||start>end||start>=size){res.writeHead(416,{"content-range":`bytes */${size}`});res.end();return;}
    status=206;headers["content-range"]=`bytes ${start}-${end}/${size}`;
  }
  headers["content-length"]=size?end-start+1:0;res.writeHead(status,headers);
  if(req.method==="HEAD"||!size){res.end();return;}
  createReadStream(file,{start,end}).on("error",()=>res.destroy()).pipe(res);
}

/** Native-session Photos API. The database re-checks the account, ownership, the Locked folder and app availability. */
export async function handlePhotosHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,runtime:PhotosRuntime,root:string,origin:string,account:{userId:string;immich:ImmichConnector},
  download:(entry:BlobFile,inline?:boolean)=>Promise<void>,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",route=url.pathname.slice("/api/v1/photos".length);
  const send=(value:unknown,status=200)=>{res.writeHead(status,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value));};
  const call=async<T=unknown>(operation:string,value:object)=>{
    try{return await db.call<T>("service_photos",[sessionId,operation,JSON.stringify(value)]);}
    catch(error){if(error instanceof AppError&&messages[operation]?.[error.code])throw new AppError(error.code,error.status,messages[operation][error.code]);throw error;}
  };
  const input=async(allowed:string[])=>{const value=await body();if(Object.keys(value).some(key=>!allowed.includes(key)))throw new AppError("invalidInput");return value;};
  if(route==="/overview"&&method==="GET"){const value=await call("overview",{});runtime.kick();return send(value);}
  if(route==="/assets"&&method==="GET"){
    const limit=Number(url.searchParams.get("limit")??120);if(!Number.isInteger(limit))throw new AppError("invalidInput");
    return send(await call("page",{view:url.searchParams.get("view")??"library",...url.searchParams.get("album")?{album:url.searchParams.get("album")}:{},cursor:url.searchParams.get("cursor")??"",limit}));
  }
  if(route==="/assets"&&method==="PATCH")return send(await call("update",await input(["ids","favorite","archived"])));
  const bulk=/^\/assets\/(trash|restore|delete|lock|unlock)$/.exec(route);
  if(bulk&&method==="POST")return send(await call(bulk[1],await input(bulk[1]==="unlock"?["ids","to"]:["ids"])));
  if(route==="/trash/empty"&&method==="POST")return send(await call("emptyTrash",{}));
  if(route==="/upload-target"&&method==="POST")return send(await call("uploadTarget",await input(["year","month"])));
  if(route==="/albums"&&method==="POST"){const value=await input(["name","ids"]);return send(await call("createAlbum",{id:randomUUID(),...value}),201);}
  if(route==="/duplicates"&&method==="GET")return send(await call("duplicates",{cursor:url.searchParams.get("cursor")??""}));
  if(route==="/duplicates/resolve"&&method==="POST")return send(await call("resolveDuplicates",await input(["key","keep"])));
  if(route==="/duplicates/dismiss"&&method==="POST")return send(await call("dismissDuplicates",await input(["key"])));
  // Immich: devices signed in with the mobile app, and Dar's connection to an Immich server.
  if(route==="/immich"&&method==="GET"){const [devices,connection]=await Promise.all([call<{devices:unknown[]}>("immichDevices",{}),call("immichConnection",{})]);return send({serverUrl:origin,...devices,connection});}
  const device=/^\/immich\/devices\/([a-f0-9-]{36})$/.exec(route);
  if(device&&method==="DELETE")return send(await call("revokeImmichDevice",{id:device[1]}));
  if(route==="/immich/connection"&&method==="PUT"){
    const value=await input(["url","apiKey","direction","intervalMinutes","enabled","revision"]),{apiKey,...rest}=value,current=await call<{connected:boolean;url?:string}>("immichConnection",{});
    const url=typeof rest.url==="string"?rest.url.trim():"";
    if(typeof apiKey==="string"&&apiKey){
      if(apiKey.length>500)throw new AppError("invalidInput");
      try{await account.immich.test(url,apiKey);}catch(cause){throw new AppError("immichUnavailable",400,(cause as Error).message||"The Immich server could not be reached.");}
      return send(await call("saveImmichConnection",{...rest,sealedKey:runtime.sealImmichKey(apiKey)}));
    }
    if(!current.connected||url!==current.url)throw new AppError("invalidInput",400,"Enter the API key for this Immich server.");
    return send(await call("saveImmichConnection",rest));
  }
  if(route==="/immich/connection"&&method==="DELETE")return send(await call("deleteImmichConnection",{}));
  if(route==="/immich/connection/sync"&&method==="POST"){account.immich.syncNow(account.userId);return send({started:true},202);}
  if(route==="/stacks"&&method==="POST")return send(await call("stack",await input(["ids","primaryId"])),201);
  // Sharing: people on this Dar and shared links. Link tokens are made here; the database keeps their digest and a sealed copy.
  const present=(link:SealedLink):PhotoLink=>{const {sealed,...rest}=link,token=runtime.openLink(sealed);return {...rest,url:token?`${origin}/s/${token}`:""};};
  const linkInput=async(value:Record<string,unknown>)=>{
    const {password,...rest}=value;
    if(password===undefined)return rest;
    if(password===null||password==="")return {...rest,passwordVerifier:""};
    if(typeof password!=="string"||!validLinkPassword(password))throw new AppError("invalidInput",400,"Use a link password of 4 to 200 characters.");
    return {...rest,passwordVerifier:await runtime.hashLinkPassword(password)};
  };
  if(route==="/links"&&method==="GET"){const {links}=await call<{links:SealedLink[]}>("links",{});return send({links:links.map(present)});}
  if(route==="/links"&&method==="POST"){
    const value=await input(["albumId","ids","expiresAt","password","allowDownload","allowUpload","showMetadata"]),token=createOpaqueToken();
    return send(present(await call<SealedLink>("createLink",{id:randomUUID(),digest:token.digest,sealed:runtime.sealLink(token.secret),...await linkInput(value)})),201);
  }
  const linkRoute=/^\/links\/([^/]+)$/.exec(route);
  if(linkRoute){
    const id=linkRoute[1];if(!opaque.test(id))throw new AppError("notFound",404);
    if(method==="PATCH")return send(present(await call<SealedLink>("editLink",{id,...await linkInput(await input(["revision","expiresAt","password","allowDownload","allowUpload","showMetadata"]))})));
    if(method==="DELETE")return send(await call("deleteLink",{id}));
  }
  const members=/^\/albums\/([^/]+)\/(sharing|members)(?:\/([^/]+))?$/.exec(route);
  if(members){
    const [,id,kind,userId]=members;if(!opaque.test(id)||userId&&!opaque.test(userId))throw new AppError("notFound",404);
    if(kind==="sharing"&&!userId&&method==="GET"){const value=await call<{members:PhotoAlbumMember[];links:SealedLink[]}>("albumSharing",{id});return send({members:value.members,links:value.links.map(present)});}
    if(kind==="members"&&!userId&&method==="POST")return send(await call("shareAlbum",{id,...await input(["username","role"])}));
    if(kind==="members"&&userId&&method==="DELETE")return send(await call("unshareAlbum",{id,userId}));
  }
  const stack=/^\/stacks\/([^/]+)(\/remove)?$/.exec(route);
  if(stack){
    const [,id,remove]=stack;if(!opaque.test(id))throw new AppError("notFound",404);
    if(!remove&&method==="GET")return send(await call("stackDetail",{id}));
    if(!remove&&method==="PATCH")return send(await call("stackCover",{id,...await input(["primaryId"])}));
    if(!remove&&method==="DELETE")return send(await call("unstack",{id}));
    if(remove&&method==="POST")return send(await call("unstack",{id,...await input(["ids"])}));
  }
  const asset=/^\/assets\/([^/]+)(\/[a-z]+)?$/.exec(route);
  if(asset){
    const [,id,action=""]=asset;if(!opaque.test(id))throw new AppError("notFound",404);
    if(!action&&method==="GET")return send(await call("detail",{id}));
    if(action==="/description"&&method==="PUT")return send(await call("describe",{id,...await input(["description","revision"])}));
    if(action==="/index"&&method==="POST"){const {job}=await call<{job:PhotoJob|null}>("indexJob",{id});return send({indexed:await runtime.indexNow(job)});}
    if(action==="/original"&&["GET","HEAD"].includes(method)){
      // Originals of photos shared with you (and your own) download through Photos.
      const info=await call<AssetInfo>("asset",{id});return await download({id:info.id,name:info.name,blobId:info.blobId,size:info.size,mediaType:info.mediaType,revision:info.revision},false);
    }
    if(action==="/thumbnail"&&["GET","HEAD"].includes(method)){
      const size=url.searchParams.get("size")==="m"?"m":"s";
      const info=await call<AssetInfo>("asset",{id});
      const tag=`"${info.blobId}-${size}"`;
      if(req.headers["if-none-match"]===tag){res.writeHead(304,{etag:tag,"cache-control":"private, max-age=86400"});res.end();return;}
      const file=await runtime.thumbnailFile(info.blobId,info.kind,size);
      if(!file){
        // Without media tools, browser-decodable originals stand in; anything else is a 404 the grid shows as a tile.
        if(info.kind==="image"&&renderable.test(info.mediaType))return await download(await db.call<BlobFile>("service_download",[sessionId,id,""]));
        throw new AppError("notFound",404,"No preview is available for this file.");
      }
      return await sendFile(req,res,file,"image/webp",tag,"private, max-age=86400");
    }
    if(action==="/playback"&&method==="POST"){
      // Videos browsers cannot play are remuxed or transcoded once into the cache.
      const info=await call<AssetInfo>("asset",{id});if(info.kind!=="video")throw new AppError("invalidInput");
      if(info.playback==="original")return send({state:"ready",progress:1,url:`/api/v1/photos/assets/${id}/video`});
      const state=await runtime.video({blobId:info.blobId,mode:info.playback==="remux"?"remux":"transcode",audio:info.audioCodec,duration:info.duration});
      return send({state:state.state,progress:state.progress,url:state.state==="ready"?`/api/v1/photos/assets/${id}/video?v=${info.blobId}`:""});
    }
    if(action==="/video"&&["GET","HEAD"].includes(method)){
      const info=await call<AssetInfo>("asset",{id});if(info.kind!=="video")throw new AppError("notFound",404);
      // Originals stream as is (also HEVC for browsers that decode it); MOV and M4V share MP4's container.
      if(info.playback==="original"||url.searchParams.get("source")==="original")return await sendFile(req,res,blobPath(root,info.blobId),info.mediaType==="video/webm"?"video/webm":"video/mp4",`"${info.blobId}"`,"private, no-store");
      const file=runtime.videoFile(info.blobId);
      try{return await sendFile(req,res,file,"video/mp4",`"${info.blobId}-v"`,"private, max-age=86400");}
      catch{throw new AppError("notFound",404,"This video is still being prepared.");}
    }
  }
  const album=/^\/albums\/([^/]+)(\/[a-z]+)?$/.exec(route);
  if(album){
    const [,id,action=""]=album;if(!opaque.test(id))throw new AppError("notFound",404);
    if(!action&&method==="PATCH")return send(await call("editAlbum",{id,...await input(["name","description","coverId","revision"])}));
    if(!action&&method==="DELETE")return send(await call("deleteAlbum",{id}));
    if(action==="/add"&&method==="POST")return send(await call("addToAlbum",{id,...await input(["ids"])}));
    if(action==="/remove"&&method==="POST")return send(await call("removeFromAlbum",{id,...await input(["ids"])}));
  }
  throw new AppError("notFound",404);
}
