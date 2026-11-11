import {constants} from "node:fs";
import {mkdir,lstat,open,readdir,writeFile,rm} from "node:fs/promises";
import {spawn} from "node:child_process";
import {isUtf8} from "node:buffer";
import path from "node:path";
import type {Database} from "../database.ts";
import type {AgentProject,AgentChange} from "../../shared/agent.ts";
import type {BlobFile} from "../files/blobs.ts";
import {blobPath} from "../files/blobs.ts";
import {createDurableFile} from "../atomic.ts";
import {AppError} from "../errors.ts";
import type {CodexConfig} from "./codex.ts";
export interface Source {project:AgentProject;files:{id:string;path:string;revision:number;size:number;mediaType:string}[]}
interface Baseline {source:Source;files:{path:string;content:string}[]}

async function directory(value:string){
  await mkdir(value,{recursive:true,mode:0o700});const stat=await lstat(value);if(!stat.isDirectory()||stat.isSymbolicLink())throw new AppError("unsafeWorkspace",409);
}
function safePath(relative:string){if(!relative||relative.length>2048||relative.split("/").some(p=>!p||p==="."||p===".."||p===".git"||p===".codex"||/[\x00-\x1f\\]/.test(p)))throw new AppError("unsafeWorkspace",409);return relative;}
export async function git(workspace:string,args:string[],signal:AbortSignal){
  signal.throwIfAborted();
  const child=spawn("git",["-c","core.hooksPath=/dev/null","-c","core.fsmonitor=false","-c","protocol.file.allow=never","-c","http.followRedirects=false",...args],{cwd:workspace,detached:true,stdio:["ignore","pipe","pipe"],env:{PATH:process.env.PATH,HOME:workspace,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",GIT_TERMINAL_PROMPT:"0"}});
  const output:Buffer[]=[];let bytes=0,stopped=false,reason:Error|undefined,checking:Promise<void>|undefined;
  const stop=(error:Error)=>{reason??=error;try{if(child.pid)process.kill(-child.pid,"SIGKILL");}catch{}};
  const abort=()=>stop(new AppError("runStopped",409));signal.addEventListener("abort",abort,{once:true});
  const timeout=setTimeout(()=>stop(new AppError("gitTimedOut",504)),60000);
  child.stdout.on("data",(chunk:Buffer)=>{bytes+=chunk.length;if(bytes>1024*1024)stop(new AppError("operationTooLarge",413));else output.push(chunk);});
  child.stderr.on("data",(chunk:Buffer)=>{bytes+=chunk.length;if(bytes>1024*1024)stop(new AppError("operationTooLarge",413));});
  // Bound provisioning growth as well as the final materialized source. This is a
  // periodic guard, not a substitute for an operator's filesystem disk quota.
  const timer=setInterval(()=>{
    if(checking||stopped)return;
    checking=(async()=>{
      let size=0,count=0;
      async function visit(directory:string){for(const item of await readdir(directory,{withFileTypes:true})){
        if(stopped)return;if(++count>10000)throw new AppError("workspaceTooLarge",413);
        const filename=path.join(directory,item.name),stat=await lstat(filename);if(stat.isSymbolicLink())continue;
        if(stat.isDirectory())await visit(filename);else{size+=stat.size;if(stat.size>16*1024*1024||size>64*1024*1024)throw new AppError("workspaceTooLarge",413);}
      }}
      await visit(workspace);
    })().catch(error=>{if(!stopped)stop(error);}).finally(()=>{checking=undefined;});
  },500);
  try{return await new Promise<{stdout:string}>((resolve,reject)=>{
    child.once("error",()=>{reason=new AppError("gitUnavailable",503);});
    child.once("close",code=>{stopped=true;if(reason||code!==0)reject(reason??new AppError("gitFailed",502));else resolve({stdout:Buffer.concat(output).toString("utf8")});});
  });}finally{stopped=true;clearTimeout(timeout);clearInterval(timer);signal.removeEventListener("abort",abort);await checking;}
}

/** Reject links/special files and bound scans before any file is read into a result. */
async function scan(root:string){
  const result=new Map<string,Buffer>();let bytes=0,visited=0;
  async function walk(relative:string,depth:number){
    if(depth>32)throw new AppError("workspaceTooLarge",413);
    for(const row of await readdir(path.join(root,relative),{withFileTypes:true})){
      if(++visited>500)throw new AppError("workspaceTooLarge",413);
      if(row.name===".git"||row.name===".codex")continue;
      const name=safePath(relative+row.name),filename=path.join(root,name),stat=await lstat(filename);
      if(stat.isSymbolicLink()||!stat.isFile()&&!stat.isDirectory())throw new AppError("unsafeWorkspace",409,"Projects cannot contain symbolic links or special files.");
      if(stat.isDirectory()){await walk(name+"/",depth+1);continue;}
      bytes+=stat.size;if(result.size>=200||stat.size>1024*1024||bytes>8*1024*1024)throw new AppError("workspaceTooLarge",413,"This workspace exceeds 200 files, 1 MiB per file or 8 MiB total.");
      const file=await open(filename,constants.O_RDONLY|constants.O_NOFOLLOW);try{
        if(!(await file.stat()).isFile())throw new AppError("unsafeWorkspace",409);
        const buffer=Buffer.alloc(stat.size+1);let offset=0;
        while(offset<buffer.length){const {bytesRead}=await file.read(buffer,offset,buffer.length-offset,offset);if(!bytesRead)break;offset+=bytesRead;}
        if(offset!==stat.size||(await file.stat()).size!==stat.size)throw new AppError("workspaceChanged",409,"A workspace file changed while it was being inspected.");
        result.set(name,buffer.subarray(0,offset));
      }finally{await file.close();}
    }
  }
  await walk("",0);return result;
}

export async function prepareWorkspace(root:string,userId:string,threadId:string,source:Source,config:CodexConfig,db:Database,sessionId:string,signal:AbortSignal){
  const owner=path.join(root,"agent-workspaces",userId),thread=path.join(owner,threadId),workspace=path.join(thread,"work"),baselinePath=path.join(thread,"baseline.json");
  for(const value of [path.join(root,"agent-workspaces"),owner,thread])await directory(value);
  let baseline:Baseline|undefined;
  try{const file=await open(baselinePath,constants.O_RDONLY|constants.O_NOFOLLOW);try{baseline=JSON.parse(await file.readFile("utf8")) as Baseline;}finally{await file.close();}}catch(error){if((error as NodeJS.ErrnoException).code!=="ENOENT")throw error;}
  if(baseline){
    await directory(workspace);
    if(baseline.source.project.sourceType!==source.project.sourceType||baseline.source.project.sourceRef!==source.project.sourceRef||source.project.sourceType==="files"&&JSON.stringify(baseline.source.files)!==JSON.stringify(source.files))throw new AppError("sourceChanged",409,"The source changed after this thread’s workspace was created. Start a new thread to use the current source.");
    await scan(workspace);
  }else{
    // Never overwrite a partial/unknown working copy left by a failed provisioning attempt.
    try{await lstat(workspace);throw new AppError("workspaceIncomplete",409,"Workspace setup was interrupted. Start a new thread; existing files were retained.");}catch(error){if((error as NodeJS.ErrnoException).code!=="ENOENT")throw error;}
    if(source.project.sourceType==="git"){
      const url=source.project.sourceRef;
      if(!url.startsWith("https://"))throw new AppError("gitAuthenticationUnavailable",409,"Use a credential-free HTTPS repository. SSH credential provisioning is not connected yet.");
      if(!config.gitHosts.includes(new URL(url).hostname))throw new AppError("gitHostDenied",403,"This repository host is not enabled by the administrator.");
      try{await git(thread,["clone","--depth=1","--single-branch","--no-hardlinks","--",url,"work"],signal);}catch{throw new AppError("cloneFailed",502,"The repository could not be cloned. Verify the URL, allowed host and public access.");}
      const tree=await git(workspace,["ls-tree","-r","HEAD"],signal);if(tree.stdout.split("\n").some(line=>line.startsWith("160000 ")))throw new AppError("submoduleUnavailable",409,"Submodules are not materialized yet. Choose a repository without submodules.");
      // Repository config is not execution authority. Only the isolated copy is filtered.
      await rm(path.join(workspace,".codex"),{recursive:true,force:true});
    }else{
      await directory(workspace);
      for(const row of source.files){
        signal.throwIfAborted();const entry=await db.call<BlobFile>("service_download",[sessionId,row.id,""]);
        if(entry.revision!==row.revision||entry.size!==row.size)throw new AppError("conflict",409,"A source file changed during workspace setup. Start a new thread.");
        const destination=path.join(workspace,safePath(row.path));await directory(path.dirname(destination));
        const input=await open(blobPath(root,entry.blobId),constants.O_RDONLY|constants.O_NOFOLLOW);
        try{const stat=await input.stat();if(!stat.isFile()||stat.size!==row.size)throw new AppError("contentUnavailable",503);await writeFile(destination,await input.readFile(),{flag:"wx",mode:0o600});}finally{await input.close();}
      }
      await git(workspace,["init","-b","agent"],signal);
      await git(workspace,["add","--all"],signal);
      await git(workspace,["-c","user.name=Dar","-c","user.email=agent@dar.invalid","commit","--allow-empty","-m","Dar workspace baseline"],signal);
    }
    baseline={source,files:[...await scan(workspace)].map(([name,data])=>({path:name,content:data.toString("base64")}))};
    await createDurableFile(baselinePath,JSON.stringify(baseline));
  }
  // Cached workspaces from older Dar versions may predate this deny root.
  // Both 0.163 and 0.166 need it to exist before bubblewrap masks the directory.
  await directory(path.join(workspace,".codex"));
  return {workspace,async validate(){await scan(workspace);},async changes(){
    const after=await scan(workspace),before=new Map(baseline!.files.map(row=>[row.path,Buffer.from(row.content,"base64")])),changes:AgentChange[]=[];let remaining=180000,truncated=false;
    for(const name of new Set([...before.keys(),...after.keys()])){
      const old=before.get(name),next=after.get(name);if(old&&next&&old.equals(next))continue;
      const kind=old?next?"modified":"deleted":"added",text=(!old||isUtf8(old))&&(!next||isUtf8(next));
      const full=text?`--- a/${name}\n+++ b/${name}\n@@ ${kind} @@\n`+(old?.toString("utf8").split("\n").map(line=>"-"+line).join("\n")??"")+"\n"+(next?.toString("utf8").split("\n").map(line=>"+"+line).join("\n")??""):"Binary content changed.";
      const limit=Math.min(remaining,32768),diff=full.slice(0,limit);remaining-=diff.length;if(diff.length<full.length)truncated=true;
      const file=source.files.find(row=>row.path===name),content=kind==="modified"&&text&&next&&next.length<=65536&&file?next.toString("utf8"):undefined;
      changes.push({path:name,kind,diff,...content!==undefined&&remaining>=content.length?{content,fileId:file!.id,expectedRevision:file!.revision,mediaType:file!.mediaType}:{}});if(content!==undefined&&remaining>=content.length)remaining-=content.length;
    }
    return {changes,truncated};
  }};
}
