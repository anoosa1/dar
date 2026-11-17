import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {historyReadOperations} from "../../shared/read-operations.ts";
import {agentBranchDone,dropTracker} from "./tracker.ts";
import {historyLimits,historyBranchName,type HistoryChange,type HistoryChangeKind,type HistoryChangeset,type HistoryBranch} from "../../shared/history.ts";

type FileEntry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
type Version=NonNullable<ReturnType<Ctx["db"]["versions"]["id"]["find"]>>;
type Repo=NonNullable<ReturnType<Ctx["db"]["fileRepos"]["id"]["find"]>>;
type Changeset=NonNullable<ReturnType<Ctx["db"]["fileChangesets"]["id"]["find"]>>;
type Change=NonNullable<ReturnType<Ctx["db"]["fileChanges"]["id"]["find"]>>;
type Branch=NonNullable<ReturnType<Ctx["db"]["fileBranches"]["id"]["find"]>>;
type TreeItem={fileId:string;versionId:string;mode:string};
/** A path that differs between the live folder and main's head tree. */
interface Pending {path:string;previousPath:string;kind:HistoryChangeKind;entry?:FileEntry;previous?:TreeItem}

/** File operations from index.ts, bound to the acting account and browser session. */
export interface HistoryFiles {
  role(entry:FileEntry):"owner"|"write"|"read"|undefined;
  inLocked(entry:FileEntry):boolean;
  uuid():string;
  event(ownerId:string,fileId:string,action:string,parentId?:string):void;
  key(ownerId:string,parentId:string,name:string):string;
  findKey(listKey:string):FileEntry|undefined;
  nameOf(name:string):string;
  fitsQuota(meter:{used:number;reserved:number;quota:number},bytes:number):boolean;
  /** Moves or renames a file the caller may write, keeping its favorite and share listings in step. */
  move(entry:FileEntry,parentId:string,name:string):void;
}

export function fail(code:string):never{throw new SenderError(code);}
export const now=(ctx:Ctx)=>Number(ctx.timestamp.toMillis());
const uuid=/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
export const first=<T>(rows:Iterable<T>)=>rows[Symbol.iterator]().next().value as T|undefined;
export function id(value:unknown){if(typeof value!=="string"||!uuid.test(value))fail("invalidInput");return value;}
export function keys(p:Record<string,unknown>,allowed:string[]){if(Object.keys(p).some(k=>!allowed.includes(k)))fail("invalidInput");}
function text(value:unknown,required:boolean){
  if(value===undefined&&!required)return "";
  if(typeof value!=="string"||value.includes("\0")||[...value].some(c=>c.codePointAt(0)!>=0xd800&&c.codePointAt(0)!<=0xdfff))fail("invalidInput");
  const result=value.trim();
  if(required&&!result||encodeURIComponent(result).replace(/%[A-F\d]{2}/g,"x").length>historyLimits.messageBytes)fail("invalidInput");
  return result;
}
function revision(repo:Repo,value:unknown){if(typeof value!=="number"||repo.revision!==value)fail("conflict");}
const referenced=(ctx:Ctx,versionId:string)=>!!first(ctx.db.fileChanges.versionId.filter(versionId));
export function charge(ctx:Ctx,files:HistoryFiles,ownerId:string,bytes:number){
  const meter=ctx.db.usage.userId.find(ownerId)??ctx.db.usage.insert({userId:ownerId,used:0,reserved:0,quota:0});
  if(!files.fitsQuota(meter,bytes))fail("quotaExceeded");ctx.db.usage.userId.update({...meter,used:meter.used+bytes});
}
export function release(ctx:Ctx,ownerId:string,bytes:number){const meter=ctx.db.usage.userId.find(ownerId);if(meter&&bytes)ctx.db.usage.userId.update({...meter,used:Math.max(0,meter.used-bytes)});}
export function fresh(ctx:Ctx,files:HistoryFiles){let value=files.uuid();while(ctx.db.fileChangesets.id.find(value)||ctx.db.fileBranches.id.find(value))value=files.uuid();return value;}

/** A folder the caller can read (or write); never a trashed one or the Locked folder root. */
export function folderOf(ctx:Ctx,files:HistoryFiles,value:unknown,write:boolean){
  const entry=ctx.db.fileEntries.id.find(id(value));
  if(!entry||entry.kind!=="folder"||entry.trashed)fail("notFound");
  const role=files.role(entry);if(!role)fail("notFound");
  if(write&&role==="read")fail("forbidden");
  return {entry,role};
}
function trackedAncestor(ctx:Ctx,entry:FileEntry){
  let parentId=entry.parentId;
  for(let depth=0;parentId&&depth<128;depth++){if(ctx.db.fileRepos.id.find(parentId))return true;parentId=ctx.db.fileEntries.id.find(parentId)?.parentId??"";}
  return false;
}

/** Live files under a tracked folder by path. Tracked folders inside it keep their own history. */
export function scan(ctx:Ctx,root:FileEntry){
  const result=new Map<string,FileEntry>(),queue=[{id:root.id,path:"",depth:0}];
  for(let i=0;i<queue.length;i++){
    const folder=queue[i],prefix=`${root.ownerId}/${folder.id}/`,range=new Range<string>({tag:"included",value:prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
    for(const entry of ctx.db.fileEntries.listKey.filter(range)){
      if(entry.trashed)continue;
      const path=folder.path+entry.name;
      if(entry.kind==="folder"){
        if(ctx.db.fileRepos.id.find(entry.id))continue;
        if(folder.depth+1>=historyLimits.depth)fail("operationTooLarge");
        queue.push({id:entry.id,path:path+"/",depth:folder.depth+1});
      }else{if(result.size>=historyLimits.files)fail("operationTooLarge");result.set(path,entry);}
    }
  }
  return result;
}
export function headTree(ctx:Ctx,repoId:string){
  const result=new Map<string,TreeItem>();
  for(const row of ctx.db.fileTrees.repoId.filter(repoId))result.set(row.path,{fileId:row.fileId,versionId:row.versionId,mode:row.mode});
  return result;
}
const byPath=(a:{path:string},b:{path:string})=>a.path<b.path?-1:a.path>b.path?1:0;
/** Uncommitted changes: the live folder against main's head, by content. A file that moved is a rename. */
export function status(ctx:Ctx,repo:Repo,root:FileEntry){
  const live=scan(ctx,root),head=headTree(ctx,repo.id),changes:Pending[]=[],added=new Map<string,Pending>(),deleted:Pending[]=[];
  for(const [path,entry] of live){
    const old=head.get(path);
    if(!old)added.set(entry.id,{path,previousPath:"",kind:"added",entry});
    else if(ctx.db.versions.id.find(old.versionId)?.blobId!==entry.blobId)changes.push({path,previousPath:"",kind:"modified",entry,previous:old});
  }
  for(const [path,old] of head)if(!live.has(path))deleted.push({path,previousPath:"",kind:"deleted",previous:old});
  for(const gone of deleted){
    const moved=added.get(gone.previous!.fileId);
    if(moved){added.delete(gone.previous!.fileId);changes.push({path:moved.path,previousPath:gone.path,kind:"renamed",entry:moved.entry,previous:gone.previous});}
    else changes.push(gone);
  }
  changes.push(...added.values());changes.sort(byPath);
  return {live,changes,token:token(repo,changes)};
}
/** Identifies a status, so a commit fails if the folder changed after it was reviewed. */
function token(repo:Repo,changes:Pending[]){
  const value=repo.headId+"\n"+changes.map(c=>`${c.kind}\0${c.path}\0${c.previousPath}\0${c.entry?.blobId??""}`).join("\n");
  let a=0x811c9dc5,b=0x9747b28c;
  for(let i=0;i<value.length;i++){const c=value.charCodeAt(i);a=Math.imul(a^c,0x01000193)>>>0;b=Math.imul(b^c,0x5bd1e995)>>>0;b=(b^(b>>>15))>>>0;}
  return a.toString(16).padStart(8,"0")+b.toString(16).padStart(8,"0");
}
function pendingView(ctx:Ctx,item:Pending):HistoryChange{
  const old=item.previous&&ctx.db.versions.id.find(item.previous.versionId);
  return {path:item.path,kind:item.kind,...item.previousPath?{previousPath:item.previousPath}:{},
    ...item.entry?{fileId:item.entry.id,size:item.entry.size,mediaType:item.entry.mediaType}:{},
    ...old?{previousVersionId:old.id,previousSize:old.size,previousMediaType:old.mediaType}:{}};
}
function changeView(ctx:Ctx,change:Change):HistoryChange{
  const next=change.versionId?ctx.db.versions.id.find(change.versionId):undefined,old=change.previousVersionId?ctx.db.versions.id.find(change.previousVersionId):undefined;
  return {path:change.path,kind:change.kind as HistoryChangeKind,...change.previousPath?{previousPath:change.previousPath}:{},...change.fileId?{fileId:change.fileId}:{},
    ...next?{versionId:next.id,size:next.size,mediaType:next.mediaType}:{},...old?{previousVersionId:old.id,previousSize:old.size,previousMediaType:old.mediaType}:{}};
}
function author(ctx:Ctx,userId:string){const account=ctx.db.accounts.id.find(userId);return account?account.displayName||account.username:"Removed account";}
/** Commits Git has seen show their Git ID and author, which for pushed commits is not the Dar account that pushed. */
export function changesetView(ctx:Ctx,row:Changeset):HistoryChangeset{
  const git=first(ctx.db.fileGitObjects.changesetId.filter(row.id));
  return {id:row.id,sequence:row.sequence,kind:row.kind as HistoryChangeset["kind"],message:row.message,author:git?.author||author(ctx,row.authorId),createdAt:row.createdAt,fileCount:row.fileCount,...git?{sha:git.sha}:{}};
}
function branchView(ctx:Ctx,branch:Branch):HistoryBranch{
  const set=ctx.db.fileChangesets.id.find(branch.changesetId);
  return {name:branch.name,threadId:branch.threadId,message:set?.message??"",author:author(ctx,branch.authorId),createdAt:branch.createdAt,modifiedAt:branch.modifiedAt,
    changes:set?[...ctx.db.fileChanges.changesetId.filter(set.id)].map(change=>changeView(ctx,change)).sort(byPath):[]};
}

/** The version holding a file's current content; files without one (from before versions) get one. */
function currentVersion(ctx:Ctx,files:HistoryFiles,entry:FileEntry):Version{
  let best:Version|undefined;
  for(const version of ctx.db.versions.fileId.filter(entry.id))if(version.blobId===entry.blobId&&(!best||version.createdAt>best.createdAt))best=version;
  if(best)return best;
  charge(ctx,files,entry.ownerId,entry.size);
  return ctx.db.versions.insert({id:files.uuid(),fileId:entry.id,blobId:entry.blobId,size:entry.size,mediaType:entry.mediaType,createdAt:now(ctx)});
}
/**
 * Appends a changeset to main and moves the head tree. A file keeps its Git mode (executable,
 * symbolic link) through edits and renames; `modes` sets them explicitly, as restore does.
 */
function record(ctx:Ctx,files:HistoryFiles,repo:Repo,userId:string,items:Pending[],kind:HistoryChangeset["kind"],message:string,modes?:Map<string,string>){
  if(!items.length)fail("nothingToCommit");
  if(items.length>historyLimits.changes||repo.sequence>=historyLimits.changesets)fail("operationTooLarge");
  const changesetId=fresh(ctx,files),sequence=repo.sequence+1;
  ctx.db.fileChangesets.insert({id:changesetId,repoId:repo.id,branch:"",parentId:repo.headId,sequence,kind,message,authorId:userId,fileCount:items.length,createdAt:now(ctx)});
  items.forEach((item,index)=>{
    const version=item.entry?currentVersion(ctx,files,item.entry):undefined,previousMode=item.previous?.mode??"",mode=item.kind==="deleted"?"":modes?.get(item.path)??previousMode;
    ctx.db.fileChanges.insert({id:`${changesetId}/${index}`,changesetId,path:item.path,previousPath:item.previousPath,kind:item.kind,fileId:item.entry?.id??item.previous?.fileId??"",versionId:version?.id??"",previousVersionId:item.previous?.versionId??"",expectRevision:0,mode,previousMode});
    if(item.kind==="deleted")ctx.db.fileTrees.id.delete(`${repo.id}/${item.path}`);
    if(item.kind==="renamed")ctx.db.fileTrees.id.delete(`${repo.id}/${item.previousPath}`);
    if(version){
      const row={id:`${repo.id}/${item.path}`,repoId:repo.id,path:item.path,fileId:item.entry!.id,versionId:version.id,mode};
      if(ctx.db.fileTrees.id.find(row.id))ctx.db.fileTrees.id.update(row);else ctx.db.fileTrees.insert(row);
    }
  });
  ctx.db.fileRepos.id.update({...repo,headId:changesetId,sequence,revision:repo.revision+1,modifiedAt:now(ctx)});
  return {id:changesetId,sequence};
}
/** Main's tree at a changeset: the head tree with every later changeset undone, newest first. */
function treeAt(ctx:Ctx,repo:Repo,target:Changeset){
  const result=headTree(ctx,repo.id);
  const later=[...ctx.db.fileChangesets.repoId.filter(repo.id)].filter(row=>!row.branch&&row.sequence>target.sequence).sort((a,b)=>b.sequence-a.sequence);
  for(const set of later){
    const changes=[...ctx.db.fileChanges.changesetId.filter(set.id)];
    // Removals first: a path can be renamed away and another file added there in one changeset.
    for(const change of changes)if(change.kind==="added"||change.kind==="renamed")result.delete(change.path);
    for(const change of changes)if(change.kind!=="added")result.set(change.kind==="renamed"?change.previousPath:change.path,{fileId:change.fileId,versionId:change.previousVersionId,mode:change.previousMode});
  }
  return result;
}

export function trash(ctx:Ctx,files:HistoryFiles,entry:FileEntry){
  ctx.db.fileEntries.id.update({...entry,trashed:true,listKey:`${entry.ownerId}/trash/${entry.name.toLowerCase()}/${entry.id}`,revision:entry.revision+1,modifiedAt:now(ctx)});
  files.event(entry.ownerId,entry.id,"trashed");
}
/**
 * Gives a file a version's content. A version from another file is copied and counted, as
 * Restore version does; `adopt` hands over a staged branch version that is already counted.
 */
export function setContent(ctx:Ctx,files:HistoryFiles,entry:FileEntry,version:Version,adopt:boolean){
  if(adopt)ctx.db.versions.id.update({...version,fileId:entry.id});
  else if(version.fileId!==entry.id){charge(ctx,files,entry.ownerId,version.size);ctx.db.versions.insert({...version,id:files.uuid(),fileId:entry.id,createdAt:now(ctx)});}
  ctx.db.fileEntries.id.update({...entry,blobId:version.blobId,size:version.size,mediaType:version.mediaType,revision:entry.revision+1,modifiedAt:now(ctx)});
  files.event(entry.ownerId,entry.id,"updated");
}
/** The folder at a path under the tracked folder, created with any missing parents. */
function folderFor(ctx:Ctx,files:HistoryFiles,root:FileEntry,parts:string[]){
  let parentId=root.id;
  for(const part of parts){
    const existing=files.findKey(files.key(root.ownerId,parentId,part));
    if(existing){if(existing.kind!=="folder")fail("conflict");parentId=existing.id;continue;}
    const folderId=files.uuid();
    ctx.db.fileEntries.insert({id:folderId,ownerId:root.ownerId,parentId,name:part,kind:"folder",blobId:"",size:0,mediaType:"inode/directory",revision:1,modifiedAt:now(ctx),trashed:false,favorite:false,listKey:files.key(root.ownerId,parentId,part)});
    files.event(root.ownerId,folderId,"created");parentId=folderId;
  }
  return parentId;
}
/** Moves a file to a path under the tracked folder. */
export function moveFile(ctx:Ctx,files:HistoryFiles,root:FileEntry,entry:FileEntry,path:string){
  const parts=path.split("/"),name=parts.pop()!,parentId=folderFor(ctx,files,root,parts);
  if(files.findKey(files.key(root.ownerId,parentId,name)))fail("conflict");
  files.move(entry,parentId,name);
}
/** Creates a file at a path under the tracked folder, with any missing folders. */
export function createFile(ctx:Ctx,files:HistoryFiles,root:FileEntry,path:string,version:Version,adopt:boolean){
  const parts=path.split("/"),name=parts.pop()!,parentId=folderFor(ctx,files,root,parts);
  const listKey=files.key(root.ownerId,parentId,name);
  if(files.findKey(listKey))fail("conflict");
  // A file deleted for good comes back with its own ID, and so with its kept versions.
  const own=!adopt&&uuid.test(version.fileId)&&!ctx.db.fileEntries.id.find(version.fileId),fileId=own?version.fileId:files.uuid();
  ctx.db.fileEntries.insert({id:fileId,ownerId:root.ownerId,parentId,name,kind:"file",blobId:version.blobId,size:version.size,mediaType:version.mediaType,revision:1,modifiedAt:now(ctx),trashed:false,favorite:false,listKey});
  if(adopt)ctx.db.versions.id.update({...version,fileId});
  else if(!own){charge(ctx,files,root.ownerId,version.size);ctx.db.versions.insert({...version,id:files.uuid(),fileId,createdAt:now(ctx)});}
  files.event(root.ownerId,fileId,"created");
}

/** Drops a branch and the staged versions nothing else uses. */
function dropBranch(ctx:Ctx,repo:Repo,branch:Branch){
  const set=ctx.db.fileChangesets.id.find(branch.changesetId);
  if(set){for(const change of [...ctx.db.fileChanges.changesetId.filter(set.id)])ctx.db.fileChanges.id.delete(change.id);ctx.db.fileChangesets.id.delete(set.id);}
  let bytes=0;
  for(const version of [...ctx.db.versions.fileId.filter(`staged:${branch.id}`)])if(!referenced(ctx,version.id)){bytes+=version.size;ctx.db.versions.id.delete(version.id);}
  release(ctx,repo.ownerId,bytes);ctx.db.fileBranches.id.delete(branch.id);
}
/**
 * Deletes a folder's history. Versions of live files stay in their Versions list; versions
 * only history kept (of files deleted for good, or staged) are released from the quota.
 */
export function dropRepo(ctx:Ctx,repo:Repo){
  dropTracker(ctx,repo.id);
  for(const branch of [...ctx.db.fileBranches.repoId.filter(repo.id)])dropBranch(ctx,repo,branch);
  const versionIds=new Set<string>();let rows=0;
  for(const set of [...ctx.db.fileChangesets.repoId.filter(repo.id)]){
    for(const change of [...ctx.db.fileChanges.changesetId.filter(set.id)]){
      if(++rows>250_000)fail("operationTooLarge");
      if(change.versionId)versionIds.add(change.versionId);ctx.db.fileChanges.id.delete(change.id);
    }
    ctx.db.fileChangesets.id.delete(set.id);
  }
  for(const row of [...ctx.db.fileTrees.repoId.filter(repo.id)])ctx.db.fileTrees.id.delete(row.id);
  for(const row of [...ctx.db.fileGitObjects.repoId.filter(repo.id)])ctx.db.fileGitObjects.id.delete(row.id);
  for(const row of [...ctx.db.fileGitRefs.repoId.filter(repo.id)])ctx.db.fileGitRefs.id.delete(row.id);
  for(const row of [...ctx.db.fileGitBlobs.repoId.filter(repo.id)])ctx.db.fileGitBlobs.id.delete(row.id);
  let bytes=0;
  for(const versionId of versionIds){
    const version=ctx.db.versions.id.find(versionId);
    if(!version||referenced(ctx,versionId)||ctx.db.fileEntries.id.find(version.fileId))continue;
    bytes+=version.size;ctx.db.versions.id.delete(versionId);
  }
  release(ctx,repo.ownerId,bytes);ctx.db.fileRepos.id.delete(repo.id);
}
/** Versions history keeps when their file is deleted for good. */
export const historyKeeps=(ctx:Ctx,versionId:string)=>referenced(ctx,versionId);

function agentThread(ctx:Ctx,userId:string,value:unknown,folderId:string){
  const thread=ctx.db.agentThreads.id.find(id(value));
  const project=thread&&ctx.db.agentProjects.id.find(thread.projectId);
  if(!thread||thread.ownerId!==userId||!project||project.sourceType!=="files"||project.sourceRef!==folderId)fail("notFound");
  return thread;
}
function safePath(files:HistoryFiles,value:unknown){
  if(typeof value!=="string"||!value||value.length>2048)fail("invalidInput");
  const parts=value.split("/");if(parts.length>historyLimits.depth||parts.some(part=>files.nameOf(part)!==part))fail("invalidInput");
  return value;
}
const mediaType=(value:unknown)=>typeof value==="string"&&value.length<=150&&/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(value)?value.toLowerCase():fail("invalidInput");

/** Every check runs in the caller's transaction; `uploadKey` ties staged blobs to the browser session. */
export function historyOperation(ctx:Ctx,files:HistoryFiles,userId:string,uploadKey:string,operation:string,p:Record<string,unknown>){
  const write=!historyReadOperations.has(operation);
  const {entry:root,role}=folderOf(ctx,files,p.folderId,write);
  const repo=ctx.db.fileRepos.id.find(root.id);
  if(operation==="track"||operation==="overview"){
    const blocked=files.inLocked(root)?"locked" as const:trackedAncestor(ctx,root)?"nested" as const:[...ctx.db.fileRepos.ownerId.filter(root.ownerId)].length>=historyLimits.repos?"limit" as const:undefined;
    if(operation==="track"){
      keys(p,["folderId"]);if(role!=="owner")fail("forbidden");if(repo)return {revision:repo.revision};
      if(blocked==="limit")fail("quotaExceeded");if(blocked)fail("forbidden");
      ctx.db.fileRepos.insert({id:root.id,ownerId:root.ownerId,headId:"",sequence:0,revision:1,createdAt:now(ctx),modifiedAt:now(ctx)});
      return {revision:1};
    }
    keys(p,["folderId","before"]);
    const base={folder:{id:root.id,name:root.name},tracked:!!repo,canWrite:role!=="read",owner:role==="owner"};
    if(!repo)return {...base,...blocked?{blocked}:{},branches:[],gitRefs:[],changesets:[],nextBefore:null};
    const before=p.before===undefined?Infinity:typeof p.before==="number"&&Number.isInteger(p.before)&&p.before>0?p.before:fail("invalidInput");
    const current=status(ctx,repo,root),rows=[...ctx.db.fileChangesets.repoId.filter(repo.id)].filter(row=>!row.branch&&row.sequence<before).sort((a,b)=>b.sequence-a.sequence),page=rows.slice(0,historyLimits.page);
    return {...base,repo:{revision:repo.revision,headId:repo.headId,sequence:repo.sequence,createdAt:repo.createdAt},
      status:{changes:current.changes.map(item=>pendingView(ctx,item)),token:current.token},
      branches:[...ctx.db.fileBranches.repoId.filter(repo.id)].map(branch=>branchView(ctx,branch)).sort((a,b)=>b.modifiedAt-a.modifiedAt),
      gitRefs:[...ctx.db.fileGitRefs.repoId.filter(repo.id)].map(row=>({name:row.name,sha:row.target})).sort((a,b)=>a.name<b.name?-1:a.name>b.name?1:0),
      changesets:page.map(row=>changesetView(ctx,row)),nextBefore:rows.length>page.length?page.at(-1)!.sequence:null};
  }
  if(!repo)fail("notFound");
  switch(operation){
    case "untrack":{
      keys(p,["folderId","revision"]);if(role!=="owner")fail("forbidden");revision(repo,p.revision);
      dropRepo(ctx,repo);return {tracked:false};
    }
    case "commit":{
      keys(p,["folderId","message","token","paths","snapshot"]);
      const current=status(ctx,repo,root);if(p.token!==current.token)fail("conflict");
      const snapshot=p.snapshot===true,message=text(p.message,!snapshot)||"Snapshot";
      let items=current.changes;
      if(p.paths!==undefined){
        if(!Array.isArray(p.paths)||p.paths.length>historyLimits.changes||p.paths.some(path=>typeof path!=="string"))fail("invalidInput");
        const chosen=new Set(p.paths as string[]);items=items.filter(item=>chosen.has(item.path));
        if(items.length!==chosen.size)fail("conflict");
      }
      return record(ctx,files,repo,userId,items,snapshot?"snapshot":"commit",message);
    }
    case "changeset":{
      keys(p,["folderId","id"]);const row=ctx.db.fileChangesets.id.find(id(p.id));
      if(!row||row.repoId!==repo.id||row.branch)fail("notFound");
      return {changeset:changesetView(ctx,row),changes:[...ctx.db.fileChanges.changesetId.filter(row.id)].map(change=>changeView(ctx,change)).sort(byPath)};
    }
    case "content":{
      // Any version this folder's history or branches refer to; live content uses the file's own route.
      keys(p,["folderId","versionId"]);const versionId=id(p.versionId),version=ctx.db.versions.id.find(versionId);
      if(!version)fail("notFound");
      let path="";
      for(const change of ctx.db.fileChanges.versionId.filter(versionId))if(ctx.db.fileChangesets.id.find(change.changesetId)?.repoId===repo.id){path=change.path;break;}
      // A branch change's old side is a version of a file in this folder.
      if(!path)for(const [livePath,entry] of scan(ctx,root))if(entry.id===version.fileId){path=livePath;break;}
      if(!path)fail("notFound");
      return {id:version.id,name:path.split("/").at(-1),blobId:version.blobId,size:version.size,mediaType:version.mediaType,revision:0};
    }
    case "restore":{
      keys(p,["folderId","id","revision"]);revision(repo,p.revision);
      const target=ctx.db.fileChangesets.id.find(id(p.id));if(!target||target.repoId!==repo.id||target.branch)fail("notFound");
      const current=status(ctx,repo,root);if(current.changes.length)fail("uncommittedChanges");
      const want=treeAt(ctx,repo,target);
      for(const [path,entry] of current.live)if(!want.has(path))trash(ctx,files,entry);
      for(const [path,item] of want){
        const version=ctx.db.versions.id.find(item.versionId);if(!version)fail("contentUnavailable");
        const entry=current.live.get(path);
        if(!entry)createFile(ctx,files,root,path,version,false);
        else if(entry.blobId!==version.blobId)setContent(ctx,files,entry,version,false);
      }
      const label=target.message.length>120?target.message.slice(0,119)+"…":target.message,after=status(ctx,repo,root),items=after.changes,head=headTree(ctx,repo.id);
      // Same content with another Git mode (an executable bit) is a change too.
      for(const [path,item] of want){const old=head.get(path),entry=after.live.get(path);if(old&&entry&&old.mode!==item.mode&&!items.some(change=>change.path===path))items.push({path,previousPath:"",kind:"modified",entry,previous:old});}
      items.sort(byPath);
      return record(ctx,files,ctx.db.fileRepos.id.find(repo.id)!,userId,items,"restore",`Restore to “${label}”`,new Map([...want].map(([path,item])=>[path,item.mode])));
    }
    case "branch":{
      keys(p,["folderId","threadId"]);const thread=agentThread(ctx,userId,p.threadId,root.id);
      const branch=[...ctx.db.fileBranches.repoId.filter(repo.id)].find(row=>row.threadId===thread.id);
      return {revision:repo.revision,branch:branch?branchView(ctx,branch):null};
    }
    case "merge":
    case "discard":{
      keys(p,["folderId","name","revision"]);revision(repo,p.revision);
      const branch=[...ctx.db.fileBranches.repoId.filter(repo.id)].find(row=>row.name===p.name);if(!branch)fail("notFound");
      if(operation==="discard"){agentBranchDone(ctx,files,repo.id,branch.name,userId,false);dropBranch(ctx,repo,branch);ctx.db.fileRepos.id.update({...repo,revision:repo.revision+1,modifiedAt:now(ctx)});return {merged:false};}
      const set=ctx.db.fileChangesets.id.find(branch.changesetId)!,changes=[...ctx.db.fileChanges.changesetId.filter(set.id)],live=scan(ctx,root);
      // Each file must still be as the agent found it; nothing is applied otherwise.
      const conflicts=changes.filter(change=>{const entry=live.get(change.path);return change.kind==="added"?!!entry:!entry||entry.id!==change.fileId||entry.revision!==change.expectRevision;}).map(change=>change.path);
      if(conflicts.length)return {merged:false,conflicts};
      for(const change of changes){
        const entry=live.get(change.path),version=change.versionId?ctx.db.versions.id.find(change.versionId):undefined;
        if(change.kind==="deleted")trash(ctx,files,entry!);
        else if(!version)fail("contentUnavailable");
        else if(entry)setContent(ctx,files,entry,version,true);
        else createFile(ctx,files,root,change.path,version,true);
      }
      const paths=new Set(changes.map(change=>change.path)),items=status(ctx,repo,root).changes.filter(item=>paths.has(item.path)||paths.has(item.previousPath));
      // The changes now belong to main; the branch's staged versions became the files' own.
      for(const change of changes)ctx.db.fileChanges.id.delete(change.id);
      ctx.db.fileChangesets.id.delete(set.id);ctx.db.fileBranches.id.delete(branch.id);
      agentBranchDone(ctx,files,repo.id,branch.name,userId,true);
      if(!items.length){ctx.db.fileRepos.id.update({...repo,revision:repo.revision+1,modifiedAt:now(ctx)});return {merged:true};}
      return {merged:true,...record(ctx,files,ctx.db.fileRepos.id.find(repo.id)!,userId,items,"merge",`Merge ${branch.name}: ${set.message}`.slice(0,historyLimits.messageBytes/2))};
    }
    case "stageBegin":{
      // Reserves blobs for an Agent branch; maintenance releases what is never staged.
      keys(p,["folderId","threadId","files"]);agentThread(ctx,userId,p.threadId,root.id);
      if(!Array.isArray(p.files)||p.files.length>historyLimits.changes)fail("invalidInput");
      const rows=(p.files as unknown[]).map(row=>{const value=row as Record<string,unknown>;if(!value||typeof value.size!=="number"||!Number.isSafeInteger(value.size)||value.size<0||value.size>16*1024*1024)fail("invalidInput");return {size:value.size,mediaType:mediaType(value.mediaType)};});
      const meter=ctx.db.usage.userId.find(root.ownerId)??ctx.db.usage.insert({userId:root.ownerId,used:0,reserved:0,quota:0}),total=rows.reduce((sum,row)=>sum+row.size,0);
      if(!files.fitsQuota(meter,total))fail("quotaExceeded");
      ctx.db.usage.userId.update({...meter,reserved:meter.reserved+total});
      const expiresAt=now(ctx)+3600_000;
      return {uploads:rows.map(row=>{const uploadId=files.uuid();ctx.db.uploads.insert({id:uploadId,sessionId:uploadKey,ownerId:root.ownerId,parentId:"",fileId:"",name:"",size:row.size,mediaType:row.mediaType,expectedRevision:0,expiresAt,state:"uploading",digest:"",expiresKey:String(expiresAt).padStart(16,"0")});return uploadId;})};
    }
    case "stage":{
      keys(p,["folderId","threadId","title","files"]);const thread=agentThread(ctx,userId,p.threadId,root.id),name=historyBranchName(thread.id),title=text(p.title,true);
      if(!Array.isArray(p.files)||p.files.length>historyLimits.changes)fail("invalidInput");
      let branch=[...ctx.db.fileBranches.repoId.filter(repo.id)].find(row=>row.threadId===thread.id);
      if(branch){const kept={...branch};dropBranch(ctx,repo,branch);branch=kept;}
      const branchId=branch?.id??fresh(ctx,files),changesetId=fresh(ctx,files),seen=new Set<string>(),changes:Change[]=[];
      for(const [index,row] of (p.files as Record<string,unknown>[]).entries()){
        if(!row||typeof row!=="object")fail("invalidInput");
        const path=safePath(files,row.path),kind=row.kind;if(seen.has(path.toLowerCase()))fail("invalidInput");seen.add(path.toLowerCase());
        if(kind!=="added"&&kind!=="modified"&&kind!=="deleted")fail("invalidInput");
        let fileId="",expectRevision=0,previousVersionId="";
        if(kind!=="added"){
          const entry=ctx.db.fileEntries.id.find(id(row.fileId));if(!entry||entry.kind!=="file"||entry.ownerId!==root.ownerId||typeof row.expectRevision!=="number"||!Number.isInteger(row.expectRevision)||row.expectRevision<1)fail("invalidInput");
          fileId=entry.id;expectRevision=row.expectRevision;
          for(const version of ctx.db.versions.fileId.filter(entry.id))if(version.blobId===entry.blobId)previousVersionId=version.id;
        }
        let versionId="";
        if(kind!=="deleted"){
          const upload=ctx.db.uploads.id.find(id(row.uploadId));
          if(!upload||upload.sessionId!==uploadKey||upload.state!=="uploading"||upload.ownerId!==root.ownerId||typeof row.digest!=="string"||!/^[a-f0-9]{64}$/.test(row.digest))fail("notFound");
          ctx.db.uploads.id.update({...upload,state:"complete",digest:row.digest});
          const meter=ctx.db.usage.userId.find(upload.ownerId)!;ctx.db.usage.userId.update({...meter,reserved:Math.max(0,meter.reserved-upload.size),used:meter.used+upload.size});
          ctx.db.versions.insert({id:upload.id,fileId:`staged:${branchId}`,blobId:upload.id,size:upload.size,mediaType:upload.mediaType,createdAt:now(ctx)});
          versionId=upload.id;
        }
        changes.push({id:`${changesetId}/${index}`,changesetId,path,previousPath:"",kind,fileId,versionId,previousVersionId,expectRevision,mode:"",previousMode:""});
      }
      if(!changes.length)return {revision:repo.revision,branch:null};
      ctx.db.fileChangesets.insert({id:changesetId,repoId:repo.id,branch:name,parentId:repo.headId,sequence:0,kind:"branch",message:title,authorId:userId,fileCount:changes.length,createdAt:now(ctx)});
      for(const change of changes)ctx.db.fileChanges.insert(change);
      const row={id:branchId,repoId:repo.id,name,threadId:thread.id,changesetId,authorId:userId,createdAt:branch?.createdAt??now(ctx),modifiedAt:now(ctx)};
      ctx.db.fileBranches.insert(row);
      ctx.db.fileRepos.id.update({...repo,revision:repo.revision+1,modifiedAt:now(ctx)});
      return {revision:repo.revision+1,branch:branchView(ctx,row)};
    }
  }
  fail("invalidInput");
}

/** Backup rows; restore validates references after Files and versions are back. */
export function historySnapshot(ctx:Ctx){
  return {repos:[...ctx.db.fileRepos.iter()],changesets:[...ctx.db.fileChangesets.iter()],changes:[...ctx.db.fileChanges.iter()],trees:[...ctx.db.fileTrees.iter()],branches:[...ctx.db.fileBranches.iter()],
    gitObjects:[...ctx.db.fileGitObjects.iter()],gitRefs:[...ctx.db.fileGitRefs.iter()],gitBlobs:[...ctx.db.fileGitBlobs.iter()]};
}
/** The owner of each version only history keeps, so restore can count it against the right quota. */
export function historyVersionOwners(value:ReturnType<typeof historySnapshot>|undefined){
  const owners=new Map<string,string>();if(!value)return owners;
  const repoOwner=new Map(value.repos.map(repo=>[repo.id,repo.ownerId])),setOwner=new Map(value.changesets.map(set=>[set.id,repoOwner.get(set.repoId)]));
  for(const change of value.changes){const owner=setOwner.get(change.changesetId);if(owner&&change.versionId)owners.set(change.versionId,owner);}
  return owners;
}
export function restoreHistory(ctx:Ctx,value:ReturnType<typeof historySnapshot>|undefined){
  if(!value)return;
  for(const repo of value.repos){const folder=ctx.db.fileEntries.id.find(repo.id);if(!folder||folder.kind!=="folder"||folder.ownerId!==repo.ownerId)fail("invalidInput");ctx.db.fileRepos.insert(repo);}
  for(const set of value.changesets){if(!ctx.db.fileRepos.id.find(set.repoId))fail("invalidInput");ctx.db.fileChangesets.insert(set);}
  for(const change of value.changes){if(!ctx.db.fileChangesets.id.find(change.changesetId)||change.versionId&&!ctx.db.versions.id.find(change.versionId))fail("invalidInput");ctx.db.fileChanges.insert({...change,mode:change.mode??"",previousMode:change.previousMode??""});}
  for(const row of value.trees){if(!ctx.db.fileRepos.id.find(row.repoId)||!ctx.db.versions.id.find(row.versionId))fail("invalidInput");ctx.db.fileTrees.insert({...row,mode:row.mode??""});}
  for(const row of value.branches){if(!ctx.db.fileRepos.id.find(row.repoId)||!ctx.db.fileChangesets.id.find(row.changesetId))fail("invalidInput");ctx.db.fileBranches.insert(row);}
  // Backups from before Git support (format v9 too) have no Git rows.
  for(const row of value.gitObjects??[]){if(!ctx.db.fileRepos.id.find(row.repoId)||row.changesetId&&ctx.db.fileChangesets.id.find(row.changesetId)?.repoId!==row.repoId)fail("invalidInput");ctx.db.fileGitObjects.insert(row);}
  for(const row of value.gitRefs??[]){if(!ctx.db.fileRepos.id.find(row.repoId)||!ctx.db.fileGitObjects.id.find(`${row.repoId}/${row.target}`))fail("invalidInput");ctx.db.fileGitRefs.insert(row);}
  for(const row of value.gitBlobs??[]){if(!ctx.db.fileRepos.id.find(row.repoId))fail("invalidInput");ctx.db.fileGitBlobs.insert(row);}
}
