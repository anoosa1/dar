import {spawn} from "node:child_process";
import {createInterface} from "node:readline";
import path from "node:path";
import {AppError} from "../errors.ts";

export interface CodexConfig {executable:string;home:string;userId:string;gitHosts:string[]}
export interface CodexModel {id:string;name:string;isDefault:boolean}
type ObjectValue=Record<string,unknown>;
export function object(value:unknown):ObjectValue{if(!value||typeof value!=="object"||Array.isArray(value))throw new AppError("invalidProviderResponse",502);return value as ObjectValue;}
export function string(value:unknown,max=65536){if(typeof value!=="string"||value.length>max)throw new AppError("invalidProviderResponse",502);return value;}

function options(config:CodexConfig,workspace:string){
  const profile={":minimal":"read",":workspace_roots":"write","/nix/store":"read","/proc":"deny","/sys":"deny",[config.home]:"deny",[path.join(workspace,".git")]:"read",[path.join(workspace,".codex")]:"deny"};
  const toml=(value:Record<string,string>)=>"{"+Object.entries(value).map(([k,v])=>`${JSON.stringify(k)}=${JSON.stringify(v)}`).join(",")+"}";
  return ["--disable","apps","--disable","plugins","--disable","hooks","--disable","multi_agent","--disable","shell_snapshot","-c","mcp_servers={}","-c","web_search=\"disabled\"","-c","projects={}","-c","forced_login_method=\"chatgpt\"","-c","approval_policy=\"never\"","-c","default_permissions=\"dar\"","-c",`permissions.dar.filesystem=${toml(profile)}`,"-c","permissions.dar.network.enabled=false","-c","shell_environment_policy.inherit=\"none\"","-c",`shell_environment_policy.set=${toml({PATH:process.env.PATH??"/usr/bin:/bin",HOME:workspace})}`];
}

/** Spawn the CLI directly. Prompts use stdin; credentials and parent service secrets never do. */
async function command(config:CodexConfig,workspace:string,args:string[],{signal,input,onEvent,timeout=15000}:{signal?:AbortSignal;input?:string;onEvent?:(event:ObjectValue)=>void;timeout?:number}={}){
  signal?.throwIfAborted();
  const child=spawn(config.executable,args,{cwd:workspace,detached:true,env:{PATH:process.env.PATH,HOME:config.home,CODEX_HOME:config.home,LANG:"C.UTF-8",NO_COLOR:"1"},stdio:["pipe","pipe","pipe"]});
  let failure:Error|undefined,stdoutSize=0,stderrSize=0,buffered=0;
  const stdout:Buffer[]=[],stderr:Buffer[]=[];
  const stop=(error:Error)=>{failure??=error;try{if(child.pid)process.kill(-child.pid,"SIGKILL");}catch{}};
  const abort=()=>stop(new AppError("runStopped",409));signal?.addEventListener("abort",abort,{once:true});
  const timer=setTimeout(()=>stop(new AppError("codexTimedOut",504,"The Codex CLI timed out.")),timeout);
  child.once("error",()=>{failure=new AppError("codexUnavailable",503,"The Codex CLI could not be started.");});
  child.stdin.on("error",()=>stop(new AppError("codexUnavailable",503)));child.stdout.on("error",()=>stop(new AppError("codexUnavailable",503)));child.stderr.on("error",()=>stop(new AppError("codexUnavailable",503)));
  // CLI diagnostics may contain source/configuration details. Inspect status privately;
  // never forward arbitrary stderr into browser responses or server logs.
  child.stderr.on("data",(chunk:Buffer)=>{if(stderrSize<16384){const part=chunk.subarray(0,16384-stderrSize);stderr.push(part);stderrSize+=part.length;}});
  child.stdout.on("data",(chunk:Buffer)=>{
    if(onEvent){buffered+=chunk.length;if(buffered>2*1024*1024)stop(new AppError("invalidProviderResponse",502));const newline=chunk.lastIndexOf(10);if(newline>=0)buffered=chunk.length-newline-1;}
    else{stdoutSize+=chunk.length;if(stdoutSize>4*1024*1024)stop(new AppError("invalidProviderResponse",502));else stdout.push(chunk);}
  });
  const lines=onEvent?createInterface({input:child.stdout}):undefined;
  lines?.on("line",line=>{try{if(line.length>1024*1024)throw Error();onEvent!(object(JSON.parse(line)));}catch{stop(new AppError("invalidProviderResponse",502,"Codex emitted invalid JSON output."));}});
  child.stdin.end(input??"");
  try{return await new Promise<{code:number|null;stdout:string;stderr:string}>((resolve,reject)=>{
    child.once("close",code=>{if(failure)reject(failure);else resolve({code,stdout:Buffer.concat(stdout).toString("utf8"),stderr:Buffer.concat(stderr).toString("utf8")});});
  });}finally{clearTimeout(timer);signal?.removeEventListener("abort",abort);lines?.close();}
}

export async function codexAccount(config:CodexConfig,workspace:string,signal?:AbortSignal){
  const result=await command(config,workspace,["login","status",...options(config,workspace)],{signal});
  return result.code===0&&/Logged in using ChatGPT/i.test(result.stdout+result.stderr);
}

export async function codexModels(config:CodexConfig,workspace:string,signal?:AbortSignal):Promise<CodexModel[]>{
  const result=await command(config,workspace,["debug","models","--bundled",...options(config,workspace)],{signal});
  if(result.code!==0)throw new AppError("codexUnavailable",503,"The Codex CLI could not list models.");
  const value=object(JSON.parse(result.stdout));if(!Array.isArray(value.models))throw new AppError("invalidProviderResponse",502);
  const rows=value.models.map(object).filter(row=>row.visibility==="list").sort((a,b)=>Number(a.priority??1000)-Number(b.priority??1000));
  return rows.slice(0,100).map((row,index)=>({id:string(row.slug,120),name:string(row.display_name,160),isDefault:index===0}));
}

export async function verifyCodexSandbox(config:CodexConfig,workspace:string,probe:string,signal?:AbortSignal){
  const result=await command(config,workspace,["sandbox","-P","dar","-C",workspace,...options(config,workspace),"--","/bin/sh","-c",'test -d "$1" && test ! -r "$2" && test ! -w "$2"',"dar-sandbox-check",workspace,probe],{signal});
  if(result.code!==0){
    // Match known diagnostics without returning arbitrary CLI output or paths.
    if(/NETLINK_ROUTE socket|loopback:.*Address family not supported/i.test(result.stderr))throw new AppError("sandboxAddressFamilyBlocked",503,"The Dar service blocks the socket family needed by Codex workspace isolation. On NixOS, update the Dar module; Agent needs AF_NETLINK in RestrictAddressFamilies. No model run was started.");
    if(/bwrap: Can't mount proc on \/proc: (?:Operation not permitted|Permission denied)/i.test(result.stderr))throw new AppError("sandboxProcMountBlocked",503,"The service's protected /proc mounts prevent Codex from creating its sandbox. Update the Dar NixOS module; Agent requires ProtectKernelTunables=false while retaining ProtectSystem=strict and Codex's /proc and /sys restrictions. No model run was started.");
    if(/bwrap: (?:Can't create file at [^\n]*\/\.codex: Is a directory|Destination is not a file [^\n]*\/\.codex(?:\r?\n|$))/i.test(result.stderr))throw new AppError("sandboxMetadataRootMissing",503,"This cached workspace is missing a sandbox metadata directory. Update Dar to repair it automatically, or start a new thread. No model run was started.");
    throw new AppError("sandboxUnavailable",503,"Codex could not establish workspace isolation. No model run was started.");
  }
}

export async function runCodex(config:CodexConfig,workspace:string,model:string,prompt:string,signal:AbortSignal,onEvent:(event:ObjectValue)=>void){
  let completed=false,failed=false;
  const instructions="Work only in the supplied workspace. Follow repository AGENTS.md instructions within that scope. Do not publish, deploy, access credentials, or request broader permissions. Network and host access are unavailable. Earlier conversation is context; the final user message is the current task.";
  const result=await command(config,workspace,["exec","--json","--ephemeral","--ignore-user-config","--ignore-rules","--skip-git-repo-check","--color","never","-C",workspace,"-m",model,...options(config,workspace),"-c",`developer_instructions=${JSON.stringify(instructions)}`,"-"],{signal,input:prompt,timeout:10*60000,onEvent:event=>{
    if(event.type==="turn.completed")completed=true;
    if(event.type==="turn.failed"||event.type==="error")failed=true;
    onEvent(event);
  }});
  // Item completion or a zero exit alone is insufficient evidence of a successful turn.
  if(result.code===0&&completed&&!failed)return "completed";
  if(failed||result.code!==0)return "failed";
  throw new AppError("codexIncomplete",502,"The Codex CLI exited before reporting a completed turn.");
}
