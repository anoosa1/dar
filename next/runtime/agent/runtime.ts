import {readFile,lstat,mkdir,writeFile,rm,readdir,rename,open} from "node:fs/promises";
import {homedir} from "node:os";
import path from "node:path";
import {randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {createDurableFile} from "../atomic.ts";
import {codexSearchDirectories,findCodexExecutable} from "./discovery.ts";
import {codexAccount,codexModels,verifyCodexSandbox,codexInstructions,object,string,type CodexConfig} from "./codex.ts";
import {openAppServer,assertNoCodexRules,type AppServer} from "./app-server.ts";
import {createTerminals} from "./terminal.ts";
import {prepareWorkspace,git,type Source} from "./workspace.ts";
import {activeAgentRun,agentImageTypes,agentAttachmentLimits,type AgentLimits,type AgentLimitWindow,type AgentHistory,type AgentRun,type AgentRunDocument,type AgentProvider,type AgentStep,type AgentAttachment,type AgentMessage} from "../../shared/agent.ts";
import {createWriteStream} from "node:fs";
import {pipeline} from "node:stream/promises";
import {Transform,type Readable} from "node:stream";

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
  const terminals=createTerminals(root);
  const active=new Map<string,{controller:AbortController;promise:Promise<void>;sessionId:string;turn?:string;thread?:string;server?:AppServer;answer?:{id:string;resolve:(answers:Record<string,string[]>)=>void};steered:Set<string>;steer?:(id:string)=>void}>();
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
  async function attachmentInfo(userId:string,threadId:string,attachmentId:string):Promise<AgentAttachment>{
    if(!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(attachmentId))throw new AppError("invalidInput");
    try{const info=JSON.parse(await readFile(path.join(root,"agent-workspaces",userId,threadId,"attachments",attachmentId+".json"),"utf8")) as AgentAttachment;if(info.id!==attachmentId)throw Error();return info;}
    catch{throw new AppError("notFound",404,"That attachment is not available.");}
  }
  async function execute(id:string,sessionId:string,controller:AbortController,config:CodexConfig){
    const lease=randomUUID(),data=blank();let timer:ReturnType<typeof setInterval>|undefined,checkpoint:Promise<void>|undefined,dirty=true;
    let workspaceCheck:(()=>Promise<void>)|undefined,lastWorkspaceCheck=0;
    const timeout=setTimeout(()=>controller.abort(),10*60000);
    try{
      const row=await worker<{ownerId:string;threadId:string;messageId:string;model:string;document:string}|null>(id,lease,"claim");if(!row)return;
      const initial=JSON.parse(row.document) as AgentRunDocument;let result:"completed"|"failed"|"cancelled"="failed";
      if(initial.effort)data.effort=initial.effort;if(initial.mode)data.mode=initial.mode;if(initial.attachments)data.attachments=initial.attachments;
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
      await verifyCodexSandbox(config!,prepared.workspace,probe,controller.signal);await assertNoCodexRules(config!);
      const messages=new Map<string,string>(),outputs=new Map<string,string>();
      const limit=(value:string,max:number)=>{if(value.length>max)data.truncated=true;return value.slice(0,max).replace(/[\uD800-\uDBFF]$/,"");};
      const remember=(items:Map<string,string>,id:string,value:string)=>{if(items.size>=100&&!items.has(id)){data.truncated=true;return;}items.set(id,limit(value,65536));};
      const steps=new Map<string,AgentStep>(),text=(value:unknown,max:number)=>typeof value==="string"?limit(value,max):"";
      const record=(step:AgentStep)=>{if(!steps.has(step.id)&&steps.size>=100){data.truncated=true;return;}steps.set(step.id,step);data.steps=[...steps.values()];dirty=true;};
      const output=()=>{data.output=limit([...messages.values()].filter(Boolean).join("\n\n"),65536);data.terminal=limit([...outputs.values()].join("\n"),65536);dirty=true;};
      // Continue the saved Codex thread when possible; only messages it has not seen are sent.
      let previous:AgentRunDocument|undefined;
      for(const summary of history.runs){if(summary.id===id||activeAgentRun(summary.status))continue;const earlier=await operation<AgentRun>(sessionId,"run",{id:summary.id}).catch(()=>undefined);if(earlier?.document.codexThread&&earlier.document.contextRevision!==undefined){previous=earlier.document;}break;}
      const unseen=previous?history.items.filter(message=>message.sequence>previous!.contextRevision!):history.items;
      const format=(items:typeof history.items)=>items.map(message=>`${message.role==="user"?"User":"Assistant"}:\n${message.content}`).join("\n\n");
      const attachments=(data.attachments??[]).map(item=>({type:"localImage" as const,path:path.join(root,"agent-workspaces",row.ownerId,row.threadId,"attachments",item.id)}));
      for(const item of attachments)if(!(await lstat(item.path)).isFile())throw new AppError("attachmentMissing",409,"An attached image is no longer available. Attach it again.");
      let completion:{resolve:(status:string)=>void;reject:(error:Error)=>void}|undefined;
      const finished=new Promise<string>((resolve,reject)=>{completion={resolve,reject};});finished.catch(()=>{});// observed only when a turn starts
      const turnStatus=(item:Record<string,unknown>,done:boolean)=>done?item.status==="failed"||item.status==="declined"?"failed" as const:"completed" as const:"running" as const;
      const server=openAppServer(config!,prepared.workspace,{
        notify(method,params){
          const item=params.item&&typeof params.item==="object"?params.item as Record<string,unknown>:undefined,done=method==="item/completed";
          if(item&&(method==="item/started"||done)){
            const itemId=text(item.id,200)||randomUUID(),previousStep=steps.get(itemId);
            if(item.type==="commandExecution"){const exit=Number.isSafeInteger(item.exitCode)?Number(item.exitCode):undefined,tail=typeof item.aggregatedOutput==="string"?item.aggregatedOutput.slice(-2000):previousStep?.output;record({id:itemId,kind:"command",label:text(item.command,2000)||previousStep?.label||"Command",status:turnStatus(item,done&&!(exit!==undefined&&exit!==0&&item.status!=="failed"))==="completed"&&exit!==undefined&&exit!==0?"failed":turnStatus(item,done),...exit!==undefined?{exitCode:exit}:{},...tail?{output:tail}:{}});if(typeof item.aggregatedOutput==="string"){remember(outputs,itemId,item.aggregatedOutput);output();}if(!done)data.activity=[...data.activity,limit(text(item.command,500)||"Running command",500)].slice(-100);}
            else if(item.type==="fileChange"){const paths=Array.isArray(item.changes)?item.changes.map(change=>change&&typeof change==="object"?String((change as Record<string,unknown>).path??""):"").filter(Boolean):[];record({id:itemId,kind:"edit",label:limit(paths.join("\n")||previousStep?.label||"Workspace files",2000),status:turnStatus(item,done)});if(!done)data.activity=[...data.activity,"Editing workspace files"].slice(-100);}
            else if(item.type==="reasoning"){const summary=[...Array.isArray(item.summary)?item.summary:[],...Array.isArray(item.content)&&!(Array.isArray(item.summary)&&item.summary.length)?item.content:[]].filter(value=>typeof value==="string").join("\n").trim();if(summary)record({id:itemId,kind:"reasoning",label:limit(summary,2000),status:done?"completed":"running"});}
            else if(item.type==="agentMessage"&&typeof item.text==="string"){remember(messages,itemId,item.text);output();}
            else if(item.type==="mcpToolCall")record({id:itemId,kind:"tool",label:limit(`${text(item.server,200)}${item.server?" · ":""}${text(item.tool,200)||"Tool call"}`,2000),status:turnStatus(item,done)});
            else if(item.type==="webSearch")record({id:itemId,kind:"search",label:text(item.query,2000)||"Web search",status:done?"completed":"running"});
          }
          else if(method==="item/agentMessage/delta"&&typeof params.itemId==="string"&&typeof params.delta==="string"){remember(messages,params.itemId,(messages.get(params.itemId)??"")+params.delta);output();}
          else if(method==="item/commandExecution/outputDelta"&&typeof params.itemId==="string"&&typeof params.delta==="string"){const all=(outputs.get(params.itemId)??"")+params.delta;remember(outputs,params.itemId,all);const step=steps.get(params.itemId);if(step)record({...step,output:all.slice(-2000)});output();}
          else if(method==="turn/plan/updated"&&Array.isArray(params.plan))record({id:"plan-"+text(params.turnId,100),kind:"plan",label:text(params.explanation,2000)||"Plan",status:"running",items:params.plan.slice(0,50).map(entry=>{const row=entry&&typeof entry==="object"?entry as Record<string,unknown>:{};return {text:text(row.step,500)||"Step",done:row.status==="completed"};})});
          else if(method==="thread/tokenUsage/updated"&&params.tokenUsage&&typeof params.tokenUsage==="object"){
            const usage=params.tokenUsage as Record<string,unknown>,last=(usage.last??{}) as Record<string,unknown>,total=(usage.total??{}) as Record<string,unknown>,count=(value:unknown)=>Number.isSafeInteger(value)&&Number(value)>=0?Number(value):0;
            // Context use is the latest request's input; billed totals are summed across the turn.
            data.usage={input:count(total.inputTokens),cached:count(total.cachedInputTokens),output:count(total.outputTokens)};if(count(last.inputTokens))data.contextUsed=count(last.inputTokens)+count(last.outputTokens);
            if(Number.isSafeInteger(usage.modelContextWindow))data.contextWindow=Number(usage.modelContextWindow);dirty=true;
          }
          else if(method==="thread/compacted"){record({id:"compacted",kind:"tool",label:"Compacted the conversation context",status:"completed"});if(data.mode==="compact")completion?.resolve("completed");}
          else if(method==="turn/started"&&data.mode!=="default"){const turn=(params.turn??{}) as Record<string,unknown>,entry=active.get(id);if(entry&&typeof turn.id==="string")entry.turn=turn.id;}
          else if(method==="turn/completed"){const turn=(params.turn??{}) as Record<string,unknown>,error=turn.error as Record<string,unknown>|undefined;if(error&&typeof error.message==="string")data.error=limit(error.message,2000);completion?.resolve(turn.status==="completed"?"completed":turn.status==="interrupted"?"cancelled":"failed");}
          else if(method==="error"&&params.willRetry!==true){const error=params.error as Record<string,unknown>|undefined;if(error&&typeof error.message==="string")data.error=limit(error.message,2000);dirty=true;}
        },
        async request(method,params){
          if(method==="item/tool/requestUserInput"){
            const key=randomUUID(),questions=(Array.isArray(params.questions)?params.questions:[]).slice(0,10).map(input=>{const q=(input??{}) as Record<string,unknown>;return {id:text(q.id,200)||randomUUID(),header:text(q.header,200),question:text(q.question,2000),options:(Array.isArray(q.options)?q.options:[]).slice(0,20).map(option=>{const o=(option??{}) as Record<string,unknown>;return {label:text(o.label,200),description:text(o.description,1000)};}),other:q.isOther===true,secret:q.isSecret===true};});
            data.pending={id:key,questions};dirty=true;
            const answers=await new Promise<Record<string,string[]>>((resolve,reject)=>{const entry=active.get(id);if(!entry)return reject(new AppError("runStopped",409));entry.answer={id:key,resolve};controller.signal.addEventListener("abort",()=>reject(new AppError("runStopped",409)),{once:true});});
            data.pending=null;for(const question of questions)record({id:`question-${key}-${question.id}`,kind:"question",label:question.question||question.header||"Question",status:"completed",items:(question.secret?["Answered"]:answers[question.id]??[]).slice(0,50).map(answer=>({text:limit(answer,500),done:true}))});
            return {answers:Object.fromEntries(Object.entries(answers).map(([key,value])=>[key,{answers:value}]))};
          }
          // Approval requests ask to escape Dar's sandbox, widen writes or reach the network. Never granted.
          if(["item/commandExecution/requestApproval","item/fileChange/requestApproval","execCommandApproval","applyPatchApproval"].includes(method)){
            record({id:"declined-"+randomUUID(),kind:"tool",label:limit(`Declined: ${text(params.command,1500)||text(params.reason,1500)||"request outside Dar's sandbox"}`,2000),status:"failed"});
            return method.startsWith("item/")?{decision:"decline"}:{decision:"denied"};
          }
          throw new AppError("declined",403,"Dar does not support this request.");
        },
      },["-c",'approval_policy="never"']);
      const abort=()=>{const entry=active.get(id);if(entry?.turn&&data.codexThread)void server.request("turn/interrupt",{threadId:data.codexThread,turnId:entry.turn}).catch(()=>{});setTimeout(()=>server.close(),1500);};
      controller.signal.addEventListener("abort",abort,{once:true});
      void server.exited.then(()=>completion?.reject(new AppError("codexIncomplete",502,"The Codex CLI exited before reporting a completed turn.")));
      try{
        await server.request("initialize",{clientInfo:{name:"dar",title:"Dar",version:"1.0.0"},capabilities:{experimentalApi:true}});server.notify("initialized",{});
        let threadKey:string|undefined,input=format(history.items);
        if(previous?.codexThread){try{threadKey=(await server.request<{thread:{id:string}}>("thread/resume",{threadId:previous.codexThread,cwd:prepared.workspace,approvalPolicy:"never",developerInstructions:codexInstructions})).thread.id;input=format(unseen);}catch{threadKey=undefined;}}
        if(!threadKey)threadKey=(await server.request<{thread:{id:string}}>("thread/start",{model:row.model,cwd:prepared.workspace,approvalPolicy:"never",developerInstructions:codexInstructions})).thread.id;
        if(typeof threadKey!=="string"||!/^[A-Za-z0-9-]{1,100}$/.test(threadKey))throw new AppError("invalidProviderResponse",502);
        data.codexThread=threadKey;data.contextRevision=history.thread.revision;dirty=true;
        if(data.mode==="compact"&&!previous?.codexThread)throw new AppError("nothingToCompact",409,"There is no Codex context to compact yet. Run a turn first.");
        const turn=data.mode==="review"?await server.request<{turn:{id:string}}>("review/start",{threadId:threadKey,target:{type:"uncommittedChanges"}})
          :data.mode==="compact"?await server.request("thread/compact/start",{threadId:threadKey}).then(()=>({turn:{id:"compact"}})):await server.request<{turn:{id:string}}>("turn/start",{threadId:threadKey,input:[{type:"text",text:input,text_elements:[]},...attachments],model:row.model,approvalPolicy:"never",...data.effort?{effort:data.effort}:{},...data.mode==="plan"?{collaborationMode:{mode:"plan",settings:{model:row.model,reasoning_effort:data.effort??null,developer_instructions:null}}}:{}});
        const entry=active.get(id);if(entry){entry.turn=turn.turn.id;entry.thread=threadKey;entry.server=server;entry.steer=messageId=>{data.steered=[...data.steered??[],messageId].slice(-50);dirty=true;};}
        result=await finished as "completed"|"failed"|"cancelled";
      }finally{controller.signal.removeEventListener("abort",abort);server.close();}
      const final=await operation<AgentHistory>(sessionId,"history",{threadId:row.threadId});data.contextRevision=final.thread.revision+(result==="completed"&&data.output?1:0);
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
    /** Account usage windows from the CLI's own rate-limit report; nothing is cached or estimated. */
    async limits(userId:string):Promise<AgentLimits>{
      const {config,provider}=await connection(userId);if(!config||!provider.connected)return {available:false,plan:null,windows:[],message:provider.message};
      const workspace=path.join(root,"agent-workspaces","provider-status"),server=openAppServer(config,workspace,{notify(){},async request(){throw new AppError("declined",403);}},["-c",'approval_policy="never"']);
      const timer=setTimeout(()=>server.close(),15000);
      try{
        await assertNoCodexRules(config);await server.request("initialize",{clientInfo:{name:"dar",title:"Dar",version:"1.0.0"},capabilities:{experimentalApi:true}});server.notify("initialized",{});
        const value=await server.request<Record<string,unknown>>("account/rateLimits/read",{}),snapshots=value.rateLimitsByLimitId&&typeof value.rateLimitsByLimitId==="object"?Object.values(value.rateLimitsByLimitId as Record<string,Record<string,unknown>>):[value.rateLimits as Record<string,unknown>];
        const windows:AgentLimitWindow[]=[],number=(input:unknown)=>typeof input==="number"&&Number.isFinite(input)?input:null;let plan:string|null=null;
        for(const snapshot of snapshots.filter(Boolean).slice(0,6)){
          plan??=typeof snapshot.planType==="string"?snapshot.planType:null;const name=typeof snapshot.limitName==="string"?snapshot.limitName:typeof snapshot.limitId==="string"?snapshot.limitId:"Codex";
          for(const [key,label] of [["primary","Short window"],["secondary","Weekly window"]] as const){const window=snapshot[key] as Record<string,unknown>|null;const used=number(window?.usedPercent);if(window&&used!==null){const minutes=number(window.windowDurationMins),resets=number(window.resetsAt);windows.push({label:`${name} · ${minutes?minutes>=1440?`${Math.round(minutes/1440)} day`:`${Math.round(minutes/60)} hour`:label}`,usedPercent:Math.min(100,Math.max(0,used)),windowMinutes:minutes,resetsAt:resets===null?null:resets<1e12?resets*1000:resets});}}
        }
        return {available:true,plan,windows};
      }catch(error){return {available:false,plan:null,windows:[],message:error instanceof AppError?error.message:"Usage limits are unavailable."};}
      finally{clearTimeout(timer);server.close();}
    },
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
      const model=provider.models.find(item=>item.id===p.model);if(!model)throw new AppError("invalidModel",400,"Choose one of the available Codex models.");
      if(p.effort!==undefined&&!model.efforts?.includes(String(p.effort)))throw new AppError("invalidEffort",400,"Choose a reasoning level this model supports.");
      // Attachments are referenced by ID; metadata is read from this account's private store.
      if(p.attachments!==undefined){
        if(!Array.isArray(p.attachments)||p.attachments.length>agentAttachmentLimits.count||new Set(p.attachments).size!==p.attachments.length)throw new AppError("invalidInput");
        p={...p,attachments:await Promise.all(p.attachments.map(value=>attachmentInfo(userId,String(p.threadId),String(value))))};
      }
      const run=await operation<AgentRun>(sessionId,"startRun",p);
      if(run.status==="queued"&&!active.has(run.id)){const controller=new AbortController(),promise=execute(run.id,sessionId,controller,config);active.set(run.id,{controller,promise,sessionId,steered:new Set()});}
      return run;
    },
    /** Delivers a follow-up into the running turn, then records it as a user message. Retries are idempotent. */
    async steer(sessionId:string,runId:string,p:Record<string,unknown>){
      const run=await operation<AgentRun>(sessionId,"run",{id:runId}),entry=active.get(runId);
      if(Object.keys(p).some(key=>key!=="id"&&key!=="content")||typeof p.id!=="string"||typeof p.content!=="string"||!p.content.trim())throw new AppError("invalidInput");
      // A retry after the turn ended still returns the saved message; new text is refused by the module.
      if(!entry?.server||!entry.turn||!entry.thread)return operation(sessionId,"steerRun",{threadId:run.threadId,runId,id:p.id,content:p.content}).catch(error=>{throw error instanceof AppError&&error.status===409?new AppError("turnUnavailable",409,"This turn is not running. Send the message as a new turn."):error;});
      if(!entry.steered.has(p.id)){await entry.server.request("turn/steer",{threadId:entry.thread,expectedTurnId:entry.turn,input:[{type:"text",text:p.content,text_elements:[]}]});entry.steered.add(p.id);}
      const saved=await operation<{thread:unknown;message:AgentMessage}>(sessionId,"steerRun",{threadId:run.threadId,runId,id:p.id,content:p.content});
      entry.steer?.(p.id);return saved;
    },
    /** Answers the running turn's pending question. Only the run owner's session reaches it. */
    async answer(sessionId:string,runId:string,p:Record<string,unknown>){
      await operation<AgentRun>(sessionId,"run",{id:runId});const entry=active.get(runId);
      if(!entry?.answer||p.requestId!==entry.answer.id)throw new AppError("questionUnavailable",409,"This question is no longer waiting for an answer.");
      const answers=p.answers;if(!answers||typeof answers!=="object"||Array.isArray(answers))throw new AppError("invalidInput");
      const clean:Record<string,string[]>={};for(const [key,value] of Object.entries(answers as Record<string,unknown>).slice(0,10)){if(!Array.isArray(value)||value.length>20||value.some(item=>typeof item!=="string"||item.length>2000))throw new AppError("invalidInput");clean[key.slice(0,200)]=value as string[];}
      const resolve=entry.answer.resolve;entry.answer=undefined;resolve(clean);return {ok:true};
    },
    /** Streams one image into the thread's private attachment store with a size cap. */
    async upload(sessionId:string,userId:string,threadId:string,attachmentId:string,name:string,mediaType:string,body:Readable){
      await operation<AgentHistory>(sessionId,"history",{threadId});
      if(!agentImageTypes.includes(mediaType as typeof agentImageTypes[number]))throw new AppError("unsupportedMediaType",415,"Attach PNG, JPEG, GIF or WebP images.");
      if(!name||name.length>200||/[\x00-\x1f/\\]/.test(name))throw new AppError("invalidInput");
      const folder=path.join(root,"agent-workspaces",userId,threadId,"attachments");await mkdir(folder,{recursive:true,mode:0o700});
      if((await lstat(folder)).isSymbolicLink())throw new AppError("unsafeWorkspace",409);
      const target=path.join(folder,attachmentId),temporary=target+".part";let size=0;
      const cap=new Transform({transform(chunk:Buffer,_encoding,done){size+=chunk.length;done(size>agentAttachmentLimits.bytes?new AppError("requestTooLarge",413,"Images must be 8 MiB or smaller."):undefined,chunk);}});
      try{await pipeline(body,cap,createWriteStream(temporary,{flags:"wx",mode:0o600}));}catch(error){await rm(temporary,{force:true});throw error;}
      if(!size){await rm(temporary,{force:true});throw new AppError("invalidInput");}
      await rename(temporary,target);const info:AgentAttachment={id:attachmentId,name,mediaType,size};await writeFile(target+".json",JSON.stringify(info),{mode:0o600});return info;
    },
    async attachment(sessionId:string,userId:string,threadId:string,attachmentId:string){
      await operation<AgentHistory>(sessionId,"history",{threadId});const info=await attachmentInfo(userId,threadId,attachmentId);
      return {info,file:await open(path.join(root,"agent-workspaces",userId,threadId,"attachments",attachmentId),"r")};
    },
    /**
     * Local Git for repository threads, run by Dar rather than the sandbox (which keeps .git
     * read-only). No remotes are contacted: pushing and pull requests stay disabled.
     */
    async git(sessionId:string,userId:string,threadId:string,p:Record<string,unknown>){
      const history=await operation<AgentHistory>(sessionId,"history",{threadId}),work=path.join(root,"agent-workspaces",userId,threadId,"work");
      try{if(!(await lstat(path.join(work,".git"))).isDirectory())throw Error();}catch{return {available:false};}
      const signal=AbortSignal.timeout(30000),run=(args:string[])=>git(work,args,signal).then(value=>String((value as {stdout?:string}).stdout??value).trim());
      if(p.action!=="status"&&history.runs.some(item=>activeAgentRun(item.status)))throw new AppError("runActive",409,"Wait for Codex to finish before changing branches or committing.");
      if(p.action==="branch"){
        const name=typeof p.name==="string"?p.name.trim():"";
        if(!/^[A-Za-z0-9][A-Za-z0-9._\/-]{0,99}$/.test(name)||name.includes("..")||name.endsWith(".lock")||name.endsWith("/"))throw new AppError("invalidInput",400,"Use letters, numbers, dots, dashes, underscores and slashes for branch names.");
        await run(["switch","-c",name]).catch(()=>{throw new AppError("branchFailed",409,"That branch could not be created. It may already exist.");});
      }else if(p.action==="commit"){
        const message=typeof p.message==="string"?p.message.trim():"";if(!message||message.length>500)throw new AppError("invalidInput",400,"Write a commit message of up to 500 characters.");
        await run(["add","-A"]);await run(["-c","user.name=Dar Agent","-c","user.email=agent@dar.invalid","commit","-m",message]).catch(()=>{throw new AppError("commitFailed",409,"Nothing to commit, or the commit could not be created.");});
      }else if(p.action!=="status")throw new AppError("invalidInput");
      const [branch,status,log]=await Promise.all([run(["rev-parse","--abbrev-ref","HEAD"]),run(["status","--porcelain"]),run(["log","-5","--format=%h%x09%s"])]);
      return {available:true,branch,changed:status?status.split("\n").length:0,commits:log?log.split("\n").map(line=>{const [hash,...subject]=line.split("\t");return {hash,subject:subject.join("\t")};}):[]};
    },
    /** Interactive shell in the thread working copy; every call re-checks thread ownership. */
    async terminal(sessionId:string,userId:string,threadId:string,action:string,p:Record<string,unknown>,signal:AbortSignal){
      await operation<AgentHistory>(sessionId,"history",{threadId});
      const size={rows:Number(p.rows),cols:Number(p.cols)};
      if(action==="open"){const {config,provider}=await connection(userId);if(!config||!provider.connected)throw new AppError("codexLoginRequired",409,provider.message);await assertNoCodexRules(config);return terminals.open(config,userId,threadId,size);}
      if(action==="read"){const after=Number(p.after??0);if(!Number.isSafeInteger(after)||after<0)throw new AppError("invalidInput");return terminals.read(userId,threadId,after,signal);}
      if(action==="input"){if(typeof p.data!=="string")throw new AppError("invalidInput");return terminals.write(userId,threadId,p.data);}
      if(action==="resize")return terminals.resize(userId,threadId,size);
      if(action==="close")return terminals.close(userId,threadId);
      throw new AppError("notFound",404);
    },
    /** Deletes saved thread data first, then this account's private working copy. */
    async deleteThread(sessionId:string,userId:string,threadId:string){
      const result=await operation<{id:string}>(sessionId,"deleteThread",{threadId});terminals.close(userId,threadId);
      await rm(path.join(root,"agent-workspaces",userId,threadId),{recursive:true,force:true});return result;
    },
    /** Bounded relative paths for @ mentions. Symlinks are listed but never followed. */
    async files(sessionId:string,userId:string,threadId:string,query:string){
      const history=await operation<AgentHistory>(sessionId,"history",{threadId}),work=path.join(root,"agent-workspaces",userId,threadId,"work");
      let paths:string[]=[],source="workspace";
      try{
        if(!(await lstat(work)).isDirectory())throw Error();
        const pending=[""];let seen=0;
        while(pending.length&&seen<5000){
          const relative=pending.shift()!;
          for(const entry of await readdir(path.join(work,relative),{withFileTypes:true})){
            if(++seen>5000)break;if([".git",".codex","node_modules"].includes(entry.name))continue;
            const next=relative?`${relative}/${entry.name}`:entry.name;
            if(entry.isDirectory())pending.push(next);else paths.push(next);
          }
        }
      }catch{
        const project=await operation<Source>(sessionId,"source",{projectId:history.thread.projectId}).catch(()=>undefined);
        paths=project?.files.map(file=>file.path)??[];source=project?.project.sourceType==="files"?"files":"none";
      }
      const needle=query.toLocaleLowerCase(),score=(value:string)=>{const lower=value.toLocaleLowerCase();if(!needle)return 1;const name=lower.split("/").at(-1)!;if(name.startsWith(needle))return 3;if(lower.includes(needle))return 2;let index=0;for(const char of lower)if(char===needle[index])index++;return index===needle.length?1:0;};
      return {source,items:paths.map(value=>({path:value,score:score(value)})).filter(item=>item.score>0).sort((a,b)=>b.score-a.score||a.path.length-b.path.length||a.path.localeCompare(b.path)).slice(0,50).map(item=>item.path)};
    },
    async cancel(sessionId:string,id:string){const result=await operation<AgentRun>(sessionId,"cancelRun",{id});active.get(id)?.controller.abort();return result;},
    cancelSession(sessionId:string){for(const run of active.values())if(run.sessionId===sessionId)run.controller.abort();},
    cancelAll(){for(const run of active.values())run.controller.abort();},
    async close(){closing=true;terminals.closeAll();for(const run of active.values())run.controller.abort();await Promise.allSettled([...active.values()].map(run=>run.promise));},
  };
}
export type AgentRuntime=Awaited<ReturnType<typeof createAgentRuntime>>;
