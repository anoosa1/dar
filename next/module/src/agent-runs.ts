import {SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {activeAgentRun,agentLimits,type AgentRunDocument} from "../../shared/agent.ts";
import {appEnabled} from "./app-settings.ts";
type Run=NonNullable<ReturnType<Ctx["db"]["agentRuns"]["id"]["find"]>>;
type FileEntry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
function fail(code:string):never{throw new SenderError(code);}
export function runSummary(row:Run){return {id:row.id,threadId:row.threadId,messageId:row.messageId,model:row.model,status:row.status,createdAt:row.createdAt,modifiedAt:row.modifiedAt};}
export function exposeRun(row:Run){return {...runSummary(row),document:JSON.parse(row.document)};}
export const emptyRunDocument=()=>({output:"",terminal:"",activity:[],changes:[],error:"",truncated:false});
export function validateRunDocument(value:unknown):AgentRunDocument{
  if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");
  const data=value as Record<string,unknown>;
  if(Object.keys(data).some(key=>!["output","terminal","activity","changes","error","truncated"].includes(key))||typeof data.output!=="string"||data.output.length>65536||typeof data.terminal!=="string"||data.terminal.length>65536||!Array.isArray(data.activity)||data.activity.length>100||data.activity.some((s:unknown)=>typeof s!=="string"||s.length>500)||!Array.isArray(data.changes)||data.changes.length>200||typeof data.error!=="string"||data.error.length>2000||typeof data.truncated!=="boolean")fail("invalidInput");
  for(const input of data.changes){
    if(!input||typeof input!=="object"||Array.isArray(input))fail("invalidInput");const change=input as Record<string,unknown>;
    if(Object.keys(change).some(key=>!["path","kind","diff","content","fileId","expectedRevision","mediaType"].includes(key))||typeof change.path!=="string"||change.path.length>2048||change.path.split("/").some(p=>!p||p==="."||p===".."||/[\x00-\x1f\\]/.test(p))||!["added","modified","deleted"].includes(String(change.kind))||typeof change.diff!=="string"||change.diff.length>32768)fail("invalidInput");
    if(change.mediaType!==undefined&&(typeof change.mediaType!=="string"||change.mediaType.length>150||!/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(change.mediaType)))fail("invalidInput");
    if(change.content!==undefined&&(typeof change.content!=="string"||change.content.length>65536||typeof change.fileId!=="string"||!/^[a-f0-9-]{36}$/.test(change.fileId)||!Number.isInteger(change.expectedRevision)||Number(change.expectedRevision)<1||Number(change.expectedRevision)>0xffffffff||change.kind!=="modified"))fail("invalidInput");
  }
  return data as unknown as AgentRunDocument;
}

/** A service-only executor owns the lease; stale processes cannot commit results. */
export function agentWorker(ctx:Ctx,id:string,lease:string,operation:string,document:string,authorize:(sessionId:string)=>string,canRead:(entry:FileEntry,userId:string)=>boolean){
  if(operation==="startup"){
    for(const row of ctx.db.agentRuns.iter())if(activeAgentRun(row.status))ctx.db.agentRuns.id.update({...row,status:"interrupted",lease:"",modifiedAt:Number(ctx.timestamp.toMillis()),document:JSON.stringify({...JSON.parse(row.document),error:"Dar restarted before this run completed. Inspect its output before starting another run."})});
    return {ok:true};
  }
  const row=ctx.db.agentRuns.id.find(id);if(!row)fail("notFound");
  const authority=()=>{
    if(!appEnabled(ctx,"agent")||authorize(row.sessionId)!==row.ownerId)fail("forbidden");
    const thread=ctx.db.agentThreads.id.find(row.threadId),project=thread&&ctx.db.agentProjects.id.find(thread.projectId);
    if(!project||project.ownerId!==row.ownerId)fail("forbidden");
    if(project.sourceType==="files"){const folder=ctx.db.fileEntries.id.find(project.sourceRef);if(!folder||folder.kind!=="folder"||!canRead(folder,row.ownerId))fail("forbidden");}
  };
  if(operation==="claim"){
    if(row.status!=="queued")return null;
    authority();
    const next={...row,status:"running",lease,modifiedAt:Number(ctx.timestamp.toMillis())};ctx.db.agentRuns.id.update(next);return next;
  }
  if((!row.lease||row.lease!==lease)&&!(operation==="interrupt"&&row.status==="queued")||!activeAgentRun(row.status))fail("conflict");
  if(operation==="check"){
    if(row.status==="cancelling")fail("forbidden");authority();return {ok:true};
  }
  if(operation==="interrupt"){
    const status=row.status==="cancelling"?"cancelled":"interrupted";
    ctx.db.agentRuns.id.update({...row,status,lease:"",modifiedAt:Number(ctx.timestamp.toMillis()),document:JSON.stringify({...JSON.parse(row.document),error:status==="cancelled"?"Run cancelled.":"Execution stopped. Inspect saved output before retrying."})});return {ok:true};
  }
  if(row.status==="cancelling")fail("forbidden");authority();
  if(document.length>512*1024)fail("operationTooLarge");
  const input=JSON.parse(document),data=validateRunDocument(input.document);
  const status=operation==="finish"?input.status:"running";
  if(!["running","completed","failed","cancelled"].includes(status))fail("invalidInput");
  ctx.db.agentRuns.id.update({...row,status,document:JSON.stringify(data),lease:operation==="finish"?"":lease,modifiedAt:Number(ctx.timestamp.toMillis())});
  if(operation==="finish"&&status==="completed"&&data.output){
    const thread=ctx.db.agentThreads.id.find(row.threadId)!;
    if([...ctx.db.agentMessages.ownerId.filter(row.ownerId)].length>=agentLimits.messages)fail("quotaExceeded");
    let messageId=ctx.newUuidV4().toString();while(ctx.db.agentMessages.id.find(messageId))messageId=ctx.newUuidV4().toString();
    // Full bounded output is retained on the run; the message fits the history contract.
    let content="",bytes=0;for(const character of data.output){bytes+=encodeURIComponent(character).replace(/%[A-F\d]{2}/g,"x").length;if(bytes>16000)break;content+=character;}
    if(content!==data.output)content+="\n\n[Full response is in the run output.]";
    ctx.db.agentMessages.insert({id:messageId,ownerId:row.ownerId,threadId:thread.id,sequence:thread.revision+1,role:"assistant",content,createdAt:Number(ctx.timestamp.toMillis())});
    ctx.db.agentThreads.id.update({...thread,revision:thread.revision+1,modifiedAt:Number(ctx.timestamp.toMillis())});
  }
  return {ok:true};
}
