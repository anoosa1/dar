import type {IncomingMessage,ServerResponse} from "node:http";
import {createReadStream} from "node:fs";
import {stat} from "node:fs/promises";
import {randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import type {BlobFile} from "../files/blobs.ts";
import {AppError} from "../errors.ts";
import type {PhotosRuntime,PhotoJob} from "./runtime.ts";

const messages:Record<string,Record<string,string>>={
  createAlbum:{quotaExceeded:"You have reached the limit of 500 albums."},
  addToAlbum:{quotaExceeded:"An album can hold up to 50,000 items."},
  editAlbum:{conflict:"This album changed elsewhere. Reload it and try again."},
  describe:{conflict:"This description changed elsewhere. Reload it and try again."},
  uploadTarget:{conflict:"A file named Photos (or a year or month inside it) is in the way. Rename it in Files and try again."},
};
const opaque=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
/** Types every browser decodes; used when no thumbnail could be made. */
const renderable=/^image\/(jpeg|png|gif|webp|avif|bmp)$/;

/** Native-session Photos API. The database re-checks the account, ownership and app availability. */
export async function handlePhotosHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,runtime:PhotosRuntime,
  download:(entry:BlobFile)=>Promise<void>,body:()=>Promise<Record<string,unknown>>){
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
  if(route==="/upload-target"&&method==="POST")return send(await call("uploadTarget",await input(["year","month"])));
  if(route==="/albums"&&method==="POST"){const value=await input(["name","ids"]);return send(await call("createAlbum",{id:randomUUID(),...value}),201);}
  const asset=/^\/assets\/([^/]+)(\/[a-z]+)?$/.exec(route);
  if(asset){
    const [,id,action=""]=asset;if(!opaque.test(id))throw new AppError("notFound",404);
    if(!action&&method==="GET")return send(await call("detail",{id}));
    if(action==="/description"&&method==="PUT")return send(await call("describe",{id,...await input(["description","revision"])}));
    if(action==="/index"&&method==="POST"){const {job}=await call<{job:PhotoJob|null}>("indexJob",{id});return send({indexed:await runtime.indexNow(job)});}
    if(action==="/thumbnail"&&["GET","HEAD"].includes(method)){
      const size=url.searchParams.get("size")==="m"?"m":"s";
      const info=await call<{blobId:string;kind:"image"|"video";mediaType:string}>("asset",{id});
      const tag=`"${info.blobId}-${size}"`;
      if(req.headers["if-none-match"]===tag){res.writeHead(304,{etag:tag,"cache-control":"private, max-age=86400"});res.end();return;}
      const file=await runtime.thumbnailFile(info.blobId,info.kind,size);
      if(!file){
        // Without media tools, browser-decodable originals stand in; anything else is a 404 the grid shows as a tile.
        if(info.kind==="image"&&renderable.test(info.mediaType))return await download(await db.call<BlobFile>("service_download",[sessionId,id,""]));
        throw new AppError("notFound",404,"No preview is available for this file.");
      }
      const {size:length}=await stat(file);
      res.writeHead(200,{"content-type":"image/webp","content-length":length,etag:tag,"cache-control":"private, max-age=86400","x-content-type-options":"nosniff","content-security-policy":"default-src 'none'"});
      if(method==="HEAD"){res.end();return;}
      createReadStream(file).on("error",()=>res.destroy()).pipe(res);return;
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
