import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {blobPath,type blobStore} from "../files/blobs.ts";
import {AppError} from "../errors.ts";
import {createOpaqueToken,digestToken} from "../auth/session.ts";
import {photoLinkLimits} from "../../shared/photos.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";
import {sendFile,type AssetInfo} from "./http.ts";

const passCookie="dar_link_pass";
/**
 * Shared links for visitors without an account, at /public/photos/<token>/…; the page
 * itself is /s/<token>. Only the token's digest reaches the database. A link password,
 * once entered, becomes an HttpOnly pass cookie limited to the link's path.
 */
export async function handlePhotoLinkHttp(req:IncomingMessage,res:ServerResponse,url:URL,options:{db:Database;runtime:PhotosRuntime;root:string;blobs:ReturnType<typeof blobStore>;secure:boolean;limit:(key:string)=>void;body:()=>Promise<Record<string,unknown>>}){
  const {db,runtime,root,blobs}=options,method=req.method??"GET";
  const match=/^\/public\/photos\/([A-Za-z0-9_-]{43})(\/.*)?$/.exec(url.pathname);if(!match)throw new AppError("notFound",404);
  const [,token,route=""]=match,digest=digestToken(token),base=`/public/photos/${token}`;
  const cookie=req.headers.cookie?.split(";").map(part=>part.trim()).find(part=>part.startsWith(`${passCookie}=`))?.slice(passCookie.length+1)??"";
  const pass=/^[A-Za-z0-9_-]{43}$/.test(cookie)?digestToken(cookie):"";
  const call=<T=unknown>(operation:string,value:object={})=>db.call<T>("service_photo_link",[digest,pass,operation,JSON.stringify(value)]);
  const send=(value:unknown,status=200,headers:Record<string,string>={})=>{res.writeHead(status,{"content-type":"application/json","cache-control":"no-store",...headers});res.end(JSON.stringify(value));};
  const input=async(allowed:string[])=>{const value=await options.body();if(Object.keys(value).some(key=>!allowed.includes(key)))throw new AppError("invalidInput");return value;};
  try{
    if(!route&&method==="GET")return send(await call("info"));
    if(route==="/password"&&method==="POST"){
      options.limit(`photo-link:${digest}`);
      const {password}=await input(["password"]);if(typeof password!=="string"||password.length>200)throw new AppError("invalidInput");
      const {verifier}=await call<{verifier:string}>("verifier");if(!verifier)throw new AppError("invalidInput",400,"This link has no password.");
      if(!await runtime.checkLinkPassword(password,verifier))throw new AppError("invalidCredentials",403,"That password is not correct.");
      const grant=createOpaqueToken();await call("grantPass",{digest:grant.digest});
      const age=Math.round(photoLinkLimits.passwordDays*86400);
      return send({ok:true},200,{"set-cookie":`${passCookie}=${grant.secret}; Path=${base}; HttpOnly; SameSite=Strict; Max-Age=${age}${options.secure?"; Secure":""}`});
    }
    if(route==="/assets"&&method==="GET")return send(await call("page",{cursor:url.searchParams.get("cursor")??"",limit:Number(url.searchParams.get("limit")??120)}));
    const asset=/^\/assets\/([a-f0-9-]{36})(\/[a-z]+)?$/.exec(route);
    if(asset){
      const [,id,action=""]=asset;
      if(!action&&method==="GET")return send(await call("detail",{id}));
      const info=await call<AssetInfo&{allowDownload:boolean}>("asset",{id});
      if(action==="/thumbnail"&&["GET","HEAD"].includes(method)){
        const size=url.searchParams.get("size")==="m"?"m":"s",file=await runtime.thumbnailFile(info.blobId,info.kind,size);
        if(!file)throw new AppError("notFound",404,"No preview is available for this file.");
        return await sendFile(req,res,file,"image/webp",`"${info.blobId}-${size}"`,"private, max-age=3600");
      }
      if(action==="/playback"&&method==="POST"){
        if(info.kind!=="video")throw new AppError("invalidInput");
        if(info.playback==="original")return send({state:"ready",progress:1,url:`${base}/assets/${id}/video`});
        const state=await runtime.video({blobId:info.blobId,mode:info.playback==="remux"?"remux":"transcode",audio:info.audioCodec,duration:info.duration});
        return send({state:state.state,progress:state.progress,url:state.state==="ready"?`${base}/assets/${id}/video?v=${info.blobId}`:""});
      }
      if(action==="/video"&&["GET","HEAD"].includes(method)){
        if(info.kind!=="video")throw new AppError("notFound",404);
        if(info.playback==="original"||url.searchParams.get("source")==="original")return await sendFile(req,res,blobPath(root,info.blobId),info.mediaType==="video/webm"?"video/webm":"video/mp4",`"${info.blobId}"`,"private, no-store");
        try{return await sendFile(req,res,runtime.videoFile(info.blobId),"video/mp4",`"${info.blobId}-v"`,"private, max-age=3600");}
        catch{throw new AppError("notFound",404,"This video is still being prepared.");}
      }
      if(action==="/original"&&["GET","HEAD"].includes(method)){
        if(!info.allowDownload)throw new AppError("forbidden",403,"The owner turned off downloads for this link.");
        return await blobs.download(req,res,{id:info.id,name:info.name,blobId:info.blobId,size:info.size,mediaType:info.mediaType,revision:info.revision},false);
      }
    }
    // Visitor uploads go into the owner's Photos folder and this link's album.
    if(route==="/uploads"&&method==="POST"){
      const value=await input(["name","size","mediaType","year","month"]);
      return send(await call("beginUpload",value),201);
    }
    const upload=/^\/uploads\/([a-f0-9-]{36})$/.exec(route);
    if(upload&&method==="DELETE")return send(await call("cancelUpload",{id:upload[1]}));
    if(upload&&method==="PUT"){
      const reserved=await call<{id:string;size:number;state:string;fileId:string}>("upload",{id:upload[1]});
      if(reserved.state==="complete"){req.resume();return send({id:reserved.fileId});}
      if(reserved.state!=="uploading")throw new AppError("conflict",409);
      const existing=await blobs.existingDigest(reserved.id,reserved.size);if(existing)req.resume();
      const contentDigest=existing??await blobs.receive(req,reserved.id,reserved.size);
      const {fileId,job}=await call<{fileId:string;job:PhotoJob}>("finishUpload",{id:reserved.id,digest:contentDigest});
      await runtime.indexNow(job).catch(()=>false);
      return send({id:fileId});
    }
    throw new AppError("notFound",404);
  }catch(error){
    // Expired, removed and unknown links look the same.
    if(error instanceof AppError&&error.code==="notFound"&&!route)throw new AppError("notFound",404,"This link is no longer available.");
    throw error;
  }
}
