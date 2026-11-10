import {readFile,lstat,mkdir,writeFile} from "node:fs/promises";
import {homedir} from "node:os";
import path from "node:path";
import {randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {createDurableFile} from "../atomic.ts";
import {codexSearchDirectories,findCodexExecutable} from "./discovery.ts";
import {codexAccount,codexModels,verifyCodexSandbox,runCodex,object,string,type CodexConfig} from "./codex.ts";
import {prepareWorkspace,type Source} from "./workspace.ts";
import type {AgentHistory,AgentRun,AgentRunDocument,AgentProvider} from "../../shared/agent.ts";

async function configuration(filename:string):Promise<CodexConfig|undefined>{
  try{await lstat(filename);}catch(error){if((error as NodeJS.ErrnoException).code==="ENOENT")return undefined;throw error;}
  try{
    const stat=await lstat(filename);if(!stat.isFile()||stat.isSymbolicLink()||stat.size>16384)throw Error();
    const value=object(JSON.parse(await readFile(filename,"utf8")));
    if(Object.keys(value).some(k=>!["executable","home","userId","gitHosts"].includes(k))||typeof value.executable!=="string"||!path.isAbsolute(value.executable)||typeof value.home!=="string"||!path.isAbsolute(value.home)||typeof value.userId!=="string"||!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value.userId)||!Array.isArray(value.gitHosts)||value.gitHosts.some(host=>typeof host!=="string"||!/^[A-Za-z0-9.-]+$/.test(host)))throw Error();
    const home=await lstat(value.home);if(!home.isDirectory()||home.isSymbolicLink())throw Error();
    return {...value,gitHosts:value.gitHosts.map(host=>String(host).toLowerCase())} as unknown as CodexConfig;
  }catch{throw new AppError("codexConfigurationInvalid",503,"Codex configuration is invalid. Ask the administrator to check the executable, Codex home and Dar account ID.");}
}

/** Owns all provider processes. A browser disconnect never becomes a run cancellation. */
export async function createAgentRuntime(db:Database,root:string,filename:string){
  let closing=false;
  await db.call("service_agent_worker",["","","startup",""]);
  const active=new Map<string,{controller:AbortController;promise:Promise<void>;sessionId:string}>();
  const operation=<T>(sessionId:string,name:string,value:object)=>db.call<T>("service_agent",[sessionId,name,JSON.stringify(value)]);
  const worker=<T>(id:string,lease:string,name:string,value:unknown={})=>db.call<T>("service_agent_worker",[id,lease,name,JSON.stringify(value)]);
  const blank=():AgentRunDocument=>({output:"",terminal:"",activity:[],changes:[],error:"",truncated:false});
  // Re-read the server binding and CLI login on each check. A refresh must not
  // reuse a startup snapshot or a cached failure after an administrator fixes it.
  async function connection(userId:string):Promise<{config?:CodexConfig;provider:AgentProvider}>{
    let config:CodexConfig|undefined;
    const unavailable=(message:string,configured=false)=>({config,provider:{configured,connected:false,models:[],message}});
    try{config=await configuration(filename);}catch{return unavailable("Codex configuration is invalid. Ask the administrator to check the executable, Codex home and Dar account ID, then refresh.");}
    if(!config)return unavailable("No server Codex login is linked to this Dar account. An administrator can use Connect server Codex, or configure services.dar.agent.");
    if(config.userId!==userId)return unavailable("The server Codex login is linked to a different Dar account. Ask the administrator to check the account binding.");
    const workspace=path.join(root,"agent-workspaces","provider-status");await mkdir(workspace,{recursive:true,mode:0o700});if((await lstat(workspace)).isSymbolicLink())throw new AppError("unsafeWorkspace",409);
    try{
      if(!await codexAccount(config,workspace))return unavailable("Codex is configured, but its ChatGPT login is unavailable to the Dar service. Sign in as the Linux account running Dar using the configured Codex home, then refresh.",true);
    }catch{return unavailable("Dar could not run the configured Codex CLI. Check the executable and service permissions, then refresh.",true);}
    try{
      const models=await codexModels(config,workspace);
      if(!models.length)return unavailable("Codex is signed in, but the CLI returned no available models. Check the installed Codex version, then refresh.",true);
      return {config,provider:{configured:true,connected:true,models,message:"Connected with the existing Codex CLI ChatGPT login."}};
    }catch{return unavailable("Codex is signed in, but Dar could not read its model catalog. Check the installed Codex version, then refresh.",true);}
  }
  async function execute(id:string,sessionId:string,controller:AbortController,config:CodexConfig){
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
    async status(userId:string){return (await connection(userId)).provider;},
    /** Called only by the administrator-only native-session connection action.
     * Discovery uses the service environment, never browser-supplied paths.
     * Publishing is exclusive: an existing binding can never be taken over.
     */
    async connect(userId:string){
      if(closing)throw new AppError("codexUnavailable",503);
      const existing=await configuration(filename);
      if(existing){if(existing.userId!==userId)throw new AppError("codexAlreadyLinked",409,"Codex is already linked to another Dar account. The administrator must update its server configuration.");return (await connection(userId)).provider;}
      const home=process.env.CODEX_HOME??path.join(homedir(),".codex");
      const executable=await findCodexExecutable(codexSearchDirectories(process.env.PATH??"",homedir()));
      if(!executable)throw new AppError("codexUnavailable",503,"Dar could not access Codex on its service PATH or standard installation paths. A private SSH-user installation may be hidden from the service. Set services.dar.agent.package on NixOS, or set the absolute executable path in codex.json.");
      try{const stat=await lstat(home);if(!path.isAbsolute(home)||!stat.isDirectory()||stat.isSymbolicLink())throw Error();}catch{throw new AppError("codexLoginRequired",409,"The Dar service has no accessible Codex home. Sign in to Codex as the Linux account running Dar, then connect again.");}
      const discovered={executable,home,userId,gitHosts:["github.com"]},workspace=path.join(root,"agent-workspaces","provider-status");await mkdir(workspace,{recursive:true,mode:0o700});
      if((await lstat(workspace)).isSymbolicLink())throw new AppError("unsafeWorkspace",409);
      if(!await codexAccount(discovered,workspace))throw new AppError("codexLoginRequired",409,"No ChatGPT login is available to the Dar service. Run codex login as the Linux account running Dar, then connect again.");
      try{await createDurableFile(filename,JSON.stringify(discovered));}catch(error){if((error as NodeJS.ErrnoException).code!=="EEXIST")throw new AppError("codexConfigurationReadOnly",409,"Dar cannot save the Codex binding. Configure services.dar.agent or make the private configuration directory writable by the Dar service.");}
      return (await connection(userId)).provider;
    },
    async start(sessionId:string,userId:string,p:Record<string,unknown>){
      if(closing)throw new AppError("codexUnavailable",503);
      const {config,provider}=await connection(userId);
      if(!config||!provider.configured)throw new AppError("codexNotConfigured",409,provider.message);
      if(!provider.connected)throw new AppError("codexLoginRequired",409,provider.message);
      if(!provider.models.some(model=>model.id===p.model))throw new AppError("invalidModel",400,"Choose one of the available Codex models.");
      const run=await operation<AgentRun>(sessionId,"startRun",p);
      if(run.status==="queued"&&!active.has(run.id)){const controller=new AbortController(),promise=execute(run.id,sessionId,controller,config);active.set(run.id,{controller,promise,sessionId});}
      return run;
    },
    async cancel(sessionId:string,id:string){const result=await operation<AgentRun>(sessionId,"cancelRun",{id});active.get(id)?.controller.abort();return result;},
    cancelSession(sessionId:string){for(const run of active.values())if(run.sessionId===sessionId)run.controller.abort();},
    cancelAll(){for(const run of active.values())run.controller.abort();},
    async close(){closing=true;for(const run of active.values())run.controller.abort();await Promise.allSettled([...active.values()].map(run=>run.promise));},
  };
}
export type AgentRuntime=Awaited<ReturnType<typeof createAgentRuntime>>;
