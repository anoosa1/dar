import {spawn} from "node:child_process";
import {createInterface} from "node:readline";
import {randomUUID} from "node:crypto";
import {homedir} from "node:os";
import path from "node:path";
import {AppError} from "../errors.ts";
import {object,codexInstructions} from "./codex.ts";
import {codexSearchDirectories,findCodexExecutable} from "./discovery.ts";
import {agentEffort,type AgentModel,type AgentLimitWindow} from "../../shared/agent.ts";

/**
 * Claude Code through its own CLI and server-side login, speaking the stream-json
 * control protocol the Agent SDK uses. Runs therefore count as Claude Code usage on
 * the signed-in subscription. Credentials stay in the configured Claude home.
 */
export interface ClaudeConfig {executable:string;home:string;userId:string;gitHosts:string[]}
type Json=Record<string,unknown>;
export interface ClaudeProcess {send:(value:Json)=>void;control:<T=Json>(subtype:string,extra?:Json)=>Promise<T>;end:()=>void;close:(error?:Error)=>void;exited:Promise<number|null>;diagnostics:()=>string}

const environment=(config:ClaudeConfig)=>({PATH:process.env.PATH??"/usr/bin:/bin",HOME:config.home,CLAUDE_CONFIG_DIR:config.home,LANG:"C.UTF-8",NO_COLOR:"1",DISABLE_AUTOUPDATER:"1"});
const safeModel=/^[A-Za-z0-9._[\]-]{1,100}$/;

/** `claude auth status` prints JSON. Only first-party Claude logins are accepted; the email is never returned. */
export async function claudeAccount(config:ClaudeConfig,workspace:string,signal?:AbortSignal):Promise<{plan:string}|undefined>{
  signal?.throwIfAborted();
  const child=spawn(config.executable,["auth","status"],{cwd:workspace,detached:true,env:environment(config),stdio:["ignore","pipe","ignore"]});
  const chunks:Buffer[]=[];let size=0;
  const stop=()=>{try{if(child.pid)process.kill(-child.pid,"SIGKILL");}catch{}};
  const timer=setTimeout(stop,15000);signal?.addEventListener("abort",stop,{once:true});
  child.stdout.on("data",(chunk:Buffer)=>{size+=chunk.length;if(size>65536)stop();else chunks.push(chunk);});
  try{
    const code=await new Promise<number|null>((resolve,reject)=>{child.once("error",()=>reject(new AppError("claudeUnavailable",503,"The Claude Code CLI could not be started.")));child.once("close",resolve);});
    signal?.throwIfAborted();
    let value:Json;try{value=object(JSON.parse(Buffer.concat(chunks).toString("utf8")));}catch{return undefined;}
    if(code!==0||value.loggedIn!==true||value.apiProvider!=="firstParty")return undefined;
    return {plan:typeof value.subscriptionType==="string"&&/^[a-z0-9_ -]{1,40}$/i.test(value.subscriptionType)?value.subscriptionType:""};
  }finally{clearTimeout(timer);signal?.removeEventListener("abort",stop);}
}

/** Commands run only inside Claude Code's own sandbox, which needs bubblewrap and socat on Linux. */
export async function claudeCommandsAvailable(){
  const directories=codexSearchDirectories(process.env.PATH??"",homedir());
  return !!(await findCodexExecutable(directories,"bwrap")&&await findCodexExecutable(directories,"socat"));
}

export interface ClaudeRunOptions {workspace:string;model:string;effort?:string;plan:boolean;resume?:string;commands:boolean;deny:string[]}
/**
 * Every policy Dar relies on is pinned on the command line: restricted mode confines the
 * file tools to the working copy and ignores user/project settings, hooks and plugins;
 * no MCP servers, skills, web or sub-agent tools; prompts go to Dar, which declines them.
 */
export function claudeArgs(options:ClaudeRunOptions){
  if(!safeModel.test(options.model))throw new AppError("invalidModel",400);
  const tools=["Read","Edit","Write","Glob","Grep","TodoWrite","AskUserQuestion",...options.commands?["Bash"]:[]];
  // Bash is offered only with an enforced sandbox: other accounts, Dar's data and the
  // Claude home are unreadable, writes stay in the working copy (not .git), no network.
  const settings=options.commands?{sandbox:{enabled:true,failIfUnavailable:true,autoAllowBashIfSandboxed:true,allowUnsandboxedCommands:false,network:{allowedDomains:[]},
    filesystem:{denyRead:options.deny,allowRead:[options.workspace],allowWrite:[],denyWrite:[path.join(options.workspace,".git")]}}}:{};
  const instructions=codexInstructions+(options.commands?"":" Shell commands are unavailable; use the file tools.");
  return ["--model",options.model,"--tools",tools.join(","),"--permission-mode",options.plan?"plan":"acceptEdits","--include-partial-messages",
    "--append-system-prompt",instructions,"--settings",JSON.stringify(settings),...options.effort?["--effort",options.effort]:[],...options.resume?["--resume",options.resume]:[]];
}

/** One Claude Code process. stdout is newline-delimited JSON; stderr is kept privately for known diagnostics only. */
export function openClaude(config:ClaudeConfig,workspace:string,args:string[],handlers:{message:(value:Json)=>void;request:(subtype:string,request:Json)=>Promise<Json>}):ClaudeProcess{
  const child=spawn(config.executable,["-p","--input-format","stream-json","--output-format","stream-json","--verbose","--permission-prompt-tool","stdio","--restricted","--strict-mcp-config","--disable-slash-commands",...args],{cwd:workspace,detached:true,env:environment(config),stdio:["pipe","pipe","pipe"]});
  let failure:Error|undefined,ended=false,stderr="";
  const pending=new Map<string,{resolve:(value:Json)=>void;reject:(error:Error)=>void}>();
  const write=(value:Json)=>{if(!failure&&!ended&&child.stdin.writable)child.stdin.write(JSON.stringify(value)+"\n");};
  const close=(error:Error=new AppError("runStopped",409))=>{failure??=error;for(const item of pending.values())item.reject(failure);pending.clear();try{if(child.pid)process.kill(-child.pid,"SIGKILL");}catch{}};
  child.once("error",()=>close(new AppError("claudeUnavailable",503,"The Claude Code CLI could not be started.")));
  for(const stream of [child.stdin,child.stdout,child.stderr])stream.on("error",()=>close(new AppError("claudeUnavailable",503)));
  child.stderr.on("data",(chunk:Buffer)=>{if(stderr.length<16384)stderr+=chunk.toString("utf8").slice(0,16384-stderr.length);});
  const lines=createInterface({input:child.stdout});
  lines.on("line",line=>{
    let message:Json;
    try{if(line.length>16*1024*1024)throw Error();message=object(JSON.parse(line));}catch{close(new AppError("invalidProviderResponse",502,"Claude Code emitted invalid JSON output."));return;}
    if(message.type==="control_response"){
      const response=(message.response??{}) as Json,id=String(response.request_id??""),waiting=pending.get(id);if(!waiting)return;pending.delete(id);
      if(response.subtype==="success")waiting.resolve((response.response??{}) as Json);
      else waiting.reject(new AppError("claudeRequestFailed",502,typeof response.error==="string"?response.error.slice(0,500):"Claude Code rejected the request."));
    }else if(message.type==="control_request"){
      const id=message.request_id,request=(message.request??{}) as Json;if(typeof id!=="string")return;
      handlers.request(String(request.subtype??""),request).then(
        response=>write({type:"control_response",response:{subtype:"success",request_id:id,response}}),
        error=>write({type:"control_response",response:{subtype:"error",request_id:id,error:error instanceof AppError?error.message:"Declined by Dar."}}));
    }else if(message.type!=="control_cancel_request"&&message.type!=="keep_alive")handlers.message(message);
  });
  const exited=new Promise<number|null>(resolve=>child.once("close",code=>{lines.close();close(failure??new AppError("claudeIncomplete",502,"Claude Code exited before completing the turn."));resolve(code);}));
  return {
    send:write,
    control:<T,>(subtype:string,extra:Json={})=>new Promise<T>((resolve,reject)=>{if(failure)return reject(failure);const id=randomUUID();pending.set(id,{resolve:value=>resolve(value as T),reject});write({type:"control_request",request_id:id,request:{subtype,...extra}});}),
    end(){if(!ended){ended=true;child.stdin.end();}},close,exited,diagnostics:()=>stderr,
  };
}

/** Briefly starts the CLI without a model request, for the catalog or usage report. */
async function inspect<T>(config:ClaudeConfig,workspace:string,read:(process:ClaudeProcess,init:Json)=>Promise<T>,signal?:AbortSignal){
  const claude=openClaude(config,workspace,["--tools",""],{message(){},async request(){throw new AppError("declined",403);}});
  const stop=()=>claude.close(new AppError("claudeTimedOut",504,"The Claude Code CLI timed out."));
  const timer=setTimeout(stop,20000);signal?.addEventListener("abort",stop,{once:true});
  try{return await read(claude,await claude.control<Json>("initialize"));}finally{clearTimeout(timer);signal?.removeEventListener("abort",stop);claude.end();claude.close();}
}

export async function claudeModels(config:ClaudeConfig,workspace:string,signal?:AbortSignal):Promise<AgentModel[]>{
  return inspect(config,workspace,async(_claude,init)=>{
    if(!Array.isArray(init.models))throw new AppError("invalidProviderResponse",502);
    const rows=init.models.slice(0,100).map(object),fallback=rows.find(row=>row.value==="default")?.resolvedModel;let marked=false;
    return rows.filter(row=>row.value!=="default"&&typeof row.value==="string"&&safeModel.test(row.value)&&typeof row.displayName==="string").slice(0,50).map(row=>{
      const efforts=Array.isArray(row.supportedEffortLevels)?row.supportedEffortLevels.filter((level):level is string=>typeof level==="string"&&agentEffort.test(level)).slice(0,10):[];
      const isDefault=!marked&&row.resolvedModel===fallback;if(isDefault)marked=true;
      return {id:`claude/${row.value}`,name:String(row.displayName).slice(0,160),isDefault,provider:"claude" as const,...efforts.length?{efforts}:{}};
    });
  },signal);
}

/** Subscription windows from the CLI's own usage report; nothing is cached or estimated. */
export async function claudeLimits(config:ClaudeConfig,workspace:string){
  return inspect(config,workspace,async claude=>{
    const usage=await claude.control<Json>("get_usage"),limits=((usage.rate_limits??{}) as Json).limits,windows:AgentLimitWindow[]=[];
    for(const row of (Array.isArray(limits)?limits:[]).slice(0,8).map(item=>(item??{}) as Json)){
      const percent=typeof row.percent==="number"&&Number.isFinite(row.percent)?Math.min(100,Math.max(0,row.percent)):null;if(percent===null)continue;
      const scope=((row.scope??{}) as Json).model as Json|undefined,model=typeof scope?.display_name==="string"?` · ${scope.display_name.slice(0,40)}`:"";
      const session=row.group==="session",resets=typeof row.resets_at==="string"?Date.parse(row.resets_at):NaN;
      windows.push({label:`Claude Code · ${session?"5 hour":"Weekly"}${model}`,usedPercent:percent,windowMinutes:session?300:10080,resetsAt:Number.isFinite(resets)?resets:null});
    }
    const plan=typeof usage.subscription_type==="string"&&/^[a-z0-9_ -]{1,40}$/i.test(usage.subscription_type)?usage.subscription_type:null;
    return {available:usage.rate_limits_available!==false,plan,windows};
  });
}
