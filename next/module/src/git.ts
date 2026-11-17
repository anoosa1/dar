import type {Ctx} from "./index.ts";
import {historyLimits,gitLimits} from "../../shared/history.ts";
import {fail,now,first,keys,charge,release,fresh,folderOf,scan,headTree,status,trash,setContent,createFile,moveFile,type HistoryFiles} from "./history.ts";

/**
 * Git over HTTPS for tracked folders (docs/spacetimedb/folder-history.md#git). The runtime keeps a
 * Git repository as a cache and does the Git work; these operations keep the history itself:
 * commit and tag objects, refs, and pushes applied to main and the live folder in one transaction.
 */
type FileEntry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
type Repo=NonNullable<ReturnType<Ctx["db"]["fileRepos"]["id"]["find"]>>;
type Changeset=NonNullable<ReturnType<Ctx["db"]["fileChangesets"]["id"]["find"]>>;
type GitObject=NonNullable<ReturnType<Ctx["db"]["fileGitObjects"]["id"]["find"]>>;
type Kind="added"|"modified"|"deleted"|"renamed";
/** One path in a pushed commit, against the commit's base tree. Blobs are Git IDs. */
interface PushChange {path:string;previousPath:string;kind:Kind;mode:string;previousMode:string;blob:string;previousBlob:string;mediaType:string}
interface PushCommit {sha:string;raw:string;parents:string[];author:string;message:string;time:number;changes:PushChange[]}
interface PushTag {sha:string;raw:string;target:string;author:string}
interface PushUpdate {name:string;old:string;new:string}

const zero="0".repeat(40),shaPattern=/^[0-9a-f]{40}$/,modes=new Set(["","100755","120000"]),mainRef="refs/heads/main";
const pseudoFile=(repoId:string)=>`git:${repoId}`;
function sha(value:unknown){if(typeof value!=="string"||!shaPattern.test(value))fail("invalidInput");return value;}
function list<T>(value:unknown,max:number):T[]{if(!Array.isArray(value)||value.length>max)fail("invalidInput");return value as T[];}
function line(value:unknown,max:number){if(typeof value!=="string"||value.length>max||/[\0\n\r]/.test(value))fail("invalidInput");return value;}
function base64(value:unknown,max:number){if(typeof value!=="string"||!value||value.length>max||!/^[A-Za-z0-9+/]+=*$/.test(value))fail("invalidInput");return value;}
const objectOf=(ctx:Ctx,repo:Repo,value:string)=>ctx.db.fileGitObjects.id.find(`${repo.id}/${value}`);
const pinOf=(ctx:Ctx,changesetId:string)=>changesetId?first(ctx.db.fileGitObjects.changesetId.filter(changesetId)):undefined;
const referenced=(ctx:Ctx,versionId:string)=>!!first(ctx.db.fileChanges.versionId.filter(versionId));
/** Stored content with this Git blob ID, if a version still holds it. */
function knownBlob(ctx:Ctx,repo:Repo,value:string){
  const row=ctx.db.fileGitBlobs.id.find(`${repo.id}/${value}`);
  return row&&first(ctx.db.versions.blobId.filter(row.blobId))?row:undefined;
}
function gitShaOf(ctx:Ctx,repo:Repo,blobId:string){for(const row of ctx.db.fileGitBlobs.blobId.filter(blobId))if(row.repoId===repo.id)return row.sha;return "";}
/** A ref name Git accepts (git check-ref-format) under the namespaces Dar keeps. */
export function validRef(name:string){
  if(name.length>255||!/^refs\/(heads|tags|notes)\/./.test(name)||/[\x00-\x20\x7f~^:?*[\\]|\.\.|@\{|\/\/|\/$|\.$|\.lock(\/|$)|\/\./.test(name))return false;
  return name!=="refs/heads/HEAD";
}

function repoOf(ctx:Ctx,files:HistoryFiles,p:Record<string,unknown>,write:boolean){
  const {entry:root}=folderOf(ctx,files,p.folderId,write),repo=ctx.db.fileRepos.id.find(root.id);
  if(!repo)fail("notFound");return {root,repo};
}
function person(ctx:Ctx,userId:string){const account=ctx.db.accounts.id.find(userId);return {name:account?.displayName||account?.username||"Removed account",username:account?.username??"removed"};}
/** A changeset as the runtime needs it to write Git objects: its base, content and, once seen, its Git commit. */
function commitView(ctx:Ctx,repo:Repo,set:Changeset){
  const pin=pinOf(ctx,set.id);
  return {id:set.id,parentId:set.parentId,main:!set.branch,message:set.message,createdAt:set.createdAt,author:person(ctx,set.authorId),
    ...pin?{sha:pin.sha,raw:pin.raw}:{},
    changes:[...ctx.db.fileChanges.changesetId.filter(set.id)].map(change=>{
      const version=change.versionId?ctx.db.versions.id.find(change.versionId):undefined;
      if(change.versionId&&!version)fail("contentUnavailable");
      return {path:change.path,previousPath:change.previousPath,kind:change.kind,mode:change.mode,...version?{blobId:version.blobId,size:version.size,sha:gitShaOf(ctx,repo,version.blobId)}:{}};
    })};
}
/** A folder's path from its owner's root, by name. */
function pathOf(ctx:Ctx,entry:FileEntry){
  const names=[entry.name];let parentId=entry.parentId;
  for(let depth=0;parentId&&depth<128;depth++){const parent=ctx.db.fileEntries.id.find(parentId);if(!parent)break;names.unshift(parent.name);parentId=parent.parentId;}
  return names.join("/");
}
/**
 * The caller's tracked folders for the Git app: where each is, how many files differ from main
 * (-1 when the folder is too large to compare), its Agent branches and the bytes its history holds.
 */
function repositories(ctx:Ctx,userId:string){
  return [...ctx.db.fileRepos.ownerId.filter(userId)].flatMap(repo=>{
    const root=ctx.db.fileEntries.id.find(repo.id);if(!root||root.trashed)return [];
    let dirty=-1;try{dirty=status(ctx,repo,root).changes.length;}catch{/* larger than history allows */}
    const seen=new Set<string>();let size=0;
    for(const set of ctx.db.fileChangesets.repoId.filter(repo.id))for(const change of ctx.db.fileChanges.changesetId.filter(set.id)){
      if(!change.versionId||seen.has(change.versionId))continue;
      seen.add(change.versionId);size+=ctx.db.versions.id.find(change.versionId)?.size??0;
    }
    return [{id:repo.id,name:root.name,path:pathOf(ctx,root),parentId:root.parentId,revision:repo.revision,createdAt:repo.createdAt,modifiedAt:repo.modifiedAt,
      dirty,agentBranches:[...ctx.db.fileBranches.repoId.filter(repo.id)].length,size}];
  });
}
function refsOf(ctx:Ctx,repo:Repo){return [...ctx.db.fileGitRefs.repoId.filter(repo.id)].map(row=>({name:row.name,sha:row.target}));}

/** Operations for serviceGit; `uploadKey` ties a push's blobs to the pushing session. */
export function gitOperation(ctx:Ctx,files:HistoryFiles,userId:string,uploadKey:string,operation:string,p:Record<string,unknown>){
  switch(operation){
    case "repositories":keys(p,[]);return repositories(ctx,userId);
    case "changesetOf":{
      // The main commit a Git ID is, so the Git app can restore the folder to it.
      keys(p,["folderId","sha"]);const {repo}=repoOf(ctx,files,p,false),row=objectOf(ctx,repo,sha(p.sha)),set=row?.changesetId?ctx.db.fileChangesets.id.find(row.changesetId):undefined;
      return {revision:repo.revision,changesetId:set&&!set.branch?set.id:null};
    }
    case "resolve":{
      // A clone alias: the caller's own tracked folder at a path from their root.
      keys(p,["path"]);const parts=typeof p.path==="string"&&p.path.length<=4096?p.path.split("/"):[];
      if(!parts.length||parts.length>historyLimits.depth||parts.some(part=>!part))fail("notFound");
      let parentId="",entry:FileEntry|undefined;
      for(const part of parts){
        entry=[...ctx.db.fileEntries.listKey.filter(`${userId}/${parentId}/${part.toLowerCase()}`)].find(row=>!row.trashed&&row.kind==="folder");
        if(!entry)fail("notFound");parentId=entry.id;
      }
      if(!entry||!ctx.db.fileRepos.id.find(entry.id))fail("notFound");
      return {folderId:entry.id};
    }
    case "state":{
      // What a fetch needs: the refs, and main's commits Git has not seen yet, oldest first.
      keys(p,["folderId"]);const {repo}=repoOf(ctx,files,p,false),pending:Changeset[]=[];
      let cursor=repo.headId;
      while(cursor&&!pinOf(ctx,cursor)){const set=ctx.db.fileChangesets.id.find(cursor);if(!set)fail("contentUnavailable");pending.push(set);cursor=set.parentId;}
      return {revision:repo.revision,generation:repo.createdAt,main:pinOf(ctx,repo.headId)?.sha??null,base:cursor?pinOf(ctx,cursor)!.sha:null,
        pending:pending.reverse().map(set=>commitView(ctx,repo,set)),refs:refsOf(ctx,repo)};
    }
    case "export":{
      // Everything, to rebuild the runtime's Git repository from scratch.
      keys(p,["folderId"]);const {repo}=repoOf(ctx,files,p,false);
      const sets=[...ctx.db.fileChangesets.repoId.filter(repo.id)].filter(set=>set.branch===""||set.branch==="git");
      return {revision:repo.revision,generation:repo.createdAt,main:pinOf(ctx,repo.headId)?.sha??null,headId:repo.headId,
        commits:sets.map(set=>commitView(ctx,repo,set)),
        tags:[...ctx.db.fileGitObjects.repoId.filter(repo.id)].filter(row=>row.type==="tag").map(row=>({sha:row.sha,raw:row.raw})),refs:refsOf(ctx,repo)};
    }
    case "pin":{
      // Writes down the Git commits the runtime made for Dar's own commits, and the blob IDs it hashed.
      keys(p,["folderId","commits","blobs"]);const {repo}=repoOf(ctx,files,p,false);let conflict=false;
      for(const value of list<Record<string,unknown>>(p.commits,historyLimits.changesets)){
        const commit=value&&typeof value==="object"?value:fail("invalidInput"),set=ctx.db.fileChangesets.id.find(line(commit.id,64)),objectId=sha(commit.sha);
        const parents=list<string>(commit.parents,1).map(sha),raw=base64(commit.raw,gitLimits.objectBytes*2),author=line(commit.author,256);
        if(!set||set.repoId!==repo.id||set.branch)fail("invalidInput");
        const existing=pinOf(ctx,set.id);
        if(existing){if(existing.sha!==objectId)conflict=true;continue;}
        if(objectOf(ctx,repo,objectId)){conflict=true;continue;}
        ctx.db.fileGitObjects.insert({id:`${repo.id}/${objectId}`,repoId:repo.id,sha:objectId,type:"commit",changesetId:set.id,parents:parents.join(" "),author,raw,createdAt:now(ctx)});
      }
      for(const value of list<Record<string,unknown>>(p.blobs,historyLimits.changes*10)){
        const blob=value&&typeof value==="object"?value:fail("invalidInput"),objectId=sha(blob.sha),blobId=line(blob.blobId,64);
        if(typeof blob.size!=="number"||!Number.isSafeInteger(blob.size)||blob.size<0)fail("invalidInput");
        if(!ctx.db.fileGitBlobs.id.find(`${repo.id}/${objectId}`)&&first(ctx.db.versions.blobId.filter(blobId))?.size===blob.size)ctx.db.fileGitBlobs.insert({id:`${repo.id}/${objectId}`,repoId:repo.id,sha:objectId,blobId,size:blob.size});
      }
      return {conflict};
    }
    case "pushBegin":{
      // Content the folder already has is reused; the rest is reserved for upload, as Files uploads are.
      keys(p,["folderId","blobs"]);const {root,repo}=repoOf(ctx,files,p,true),expiresAt=now(ctx)+3600_000;let total=0;
      const rows=list<Record<string,unknown>>(p.blobs,gitLimits.changes).map(value=>{
        const objectId=sha(value?.sha),size=value?.size;
        if(typeof size!=="number"||!Number.isSafeInteger(size)||size<0||size>16*1024**3)fail("invalidInput");
        const known=knownBlob(ctx,repo,objectId);
        if(known&&known.size===size)return {sha:objectId,blobId:known.blobId};
        total+=size;return {sha:objectId,size};
      });
      const meter=ctx.db.usage.userId.find(root.ownerId)??ctx.db.usage.insert({userId:root.ownerId,used:0,reserved:0,quota:0});
      if(!files.fitsQuota(meter,total))fail("quotaExceeded");
      ctx.db.usage.userId.update({...meter,reserved:meter.reserved+total});
      return {blobs:rows.map(row=>{
        if("blobId" in row)return row;
        const uploadId=files.uuid();
        ctx.db.uploads.insert({id:uploadId,sessionId:uploadKey,ownerId:root.ownerId,parentId:"",fileId:"",name:"",size:row.size,mediaType:"application/octet-stream",expectedRevision:0,expiresAt,state:"uploading",digest:"",expiresKey:String(expiresAt).padStart(16,"0")});
        return {sha:row.sha,uploadId};
      })};
    }
    case "push":return push(ctx,files,userId,uploadKey,p);
  }
  fail("invalidInput");
}

function changesOf(value:unknown):PushChange[]{
  return list<Record<string,unknown>>(value,historyLimits.changes).map(row=>{
    if(!row||typeof row!=="object")fail("invalidInput");
    const kind=row.kind;if(kind!=="added"&&kind!=="modified"&&kind!=="deleted"&&kind!=="renamed")fail("invalidInput");
    const path=line(row.path,4096),previousPath=kind==="renamed"?line(row.previousPath,4096):"";
    const mode=kind==="deleted"?"":String(row.mode??""),previousMode=kind==="added"?"":String(row.previousMode??"");
    if(!path||!modes.has(mode)||!modes.has(previousMode))fail("invalidInput");
    const blob=kind==="deleted"?"":sha(row.blob),previousBlob=kind==="added"?"":sha(row.previousBlob);
    const mediaType=kind==="deleted"||row.mediaType===""?"":typeof row.mediaType==="string"&&row.mediaType.length<=150&&/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(row.mediaType)?row.mediaType.toLowerCase():fail("invalidInput");
    return {path,previousPath,kind,mode,previousMode,blob,previousBlob,mediaType};
  });
}

/**
 * Applies a push. Every ref is checked like Git's receive-pack: the old value must match, main
 * only fast-forwards and is never deleted. main's new commits join the folder's history and
 * change the live folder, unless a file they touch has uncommitted changes in Dar. Other commits
 * are kept for the branches and tags that reach them; anything no ref reaches any more is released.
 */
function push(ctx:Ctx,files:HistoryFiles,userId:string,uploadKey:string,p:Record<string,unknown>){
  keys(p,["folderId","updates","atomic","commits","tags","main","blobs"]);
  const {root,repo}=repoOf(ctx,files,p,true),at=now(ctx);
  const updates=list<Record<string,unknown>>(p.updates,1000).map(row=>({name:line(row?.name,255),old:sha(row?.old),new:sha(row?.new)}) as PushUpdate);
  if(new Set(updates.map(update=>update.name)).size!==updates.length||!updates.length)fail("invalidInput");
  const commits=new Map<string,PushCommit>(),tags=new Map<string,PushTag>();
  for(const row of list<Record<string,unknown>>(p.commits,gitLimits.commits)){
    const value={sha:sha(row?.sha),raw:base64(row.raw,gitLimits.objectBytes*2),parents:list<string>(row.parents,64).map(sha),author:line(row.author,256),
      message:typeof row.message==="string"?row.message.slice(0,historyLimits.messageBytes/2):fail("invalidInput"),time:typeof row.time==="number"&&Number.isFinite(row.time)?row.time:fail("invalidInput"),changes:changesOf(row.changes)};
    if(!objectOf(ctx,repo,value.sha))commits.set(value.sha,value);
  }
  for(const row of list<Record<string,unknown>>(p.tags,1000)){
    const value={sha:sha(row?.sha),raw:base64(row.raw,gitLimits.objectBytes*2),target:sha(row.target),author:line(row.author,256)};
    if(!objectOf(ctx,repo,value.sha))tags.set(value.sha,value);
  }
  if([...commits.values()].reduce((sum,commit)=>sum+commit.changes.length,0)>gitLimits.changes)fail("operationTooLarge");
  const typeOf=(value:string)=>objectOf(ctx,repo,value)?.type??(commits.has(value)?"commit":tags.has(value)?"tag":undefined);
  const parentsOf=(value:string):string[]=>{const row=objectOf(ctx,repo,value);return row?row.parents.split(" ").filter(Boolean):commits.get(value)?.parents??(tags.has(value)?[tags.get(value)!.target]:[]);};
  const peel=(value:string)=>{let current=value;for(let i=0;i<16&&typeOf(current)==="tag";i++)current=parentsOf(current)[0]??"";return current;};
  function isAncestor(ancestor:string,descendant:string){
    const seen=new Set<string>(),queue=[descendant];
    for(let i=0;i<queue.length&&seen.size<200_000;i++){const value=queue[i];if(value===ancestor)return true;for(const parent of parentsOf(value))if(!seen.has(parent)){seen.add(parent);queue.push(parent);}}
    return false;
  }
  for(const commit of commits.values())for(const parent of commit.parents)if(!typeOf(parent))fail("invalidInput");
  for(const tag of tags.values())if(!typeOf(tag.target))fail("invalidInput");

  // Ref checks, as receive-pack makes them.
  const mainPin=pinOf(ctx,repo.headId),currentMain=repo.headId?mainPin?.sha??"":zero,current=new Map(refsOf(ctx,repo).map(ref=>[ref.name,ref.sha]));
  const errors=new Map<string,string>();
  for(const update of updates){
    const reason=((): string|undefined=>{
      if(!validRef(update.name))return "Dar keeps branches, tags and notes with valid names";
      if(update.name===mainRef){
        if(update.new===zero)return "main cannot be deleted";
        if(update.old!==currentMain)return "fetch first";
        if(typeOf(update.new)!=="commit")return "main must point at a commit";
        if(currentMain!==zero&&!isAncestor(currentMain,update.new))return "non-fast-forward";
        return undefined;
      }
      if(update.name.startsWith("refs/heads/agent/"))return "agent/ branches belong to Agent";
      if(update.old!==(current.get(update.name)??zero))return "fetch first";
      if(update.new===zero)return undefined;
      const type=typeOf(update.new);
      if(type!=="commit"&&type!=="tag")return "missing object";
      if(update.name.startsWith("refs/heads/")&&type!=="commit")return "branches must point at commits";
      if(typeOf(peel(update.new))!=="commit")return "tags must point at commits";
      return undefined;
    })();
    if(reason)errors.set(update.name,reason);
  }
  const atomic=p.atomic===true,finish=()=>({results:updates.map(update=>({name:update.name,...errors.has(update.name)?{error:errors.get(update.name)}:{}}))});
  if(atomic&&errors.size){for(const update of updates)if(!errors.has(update.name))errors.set(update.name,"atomic push failed");return finish();}

  // Content: uploads finished by this push, or content the folder already holds.
  const blobs=new Map<string,{blobId:string;size:number}>();let reservedBytes=0;
  for(const row of list<Record<string,unknown>>(p.blobs,gitLimits.changes)){
    const objectId=sha(row?.sha),size=row.size;
    if(typeof size!=="number"||!Number.isSafeInteger(size)||size<0)fail("invalidInput");
    if(typeof row.uploadId==="string"){
      const upload=ctx.db.uploads.id.find(row.uploadId);
      if(!upload||upload.sessionId!==uploadKey||upload.state!=="uploading"||upload.ownerId!==root.ownerId||upload.size!==size||typeof row.digest!=="string"||!/^[a-f0-9]{64}$/.test(row.digest))fail("notFound");
      ctx.db.uploads.id.update({...upload,state:"complete",digest:row.digest});reservedBytes+=size;
      blobs.set(objectId,{blobId:upload.id,size});
      const existing=ctx.db.fileGitBlobs.id.find(`${repo.id}/${objectId}`);
      if(existing)ctx.db.fileGitBlobs.id.update({...existing,blobId:upload.id,size});else ctx.db.fileGitBlobs.insert({id:`${repo.id}/${objectId}`,repoId:repo.id,sha:objectId,blobId:upload.id,size});
    }else{
      const known=knownBlob(ctx,repo,objectId);
      if(!known||known.blobId!==row.blobId||known.size!==size)fail("conflict");
      blobs.set(objectId,{blobId:known.blobId,size});
    }
  }
  const meter=ctx.db.usage.userId.find(root.ownerId);
  if(meter&&reservedBytes)ctx.db.usage.userId.update({...meter,reserved:Math.max(0,meter.reserved-reservedBytes)});
  const newVersion=(fileId:string,blob:string,mediaType:string)=>{
    const content=blobs.get(blob)??(()=>{const known=knownBlob(ctx,repo,blob);return known?{blobId:known.blobId,size:known.size}:fail("contentUnavailable");})();
    charge(ctx,files,root.ownerId,content.size);
    // An unknown type is the one the same content already has here.
    const type=mediaType||first(ctx.db.versions.blobId.filter(content.blobId))?.mediaType||"application/octet-stream";
    return ctx.db.versions.insert({id:files.uuid(),fileId,blobId:content.blobId,size:content.size,mediaType:type,createdAt:at}).id;
  };

  // main: replay its new commits over the head tree, then check the live folder before changing anything.
  const mainUpdate=updates.find(update=>update.name===mainRef&&!errors.has(update.name));
  const chain=mainUpdate?list<Record<string,unknown>>((p.main as Record<string,unknown>|undefined)?.chain,gitLimits.commits).map(row=>({sha:sha(row?.sha),changes:row.changes===undefined?undefined:changesOf(row.changes)})):[];
  let plan:Plan|undefined;
  if(mainUpdate){
    if(!chain.length||chain.at(-1)!.sha!==mainUpdate.new)fail("invalidInput");
    for(const item of chain){if(!item.changes){const commit=commits.get(item.sha);if(!commit)fail("invalidInput");item.changes=commit.changes;}}
    const planned=chain.some(item=>{const known=objectOf(ctx,repo,item.sha);return known&&!ctx.db.fileChangesets.id.find(known.changesetId)?.branch;})?"main's earlier commits cannot be rewritten":planMain(ctx,files,repo,root,chain as ChainItem[]);
    if(typeof planned!=="string")plan=planned;
    else{errors.set(mainRef,planned);if(atomic){for(const update of updates)if(!errors.has(update.name))errors.set(update.name,"atomic push failed");return finish();}}
  }
  const onMain=new Set(plan?chain.map(item=>item.sha):[]);
  const changesetOf=new Map<string,string>();
  for(const commit of commits.values())changesetOf.set(commit.sha,fresh(ctx,files));
  const setFor=(value:string)=>changesetOf.get(value)??objectOf(ctx,repo,value)?.changesetId??fail("invalidInput");
  const total=[...ctx.db.fileChangesets.repoId.filter(repo.id)].length;
  if(total+commits.size>historyLimits.changesets)fail("operationTooLarge");

  // Commits off main: their changes are against their first parent; content belongs to no file.
  const shared=new Map<string,string>();
  const sideVersion=(blob:string,mediaType:string)=>{
    let versionId=shared.get(blob);
    if(!versionId){
      const content=blobs.get(blob)??knownBlob(ctx,repo,blob);
      for(const version of content?ctx.db.versions.blobId.filter(content.blobId):[])if(version.fileId===pseudoFile(repo.id)){versionId=version.id;break;}
      versionId??=newVersion(pseudoFile(repo.id),blob,mediaType);shared.set(blob,versionId);
    }
    return versionId;
  };
  for(const commit of commits.values()){
    if(onMain.has(commit.sha))continue;
    const changesetId=changesetOf.get(commit.sha)!;
    if(commit.changes.length>historyLimits.changes)fail("operationTooLarge");
    ctx.db.fileChangesets.insert({id:changesetId,repoId:repo.id,branch:"git",parentId:commit.parents.length?setFor(commit.parents[0]):"",sequence:0,kind:commit.parents.length>1?"merge":"commit",message:commit.message,authorId:userId,fileCount:commit.changes.length,createdAt:commit.time});
    commit.changes.forEach((change,index)=>ctx.db.fileChanges.insert({id:`${changesetId}/${index}`,changesetId,path:change.path,previousPath:change.previousPath,kind:change.kind,fileId:"",
      versionId:change.kind==="deleted"?"":sideVersion(change.blob,change.mediaType),previousVersionId:"",expectRevision:0,mode:change.mode,previousMode:change.previousMode}));
  }
  if(plan)applyMain(ctx,files,repo,root,userId,plan,chain as ChainItem[],commits,changesetOf,newVersion);
  for(const commit of commits.values())ctx.db.fileGitObjects.insert({id:`${repo.id}/${commit.sha}`,repoId:repo.id,sha:commit.sha,type:"commit",changesetId:changesetOf.get(commit.sha)!,parents:commit.parents.join(" "),author:commit.author,raw:commit.raw,createdAt:at});
  for(const tag of tags.values())ctx.db.fileGitObjects.insert({id:`${repo.id}/${tag.sha}`,repoId:repo.id,sha:tag.sha,type:"tag",changesetId:"",parents:tag.target,author:tag.author,raw:tag.raw,createdAt:at});
  for(const update of updates){
    if(errors.has(update.name)||update.name===mainRef)continue;
    const rowId=`${repo.id}/${update.name}`;
    if(update.new===zero)ctx.db.fileGitRefs.id.delete(rowId);
    else if(ctx.db.fileGitRefs.id.find(rowId))ctx.db.fileGitRefs.id.update({id:rowId,repoId:repo.id,name:update.name,target:update.new,updatedAt:at});
    else ctx.db.fileGitRefs.insert({id:rowId,repoId:repo.id,name:update.name,target:update.new,updatedAt:at});
  }
  collect(ctx,repo);
  const latest=ctx.db.fileRepos.id.find(repo.id)!;
  ctx.db.fileRepos.id.update({...latest,revision:latest.revision+1,modifiedAt:at});
  return finish();
}

interface Slot {fileId:string;versionId:string;blob:string;mode:string}
interface Step {fileId:string;previous?:Slot}
interface ChainItem {sha:string;changes:PushChange[]}
interface Plan {head:Map<string,{fileId:string;versionId:string;mode:string}>;steps:Step[][]}
const darName=(files:HistoryFiles,part:string)=>{try{return files.nameOf(part)===part;}catch{return false;}};

/**
 * Replays main's new commits over the head tree without writing: assigns file IDs (a rename keeps its
 * file), checks names and limits, and checks that every path the push changes is committed in Dar.
 * Returns why main is refused, or the plan.
 */
function planMain(ctx:Ctx,files:HistoryFiles,repo:Repo,root:FileEntry,chain:ChainItem[]):Plan|string{
  const head=headTree(ctx,repo.id),tree=new Map<string,Slot>([...head].map(([path,item])=>[path,{...item,blob:""}])),steps:Step[][]=[];
  if(repo.sequence+chain.length>historyLimits.changesets)return "the folder's history is full";
  for(const commit of chain){
    if(commit.changes.length>historyLimits.changes)return "a commit changes too many files for Dar";
    const result:Step[]=[];
    // Removals first: a path can be renamed away and another file added there in one commit.
    for(const change of commit.changes){
      if(change.kind!=="deleted"&&change.kind!=="renamed"){result.push({fileId:""});continue;}
      const from=change.kind==="renamed"?change.previousPath:change.path,slot=tree.get(from);
      if(!slot)fail("invalidInput");tree.delete(from);result.push({fileId:slot.fileId,previous:slot});
    }
    for(const [index,change] of commit.changes.entries()){
      if(change.kind==="deleted")continue;
      const parts=change.path.split("/");
      if(parts.some(part=>!darName(files,part)))return `“${change.path}” is not a file name Dar can keep`;
      if(parts.length>historyLimits.depth)return "folders are nested too deeply for Dar";
      const same=change.blob===change.previousBlob;
      if(change.kind==="renamed"){const step=result[index];tree.set(change.path,{fileId:step.fileId,versionId:same?step.previous!.versionId:"",blob:same?step.previous!.blob:change.blob,mode:change.mode});continue;}
      if(change.kind==="modified"){
        const slot=tree.get(change.path);if(!slot)fail("invalidInput");
        result[index]={fileId:slot.fileId,previous:slot};tree.set(change.path,{fileId:slot.fileId,versionId:same?slot.versionId:"",blob:same?slot.blob:change.blob,mode:change.mode});continue;
      }
      if(tree.has(change.path))fail("invalidInput");
      const fileId=files.uuid();result[index]={fileId};tree.set(change.path,{fileId,versionId:"",blob:change.blob,mode:change.mode});
    }
    steps.push(result);
  }
  if(tree.size>historyLimits.files)return "the folder would have more files than Dar tracks";
  // Dar names are unique without regard to case, among files and folders alike.
  const names=new Set<string>();
  for(const path of tree.keys()){
    const parts=path.toLowerCase().split("/");
    for(let i=1;i<=parts.length;i++){
      const prefix=parts.slice(0,i).join("/"),folder=i<parts.length,clash=folder?names.has(prefix+"\0file"):names.has(prefix+"\0folder");
      if(clash||!folder&&names.has(prefix+"\0file"))return `“${path}” differs from another name only by case`;
      names.add(prefix+(folder?"\0folder":"\0file"));
    }
  }
  // Every path the push changes must be as main last committed it.
  const live=scan(ctx,root),pathOf=new Map([...tree].map(([path,slot])=>[slot.fileId,path]));
  for(const [path,old] of head){
    const next=tree.get(path);
    if(next&&next.fileId===old.fileId&&next.versionId===old.versionId&&pathOf.get(old.fileId)===path)continue;
    const entry=live.get(path),version=ctx.db.versions.id.find(old.versionId);
    if(!entry||entry.id!==old.fileId||entry.blobId!==version?.blobId)return `“${path}” has uncommitted changes in Dar; commit them there, then pull`;
  }
  for(const [path,slot] of tree){
    if(head.get(path)?.fileId===slot.fileId)continue;
    if(live.has(path)&&!head.has(path))return `“${path}” is in the Dar folder but not committed; commit it there, then pull`;
    // New paths must not reach into a folder that keeps its own history.
    let parentId=root.id;
    for(const part of path.split("/").slice(0,-1)){
      const entry=files.findKey(files.key(root.ownerId,parentId,part));if(!entry)break;
      if(entry.kind!=="folder"||ctx.db.fileRepos.id.find(entry.id))return `“${path}” would go inside a file or a folder with its own history`;
      parentId=entry.id;
    }
  }
  return {head,steps};
}

/** Records main's new commits (or ones Git branches already had) and brings the live folder to the last one. */
function applyMain(ctx:Ctx,files:HistoryFiles,repo:Repo,root:FileEntry,userId:string,plan:Plan,chain:ChainItem[],commits:Map<string,PushCommit>,changesetOf:Map<string,string>,newVersion:(fileId:string,blob:string,mediaType:string)=>string){
  const at=now(ctx),{head,steps}=plan,tree=new Map<string,Slot>([...head].map(([path,item])=>[path,{...item,blob:""}]));
  let headId=repo.headId,sequence=repo.sequence;
  chain.forEach((item,commitIndex)=>{
    const commit=commits.get(item.sha),changesetId=commit?changesetOf.get(item.sha)!:objectOf(ctx,repo,item.sha)!.changesetId;
    const existing=commit?undefined:ctx.db.fileChangesets.id.find(changesetId)!;
    // A commit Git branches already had joins main: its changes are now against main's previous commit.
    if(existing)for(const row of [...ctx.db.fileChanges.changesetId.filter(changesetId)])ctx.db.fileChanges.id.delete(row.id);
    const before=item.changes.map(change=>change.kind==="added"?undefined:tree.get(change.kind==="renamed"?change.previousPath:change.path));
    for(const change of item.changes)if(change.kind==="deleted"||change.kind==="renamed")tree.delete(change.kind==="renamed"?change.previousPath:change.path);
    item.changes.forEach((change,index)=>{
      const step=steps[commitIndex][index],previous=before[index];
      let versionId="";
      if(change.kind!=="deleted"){
        versionId=previous&&change.blob===change.previousBlob?previous.versionId:newVersion(step.fileId,change.blob,change.mediaType);
        tree.set(change.path,{fileId:step.fileId,versionId,blob:change.blob,mode:change.mode});
      }
      ctx.db.fileChanges.insert({id:`${changesetId}/${index}`,changesetId,path:change.path,previousPath:change.previousPath,kind:change.kind,fileId:step.fileId,versionId,previousVersionId:previous?.versionId??"",expectRevision:0,mode:change.mode,previousMode:change.previousMode});
    });
    sequence++;
    const parents=commit?.parents.length??objectOf(ctx,repo,item.sha)!.parents.split(" ").filter(Boolean).length;
    const row={id:changesetId,repoId:repo.id,branch:"",parentId:headId,sequence,kind:parents>1?"merge":"commit",message:commit?.message??existing!.message,authorId:existing?.authorId??userId,fileCount:item.changes.length,createdAt:commit?.time??existing!.createdAt};
    if(existing)ctx.db.fileChangesets.id.update(row);else ctx.db.fileChangesets.insert(row);
    headId=changesetId;
  });
  // The live folder: trash what main no longer has, move renamed files, then set content and add files.
  const live=scan(ctx,root),pathOf=new Map([...tree].map(([path,slot])=>[slot.fileId,path]));
  for(const [path,old] of head)if(!pathOf.has(old.fileId))trash(ctx,files,live.get(path)!);
  const moved=[...head].filter(([path,old])=>pathOf.has(old.fileId)&&pathOf.get(old.fileId)!==path);
  // Moves free their old names first, so files can swap places.
  for(const [,old] of moved){const entry=ctx.db.fileEntries.id.find(old.fileId)!;ctx.db.fileEntries.id.update({...entry,listKey:`${entry.ownerId}/moving/${entry.id}`});}
  for(const [,old] of moved)moveFile(ctx,files,root,ctx.db.fileEntries.id.find(old.fileId)!,pathOf.get(old.fileId)!);
  // Only content the push changed: other files keep any uncommitted edits made in Dar.
  const headVersion=new Map([...head.values()].map(item=>[item.fileId,item.versionId]));
  for(const [path,slot] of tree){
    if(headVersion.get(slot.fileId)===slot.versionId)continue;
    const version=ctx.db.versions.id.find(slot.versionId)!,entry=ctx.db.fileEntries.id.find(slot.fileId);
    if(entry){if(entry.blobId!==version.blobId)setContent(ctx,files,entry,version,true);}
    else createFile(ctx,files,root,path,version,false);
  }
  for(const row of [...ctx.db.fileTrees.repoId.filter(repo.id)])if(!tree.has(row.path))ctx.db.fileTrees.id.delete(row.id);
  for(const [path,slot] of tree){
    const row={id:`${repo.id}/${path}`,repoId:repo.id,path,fileId:slot.fileId,versionId:slot.versionId,mode:slot.mode};
    if(ctx.db.fileTrees.id.find(row.id))ctx.db.fileTrees.id.update(row);else ctx.db.fileTrees.insert(row);
  }
  const latest=ctx.db.fileRepos.id.find(repo.id)!;
  ctx.db.fileRepos.id.update({...latest,headId,sequence,modifiedAt:at});
}

/** Drops commits and tags no ref reaches any more, and the content only they kept. main's commits always stay. */
function collect(ctx:Ctx,repo:Repo){
  const objects=new Map<string,GitObject>([...ctx.db.fileGitObjects.repoId.filter(repo.id)].map(row=>[row.sha,row]));
  const queue=[...ctx.db.fileGitRefs.repoId.filter(repo.id)].map(row=>row.target);
  for(const row of objects.values())if(row.type==="commit"&&!ctx.db.fileChangesets.id.find(row.changesetId)?.branch)queue.push(row.sha);
  const seen=new Set(queue);
  for(let i=0;i<queue.length;i++)for(const parent of objects.get(queue[i])?.parents.split(" ").filter(Boolean)??[])if(!seen.has(parent)){seen.add(parent);queue.push(parent);}
  for(const row of objects.values()){
    if(seen.has(row.sha))continue;
    if(row.type==="commit"){
      const set=ctx.db.fileChangesets.id.find(row.changesetId);
      if(set?.branch!=="git")continue;
      for(const change of [...ctx.db.fileChanges.changesetId.filter(set.id)])ctx.db.fileChanges.id.delete(change.id);
      ctx.db.fileChangesets.id.delete(set.id);
    }
    ctx.db.fileGitObjects.id.delete(row.id);
  }
  let bytes=0;
  for(const version of [...ctx.db.versions.fileId.filter(pseudoFile(repo.id))])if(!referenced(ctx,version.id)){bytes+=version.size;ctx.db.versions.id.delete(version.id);}
  release(ctx,repo.ownerId,bytes);
  for(const row of [...ctx.db.fileGitBlobs.repoId.filter(repo.id)])if(!first(ctx.db.versions.blobId.filter(row.blobId)))ctx.db.fileGitBlobs.id.delete(row.id);
}

