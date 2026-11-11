import {spawn} from "node:child_process";
import {readdir} from "node:fs/promises";
import path from "node:path";
import {createInterface} from "node:readline";
import {AppError} from "../errors.ts";
import {options,type CodexConfig} from "./codex.ts";

export type Json=Record<string,unknown>;
export interface AppServerHandlers {notify:(method:string,params:Json)=>void;request:(method:string,params:Json)=>Promise<unknown>}
export interface AppServer {request:<T=Json>(method:string,params:Json)=>Promise<T>;notify:(method:string,params:Json)=>void;close:(error?:Error)=>void;exited:Promise<number|null>}

/** Execution-policy rule files cannot be ignored by app-server, so their presence blocks execution. */
export async function assertNoCodexRules(config:CodexConfig){
  const entries=await readdir(path.join(config.home,"rules")).catch(()=>[]);
  if(entries.length)throw new AppError("codexRulesPresent",409,"The Dar Codex home contains execution-policy rules, which Dar cannot ignore. Ask the administrator to remove its rules directory.");
}
/**
 * One `codex app-server` process per Dar run. Messages are newline-delimited JSON-RPC
 * on stdio. Credentials stay in the server-side Codex home; stderr is never forwarded.
 */
export function openAppServer(config:CodexConfig,workspace:string,handlers:AppServerHandlers,extra:string[]=[]):AppServer{
  // app-server cannot ignore user config like exec did; pin settings that would act outside the sandbox.
  const child=spawn(config.executable,["app-server",...options(config,workspace),"-c","notify=[]",...extra],{cwd:workspace,detached:true,env:{PATH:process.env.PATH,HOME:config.home,CODEX_HOME:config.home,LANG:"C.UTF-8",NO_COLOR:"1"},stdio:["pipe","pipe","pipe"]});
  let next=1,failure:Error|undefined;
  const pending=new Map<number,{resolve:(value:unknown)=>void;reject:(error:Error)=>void}>();
  const write=(value:Json)=>{if(!failure&&child.stdin.writable)child.stdin.write(JSON.stringify(value)+"\n");};
  const close=(error:Error=new AppError("runStopped",409))=>{
    failure??=error;for(const item of pending.values())item.reject(failure);pending.clear();
    try{if(child.pid)process.kill(-child.pid,"SIGKILL");}catch{}
  };
  child.once("error",()=>close(new AppError("codexUnavailable",503,"The Codex CLI could not be started.")));
  for(const stream of [child.stdin,child.stdout,child.stderr])stream.on("error",()=>close(new AppError("codexUnavailable",503)));
  child.stderr.on("data",()=>{});
  const lines=createInterface({input:child.stdout});
  lines.on("line",line=>{
    let message:Json;
    try{if(line.length>8*1024*1024)throw Error();const value=JSON.parse(line);if(!value||typeof value!=="object"||Array.isArray(value))throw Error();message=value as Json;}
    catch{close(new AppError("invalidProviderResponse",502,"Codex emitted invalid JSON output."));return;}
    const id=message.id,method=typeof message.method==="string"?message.method:undefined,params=(message.params&&typeof message.params==="object"?message.params:{}) as Json;
    if(method&&(typeof id==="number"||typeof id==="string")){
      handlers.request(method,params).then(result=>write({id,result}),error=>write({id,error:{code:-32000,message:error instanceof AppError?error.message:"Declined by Dar."}}));
    }else if(method)handlers.notify(method,params);
    else if(typeof id==="number"){
      const waiting=pending.get(id);if(!waiting)return;pending.delete(id);
      if(message.error){const detail=message.error as Json;waiting.reject(new AppError("codexRequestFailed",502,typeof detail.message==="string"?detail.message.slice(0,500):"Codex rejected the request."));}
      else waiting.resolve(message.result);
    }
  });
  const exited=new Promise<number|null>(resolve=>child.once("close",code=>{lines.close();close(failure??new AppError("codexIncomplete",502,"The Codex CLI exited before reporting a completed turn."));resolve(code);}));
  return {
    request:<T,>(method:string,params:Json)=>new Promise<T>((resolve,reject)=>{if(failure)return reject(failure);const id=next++;pending.set(id,{resolve:value=>resolve(value as T),reject});write({id,method,params});}),
    notify:(method,params)=>write({method,params}),close,exited,
  };
}
