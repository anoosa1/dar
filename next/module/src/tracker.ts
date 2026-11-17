import type {Ctx} from "./index.ts";
import {fail,now,keys,first,folderOf,type HistoryFiles} from "./history.ts";
import {gitLabelColors,trackerLimits,issueReference,closingReference,type GitIssue,type GitIssueList,type GitIssueSummary,type GitLabel,type GitTimelineItem,type GitTimelineEvent} from "../../shared/git.ts";

/**
 * Issues and pull requests (docs/git.md#issues-and-pull-requests). Anyone who can read the folder
 * opens issues and comments; people who can write also label, open, close and merge pull requests.
 * A pull request is merged by whatever moves its base past its head: the Git app's merge and a plain
 * `git push` both go through push() in git.ts, which calls pullsAfterPush in the same transaction.
 */
type Repo=NonNullable<ReturnType<Ctx["db"]["fileRepos"]["id"]["find"]>>;
type Issue=NonNullable<ReturnType<Ctx["db"]["gitIssues"]["id"]["find"]>>;
type Role="owner"|"write"|"read";
const issueId=(repoId:string,number:number)=>`${repoId}/${number}`;
const writer=(role:Role)=>role!=="read";
const agentHead=(name:string)=>name.startsWith("agent/");

function repoFor(ctx:Ctx,files:HistoryFiles,p:Record<string,unknown>){
  const {entry:root,role}=folderOf(ctx,files,p.folderId,false),repo=ctx.db.fileRepos.id.find(root.id);
  if(!repo)fail("notFound");return {repo,role:role as Role};
}
function issueOf(ctx:Ctx,repo:Repo,value:unknown){
  if(typeof value!=="number"||!Number.isInteger(value)||value<1)fail("invalidInput");
  return ctx.db.gitIssues.id.find(issueId(repo.id,value))??fail("notFound");
}
function title(value:unknown){
  const text=typeof value==="string"?value.replace(/\s+/g," ").trim():"";
  if(!text||text.length>trackerLimits.title)fail("invalidInput");return text;
}
function body(value:unknown){
  if(value===undefined)return "";
  if(typeof value!=="string"||value.length>trackerLimits.body||value.includes("\0"))fail("invalidInput");
  return value.replace(/\r\n/g,"\n").replace(/\s+$/,"");
}
function branchName(value:unknown){
  if(typeof value!=="string"||!value||value.length>200||/[\x00-\x20\x7f~^:?*[\\]|\.\.|@\{|\/\/|^\/|\/$|\.$|\.lock(\/|$)/.test(value))fail("invalidInput");
  return value;
}
const person=(ctx:Ctx,userId:string)=>{const account=ctx.db.accounts.id.find(userId);return account?.displayName||account?.username||"Removed account";};
const labelRows=(ctx:Ctx,repoId:string)=>[...ctx.db.gitLabels.repoId.filter(repoId)];
function labelView(row:ReturnType<typeof labelRows>[number]):GitLabel{return {id:row.id,name:row.name,color:row.color as GitLabel["color"],description:row.description};}
const labelList=(value:string)=>value.split(" ").filter(Boolean);

/** A branch's tip as the database has it: main follows the folder's history; agent branches have none. */
function tipOf(ctx:Ctx,repo:Repo,name:string){
  if(name==="main"){
    if(!repo.headId)return undefined;
    const pin=first(ctx.db.fileGitObjects.changesetId.filter(repo.headId));
    // main with commits Git has not seen yet has no Git ID until the next fetch; it still exists.
    return pin?.sha??"";
  }
  return ctx.db.fileGitRefs.id.find(`${repo.id}/refs/heads/${name}`)?.target;
}
function branchExists(ctx:Ctx,repo:Repo,name:string){
  if(agentHead(name))return [...ctx.db.fileBranches.repoId.filter(repo.id)].some(branch=>branch.name===name);
  return tipOf(ctx,repo,name)!==undefined;
}

function touch(ctx:Ctx,row:Issue,at:number,changes:Partial<Issue>={}){
  const next={...row,...changes,updatedAt:at,revision:row.revision+1};ctx.db.gitIssues.id.update(next);return next;
}
const nextSequence=(ctx:Ctx,id:string)=>{let max=0;for(const item of ctx.db.gitComments.issueId.filter(id))max=Math.max(max,item.sequence);return max+1;};
function addEvent(ctx:Ctx,files:{uuid():string},row:Issue,event:GitTimelineEvent,authorId:string,at:number,data:Record<string,unknown>={}){
  ctx.db.gitComments.insert({id:files.uuid(),issueId:row.id,repoId:row.repoId,sequence:nextSequence(ctx,row.id),kind:event,authorId,body:"",data:JSON.stringify(data),createdAt:at,editedAt:0});
}
/** Records "referenced from #n" on each item a text mentions, once per pair. */
function mention(ctx:Ctx,files:{uuid():string},row:Issue,text:string,authorId:string,at:number){
  const numbers=new Set([...text.matchAll(issueReference)].map(match=>Number(match[2])).filter(number=>number!==row.number));
  for(const number of [...numbers].slice(0,50)){
    const target=ctx.db.gitIssues.id.find(issueId(row.repoId,number));if(!target)continue;
    const seen=[...ctx.db.gitComments.issueId.filter(target.id)].some(item=>item.kind==="referenced"&&JSON.parse(item.data).number===row.number);
    if(!seen){addEvent(ctx,files,target,"referenced",authorId,at,{number:row.number});touch(ctx,target,at);}
  }
}
/** Closes the open issues a merged pull request's description names with "Closes #n". */
function closeNamed(ctx:Ctx,files:{uuid():string},pull:Issue,userId:string,at:number){
  for(const number of new Set([...pull.body.matchAll(closingReference)].map(match=>Number(match[1])))){
    const target=ctx.db.gitIssues.id.find(issueId(pull.repoId,number));
    if(!target||target.kind!=="issue"||target.state!=="open")continue;
    touch(ctx,target,at,{state:"closed",closedAt:at,closedBy:userId});addEvent(ctx,files,target,"closed",userId,at,{number:pull.number});
  }
}
function merged(ctx:Ctx,files:{uuid():string},pull:Issue,userId:string,at:number,shas:{baseSha:string;headSha:string;mergedSha:string}){
  const row=touch(ctx,pull,at,{state:"merged",closedAt:at,closedBy:userId,...shas});
  addEvent(ctx,files,row,"merged",userId,at,shas.mergedSha?{sha:shas.mergedSha}:{});closeNamed(ctx,files,row,userId,at);
}

function summary(ctx:Ctx,row:Issue):GitIssueSummary{
  let comments=0;for(const item of ctx.db.gitComments.issueId.filter(row.id))if(item.kind==="comment")comments++;
  return {number:row.number,kind:row.kind as GitIssueSummary["kind"],title:row.title,state:row.state as GitIssueSummary["state"],author:person(ctx,row.authorId),createdAt:row.createdAt,updatedAt:row.updatedAt,
    ...row.closedAt?{closedAt:row.closedAt}:{},comments,labels:labelList(row.labels),...row.kind==="pull"?{base:row.base,head:row.head,agent:agentHead(row.head)}:{}};
}
function detail(ctx:Ctx,repo:Repo,role:Role,userId:string,row:Issue):GitIssue{
  const timeline:GitTimelineItem[]=[...ctx.db.gitComments.issueId.filter(row.id)].sort((a,b)=>a.sequence-b.sequence).map(item=>{
    if(item.kind==="comment")return {id:item.id,kind:"comment",author:person(ctx,item.authorId),body:item.body,createdAt:item.createdAt,...item.editedAt?{editedAt:item.editedAt}:{},
      canEdit:item.authorId===userId,canDelete:item.authorId===userId||writer(role)};
    const data=JSON.parse(item.data) as {number?:number;from?:string;to?:string;label?:string;sha?:string};
    const other=data.number?ctx.db.gitIssues.id.find(issueId(repo.id,data.number)):undefined;
    return {id:item.id,kind:"event",event:item.kind as GitTimelineEvent,author:person(ctx,item.authorId),createdAt:item.createdAt,...data,...other?{title:other.title}:{}};
  });
  const own=row.authorId===userId,open=row.state==="open";
  return {...summary(ctx,row),body:row.body,revision:row.revision,...row.closedBy?{closedBy:person(ctx,row.closedBy)}:{},timeline,
    ...row.baseSha?{baseSha:row.baseSha}:{},...row.headSha?{headSha:row.headSha}:{},...row.mergedSha?{mergedSha:row.mergedSha}:{},
    permissions:{edit:own||writer(role),close:row.state!=="merged"&&(row.kind==="pull"?writer(role):own||writer(role)),label:writer(role),merge:row.kind==="pull"&&open&&writer(role),comment:true}};
}
function chosenLabels(ctx:Ctx,repo:Repo,value:unknown){
  if(!Array.isArray(value)||value.length>trackerLimits.labelsPerItem)fail("invalidInput");
  const known=new Set(labelRows(ctx,repo.id).map(row=>row.id));
  for(const id of value)if(typeof id!=="string"||!known.has(id))fail("invalidInput");
  return [...new Set(value as string[])];
}
/** An open pull request from head into base may exist only once. */
function duplicate(ctx:Ctx,repo:Repo,base:string,head:string,except?:string){
  for(const row of ctx.db.gitIssues.repoId.filter(repo.id))if(row.kind==="pull"&&row.state==="open"&&row.base===base&&row.head===head&&row.id!==except)fail("alreadyExists");
}

/** Open counts for the Git app's tabs. */
export function openCounts(ctx:Ctx,repoId:string){
  let issues=0,pulls=0;for(const row of ctx.db.gitIssues.repoId.filter(repoId))if(row.state==="open"){if(row.kind==="pull")pulls++;else issues++;}
  return {openIssues:issues,openPulls:pulls};
}
/** Removes a repository's issues, pull requests and labels with it. */
export function dropTracker(ctx:Ctx,repoId:string){
  for(const row of [...ctx.db.gitComments.repoId.filter(repoId)])ctx.db.gitComments.id.delete(row.id);
  for(const row of [...ctx.db.gitIssues.repoId.filter(repoId)])ctx.db.gitIssues.id.delete(row.id);
  for(const row of labelRows(ctx,repoId))ctx.db.gitLabels.id.delete(row.id);
}

/**
 * After a push changed refs (git.ts): open pull requests whose base now contains their head are merged;
 * one whose head branch was deleted is closed; one whose head moved records the push.
 * tip(name, "before"|"after") is a branch's commit around the push, undefined when it does not exist.
 */
export function pullsAfterPush(ctx:Ctx,files:{uuid():string},repo:Repo,userId:string,tip:(name:string,when:"before"|"after")=>string|undefined,isAncestor:(ancestor:string,descendant:string)=>boolean){
  const at=now(ctx);
  for(const row of [...ctx.db.gitIssues.repoId.filter(repo.id)]){
    if(row.kind!=="pull"||row.state!=="open"||agentHead(row.head))continue;
    const headBefore=tip(row.head,"before"),headAfter=tip(row.head,"after"),baseBefore=tip(row.base,"before"),baseAfter=tip(row.base,"after");
    const head=headAfter??headBefore;
    // Merged by this push: the base contains the head now and did not before.
    if(head&&baseAfter&&baseAfter!==baseBefore&&!(baseBefore&&isAncestor(head,baseBefore))&&isAncestor(head,baseAfter)){
      merged(ctx,files,row,userId,at,{baseSha:baseBefore??"",headSha:head,mergedSha:baseAfter});continue;
    }
    if(headBefore&&!headAfter){
      const closed=touch(ctx,row,at,{state:"closed",closedAt:at,closedBy:userId,baseSha:baseAfter??"",headSha:headBefore});
      addEvent(ctx,files,closed,"branchDeleted",userId,at,{from:row.head});
    }else if(headBefore&&headAfter&&headBefore!==headAfter){addEvent(ctx,files,row,"pushed",userId,at,{from:headBefore,to:headAfter});touch(ctx,row,at);}
  }
}
/** An Agent branch was merged into the folder (history.ts) or discarded. */
export function agentBranchDone(ctx:Ctx,files:{uuid():string},repoId:string,name:string,userId:string,merge:boolean){
  const at=now(ctx);
  for(const row of [...ctx.db.gitIssues.repoId.filter(repoId)]){
    if(row.kind!=="pull"||row.state!=="open"||row.head!==name)continue;
    if(merge)merged(ctx,files,row,userId,at,{baseSha:"",headSha:"",mergedSha:""});
    else addEvent(ctx,files,touch(ctx,row,at,{state:"closed",closedAt:at,closedBy:userId}),"branchDeleted",userId,at,{from:name});
  }
}

/** serviceGitTracker: browser sessions only. */
export function trackerOperation(ctx:Ctx,files:HistoryFiles,userId:string,operation:string,p:Record<string,unknown>){
  const {repo,role}=repoFor(ctx,files,p),at=now(ctx);
  const needWrite=()=>{if(!writer(role))fail("forbidden");};
  switch(operation){
    case "list":{
      keys(p,["folderId","kind","state","label"]);
      const kind=p.kind==="pull"?"pull":p.kind==="issue"?"issue":fail("invalidInput"),closed=p.state==="closed"?true:p.state==="open"?false:fail("invalidInput");
      const label=p.label===undefined?undefined:typeof p.label==="string"?p.label:fail("invalidInput");
      let open=0,shut=0;const rows:Issue[]=[];
      for(const row of ctx.db.gitIssues.repoId.filter(repo.id)){
        if(row.kind!==kind||label&&!labelList(row.labels).includes(label))continue;
        if(row.state==="open")open++;else shut++;
        if((row.state!=="open")===closed)rows.push(row);
      }
      rows.sort((a,b)=>b.number-a.number);
      return {items:rows.slice(0,trackerLimits.page).map(row=>summary(ctx,row)),open,closed:shut,labels:labelRows(ctx,repo.id).map(labelView).sort((a,b)=>a.name.localeCompare(b.name)),truncated:rows.length>trackerLimits.page} satisfies GitIssueList;
    }
    case "issue":{keys(p,["folderId","number"]);return detail(ctx,repo,role,userId,issueOf(ctx,repo,p.number));}
    case "labels":{keys(p,["folderId"]);return labelRows(ctx,repo.id).map(labelView).sort((a,b)=>a.name.localeCompare(b.name));}
    case "create":{
      keys(p,["folderId","kind","title","body","labels","base","head"]);
      const kind=p.kind==="pull"?"pull":p.kind==="issue"?"issue":fail("invalidInput");
      const rows=[...ctx.db.gitIssues.repoId.filter(repo.id)];
      if(rows.length>=trackerLimits.items)fail("operationTooLarge");
      const labels=p.labels===undefined?[]:(needWrite(),chosenLabels(ctx,repo,p.labels));
      let base="",head="";
      if(kind==="pull"){
        needWrite();base=branchName(p.base);head=branchName(p.head);
        if(base===head||agentHead(base)||agentHead(head)&&base!=="main")fail("invalidInput");
        if(!branchExists(ctx,repo,base)||!branchExists(ctx,repo,head))fail("notFound");
        duplicate(ctx,repo,base,head);
      }else if(p.base!==undefined||p.head!==undefined)fail("invalidInput");
      const number=rows.reduce((max,row)=>Math.max(max,row.number),0)+1;
      const row=ctx.db.gitIssues.insert({id:issueId(repo.id,number),repoId:repo.id,number,kind,title:title(p.title),body:body(p.body),state:"open",authorId:userId,createdAt:at,updatedAt:at,closedAt:0,closedBy:"",
        labels:labels.join(" "),base,head,baseSha:"",headSha:"",mergedSha:"",revision:1});
      mention(ctx,files,row,row.body,userId,at);
      return {number};
    }
    case "update":{
      keys(p,["folderId","number","revision","title","body","state","labels"]);
      let row=issueOf(ctx,repo,p.number);
      if(p.revision!==row.revision)fail("conflict");
      const own=row.authorId===userId;
      if((p.title!==undefined||p.body!==undefined)&&!own&&!writer(role))fail("forbidden");
      if(p.title!==undefined){const next=title(p.title);if(next!==row.title){addEvent(ctx,files,row,"renamed",userId,at,{from:row.title,to:next});row={...row,title:next};}}
      if(p.body!==undefined){const next=body(p.body);if(next!==row.body){row={...row,body:next};mention(ctx,files,row,next,userId,at);}}
      if(p.labels!==undefined){
        needWrite();const next=chosenLabels(ctx,repo,p.labels),before=labelList(row.labels),names=new Map(labelRows(ctx,repo.id).map(label=>[label.id,label.name]));
        for(const id of next)if(!before.includes(id))addEvent(ctx,files,row,"labeled",userId,at,{label:names.get(id)});
        for(const id of before)if(!next.includes(id)&&names.has(id))addEvent(ctx,files,row,"unlabeled",userId,at,{label:names.get(id)});
        row={...row,labels:next.join(" ")};
      }
      if(p.state!==undefined&&p.state!==row.state){
        if(p.state!=="open"&&p.state!=="closed"||row.state==="merged")fail("invalidInput");
        if(row.kind==="pull"?!writer(role):!own&&!writer(role))fail("forbidden");
        if(p.state==="closed"){
          // A closed pull request keeps the tips it had, so its commits stay readable while they exist.
          const shas=row.kind==="pull"&&!agentHead(row.head)?{baseSha:tipOf(ctx,repo,row.base)??"",headSha:tipOf(ctx,repo,row.head)??""}:{};
          row={...row,...shas,state:"closed",closedAt:at,closedBy:userId};addEvent(ctx,files,row,"closed",userId,at);
        }else{
          if(row.kind==="pull"){if(!branchExists(ctx,repo,row.base)||!branchExists(ctx,repo,row.head))fail("conflict");duplicate(ctx,repo,row.base,row.head,row.id);}
          row={...row,state:"open",closedAt:0,closedBy:"",baseSha:"",headSha:""};addEvent(ctx,files,row,"reopened",userId,at);
        }
      }
      return {revision:touch(ctx,row,at).revision};
    }
    case "comment":{
      keys(p,["folderId","number","body"]);const row=issueOf(ctx,repo,p.number),text=body(p.body);
      if(!text)fail("invalidInput");
      if([...ctx.db.gitComments.issueId.filter(row.id)].length>=trackerLimits.comments)fail("operationTooLarge");
      const id=files.uuid();
      ctx.db.gitComments.insert({id,issueId:row.id,repoId:repo.id,sequence:nextSequence(ctx,row.id),kind:"comment",authorId:userId,body:text,data:"",createdAt:at,editedAt:0});
      touch(ctx,row,at);mention(ctx,files,row,text,userId,at);
      return {id};
    }
    case "editComment":
    case "deleteComment":{
      keys(p,["folderId","id",...operation==="editComment"?["body"]:[]]);
      const item=typeof p.id==="string"?ctx.db.gitComments.id.find(p.id):undefined;
      if(!item||item.repoId!==repo.id||item.kind!=="comment")fail("notFound");
      const row=ctx.db.gitIssues.id.find(item.issueId)!;
      if(operation==="deleteComment"){if(item.authorId!==userId&&!writer(role))fail("forbidden");ctx.db.gitComments.id.delete(item.id);touch(ctx,row,at);return {deleted:true};}
      if(item.authorId!==userId)fail("forbidden");
      const text=body(p.body);if(!text)fail("invalidInput");
      ctx.db.gitComments.id.update({...item,body:text,editedAt:at});touch(ctx,row,at);mention(ctx,files,row,text,userId,at);
      return {id:item.id};
    }
    case "saveLabel":{
      keys(p,["folderId","id","name","color","description"]);needWrite();
      const name=typeof p.name==="string"?p.name.replace(/\s+/g," ").trim():"",description=typeof p.description==="string"?p.description.replace(/\s+/g," ").trim():p.description===undefined?"":fail("invalidInput");
      if(!name||name.length>trackerLimits.labelName||description.length>trackerLimits.labelDescription||!gitLabelColors.includes(p.color as never))fail("invalidInput");
      const rows=labelRows(ctx,repo.id),existing=p.id===undefined?undefined:rows.find(row=>row.id===p.id)??fail("notFound");
      if(rows.some(row=>row.id!==existing?.id&&row.name.toLowerCase()===name.toLowerCase()))fail("alreadyExists");
      if(!existing&&rows.length>=trackerLimits.labels)fail("operationTooLarge");
      const row={id:existing?.id??files.uuid(),repoId:repo.id,name,color:p.color as string,description,createdAt:existing?.createdAt??at};
      if(existing)ctx.db.gitLabels.id.update(row);else ctx.db.gitLabels.insert(row);
      return labelView(row);
    }
    case "deleteLabel":{
      keys(p,["folderId","id"]);needWrite();
      const label=labelRows(ctx,repo.id).find(row=>row.id===p.id)??fail("notFound");
      for(const row of [...ctx.db.gitIssues.repoId.filter(repo.id)]){const list=labelList(row.labels);if(list.includes(label.id))ctx.db.gitIssues.id.update({...row,labels:list.filter(id=>id!==label.id).join(" "),revision:row.revision+1});}
      ctx.db.gitLabels.id.delete(label.id);
      return {deleted:true};
    }
  }
  fail("invalidInput");
}
