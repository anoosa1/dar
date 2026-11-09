import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {appEnabled} from "./app-settings.ts";
import {agentLimits,validRepositoryUrl,activeAgentRun} from "../../shared/agent.ts";
import {emptyRunDocument,exposeRun,runSummary,validateRunDocument} from "./agent-runs.ts";
type FileEntry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;

function fail(code:string):never{throw new SenderError(code);}
function text(value:unknown,max:number){
  if(typeof value!=="string"||!value.trim()||value.includes("\0")||[...value].some(c=>c.codePointAt(0)!>=0xd800&&c.codePointAt(0)!<=0xdfff))fail("invalidInput");
  if(encodeURIComponent(value).replace(/%[A-F\d]{2}/g,"x").length>max)fail("invalidInput");
  return value;
}
function id(value:unknown){if(typeof value!=="string"||!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value))fail("invalidInput");return value;}
function keys(p:Record<string,unknown>,allowed:string[]){if(Object.keys(p).some(k=>!allowed.includes(k)))fail("invalidInput");}

/** All ownership, parent, quota and revision checks share the mutation transaction. */
export function agentOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>,canRead:(entry:FileEntry)=>boolean,sessionId:string){
  if(!appEnabled(ctx,"agent"))fail("notFound");
  const now=Number(ctx.timestamp.toMillis());
  const project=(value:unknown)=>{const row=ctx.db.agentProjects.id.find(id(value));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const thread=(value:unknown)=>{const row=ctx.db.agentThreads.id.find(id(value));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  if(operation==="run"||operation==="cancelRun"){
    keys(p,["id"]);const row=ctx.db.agentRuns.id.find(id(p.id));if(!row||row.ownerId!==userId)fail("notFound");
    if(operation==="cancelRun"&&activeAgentRun(row.status)){
      const next={...row,status:row.status==="queued"?"cancelled":"cancelling",modifiedAt:now};ctx.db.agentRuns.id.update(next);return exposeRun(next);
    }
    return exposeRun(row);
  }
  if(operation==="source"){
    keys(p,["projectId"]);const parent=project(p.projectId);
    if(parent.sourceType!=="files")return {project:parent,files:[]};
    const root=ctx.db.fileEntries.id.find(parent.sourceRef);if(!root||root.kind!=="folder"||!canRead(root))fail("notFound");
    const folders=[{row:root,path:"",depth:0}],files:{id:string;path:string;revision:number;size:number;mediaType:string}[]=[];let bytes=0,scanned=0;
    for(let i=0;i<folders.length;i++){
      const folder=folders[i],prefix=`${folder.row.ownerId}/${folder.row.id}/`;
      const range=new Range<string>({tag:"included",value:prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
      for(const entry of ctx.db.fileEntries.listKey.filter(range)){
        if(++scanned>500||folder.depth>32)fail("operationTooLarge");
        if(!canRead(entry))continue;
        if([".git",".codex"].includes(entry.name))continue;
        const path=folder.path+entry.name;
        if(path.length>2048)fail("operationTooLarge");
        if(entry.kind==="folder")folders.push({row:entry,path:path+"/",depth:folder.depth+1});
        else{bytes+=entry.size;if(files.length>=200||entry.size>1024*1024||bytes>8*1024*1024)fail("operationTooLarge");files.push({id:entry.id,path,revision:entry.revision,size:entry.size,mediaType:entry.mediaType});}
      }
    }
    return {project:parent,files};
  }
  if(operation==="folders"){
    keys(p,["parentId","cursor"]);const parentId=p.parentId? id(p.parentId):"",parent=parentId?ctx.db.fileEntries.id.find(parentId):null;
    if(parentId&&(!parent||parent.kind!=="folder"||!canRead(parent)))fail("notFound");
    const prefix=`${parent?.ownerId??userId}/${parentId}/`,cursor=typeof p.cursor==="string"?p.cursor:"";
    if(cursor&&!cursor.startsWith(prefix))fail("invalidInput");
    const range=new Range<string>({tag:cursor?"excluded":"included",value:cursor||prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
    const items:{id:string;name:string}[]=[];let scanned=0,nextCursor:string|null=null;
    for(const row of ctx.db.fileEntries.listKey.filter(range)){
      if(row.kind==="folder"&&canRead(row))items.push({id:row.id,name:row.name});
      if(++scanned>=200){nextCursor=row.listKey;break;}
    }
    return {folder:parent?{id:parent.id,name:parent.name,parentId:parent.parentId}:null,items,nextCursor};
  }
  if(operation==="projects"){
    keys(p,[]);return {items:[...ctx.db.agentProjects.ownerId.filter(userId)].sort((a,b)=>a.name.localeCompare(b.name)||a.id.localeCompare(b.id))};
  }
  if(operation==="createProject"){
    keys(p,["id","name","sourceType","sourceRef"]);
    const sourceRef=text(p.sourceRef,2048);
    if(p.sourceType==="git"){if(!validRepositoryUrl(sourceRef))fail("invalidInput");}
    else if(p.sourceType==="files"){const folder=ctx.db.fileEntries.id.find(id(sourceRef));if(!folder||folder.kind!=="folder"||!canRead(folder))fail("notFound");}
    else fail("invalidInput");
    const row={id:id(p.id),ownerId:userId,name:text(p.name,160).trim(),sourceType:p.sourceType,sourceRef,createdAt:now};
    const old=ctx.db.agentProjects.id.find(row.id);
    if(old){if(old.ownerId!==userId||old.name!==row.name||old.sourceType!==row.sourceType||old.sourceRef!==sourceRef)fail("conflict");return old;}
    if([...ctx.db.agentProjects.ownerId.filter(userId)].length>=agentLimits.projects)fail("quotaExceeded");
    ctx.db.agentProjects.insert(row);return row;
  }
  if(operation==="threads"){
    keys(p,["projectId"]);const parent=project(p.projectId);
    return {items:[...ctx.db.agentThreads.projectId.filter(parent.id)].sort((a,b)=>b.modifiedAt-a.modifiedAt||a.id.localeCompare(b.id))};
  }
  if(operation==="createThread"){
    keys(p,["id","projectId","title"]);const parent=project(p.projectId),row={id:id(p.id),ownerId:userId,projectId:parent.id,title:text(p.title,200).trim(),revision:1,createdAt:now,modifiedAt:now};
    const old=ctx.db.agentThreads.id.find(row.id);
    if(old){if(old.ownerId!==userId||old.projectId!==parent.id||old.title!==row.title)fail("conflict");return old;}
    if([...ctx.db.agentThreads.ownerId.filter(userId)].length>=agentLimits.threads)fail("quotaExceeded");
    ctx.db.agentThreads.insert(row);return row;
  }
  const current=thread(p.threadId);
  if(operation==="startRun"){
    keys(p,["threadId","id","messageId","content","revision","model"]);const runId=id(p.id),messageId=id(p.messageId),content=text(p.content,agentLimits.messageBytes),model=text(p.model,120);
    const old=ctx.db.agentRuns.id.find(runId);
    if(old){const message=ctx.db.agentMessages.id.find(old.messageId);if(old.ownerId!==userId||old.threadId!==current.id||old.messageId!==messageId||old.model!==model||message?.content!==content)fail("conflict");return exposeRun(old);}
    const runs=[...ctx.db.agentRuns.ownerId.filter(userId)];if(runs.length>=100)fail("quotaExceeded");
    // One run per account bounds provider/process usage as well as per-thread concurrency.
    if(runs.some(row=>activeAgentRun(row.status)))fail("conflict");
    if([...ctx.db.agentMessages.ownerId.filter(userId)].length>=agentLimits.messages-1)fail("quotaExceeded");
    agentOperation(ctx,userId,"appendMessage",{threadId:current.id,id:messageId,content,revision:p.revision},canRead,sessionId);
    const row={id:runId,threadId:current.id,ownerId:userId,sessionId,messageId,model,lease:"",status:"queued",document:JSON.stringify(emptyRunDocument()),createdAt:now,modifiedAt:now};
    ctx.db.agentRuns.insert(row);return exposeRun(row);
  }
  if(operation==="history"){
    keys(p,["threadId","before"]);
    const before=p.before??0xffffffff;if(typeof before!=="number"||!Number.isInteger(before)||before<1||before>0xffffffff)fail("invalidInput");
    const rows=[...ctx.db.agentMessages.threadId.filter(current.id)].filter(r=>r.sequence<before).sort((a,b)=>b.sequence-a.sequence);
    const items=rows.slice(0,agentLimits.page).reverse();return {thread:current,items,nextBefore:rows.length>agentLimits.page?items[0].sequence:null,runs:[...ctx.db.agentRuns.threadId.filter(current.id)].sort((a,b)=>b.createdAt-a.createdAt).map(runSummary)};
  }
  if(operation==="appendMessage"){
    keys(p,["threadId","id","content","revision"]);const messageId=id(p.id),content=text(p.content,agentLimits.messageBytes);
    const old=ctx.db.agentMessages.id.find(messageId);
    // Repeating an acknowledged append returns its original result without duplication.
    if(old){if(old.ownerId!==userId||old.threadId!==current.id||old.content!==content)fail("conflict");return {thread:current,message:old};}
    if([...ctx.db.agentRuns.threadId.filter(current.id)].some(row=>activeAgentRun(row.status)))fail("conflict");
    if(!Number.isInteger(p.revision)||p.revision!==current.revision)fail("conflict");
    if([...ctx.db.agentMessages.ownerId.filter(userId)].length>=agentLimits.messages)fail("quotaExceeded");
    const updated={...current,revision:current.revision+1,modifiedAt:now};
    const message={id:messageId,ownerId:userId,threadId:current.id,sequence:updated.revision,role:"user",content,createdAt:now};
    ctx.db.agentMessages.insert(message);ctx.db.agentThreads.id.update(updated);return {thread:updated,message};
  }
  fail("invalidInput");
}

export function agentSnapshot(ctx:Ctx){return {projects:[...ctx.db.agentProjects.iter()],threads:[...ctx.db.agentThreads.iter()],messages:[...ctx.db.agentMessages.iter()],runs:[...ctx.db.agentRuns.iter()].map(({lease,sessionId,...row})=>row)};}

/** V1–V5 backups have no Agent data. V6 validates links and bounded account data. */
export function restoreAgent(ctx:Ctx,value?:ReturnType<typeof agentSnapshot>){
  if(!value)return;
  if(!Array.isArray(value.projects)||!Array.isArray(value.threads)||!Array.isArray(value.messages))fail("invalidInput");
  for(const row of value.projects){
    if(!ctx.db.accounts.id.find(row.ownerId)||id(row.id)!==row.id||text(row.name,160).trim()!==row.name||!Number.isFinite(row.createdAt)||[...ctx.db.agentProjects.ownerId.filter(row.ownerId)].length>=agentLimits.projects)fail("invalidInput");
    if(row.sourceType==="git"){if(!validRepositoryUrl(text(row.sourceRef,2048)))fail("invalidInput");}
    else if(row.sourceType==="files")id(row.sourceRef);else fail("invalidInput");
    ctx.db.agentProjects.insert(row);
  }
  for(const row of value.threads){
    const parent=ctx.db.agentProjects.id.find(row.projectId);
    if(!parent||parent.ownerId!==row.ownerId||id(row.id)!==row.id||text(row.title,200).trim()!==row.title||!Number.isInteger(row.revision)||row.revision<1||row.revision>agentLimits.messages+1||!Number.isFinite(row.createdAt)||!Number.isFinite(row.modifiedAt)||[...ctx.db.agentThreads.ownerId.filter(row.ownerId)].length>=agentLimits.threads)fail("invalidInput");
    ctx.db.agentThreads.insert(row);
  }
  const sequences=new Set<string>();
  for(const row of value.messages){
    const parent=ctx.db.agentThreads.id.find(row.threadId),key=JSON.stringify([row.threadId,row.sequence]);
    if(!parent||parent.ownerId!==row.ownerId||id(row.id)!==row.id||!["user","assistant"].includes(row.role)||!Number.isInteger(row.sequence)||row.sequence<2||row.sequence>parent.revision||sequences.has(key)||!Number.isFinite(row.createdAt)||[...ctx.db.agentMessages.ownerId.filter(row.ownerId)].length>=agentLimits.messages)fail("invalidInput");
    text(row.content,agentLimits.messageBytes);sequences.add(key);ctx.db.agentMessages.insert(row);
  }
  for(const row of value.threads)if([...ctx.db.agentMessages.threadId.filter(row.id)].length!==row.revision-1)fail("invalidInput");
  for(const row of value.runs??[]){
    const parent=ctx.db.agentThreads.id.find(row.threadId),message=ctx.db.agentMessages.id.find(row.messageId);
    if(!parent||parent.ownerId!==row.ownerId||!message||message.threadId!==parent.id||message.role!=="user"||id(row.id)!==row.id||!["queued","running","cancelling","completed","failed","cancelled","interrupted"].includes(row.status)||row.document.length>512*1024||[...ctx.db.agentRuns.ownerId.filter(row.ownerId)].length>=100)fail("invalidInput");
    if(!Number.isFinite(row.createdAt)||!Number.isFinite(row.modifiedAt)||typeof row.model!=="string"||!row.model||row.model.length>120)fail("invalidInput");
    validateRunDocument(JSON.parse(row.document));ctx.db.agentRuns.insert({...row,status:activeAgentRun(row.status)?"interrupted":row.status,sessionId:"",lease:""});
  }
}
