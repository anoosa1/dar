import {readFile,lstat,mkdir,writeFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {codexAccount,codexModels,verifyCodexSandbox,runCodex,object,string,type CodexConfig} from "./codex.ts";
import {prepareWorkspace,type Source} from "./workspace.ts";
import type {AgentHistory,AgentRun,AgentRunDocument,AgentProvider} from "../../shared/agent.ts";

async function configuration(filename:string):Promise<CodexConfig|undefined>{
  try{
    const stat=await lstat(filename);if(!stat.isFile()||stat.isSymbolicLink()||stat.size>16384)throw Error();
    const value=object(JSON.parse(await readFile(filename,"utf8")));
    if(Object.keys(value).some(k=>!["executable","home","userId","gitHosts"].includes(k))||typeof value.executable!=="string"||!path.isAbsolute(value.executable)||typeof value.home!=="string"||!path.isAbsolute(value.home)||typeof value.userId!=="string"||!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value.userId)||!Array.isArray(value.gitHosts)||value.gitHosts.some(host=>typeof host!=="string"||!/^[A-Za-z0-9.-]+$/.test(host)))throw Error();
    const home=await lstat(value.home);if(!home.isDirectory()||home.isSymbolicLink())throw Error();
    return {...value,gitHosts:value.gitHosts.map(host=>String(host).toLowerCase())} as unknown as CodexConfig;
  }catch(error){if((error as NodeJS.ErrnoException).code==="ENOENT")return undefined;throw new AppError("codexConfigurationInvalid",503,"Codex configuration is invalid. Ask the administrator to check the configured paths and account.");}
}

/** Owns all provider processes. A browser disconnect never becomes a run cancellation. */
export async function createAgentRuntime(db:Database,root:string,filename:string){
  let config:CodexConfig|undefined,configError=false,closing=false;
  try{config=await configuration(filename);}catch{configError=true;}
  await db.call("service_agent_worker",["","","startup",""]);
  const active=new Map<string,{controller:AbortController;promise:Promise<void>;sessionId:string}>();
  let cache:{expires:number;value:Promise<AgentProvider>}|undefined;
  const operation=<T>(sessionId:string,name:string,value:object)=>db.call<T>("service_agent",[sessionId,name,JSON.stringify(value)]);
  const worker=<T>(id:string,lease:string,name:string,value:unknown={})=>db.call<T>("service_agent_worker",[id,lease,name,JSON.stringify(value)]);
  const blank=():AgentRunDocument=>({output:"",terminal:"",activity:[],changes:[],error:"",truncated:false});
  const configuredFor=(userId:string)=>!!config&&config.userId===userId;
  async function status(userId:string):Promise<AgentProvider>{
    if(!configuredFor(userId))return {configured:false,connected:false,models:[],message:configError?"Codex configuration needs administrator attention.":"Codex has not been configured for this Dar account."};
    if(!cache||cache.expires<Date.now())cache={expires:Date.now()+30000,value:(async()=>{
      const workspace=path.join(root,"agent-workspaces","provider-status");await mkdir(workspace,{recursive:true,mode:0o700});if((await lstat(workspace)).isSymbolicLink())throw new AppError("unsafeWorkspace",409);
      try{const connected=await codexAccount(config!,workspace);return {configured:true,connected,models:connected?await codexModels(config!,workspace):[],message:connected?"Connected with the existing Codex CLI ChatGPT login.":"Sign in to Codex on the server as the configured service user."};}
      catch{return {configured:true,connected:false,models:[],message:"Codex is unavailable. Check the executable, permissions and existing login."};}
    })()};return cache.value;
  }
  async function execute(id:string,sessionId:string,controller:AbortController){
    const lease=randomUUID(),data=blank();let timer:ReturnType<typeof setInterval>|undefined,checkpoint:Promise<void>|undefined,dirty=true;
    let workspaceCheck:(()=>Promise<void>)|undefined,lastWorkspaceCheck=0;
    const timeout=setTimeout(()=>controller.abort(),10*60000);
    try{
      const row=await worker<{ownerId:string;threadId:string;messageId:string;model:string}|null>(id,lease,"claim");if(!row)return;
      timer=setInterval(()=>{
        if(checkpoint)return;
        checkpoint=(async()=>{await worker(id,lease,"check");if(workspaceCheck&&Date.now()-lastWorkspaceCheck>2000){lastWorkspaceCheck=Date.now();await workspaceCheck();}if(dirty){dirty=false;await worker(id,lease,"update",{document:data});}})().catch(()=>controller.abort()).finally(()=>{checkpoint=undefined;});
      },500);
      const history=await operation<AgentHistory>(sessionId,"history",{threadId:row.threadId}),source=await operation<Source>(sessionId,"source",{projectId:history.thread.projectId});
      if(history.nextBefore)throw new AppError("contextTooLarge",413,"This thread has more than 50 messages. Start a new thread to provide bounded context.");
      const context=history.items.map(message=>`${message.role=== "user"?"User":"Assistant"}:\n${message.content}`).join("\n\n");
      if(Buffer.byteLength(context)>128*1024)throw new AppError("contextTooLarge",413,"The conversation is too large for this run. Start a new thread with a shorter task.");
      const prepared=await prepareWorkspace(root,row.ownerId,row.threadId,source,config!,db,sessionId,controller.signal);controller.signal.throwIfAborted();
      workspaceCheck=prepared.validate;
      const probe=path.join(root,"agent-workspaces","isolation-probe");
      try{await writeFile(probe,"Dar isolation check\n",{mode:0o600,flag:"wx"});}catch(error){if((error as NodeJS.ErrnoException).code!=="EEXIST")throw error;if(!(await lstat(probe)).isFile()||(await lstat(probe)).isSymbolicLink())throw new AppError("unsafeWorkspace",409);}
      if(!await codexAccount(config!,prepared.workspace,controller.signal))throw new AppError("codexLoginRequired",409,"The configured Codex CLI ChatGPT login is no longer available.");
      await verifyCodexSandbox(config!,prepared.workspace,probe,controller.signal);
      const messages=new Map<string,string>(),outputs=new Map<string,string>();
      const limit=(value:string,max:number)=>{if(value.length>max)data.truncated=true;return value.slice(0,max).replace(/[\uD800-\uDBFF]$/,"");};
      const remember=(items:Map<string,string>,id:string,value:string)=>{if(items.size>=100&&!items.has(id)){data.truncated=true;return;}items.set(id,limit(value,65536));};
      const result=await runCodex(config!,prepared.workspace,row.model,context,controller.signal,event=>{
        if(!["item.started","item.updated","item.completed"].includes(String(event.type)))return;
        const item=object(event.item),id=string(item.id,200);
        if(event.type==="item.started"&&(item.type==="command_execution"||item.type==="file_change"))data.activity=[...data.activity,limit(item.type==="command_execution"?String(item.command??"Running command"):"Editing workspace files",500)].slice(-100);
        if(item.type==="agent_message"&&typeof item.text==="string"){remember(messages,id,item.text);data.output=limit([...messages.values()].join("\n\n"),65536);}
        if(item.type==="command_execution"&&typeof item.aggregated_output==="string"){remember(outputs,id,item.aggregated_output);data.terminal=limit([...outputs.values()].join("\n"),65536);}
        dirty=true;
      });
      clearInterval(timer);timer=undefined;await checkpoint;controller.signal.throwIfAborted();
      const changes=await prepared.changes();data.changes=changes.changes;data.truncated||=changes.truncated;
      if(result!=="completed")data.error="The Codex CLI could not complete the turn. Inspect its saved output before retrying.";
      await worker(id,lease,"finish",{status:result,document:data});
    }catch(error){
      if(timer)clearInterval(timer);await checkpoint;
      if(!controller.signal.aborted){data.error=error instanceof AppError?error.message:"Execution failed. Inspect the saved output before retrying.";await worker(id,lease,"finish",{status:"failed",document:data}).catch(()=>worker(id,lease,"interrupt").catch(()=>{}));}
      else await worker(id,lease,"interrupt").catch(()=>{});
    }finally{clearTimeout(timeout);if(timer)clearInterval(timer);active.delete(id);}
  }
  return {
    status,
    async start(sessionId:string,userId:string,p:Record<string,unknown>){
      if(closing||!configuredFor(userId))throw new AppError("codexNotConfigured",409,"Codex is not configured for this Dar account.");
      const provider=await status(userId);if(!provider.connected)throw new AppError("codexLoginRequired",409,provider.message);
      if(!provider.models.some(model=>model.id===p.model))throw new AppError("invalidModel",400,"Choose one of the available Codex models.");
      const run=await operation<AgentRun>(sessionId,"startRun",p);
      if(run.status==="queued"&&!active.has(run.id)){const controller=new AbortController(),promise=execute(run.id,sessionId,controller);active.set(run.id,{controller,promise,sessionId});}
      return run;
    },
    async cancel(sessionId:string,id:string){const result=await operation<AgentRun>(sessionId,"cancelRun",{id});active.get(id)?.controller.abort();return result;},
    cancelSession(sessionId:string){for(const run of active.values())if(run.sessionId===sessionId)run.controller.abort();},
    cancelAll(){for(const run of active.values())run.controller.abort();},
    async close(){closing=true;for(const run of active.values())run.controller.abort();await Promise.allSettled([...active.values()].map(run=>run.promise));},
  };
}
export type AgentRuntime=Awaited<ReturnType<typeof createAgentRuntime>>;
