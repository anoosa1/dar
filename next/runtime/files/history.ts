import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {writeJson} from "../json-response.ts";
import type {BlobFile,blobStore} from "./blobs.ts";
import type {GitRepositories} from "./git.ts";
import {historyReadOperations} from "../../shared/read-operations.ts";

type Blobs=ReturnType<typeof blobStore>;

/** Folder history under /api/v1/files/history/<folderId>; the database checks access and revisions. */
export async function handleHistoryHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,repositories:GitRepositories,blobs:Blobs,body:()=>Promise<Record<string,unknown>>){
  const match=/^\/api\/v1\/files\/history\/([a-f0-9-]{36})(\/.*)?$/.exec(url.pathname);if(!match)throw new AppError("notFound",404);
  const folderId=match[1],rest=match[2]??"",method=req.method??"GET";
  const call=<T=unknown>(operation:string,document:Record<string,unknown>={})=>db.operation<T>("service_history",operation,historyReadOperations,[sessionId,operation,JSON.stringify({...document,folderId})]);
  const send=(value:unknown,status=200)=>writeJson(res,status,JSON.stringify(value));
  const input=async(allowed:string[])=>{const p=await body();if(Object.keys(p).some(key=>!allowed.includes(key)))throw new AppError("invalidInput");return p;};
  if(!rest&&method==="GET"){
    const before=url.searchParams.get("before");if(before!==null&&!/^[1-9]\d{0,9}$/.test(before))throw new AppError("invalidInput");
    return send(await call("overview",before?{before:Number(before)}:{}));
  }
  if(!rest&&method==="POST"){
    const p=await input(["action","revision"]);
    if(p.action==="track")return send(await call("track"));
    if(p.action==="untrack")return send(await call("untrack",{revision:p.revision}));
    throw new AppError("invalidInput");
  }
  if(rest==="/commits"&&method==="POST")return send(await call("commit",await input(["message","token","paths","snapshot"])),201);
  const changeset=/^\/commits\/([a-f0-9-]{36})$/.exec(rest);
  if(changeset&&method==="GET")return send(await call("changeset",{id:changeset[1]}));
  if(rest==="/restore"&&method==="POST")return send(await call("restore",await input(["id","revision"])),201);
  const branch=/^\/branches\/(merge|discard)$/.exec(rest);
  if(branch&&method==="POST")return send(await call(branch[1],await input(["name","revision"])));
  const version=/^\/versions\/([a-f0-9-]{36})\/content$/.exec(rest);
  if(version&&(method==="GET"||method==="HEAD"))return blobs.download(req,res,await call<BlobFile>("content",{versionId:version[1]}));
  // The same commits a clone gets (docs/spacetimedb/folder-history.md#git).
  if(rest==="/bundle"&&method==="GET")return repositories.bundle(sessionId,folderId,res,(await call<{folder:{name:string}}>("overview")).folder.name);
  throw new AppError("notFound",404);
}

