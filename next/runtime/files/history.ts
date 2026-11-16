import type {IncomingMessage,ServerResponse} from "node:http";
import {spawn} from "node:child_process";
import {constants,createReadStream} from "node:fs";
import {mkdtemp,open,rm,stat} from "node:fs/promises";
import {tmpdir} from "node:os";
import {once} from "node:events";
import path from "node:path";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {writeJson} from "../json-response.ts";
import {blobPath,type BlobFile,type blobStore} from "./blobs.ts";
import {historyReadOperations} from "../../shared/read-operations.ts";
import {historyLimits} from "../../shared/history.ts";

type Blobs=ReturnType<typeof blobStore>;
interface ExportChange {path:string;previousPath:string;kind:string;blobId?:string;size?:number}
interface ExportData {name:string;changesets:{sequence:number;message:string;createdAt:number;author:{name:string;username:string};changes:ExportChange[]}[]}

/** Folder history under /api/v1/files/history/<folderId>; the database checks access and revisions. */
export async function handleHistoryHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,root:string,blobs:Blobs,body:()=>Promise<Record<string,unknown>>){
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
  if(rest==="/bundle"&&method==="GET")return sendBundle(res,root,await call<ExportData>("export"));
  throw new AppError("notFound",404);
}

/** Runs Git with no user or system configuration, hooks or network. */
function git(cwd:string,args:string[],input?:(stdin:NodeJS.WritableStream)=>Promise<void>){
  const child=spawn("git",["-c","core.hooksPath=/dev/null","-c","protocol.allow=never",...args],{cwd,stdio:[input?"pipe":"ignore","ignore","pipe"],env:{PATH:process.env.PATH,HOME:cwd,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",GIT_TERMINAL_PROMPT:"0"}});
  const timer=setTimeout(()=>child.kill("SIGKILL"),300_000);
  child.stderr!.resume();
  return new Promise<void>((resolve,reject)=>{
    let failure:unknown;
    child.once("error",()=>{failure=new AppError("gitUnavailable",503,"Git is not available on this server.");});
    child.once("close",code=>{clearTimeout(timer);if(failure||code!==0)reject(failure??new AppError("exportFailed",502,"The Git export could not be created."));else resolve();});
    if(input)input(child.stdin!).then(()=>child.stdin!.end(),error=>{failure=error;child.kill("SIGKILL");});
  });
}
/** C-style quoting keeps any file name intact in a fast-import stream. */
const quote=(value:string)=>`"${value.replace(/[\\"]/g,c=>"\\"+c)}"`;
const person=(author:{name:string;username:string},at:number)=>`${author.name.replace(/[<>\n]/g,"").trim()||author.username} <${author.username.replace(/[<>\s]/g,"")}@dar.invalid> ${Math.floor(at/1000)} +0000`;

/** main as a Git bundle: `git clone <name>.bundle` restores the history as commits. */
async function sendBundle(res:ServerResponse,root:string,data:ExportData){
  if(!data.changesets.length)throw new AppError("nothingToExport",409,"Commit something before exporting.");
  const sizes=new Map<string,number>();for(const set of data.changesets)for(const change of set.changes)if(change.blobId)sizes.set(change.blobId,change.size!);
  if([...sizes.values()].reduce((sum,size)=>sum+size,0)>historyLimits.exportBytes)throw new AppError("operationTooLarge",413,"This history is too large to export as one bundle.");
  const work=await mkdtemp(path.join(tmpdir(),"dar-history-"));
  try{
    await git(work,["init","--bare","-q","-b","main","repo.git"]);
    await git(path.join(work,"repo.git"),["fast-import","--quiet","--done"],async stdin=>{
      const write=async(chunk:string|Buffer)=>{if(!stdin.write(chunk))await once(stdin,"drain");};
      const marks=new Map<string,number>();let mark=0,previous=0;
      for(const set of data.changesets){
        for(const change of set.changes)if(change.blobId&&!marks.has(change.blobId)){
          marks.set(change.blobId,++mark);await write(`blob\nmark :${mark}\ndata ${change.size}\n`);
          const file=await open(blobPath(root,change.blobId),constants.O_RDONLY|constants.O_NOFOLLOW);
          try{if((await file.stat()).size!==change.size)throw new AppError("contentUnavailable",503);for await(const chunk of file.createReadStream({autoClose:false}))await write(chunk as Buffer);}finally{await file.close();}
          await write("\n");
        }
        const message=Buffer.from(set.message+"\n"),who=person(set.author,set.createdAt);
        await write(`commit refs/heads/main\nmark :${++mark}\nauthor ${who}\ncommitter ${who}\ndata ${message.length}\n`);await write(message);
        if(previous)await write(`from :${previous}\n`);
        for(const change of set.changes){
          if(change.kind==="deleted")await write(`D ${quote(change.path)}\n`);
          else{if(change.kind==="renamed")await write(`D ${quote(change.previousPath)}\n`);await write(`M 100644 :${marks.get(change.blobId!)} ${quote(change.path)}\n`);}
        }
        await write("\n");previous=mark;
      }
      await write("done\n");
    });
    await git(path.join(work,"repo.git"),["bundle","create","-q","../history.bundle","HEAD","main"]);
    const file=path.join(work,"history.bundle"),size=(await stat(file)).size,name=`${data.name.replace(/[^\w .-]+/g,"").trim()||"history"}.bundle`;
    res.writeHead(200,{"content-type":"application/octet-stream","content-length":size,"cache-control":"private, no-store","content-disposition":`attachment; filename*=UTF-8''${encodeURIComponent(name)}`});
    const stream=createReadStream(file);stream.pipe(res);await once(stream,"close");
  }finally{await rm(work,{recursive:true,force:true});}
}
