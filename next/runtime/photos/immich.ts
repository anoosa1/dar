import type {IncomingMessage,ServerResponse} from "node:http";
import {createReadStream} from "node:fs";
import {open,mkdtemp,rm} from "node:fs/promises";
import {createHash,randomUUID} from "node:crypto";
import path from "node:path";
import type {Database} from "../database.ts";
import {blobPath,type blobStore} from "../files/blobs.ts";
import {AppError} from "../errors.ts";
import {createOpaqueToken,digestToken} from "../auth/session.ts";
import {photoKind,type PhotoAlbum,type PhotoAlbumSharing,type PhotoDetail,type PhotoOverview,type PhotoPage} from "../../shared/photos.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";
import {sendFile,type AssetInfo} from "./http.ts";

/**
 * The Immich mobile-app API (server v3.4.1 contract) at /dav/immich/api, found through
 * /.well-known/immich. Sign-in: Dar username followed by "@" and anything, and an app
 * password with Photos access. Photos changes go through the same Photos operations as
 * the web app, acting for that app password's session.
 */
export const immichBase="/dav/immich/api";
export const immichWellKnown={api:{endpoint:immichBase}};
const version={major:3,minor:4,patch:1,prerelease:null,featureLevel:1};
const mediaTypes:Record<string,string>={jpg:"image/jpeg",jpeg:"image/jpeg",png:"image/png",gif:"image/gif",webp:"image/webp",heic:"image/heic",heif:"image/heif",avif:"image/avif",tif:"image/tiff",tiff:"image/tiff",
  dng:"image/x-adobe-dng",cr2:"image/x-canon-cr2",cr3:"image/x-canon-cr3",nef:"image/x-nikon-nef",arw:"image/x-sony-arw",orf:"image/x-olympus-orf",rw2:"image/x-panasonic-rw2",raf:"image/x-fuji-raf",
  mp4:"video/mp4",m4v:"video/mp4",mov:"video/quicktime",webm:"video/webm",mkv:"video/x-matroska","3gp":"video/3gpp",avi:"video/x-msvideo"};
const iso=(ms:number)=>new Date(Math.max(0,ms)).toISOString();
const statusText:Record<number,string>={400:"Bad Request",401:"Unauthorized",403:"Forbidden",404:"Not Found",409:"Conflict",413:"Payload Too Large",429:"Too Many Requests",500:"Internal Server Error",503:"Service Unavailable",507:"Insufficient Storage"};

interface Part {name:string;filename?:string;mediaType?:string;value?:string;file?:{path:string;size:number;sha1:string}}
/**
 * A streaming multipart/form-data reader: small fields in memory (64 KiB in all), one file
 * written to a private temporary directory while its SHA-1 is computed.
 */
async function readMultipart(req:IncomingMessage,boundary:string,dir:string,maxFile:number):Promise<Part[]>{
  const delimiter=Buffer.from(`\r\n--${boundary}`),parts:Part[]=[];let buffer=Buffer.from("\r\n"),fields=0;
  let current:Part|undefined,headers=true,file:Awaited<ReturnType<typeof open>>|undefined,hash:ReturnType<typeof createHash>|undefined,size=0,done=false,chunks:Buffer[]=[];
  const finishPart=async()=>{if(!current)return;if(file){await file.close();file=undefined;current.file={...current.file!,size,sha1:hash!.digest("base64")};}else current.value=Buffer.concat(chunks).toString("utf8");parts.push(current);current=undefined;chunks=[];};
  const write=async(data:Buffer)=>{if(!data.length||!current)return;if(file){size+=data.length;if(size>maxFile)throw new AppError("uploadTooLarge",413);hash!.update(data);let offset=0;while(offset<data.length)offset+=(await file.write(data,offset,data.length-offset)).bytesWritten;}else{fields+=data.length;if(fields>64*1024)throw new AppError("invalidInput");chunks.push(data);}};
  req.setTimeout(60_000,()=>req.destroy(new Error("Upload stalled.")));
  for await (const chunk of req as AsyncIterable<Buffer>){
    if(done)continue;buffer=Buffer.concat([buffer,chunk]);
    for(;;){
      if(headers){
        const start=buffer.indexOf(delimiter);if(start<0||buffer.length<start+delimiter.length+2)break;
        if(buffer.subarray(start+delimiter.length,start+delimiter.length+2).toString()==="--"){done=true;break;}
        const end=buffer.indexOf("\r\n\r\n",start+delimiter.length);if(end<0)break;
        const lines=buffer.subarray(start+delimiter.length+2,end).toString("utf8").split("\r\n");
        const disposition=lines.find(l=>/^content-disposition:/i.test(l))??"",type=lines.find(l=>/^content-type:/i.test(l))?.split(":")[1]?.trim();
        const name=/\bname="([^"]*)"/.exec(disposition)?.[1]??"",filename=/\bfilename="([^"]*)"/.exec(disposition)?.[1];
        current={name,filename,mediaType:type};
        if(filename!==undefined){const target=path.join(dir,`${randomUUID()}.part`);file=await open(target,"wx",0o600);hash=createHash("sha1");size=0;current.file={path:target,size:0,sha1:""};}
        buffer=buffer.subarray(end+4);headers=false;
      }else{
        const at=buffer.indexOf(delimiter);
        if(at<0){const keep=Math.max(0,buffer.length-delimiter.length);await write(buffer.subarray(0,keep));buffer=buffer.subarray(keep);break;}
        await write(buffer.subarray(0,at));await finishPart();buffer=buffer.subarray(at);headers=true;
      }
    }
  }
  if(!done)throw new AppError("invalidInput",400,"The upload ended early.");
  return parts;
}

export async function handleImmichHttp(req:IncomingMessage,res:ServerResponse,url:URL,options:{db:Database;runtime:PhotosRuntime;root:string;blobs:ReturnType<typeof blobStore>;limit:(key:string)=>void;body:()=>Promise<Record<string,unknown>>;origin:string}){
  const {db,runtime,root,blobs}=options,method=req.method??"GET",route=url.pathname.slice(immichBase.length)||"/";
  const send=(value:unknown,status=200)=>{res.writeHead(status,{"content-type":"application/json; charset=utf-8","cache-control":"no-store"});res.end(JSON.stringify(value));};
  try{
    // Public server information the app reads before sign-in.
    if(method==="GET"&&route==="/server/ping")return send({res:"pong"});
    if(method==="GET"&&route==="/server/version")return send(version);
    if(method==="GET"&&route==="/server/features")return send({backups:false,configFile:false,duplicateDetection:true,email:false,facialRecognition:false,importFaces:false,map:false,oauth:false,oauthAutoLaunch:false,ocr:false,passwordLogin:true,realtimeTranscoding:false,reverseGeocoding:false,search:false,sidecar:false,smartSearch:false,trash:true});
    if(method==="GET"&&route==="/server/config")return send({externalDomain:options.origin,isInitialized:true,isOnboarded:true,loginPageMessage:"Sign in with your Dar username followed by @ (for example anas@dar) and an app password with Photos access.",maintenanceMode:false,mapDarkStyleUrl:"",mapLightStyleUrl:"",minFaces:3,oauthButtonText:"",publicUsers:false,trashDays:0,userDeleteDelay:7});
    if(method==="GET"&&route==="/server/media-types")return send({image:Object.entries(mediaTypes).filter(([,t])=>t.startsWith("image/")).map(([e])=>"."+e),video:Object.entries(mediaTypes).filter(([,t])=>t.startsWith("video/")).map(([e])=>"."+e),sidecar:[]});
    if(method==="GET"&&route==="/server/about")return send({version:"v3.4.1",versionUrl:"https://github.com/immich-app/immich/releases/tag/v3.4.1",licensed:false,repository:"Dar (Immich-compatible)"});
    if(method==="POST"&&route==="/auth/login"){
      options.limit("immich-login");
      const p=await options.body(),email=typeof p.email==="string"?p.email.trim():"",password=typeof p.password==="string"?p.password:"";
      const username=email.split("@")[0].toLowerCase();if(!username||!password||password.length>1024)throw new AppError("unauthenticated",401,"Incorrect email or password");
      const token=createOpaqueToken();
      try{
        const user=await db.call<{id:string;email:string;name:string;isAdmin:boolean}>("service_immich_login",[username,digestToken(password),token.digest,String(req.headers.devicetype??req.headers["device-type"]??"Mobile").slice(0,100),String(req.headers.devicemodel??req.headers["device-model"]??"Immich").slice(0,100)]);
        return send({accessToken:token.secret,userId:user.id,userEmail:user.email,name:user.name,isAdmin:user.isAdmin,isOnboarded:true,profileImagePath:"",shouldChangePassword:false},201);
      }catch(error){if(error instanceof AppError&&["unauthenticated","forbidden"].includes(error.code))throw new AppError("unauthenticated",401,error.code==="forbidden"?"This app password has no Photos access.":"Incorrect email or password");throw error;}
    }
    // Everything else needs an Immich access token.
    const cookie=req.headers.cookie?.split(";").map(p=>p.trim()).find(p=>p.startsWith("immich_access_token="))?.slice(19);
    const bearer=/^Bearer (.+)$/i.exec(req.headers.authorization??"")?.[1];
    const token=String(req.headers["x-immich-user-token"]??bearer??cookie??"");
    if(!/^[A-Za-z0-9_-]{43}$/.test(token))throw new AppError("unauthenticated",401,"Authentication required");
    const tokenDigest=digestToken(token);
    const immich=<T=unknown>(operation:string,value:object={})=>db.call<T>("service_immich",[tokenDigest,operation,JSON.stringify(value)]);
    const {user,protocolSessionId}=await immich<{user:{id:string;email:string;name:string;isAdmin:boolean};protocolSessionId:string}>("session");
    const photos=<T=unknown>(operation:string,value:object={})=>db.call<T>("service_photos",[protocolSessionId,operation,JSON.stringify(value)]);
    const input=async()=>await options.body();
    if(method==="POST"&&route==="/auth/validateToken")return send({authStatus:true});
    if(method==="GET"&&route==="/auth/status")return send({password:true,pinCode:false,isElevated:false});
    if(method==="POST"&&route==="/auth/logout"){await immich("logout");return send({successful:true,redirectUri:"/auth/login?autoLaunch=0"});}
    if(method==="GET"&&(route==="/users/me"||route==="/users/me/"))return send(user);
    if(method==="GET"&&route==="/users")return send([user]);
    if(route==="/users/me/preferences")return send(preferences());
    if(method==="GET"&&route==="/server/storage"){const used=(user as {quotaUsageInBytes?:number}).quotaUsageInBytes??0,size=10*1024**3;return send({diskSize:"10 GiB",diskUse:`${(used/1024**3).toFixed(1)} GiB`,diskAvailable:`${((size-used)/1024**3).toFixed(1)} GiB`,diskSizeRaw:size,diskUseRaw:used,diskAvailableRaw:size-used,diskUsagePercentage:Math.round(used/size*10000)/100});}
    if(method==="GET"&&route==="/server/version-check")return send({checkedAt:new Date().toISOString(),releaseVersion:"v3.4.1"});
    if(method==="GET"&&["/partners","/memories","/tags","/notifications","/timeline/buckets","/stacks","/shared-links"].includes(route))return send([]);
    if(method==="GET"&&route==="/people")return send({people:[],total:0,hidden:0,hasNextPage:false});
    if(method==="GET"&&route==="/sessions")return send([]);

    // Sync: checkpoints, then each requested type's channels as JSON lines, then completion.
    if(method==="POST"&&route==="/sync/stream"){
      const p=await input(),types=Array.isArray(p.types)?p.types.filter((t):t is string=>typeof t==="string"):[];
      const order=["AuthUsersV1","AuthUsersV2","UsersV1","PartnersV1","AssetsV2","StacksV1","PartnerAssetsV2","PartnerStacksV1","AlbumAssetsV2","AlbumsV2","AlbumUsersV1","AlbumToAssetsV1","AssetExifsV1","AlbumAssetExifsV1"];
      const channels:Record<string,string[]>={AuthUsersV1:["authUserV1"],AuthUsersV2:["authUser"],UsersV1:["users"],AssetsV2:["assetDeletes","assets"],StacksV1:["stackDeletes","stacks"],AlbumAssetsV2:["albumAssets"],AlbumsV2:["albumDeletes","albums"],AlbumUsersV1:["albumUserDeletes","albumUsers"],AlbumToAssetsV1:["albumToAssetDeletes","albumToAssets"],AssetExifsV1:["exifs"],AlbumAssetExifsV1:["albumAssetExifs"]};
      const acks:Record<string,string>={authUserV1:"AuthUserV1",authUser:"AuthUserV2",users:"UserV1",assetDeletes:"AssetDeleteV1",assets:"AssetV2",stackDeletes:"StackDeleteV1",stacks:"StackV1",albumAssets:"AlbumAssetCreateV2",albumDeletes:"AlbumDeleteV1",albums:"AlbumV2",albumUserDeletes:"AlbumUserDeleteV1",albumUsers:"AlbumUserV1",albumToAssetDeletes:"AlbumToAssetDeleteV1",albumToAssets:"AlbumToAssetV1",exifs:"AssetExifV1",albumAssetExifs:"AlbumAssetExifCreateV1"};
      let start:{reset:boolean;checkpoints:Record<string,string>;nowId:string;backfilling:boolean};let rounds=0;
      do{start=await immich("syncStart",{reset:p.reset===true&&rounds===0});}while(start.backfilling&&++rounds<200);
      res.writeHead(200,{"content-type":"application/jsonlines+json; charset=utf-8","cache-control":"no-store"});
      const line=(value:unknown)=>new Promise<void>(resolve=>{if(res.write(JSON.stringify(value)+"\n"))resolve();else res.once("drain",resolve);});
      if(start.reset){await line({type:"SyncResetV1",data:{},ack:"SyncResetV1|reset"});res.end();return;}
      const seqOf=(ack:string|undefined)=>{const id=ack?.split("|")[1]??"";return /^[a-f0-9-]{36}$/.test(id)?parseInt(id.slice(24),16):-1;};
      for(const type of order){
        if(!types.includes(type)||!channels[type])continue;
        for(const channel of channels[type]){
          let after=seqOf(start.checkpoints[acks[channel]]);
          for(let page=0;page<10_000;page++){
            if(res.destroyed)return;
            const result=await immich<{lines:unknown[];last:number;more:boolean}>("syncPage",{channel,after,limit:500});
            for(const value of result.lines)await line(value);
            after=result.last;if(!result.more)break;
          }
        }
      }
      await line({type:"SyncCompleteV1",data:{},ack:`SyncCompleteV1|${start.nowId}`});res.end();return;
    }
    if(route==="/sync/ack"){
      if(method==="GET")return send(await immich("acks"));
      if(method==="POST"){const p=await input();await immich("setAcks",{acks:p.acks});res.writeHead(204);res.end();return;}
      if(method==="DELETE"){const p=req.headers["content-length"]&&req.headers["content-length"]!=="0"?await input():{};await immich("deleteAcks",{types:p.types});res.writeHead(204);res.end();return;}
    }

    // Backup: duplicate check by SHA-1, then multipart uploads into the Photos folder.
    if(method==="POST"&&route==="/assets/bulk-upload-check")return send(await immich("bulkCheck",{assets:(await input()).assets}));
    if(method==="POST"&&route==="/assets"){
      const boundary=/boundary=(?:"([^"]+)"|([^;]+))/i.exec(req.headers["content-type"]??"");if(!boundary)throw new AppError("invalidInput",400,"Expected multipart form data.");
      const dir=await mkdtemp(path.join(root,"uploads","immich-"));
      try{
        const parts=await readMultipart(req,boundary[1]??boundary[2],dir,1024**3),field=(name:string)=>parts.find(p=>p.name===name&&p.value!==undefined)?.value;
        const data=parts.find(p=>p.name==="assetData"&&p.file);if(!data?.file)throw new AppError("invalidInput",400,"assetData is required.");
        const name=(field("filename")||data.filename||"upload").replace(/[\\/]/g,"_").slice(0,255),ext=name.split(".").pop()?.toLowerCase()??"";
        const mediaType=data.mediaType&&data.mediaType!=="application/octet-stream"?data.mediaType:mediaTypes[ext]??"application/octet-stream";
        if(!photoKind(mediaType,name))throw new AppError("invalidInput",400,"Only photos and videos can be backed up.");
        const created=/^(\d{4})-(\d{2})/.exec(field("fileCreatedAt")??"")??/^(\d{4})-(\d{2})/.exec(new Date().toISOString())!;
        const begun=await immich<{duplicate?:string;id?:string;size?:number}>("uploadBegin",{sha1:data.file.sha1,name,size:data.file.size,mediaType,year:Number(created[1]),month:Number(created[2])});
        if(begun.duplicate)return send({id:begun.duplicate,status:"duplicate"},200);
        const digest=await blobs.receive(createReadStream(data.file.path) as AsyncIterable<Buffer>,begun.id!,data.file.size);
        const time=Date.parse(field("fileCreatedAt")??"");
        const {fileId,job}=await immich<{fileId:string;job:PhotoJob}>("uploadFinish",{id:begun.id,digest,...Number.isFinite(time)?{fileTime:time}:{}});
        await runtime.indexNow(job).catch(()=>false);
        const favorite=field("isFavorite")==="true",archived=field("visibility")==="archive";
        if(favorite||archived)await photos("update",{ids:[fileId],...favorite?{favorite:true}:{},...archived?{archived:true}:{}}).catch(()=>undefined);
        return send({id:fileId,status:"created"},201);
      }finally{await rm(dir,{recursive:true,force:true});}
    }
    if(route==="/assets"&&(method==="PUT"||method==="DELETE")){
      const p=await input(),ids=Array.isArray(p.ids)?p.ids:[];
      if(method==="PUT"){await updateAssets(ids,p);res.writeHead(204);res.end();return;}
      // Immich moves to the trash; force deletes for good.
      await photos("trash",{ids}).catch(error=>{if(!(error instanceof AppError&&error.code==="notFound"))throw error;});
      if(p.force===true)await photos("delete",{ids});
      res.writeHead(204);res.end();return;
    }
    if(method==="POST"&&route==="/trash/restore/assets"){const p=await input();await photos("restore",{ids:p.ids});return send({count:Array.isArray(p.ids)?p.ids.length:0});}
    if(method==="POST"&&(route==="/trash/restore"||route==="/trash/empty")){
      const page=await photos<PhotoPage>("page",{view:"trash",limit:500}),ids=page.items.map(i=>i.id);
      if(ids.length)await photos(route==="/trash/restore"?"restore":"delete",{ids});return send({count:ids.length});
    }
    const asset=/^\/assets\/([a-f0-9-]{36})(\/[a-z]+(?:\/[a-z]+)?)?$/.exec(route);
    if(asset){
      const [,id,action=""]=asset;
      if(!action&&method==="GET")return send(assetResponse(await photos<PhotoDetail>("detail",{id}),user.id));
      if(!action&&method==="PUT"){await updateAssets([id],await input());return send(assetResponse(await photos<PhotoDetail>("detail",{id}),user.id));}
      const info=await photos<AssetInfo>("asset",{id});
      if(action==="/thumbnail"&&["GET","HEAD"].includes(method)){
        const size=url.searchParams.get("size")==="thumbnail"||!url.searchParams.get("size")?"s":"m",file=await runtime.thumbnailFile(info.blobId,info.kind,size);
        if(file)return await sendFile(req,res,file,"image/webp",`"${info.blobId}-${size}"`,"private, max-age=86400");
        if(info.kind==="image"&&/^image\/(jpeg|png|gif|webp)$/.test(info.mediaType))return await sendFile(req,res,blobPath(root,info.blobId),info.mediaType,`"${info.blobId}"`,"private, max-age=86400");
        throw new AppError("notFound",404,"No preview is available for this file.");
      }
      if(action==="/original"&&["GET","HEAD"].includes(method))return await blobs.download(req,res,{id:info.id,name:info.name,blobId:info.blobId,size:info.size,mediaType:info.mediaType,revision:info.revision},false);
      if(action==="/video/playback"&&["GET","HEAD"].includes(method)){
        if(info.kind!=="video")throw new AppError("notFound",404);
        // Converted copies once ready; meanwhile the original (iOS and many Android devices play HEVC).
        if(info.playback==="remux"||info.playback==="transcode"){const state=await runtime.video({blobId:info.blobId,mode:info.playback,audio:info.audioCodec,duration:info.duration});if(state.state==="ready"&&state.file)return await sendFile(req,res,state.file,"video/mp4",`"${info.blobId}-v"`,"private, max-age=86400");}
        return await sendFile(req,res,blobPath(root,info.blobId),info.mediaType==="video/webm"?"video/webm":"video/mp4",`"${info.blobId}"`,"private, no-store");
      }
    }

    // Albums.
    if(route==="/albums"&&method==="GET"){
      const overview=await photos<PhotoOverview>("overview"),shared=url.searchParams.get("shared");
      const list=shared==="true"?[...overview.albums.filter(a=>a.shared),...overview.sharedAlbums]:shared==="false"?overview.albums.filter(a=>!a.shared):[...overview.albums,...overview.sharedAlbums];
      return send(await Promise.all(list.map(a=>albumResponse(a))));
    }
    if(route==="/albums"&&method==="POST"){const p=await input();const album=await photos<PhotoAlbum>("createAlbum",{id:randomUUID(),name:String(p.albumName??"Untitled"),...Array.isArray(p.assetIds)&&p.assetIds.length?{ids:p.assetIds}:{}});return send(await albumResponse(album),201);}
    if(method==="PUT"&&route==="/albums/assets"){
      const p=await input(),albumIds=Array.isArray(p.albumIds)?p.albumIds:[],ids=Array.isArray(p.assetIds)?p.assetIds:[];
      for(const albumId of albumIds)await photos("addToAlbum",{id:albumId,ids});return send({success:true});
    }
    const album=/^\/albums\/([a-f0-9-]{36})(\/assets)?$/.exec(route);
    if(album){
      const [,id,assets]=album,find=async()=>{const o=await photos<PhotoOverview>("overview"),a=[...o.albums,...o.sharedAlbums].find(x=>x.id===id);if(!a)throw new AppError("notFound",404);return a;};
      if(!assets&&method==="GET")return send(await albumResponse(await find(),url.searchParams.get("withoutAssets")!=="true"));
      if(!assets&&method==="PATCH"){const p=await input(),current=await find();await photos("editAlbum",{id,revision:current.revision,...typeof p.albumName==="string"?{name:p.albumName}:{},...typeof p.description==="string"?{description:p.description}:{},...typeof p.albumThumbnailAssetId==="string"?{coverId:p.albumThumbnailAssetId}:{}});return send(await albumResponse(await find()));}
      if(!assets&&method==="DELETE"){await photos("deleteAlbum",{id});res.writeHead(204);res.end();return;}
      if(assets&&(method==="PUT"||method==="DELETE")){
        const p=await input(),ids=(Array.isArray(p.ids)?p.ids:[]).filter((x):x is string=>typeof x==="string");
        const before=new Set((await photos<PhotoPage>("page",{view:"album",album:id,limit:500})).items.map(i=>i.id));
        await photos(method==="PUT"?"addToAlbum":"removeFromAlbum",{id,ids});
        return send(ids.map(assetId=>method==="PUT"?(before.has(assetId)?{id:assetId,success:false,error:"duplicate"}:{id:assetId,success:true}):(before.has(assetId)?{id:assetId,success:true}:{id:assetId,success:false,error:"not_found"})));
      }
    }
    throw new AppError("notFound",404,`${method} ${route} is not available on this server.`);

    async function updateAssets(ids:unknown[],p:Record<string,unknown>){
      const change:Record<string,unknown>={};
      if(typeof p.isFavorite==="boolean")change.favorite=p.isFavorite;
      if(p.visibility==="archive"||p.visibility==="timeline")change.archived=p.visibility==="archive";
      if(Object.keys(change).length)await photos("update",{ids,...change});
      if(typeof p.description==="string"&&ids.length===1){const d=await photos<PhotoDetail>("detail",{id:ids[0]});await photos("describe",{id:ids[0],description:p.description,revision:d.revision});}
    }
    async function albumResponse(a:PhotoAlbum,withAssets=false){
      const people:{user:unknown;role:string}[]=[{user:a.owner?{id:a.owner.id,name:a.owner.name,email:"",profileImagePath:"",avatarColor:"primary",profileChangedAt:iso(0)}:user,role:"owner"}];
      if(a.role==="owner"&&a.shared){const sharing=await photos<PhotoAlbumSharing>("albumSharing",{id:a.id});for(const m of sharing.members)people.push({user:{id:m.userId,name:m.displayName,email:`${m.username}@dar`,profileImagePath:"",avatarColor:"primary",profileChangedAt:iso(0)},role:m.role});}
      const assets=withAssets?(await photos<PhotoPage>("page",{view:"album",album:a.id,limit:500})).items:[];
      return {id:a.id,albumName:a.name,description:a.description,albumThumbnailAssetId:a.coverId||null,albumUsers:people,assetCount:a.count,createdAt:iso(a.updatedAt),updatedAt:iso(a.updatedAt),
        hasSharedLink:false,shared:a.shared||!!a.owner,isActivityEnabled:false,order:"desc",ownerId:a.owner?.id??user.id,...withAssets?{assets:assets.map(i=>({id:i.id,type:i.kind==="video"?"VIDEO":"IMAGE",originalFileName:i.name,fileCreatedAt:iso(i.takenAt),isFavorite:i.favorite}))}:{}};
    }
  }catch(error){
    if(res.headersSent){res.destroy();return;}
    const failure=error instanceof AppError?error:new AppError("internalError",500,"The request could not be completed.");
    const status=failure.status===423?404:failure.status;
    res.writeHead(status,{"content-type":"application/json; charset=utf-8","cache-control":"no-store"});
    res.end(JSON.stringify({message:failure.message===failure.code?statusText[status]??failure.code:failure.message,error:statusText[status]??"Error",statusCode:status,correlationId:randomUUID().slice(0,8)}));
  }
}
function assetResponse(d:PhotoDetail,viewer:string){
  const e=d.exif;
  return {id:d.id,deviceAssetId:d.id,deviceId:"dar",ownerId:d.owner?"":viewer,originalFileName:d.name,originalMimeType:d.mediaType,originalPath:"",checksum:d.checksum??"",thumbhash:null,type:d.kind==="video"?"VIDEO":"IMAGE",
    fileCreatedAt:iso(d.takenAt),fileModifiedAt:iso(d.modifiedAt),localDateTime:iso(d.takenAt),createdAt:iso(d.modifiedAt),updatedAt:iso(d.modifiedAt),isFavorite:d.favorite,isArchived:d.archived,isTrashed:!!d.trashedAt,isOffline:false,isEdited:false,
    visibility:d.archived?"archive":"timeline",hasMetadata:true,duration:d.kind==="video"?Math.round(d.duration*1000):null,width:d.width||null,height:d.height||null,livePhotoVideoId:d.live??null,people:[],tags:[],
    exifInfo:{make:e.make??null,model:e.model??null,lensModel:e.lens??null,fNumber:e.fNumber??null,focalLength:e.focalLength??null,iso:e.iso??null,exposureTime:e.exposure?(e.exposure<1?`1/${Math.round(1/e.exposure)}`:String(e.exposure)):null,
      latitude:d.latitude,longitude:d.longitude,description:d.description,dateTimeOriginal:iso(d.takenAt),exifImageWidth:d.width||null,exifImageHeight:d.height||null,fileSizeInByte:d.size,orientation:null,timeZone:null,city:null,state:null,country:null}};
}
/** Immich's per-user preferences, at defaults; Dar keeps its own settings. */
function preferences(){
  return {albums:{defaultAssetOrder:"desc"},cast:{gCastEnabled:false},download:{archiveSize:4294967296,includeEmbeddedVideos:false},emailNotifications:{enabled:false,albumInvite:false,albumUpdate:false},
    folders:{enabled:false,sidebarWeb:false},memories:{enabled:false,duration:5},people:{enabled:false,sidebarWeb:false},purchase:{showSupportBadge:false,hideBuyButtonUntil:"2099-01-01T00:00:00.000Z"},
    ratings:{enabled:false},recentlyAdded:{enabled:true,sidebarWeb:false},sharedLinks:{enabled:false,sidebarWeb:false},tags:{enabled:false,sidebarWeb:false}};
}
export {iso as immichIso};
