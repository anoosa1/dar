import type {IncomingMessage,ServerResponse} from "node:http";
import {once} from "node:events";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {writeJson} from "../json-response.ts";
import {git,gitProcess,lines,zero,emptyTree,pushFromDar,type GitBlobs,type GitRepositories} from "../files/git.ts";
import {historyReadOperations} from "../../shared/read-operations.ts";
import {gitPath,type HistoryOverview} from "../../shared/history.ts";
import {gitAlias,validRefName,type GitActivity,type GitAgentBranch,type GitBranch,type GitBranches,type GitChangeKind,type GitCommit,type GitCommitDetail,type GitCommitPage,type GitCompare,type GitEvent,type GitFileChange,type GitRef,type GitRepository,type GitSource,type GitTag,type GitTree,type GitTreeEntry} from "../../shared/git.ts";

/**
 * The Git app's reads (docs/git.md) under /api/v1/git. Repositories are tracked folders; each read
 * brings the folder's Git cache up to date under its lock and asks Git. Deleting a branch or tag goes
 * through the same database push as `git push --delete`, so it is checked and released the same way.
 */
interface RepoRow {id:string;name:string;path:string;parentId:string;revision:number;createdAt:number;modifiedAt:number;dirty:number;agentBranches:number;size:number}
type Sync=Awaited<ReturnType<GitRepositories["syncLocked"]>>;

const unit="\x1f",record="\x1e";
const logFormat=`--format=${record}%H${unit}%P${unit}%an${unit}%ae${unit}%at${unit}%D${unit}%s`;
const limits={page:50,maxPage:200,patchFile:256*1024,patchTotal:2*1024*1024,patchLines:5000,readme:512*1024,blob:2*1024*1024,lastCommits:3000,compare:250,events:30};

function sourceOf(email:string,subject:string):GitSource{
  // Dar writes its own commits as <username>@dar.invalid; an Agent branch merge keeps its "Merge agent/…" message.
  if(!email.endsWith("@dar.invalid"))return "git";
  return /^Merge agent\//.test(subject)?"agent":"dar";
}
function refsOf(decoration:string):GitRef[]{
  return decoration.split(", ").filter(Boolean).flatMap((item):GitRef[]=>{
    const name=item.replace(/^HEAD -> /,"");
    if(name==="HEAD")return [];
    return name.startsWith("tag: ")?[{name:name.slice(5),kind:"tag" as const}]:[{name,kind:"branch" as const}];
  });
}
/** Parses `git log` with logFormat and, optionally, --shortstat. */
function parseLog(output:Buffer):GitCommit[]{
  return output.toString("utf8").split(record).filter(Boolean).map(chunk=>{
    const [head,...rest]=chunk.split("\n"),[sha,parents,author,email,time,decoration,subject]=head.split(unit),stat=rest.join("\n");
    return {sha,parents:parents.split(" ").filter(Boolean),author,time:Number(time)*1000,subject,source:sourceOf(email,subject),refs:refsOf(decoration),
      added:Number(/(\d+) insertion/.exec(stat)?.[1]??0),removed:Number(/(\d+) deletion/.exec(stat)?.[1]??0)};
  });
}
/** The README's first paragraph, without Markdown headings, badges or emphasis. */
function describe(text:string){
  const paragraph:string[]=[];
  for(const raw of text.replace(/\r/g,"").split("\n")){
    // Badges and images are dropped first, so a line that is only badges counts as empty.
    const line=raw.replace(/\[!\[[^\]]*\]\([^)]*\)\]\([^)]*\)/g,"").replace(/!\[[^\]]*\]\([^)]*\)/g,"").trim();
    if(!line||/^(#|<|=+$|-+$|```)/.test(line)){if(paragraph.length)break;continue;}
    paragraph.push(line);
  }
  const value=paragraph.join(" ").replace(/!\[[^\]]*\]\([^)]*\)/g,"").replace(/\[([^\]]*)\]\([^)]*\)/g,"$1").replace(/[*_`]+/g,"").trim();
  return value.length>300?`${value.slice(0,299)}…`:value;
}
const readmeName=(name:string)=>/^readme(\.(md|markdown|txt))?$/i.test(name);
/** A path inside a tree: no empty, dot or dot-dot parts. */
function treePath(value:string|null){
  if(!value)return "";
  const parts=value.split("/");
  if(value.length>4096||parts.some(part=>!part||part==="."||part===".."||part.includes("\0")))throw new AppError("invalidInput");
  return value;
}

export function gitApi(db:Database,repositories:GitRepositories,blobs:GitBlobs,origin:string){
  const repoRows=(sessionId:string)=>repositories.call<RepoRow[]>(sessionId,"repositories",{});
  const overview=(sessionId:string,folderId:string)=>db.operation<HistoryOverview>("service_history","overview",historyReadOperations,[sessionId,"overview",JSON.stringify({folderId})]);
  const run=(dir:string,args:string[],input?:string)=>git(dir,["-c","core.quotepath=off","--git-dir",dir,...args],input===undefined?{}:{input});
  /** Brings the cache up to date and runs work on it under the folder's lock. */
  const withRepo=<T>(sessionId:string,folderId:string,work:(dir:string,sync:Sync["state"])=>Promise<T>)=>
    repositories.locked(folderId,async()=>{const {dir,state}=await repositories.syncLocked(sessionId,folderId);return work(dir,state);});

  /** A branch or tag the repository has, as a full ref; anything else is not found. */
  async function refOf(dir:string,name:string|null,kinds:("heads"|"tags")[]=["heads","tags"]){
    if(!name||!validRefName(name))throw new AppError("notFound",404);
    for(const kind of kinds){const {code}=await git(dir,["--git-dir",dir,"show-ref","--verify","--quiet",`refs/${kind}/${name}`],{allowFailure:true});if(code===0)return `refs/${kind}/${name}`;}
    throw new AppError("notFound",404);
  }
  /** A full commit ID the repository has. */
  async function commitOf(dir:string,value:string){
    if(!/^[0-9a-f]{40}$/.test(value))throw new AppError("notFound",404);
    const {stdout,code}=await git(dir,["--git-dir",dir,"cat-file","-t",value],{allowFailure:true});
    if(code!==0||stdout.toString().trim()!=="commit")throw new AppError("notFound",404);
    return value;
  }
  async function summary(sessionId:string,username:string,row:RepoRow):Promise<GitRepository>{
    const base={id:row.id,name:row.name,path:row.path,parentId:row.parentId,createdAt:row.createdAt,revision:row.revision,dirty:row.dirty,agentBranches:row.agentBranches,size:row.size,
      clone:{alias:origin+gitAlias(username,row.path),id:origin+gitPath(row.id)}};
    try{
      return await withRepo(sessionId,row.id,async(dir,state)=>{
        const refs=lines((await run(dir,["for-each-ref","--format=%(refname)","refs/heads","refs/tags"])).stdout);
        const commits=state.main?Number((await run(dir,["rev-list","--count","refs/heads/main"])).stdout.toString().trim()):0;
        const since=Date.now()/1000-84*86400,weeks=Array<number>(12).fill(0);
        let updatedAt=row.createdAt;
        if(refs.length){
          for(const value of lines((await run(dir,["log","--all",`--since=${Math.floor(since)}`,"--format=%ct"])).stdout)){const index=Math.floor((Number(value)-since)/(7*86400));if(index>=0&&index<12)weeks[index]++;}
          const newest=Number((await run(dir,["log","--all","-1","--format=%ct"])).stdout.toString().trim());if(newest)updatedAt=Math.max(updatedAt,newest*1000);
        }
        let description="";
        if(state.main){
          const readme=lines((await run(dir,["ls-tree","-l","refs/heads/main"])).stdout).map(line=>{const [meta,name]=line.split("\t");const [,type,sha,size]=meta.split(/\s+/);return {name,type,sha,size:Number(size)};}).find(item=>item.type==="blob"&&readmeName(item.name)&&item.size<=limits.readme);
          if(readme)description=describe((await run(dir,["cat-file","blob",readme.sha])).stdout.toString("utf8"));
        }
        return {...base,description,commits,branches:refs.filter(ref=>ref.startsWith("refs/heads/")).length,tags:refs.filter(ref=>ref.startsWith("refs/tags/")).length,updatedAt,weeks};
      });
    }catch(cause){
      return {...base,description:"",commits:0,branches:0,tags:0,updatedAt:row.modifiedAt,weeks:Array<number>(12).fill(0),error:cause instanceof AppError?cause.message:"Git could not read this repository."};
    }
  }
  async function repositoriesOf(sessionId:string,username:string){
    const rows=await repoRows(sessionId),result:GitRepository[]=[];
    // A few at a time: each one is a database read and a handful of Git processes.
    for(let i=0;i<rows.length;i+=4)result.push(...await Promise.all(rows.slice(i,i+4).map(row=>summary(sessionId,username,row))));
    return result.sort((a,b)=>b.updatedAt-a.updatedAt);
  }
  async function rowOf(sessionId:string,folderId:string){
    const row=(await repoRows(sessionId)).find(item=>item.id===folderId);if(!row)throw new AppError("notFound",404);return row;
  }

  async function activity(sessionId:string):Promise<GitActivity>{
    const rows=await repoRows(sessionId),since=Math.floor(Date.now()/1000-366*86400),times:number[]=[],events:GitEvent[]=[];
    for(const row of rows){
      try{
        await withRepo(sessionId,row.id,async dir=>{
          if(!lines((await run(dir,["for-each-ref","--count=1","--format=%(refname)","refs/heads","refs/tags"])).stdout).length)return;
          for(const value of lines((await run(dir,["log","--all",`--since=${since}`,"--max-count=20000","--format=%at"])).stdout))times.push(Number(value));
          for(const commit of parseLog((await run(dir,["log","--all","--date-order",`--max-count=${limits.events}`,"--decorate=short","--decorate-refs=refs/heads/","--decorate-refs=refs/tags/",logFormat])).stdout))
            events.push({repoId:row.id,repoName:row.name,sha:commit.sha,subject:commit.subject,author:commit.author,time:commit.time,source:commit.source,refs:commit.refs});
        });
      }catch{/* a repository Git cannot read adds nothing */}
    }
    return {times,events:events.sort((a,b)=>b.time-a.time).slice(0,limits.events)};
  }

  async function tree(dir:string,state:Sync["state"],ref:string|null,pathValue:string|null):Promise<GitTree>{
    if(!state.main&&!state.refs.length)return {ref:"main",sha:"",path:"",entries:[]};
    const full=ref?/^[0-9a-f]{40}$/.test(ref)?await commitOf(dir,ref):await refOf(dir,ref):"refs/heads/main",path=treePath(pathValue);
    const sha=(await run(dir,["rev-parse","--verify","--quiet",`${full}^{commit}`])).stdout.toString().trim();
    const listed=await git(dir,["--git-dir",dir,"ls-tree","-z","-l",sha,"--",...path?[`${path}/`]:[]],{allowFailure:true});
    if(listed.code!==0)throw new AppError("notFound",404);
    const entries:GitTreeEntry[]=listed.stdout.toString("utf8").split("\0").filter(Boolean).map(item=>{
      const tab=item.indexOf("\t"),[mode,type,,size]=item.slice(0,tab).split(/\s+/),full=item.slice(tab+1),name=full.split("/").at(-1)!;
      return {name,path:full,type:type==="tree"?"tree" as const:type==="commit"?"submodule" as const:mode==="120000"?"link" as const:"blob" as const,size:Number(size)||0};
    });
    if(path&&!entries.length){
      const kind=await git(dir,["--git-dir",dir,"cat-file","-t",`${sha}:${path}`],{allowFailure:true});
      if(kind.stdout.toString().trim()!=="tree")throw new AppError("notFound",404);
    }
    entries.sort((a,b)=>(a.type==="tree"?0:1)-(b.type==="tree"?0:1)||a.name.localeCompare(b.name));
    // Each entry's newest commit: walk history once until every entry has one.
    const byName=new Map(entries.map(entry=>[entry.name,entry])),prefix=path?`${path}/`:"";
    if(entries.length){
      const {stdout}=await run(dir,["log",`--max-count=${limits.lastCommits}`,`--format=${record}%H${unit}%at${unit}%s`,"--name-only","--no-renames",sha,"--",path||"."]);
      let left=entries.length;
      for(const chunk of stdout.toString("utf8").split(record)){
        if(!left)break;
        const [head,...names]=chunk.split("\n");if(!head)continue;
        const [commit,time,subject]=head.split(unit);
        for(const name of names){
          if(!name.startsWith(prefix))continue;
          const entry=byName.get(name.slice(prefix.length).split("/")[0]);
          if(entry&&!entry.last){entry.last={sha:commit,subject,time:Number(time)*1000};left--;}
        }
      }
    }
    const readme=entries.find(entry=>entry.type==="blob"&&readmeName(entry.name)&&entry.size<=limits.readme);
    return {ref:full.replace(/^refs\/(heads|tags)\//,""),sha,path,entries,
      ...readme?{readme:{path:readme.path,text:(await run(dir,["cat-file","blob",`${sha}:${readme.path}`])).stdout.toString("utf8")}}:{}};
  }

  /** Changed files between two trees, with line counts and, while they fit, unified hunks. */
  async function changes(dir:string,from:string,to:string,patches:boolean):Promise<GitFileChange[]>{
    const numstat=(await run(dir,["diff-tree","-r","-M","--numstat","-z",from,to])).stdout.toString("utf8").split("\0");
    const status=(await run(dir,["diff-tree","-r","-M","--name-status","-z",from,to])).stdout.toString("utf8").split("\0");
    const files:GitFileChange[]=[];
    for(let i=0;i<status.length-1;){
      const code=status[i++];
      const kind:GitChangeKind=code.startsWith("R")?"renamed":code==="A"?"added":code==="D"?"deleted":"modified";
      const previousPath=kind==="renamed"?status[i++]:undefined,path=status[i++];
      files.push({path,...previousPath?{previousPath}:{},kind,added:0,removed:0,binary:false});
    }
    for(let i=0,n=0;i<numstat.length-1&&n<files.length;n++){
      const [added,removed,path]=numstat[i++].split("\t");
      if(path==="")i+=2;
      files[n].binary=added==="-";files[n].added=Number(added)||0;files[n].removed=Number(removed)||0;
    }
    if(!patches||!files.length)return files;
    const output=(await run(dir,["diff-tree","-r","-M","-p","--no-color","--no-ext-diff",from,to])).stdout;
    if(output.length>limits.patchTotal*4){for(const file of files)if(!file.binary)file.truncated=true;return files;}
    const chunks=output.toString("utf8").split(/^diff --git /m).slice(1);let total=0;
    chunks.forEach((chunk,index)=>{
      const file=files[index];if(!file||file.binary)return;
      const start=chunk.indexOf("\n@@");if(start<0)return;
      const patch=chunk.slice(start+1).replace(/\n$/,"");
      if(patch.length>limits.patchFile||total+patch.length>limits.patchTotal||patch.split("\n").length>limits.patchLines){file.truncated=true;return;}
      total+=patch.length;file.patch=patch;
    });
    return files;
  }
  async function commitDetail(dir:string,value:string):Promise<GitCommitDetail>{
    const sha=await commitOf(dir,value);
    const {stdout}=await run(dir,["log","-1","--decorate=short","--decorate-refs=refs/heads/","--decorate-refs=refs/tags/",`--format=%H${unit}%P${unit}%an${unit}%ae${unit}%at${unit}%D${unit}%s${unit}%cn${unit}%ct${unit}%b`,sha]);
    const [,parents,author,email,time,decoration,subject,committer,committedAt,...body]=stdout.toString("utf8").split(unit);
    const parentList=parents.split(" ").filter(Boolean),files=await changes(dir,parentList[0]??emptyTree,sha,true);
    return {sha,parents:parentList,author,time:Number(time)*1000,subject,source:sourceOf(email,subject),refs:refsOf(decoration),
      added:files.reduce((sum,file)=>sum+file.added,0),removed:files.reduce((sum,file)=>sum+file.removed,0),
      body:body.join(unit).replace(/\n+$/,""),committer,committedAt:Number(committedAt)*1000,files};
  }
  async function commits(dir:string,state:Sync["state"],params:URLSearchParams):Promise<GitCommitPage>{
    if(!state.main&&!state.refs.length)return {commits:[],total:0,next:null,authors:[]};
    const ref=params.get("ref"),author=params.get("author"),source=params.get("source"),skip=Number(params.get("skip")??0),limit=Math.min(limits.maxPage,Number(params.get("limit")??limits.page));
    if(!Number.isSafeInteger(skip)||skip<0||!Number.isSafeInteger(limit)||limit<1)throw new AppError("invalidInput");
    if(author!==null&&(author.length>200||/[\0\n]/.test(author)))throw new AppError("invalidInput");
    if(source!==null&&!["git","dar","agent"].includes(source))throw new AppError("invalidInput");
    const revs=!ref||ref==="all"?["--all"]:[await refOf(dir,ref)];
    const filters=author?["--fixed-strings",`--author=${author}`]:[];
    const args=["log","--date-order","--decorate=short","--decorate-refs=refs/heads/","--decorate-refs=refs/tags/","--shortstat","--diff-merges=first-parent",logFormat,...filters];
    let page:GitCommit[],total:number;
    if(source){
      // Where a commit came from is read from its author (and an Agent merge's message), which Git
      // cannot exclude by, so the IDs are filtered here over the whole history and only the page is read.
      const ids=async(extra:string[])=>new Set(lines((await run(dir,["rev-list",...extra,...revs])).stdout));
      const ordered=lines((await run(dir,["rev-list","--date-order",...filters,...revs])).stdout);
      const dar=await ids(["--author=@dar\\.invalid>"]),agent=await ids(["--author=@dar\\.invalid>","--grep=^Merge agent/"]);
      const matching=ordered.filter(sha=>source==="git"?!dar.has(sha):source==="agent"?agent.has(sha):dar.has(sha)&&!agent.has(sha));
      total=matching.length;const wanted=matching.slice(skip,skip+limit);
      page=wanted.length?parseLog((await run(dir,[...args,"--no-walk=unsorted","--stdin"],wanted.join("\n")+"\n")).stdout):[];
    }else{
      page=parseLog((await run(dir,[...args,`--skip=${skip}`,`--max-count=${limit}`,...revs])).stdout);
      total=Number((await run(dir,["rev-list","--count",...filters,...revs])).stdout.toString().trim());
    }
    const authors=skip?[]:[...new Set(lines((await run(dir,["log","--all","--max-count=5000","--format=%an"])).stdout))].sort((a,b)=>a.localeCompare(b)).slice(0,100);
    return {commits:page,total,next:skip+page.length<total?skip+page.length:null,authors};
  }
  async function aheadBehind(dir:string,base:string,head:string){
    const [behind,ahead]=(await run(dir,["rev-list","--left-right","--count",`${base}...${head}`])).stdout.toString().trim().split(/\s+/).map(Number);
    return {ahead,behind};
  }
  async function branches(sessionId:string,folderId:string,dir:string):Promise<GitBranches>{
    const rows=lines((await run(dir,["for-each-ref",`--format=%(refname:short)${unit}%(objectname)${unit}%(contents:subject)${unit}%(authorname)${unit}%(authordate:unix)`,"refs/heads"])).stdout);
    const all=rows.map(row=>{const [name,sha,subject,author,time]=row.split(unit);return {name,sha,subject,author,time:Number(time)*1000,ahead:0,behind:0};});
    const main=all.find(branch=>branch.name==="main")??null,list:GitBranch[]=[];
    for(const branch of all)if(branch!==main)list.push(main?{...branch,...await aheadBehind(dir,"refs/heads/main",`refs/heads/${branch.name}`)}:branch);
    const history=await overview(sessionId,folderId);
    const agent:GitAgentBranch[]=history.branches.map(branch=>({name:branch.name,threadId:branch.threadId,message:branch.message,author:branch.author,modifiedAt:branch.modifiedAt,files:branch.changes.length}));
    return {main,branches:list.sort((a,b)=>b.time-a.time),agent};
  }
  async function tags(dir:string):Promise<GitTag[]>{
    const rows=lines((await run(dir,["for-each-ref","--sort=-creatordate",`--format=%(refname:short)${unit}%(objectname)${unit}%(objecttype)${unit}%(*objectname)${unit}%(contents:subject)${unit}%(taggername)${unit}%(taggerdate:unix)${unit}%(authorname)${unit}%(authordate:unix)`,"refs/tags"])).stdout);
    return rows.map(row=>{
      const [name,sha,type,peeled,subject,tagger,taggedAt,author,authoredAt]=row.split(unit),annotated=type==="tag";
      return {name,sha,target:annotated?peeled:sha,message:annotated?subject:"",author:annotated?tagger:author,time:Number(annotated?taggedAt:authoredAt)*1000,annotated};
    });
  }
  async function compare(dir:string,baseName:string|null,headName:string|null):Promise<GitCompare>{
    const base=await refOf(dir,baseName),head=await refOf(dir,headName),{ahead,behind}=await aheadBehind(dir,base,head);
    const mergeBase=(await git(dir,["--git-dir",dir,"merge-base",base,head],{allowFailure:true})).stdout.toString().trim()||emptyTree;
    const list=parseLog((await run(dir,["log","--date-order",`--max-count=${limits.compare}`,"--shortstat","--diff-merges=first-parent",logFormat,`${base}..${head}`])).stdout);
    const tip=async(ref:string)=>(await run(dir,["rev-parse","--verify",ref])).stdout.toString().trim();
    // Whether Git can merge without conflicts; --quiet only answers, writing nothing.
    const mergeable=ahead>0&&(behind===0||(await git(dir,["--git-dir",dir,"merge-tree","--write-tree","--quiet",base,head],{allowFailure:true})).code===0);
    return {base:baseName!,head:headName!,baseSha:await tip(base),headSha:await tip(head),ahead:list,behind,fastForward:behind===0&&ahead>0,mergeable,files:ahead?await changes(dir,mergeBase,head,false):[]};
  }
  async function archive(res:ServerResponse,dir:string,ref:string,name:string){
    const full=/^[0-9a-f]{40}$/.test(ref)?await commitOf(dir,ref):await refOf(dir,ref);
    const sha=(await run(dir,["rev-parse","--verify",`${full}^{commit}`])).stdout.toString().trim(),label=`${name.replace(/[^\w.-]+/g,"-").replace(/^-+|-+$/g,"")||"repository"}-${ref.replace(/[^\w.-]+/g,"-")}`;
    const child=gitProcess(dir,["--git-dir",dir,"archive","--format=zip",`--prefix=${label}/`,sha]);
    const closed=new Promise<number|null>(resolve=>{child.once("close",resolve);child.once("error",()=>resolve(-1));});
    child.stdin!.end();child.stderr!.resume();res.on("close",()=>{if(child.exitCode===null)child.kill("SIGKILL");});
    res.writeHead(200,{"content-type":"application/zip","cache-control":"private, no-store","content-disposition":`attachment; filename*=UTF-8''${encodeURIComponent(label)}.zip`});
    for await(const chunk of child.stdout!)if(!res.write(chunk))await once(res,"drain");
    if(await closed!==0){res.destroy();return;}
    res.end();
  }
  async function blob(req:IncomingMessage,res:ServerResponse,dir:string,ref:string|null,pathValue:string|null){
    const full=ref&&/^[0-9a-f]{40}$/.test(ref)?await commitOf(dir,ref):await refOf(dir,ref??"main"),path=treePath(pathValue);
    if(!path)throw new AppError("notFound",404);
    const listed=(await run(dir,["ls-tree","-z","-l",full,"--",path])).stdout.toString("utf8").split("\0")[0];
    const [mode,type,sha,size]=(listed??"").split("\t")[0].split(/\s+/);
    if(type!=="blob"||Number(size)>256*1024*1024)throw new AppError("notFound",404);
    const content=(await run(dir,["cat-file","blob",sha])).stdout,text=mode!=="120000"&&content.length<=limits.blob&&!content.subarray(0,8000).includes(0);
    const name=path.split("/").at(-1)!;
    res.writeHead(200,{"content-type":text?"text/plain; charset=utf-8":"application/octet-stream","content-length":content.length,"cache-control":"private, no-store","x-content-type-options":"nosniff",
      "content-security-policy":"sandbox; default-src 'none'","content-disposition":`${text&&new URL(req.url??"/",origin).searchParams.has("view")?"inline":"attachment"}; filename*=UTF-8''${encodeURIComponent(name)}`});
    res.end(req.method==="HEAD"?undefined:content);
  }
  /**
   * Merges one branch into another from the web: a fast-forward, or a merge commit Git writes
   * without a working tree (refused on conflicts). Either way the ref moves through the same checks
   * as a push, so main still changes the folder only when no touched file has uncommitted edits.
   */
  async function merge(sessionId:string,folderId:string,user:{username:string;displayName:string},input:Record<string,unknown>){
    const {base,head,baseSha,headSha,mode,message,deleteBranch}=input;
    if(Object.keys(input).some(key=>!["base","head","baseSha","headSha","mode","message","deleteBranch"].includes(key)))throw new AppError("invalidInput");
    if(typeof base!=="string"||typeof head!=="string"||base===head||typeof baseSha!=="string"||typeof headSha!=="string"||(mode!=="fast-forward"&&mode!=="merge"))throw new AppError("invalidInput");
    if(message!==undefined&&(typeof message!=="string"||message.length>2000||message.includes("\0")))throw new AppError("invalidInput");
    if(deleteBranch!==undefined&&typeof deleteBranch!=="boolean")throw new AppError("invalidInput");
    if(head==="main"&&deleteBranch)throw new AppError("forbidden",403,"main cannot be deleted.");
    return repositories.locked(folderId,async()=>{
      const {dir}=await repositories.syncLocked(sessionId,folderId);
      const baseRef=await refOf(dir,base,["heads"]),headRef=await refOf(dir,head,["heads"]);
      const tip=async(ref:string)=>(await run(dir,["rev-parse","--verify",ref])).stdout.toString().trim();
      const currentBase=await tip(baseRef),currentHead=await tip(headRef);
      if(currentBase!==baseSha||currentHead!==headSha)throw new AppError("conflict",409,"The branches changed since you opened them. Review them again.");
      const {ahead,behind}=await aheadBehind(dir,baseRef,headRef);
      if(!ahead)throw new AppError("nothingToMerge",409,`${base} already has every commit on ${head}.`);
      let target=currentHead;
      if(mode==="fast-forward"){if(behind)throw new AppError("conflict",409,`${base} has commits ${head} does not, so it cannot fast-forward. Create a merge commit instead.`);}
      else{
        const merged=await git(dir,["--git-dir",dir,"merge-tree","--write-tree","-z","--name-only",currentBase,currentHead],{allowFailure:true});
        const fields=merged.stdout.toString("utf8").split("\0");
        if(merged.code===1){
          const files=fields.slice(1,fields.indexOf("",1)>0?fields.indexOf("",1):undefined).filter(Boolean);
          throw new AppError("mergeConflict",409,`${files.length===1?"This file conflicts":"These files conflict"}: ${files.slice(0,8).join(", ")}${files.length>8?` and ${files.length-8} more`:""}. Merge with Git, resolve them and push.`);
        }
        if(merged.code!==0||!/^[0-9a-f]{40}$/.test(fields[0]))throw new AppError("gitFailed",502,"Git could not merge these branches.");
        const name=user.displayName.replace(/[<>\n\r\0]/g,"").trim()||user.username,email=`${user.username}@dar.invalid`;
        const text=(message?.trim()||`Merge branch '${head}'${base==="main"?"":` into ${base}`}`)+"\n";
        target=(await git(dir,["--git-dir",dir,"commit-tree",fields[0],"-p",currentBase,"-p",currentHead,"-F","-"],{input:text,env:{GIT_AUTHOR_NAME:name,GIT_AUTHOR_EMAIL:email,GIT_COMMITTER_NAME:name,GIT_COMMITTER_EMAIL:email}})).stdout.toString().trim();
      }
      const commands=[{old:currentBase,new:target,name:baseRef},...deleteBranch?[{old:currentHead,new:zero,name:headRef}]:[]];
      const {results}=await pushFromDar(db,repositories,blobs,sessionId,folderId,dir,commands);
      const error=results.get(baseRef)??results.get(headRef);
      if(error)throw new AppError("conflict",409,/uncommitted|not committed/.test(error)?`${error[0].toUpperCase()}${error.slice(1)}.`:`The merge was refused: ${error}.`);
      return {sha:target,fastForward:target===currentHead,deleted:!!deleteBranch};
    });
  }
  async function deleteRef(sessionId:string,folderId:string,input:Record<string,unknown>){
    const name=input.name,old=input.old;
    if(typeof name!=="string"||!/^refs\/(heads|tags)\//.test(name)||!validRefName(name.replace(/^refs\/(heads|tags)\//,""))||typeof old!=="string"||!/^[0-9a-f]{40}$/.test(old))throw new AppError("invalidInput");
    if(name==="refs/heads/main")throw new AppError("forbidden",403,"main cannot be deleted.");
    return repositories.locked(folderId,async()=>{
      const {results}=await repositories.call<{results:{name:string;error?:string}[]}>(sessionId,"push",{folderId,updates:[{name,old,new:zero}],atomic:true,commits:[],tags:[],blobs:[]});
      const error=results.find(result=>result.error)?.error;
      if(error)throw new AppError("conflict",409,error==="fetch first"?"It changed since you opened it. Reload and try again.":`It could not be deleted: ${error}.`);
      await repositories.syncLocked(sessionId,folderId);
      return {deleted:name};
    });
  }

  return async function handleGitApi(req:IncomingMessage,res:ServerResponse,url:URL,sessionId:string,user:{username:string;displayName:string},body:()=>Promise<Record<string,unknown>>){
    const username=user.username;
    const send=(value:unknown,status=200)=>writeJson(res,status,JSON.stringify(value)),method=req.method??"GET",params=url.searchParams;
    if(url.pathname==="/api/v1/git/repositories"&&method==="GET")return send(await repositoriesOf(sessionId,username));
    if(url.pathname==="/api/v1/git/activity"&&method==="GET")return send(await activity(sessionId));
    const match=/^\/api\/v1\/git\/repositories\/([a-f0-9-]{36})(\/[a-z]+)?(?:\/([0-9a-f]{40}))?$/.exec(url.pathname);
    if(!match)throw new AppError("notFound",404);
    const folderId=match[1],rest=match[2]??"",sha=match[3];
    if(!rest&&method==="GET")return send(await summary(sessionId,username,await rowOf(sessionId,folderId)));
    if(rest==="/refs"&&method==="DELETE")return send(await deleteRef(sessionId,folderId,await body()));
    if(rest==="/merge"&&method==="POST")return send(await merge(sessionId,folderId,user,await body()));
    if(method!=="GET"&&!(method==="HEAD"&&rest==="/blob"))throw new AppError("notFound",404);
    if(rest==="/archive"){const name=(await rowOf(sessionId,folderId)).name;return withRepo(sessionId,folderId,dir=>archive(res,dir,params.get("ref")??"main",name));}
    if(rest==="/blob")return withRepo(sessionId,folderId,dir=>blob(req,res,dir,params.get("ref"),params.get("path")));
    const value=await withRepo(sessionId,folderId,async(dir,state)=>{
      if(rest==="/tree")return tree(dir,state,params.get("ref"),params.get("path"));
      if(rest==="/commits"&&sha){
        const detail=await commitDetail(dir,sha),pin=await repositories.call<{revision:number;changesetId:string|null}>(sessionId,"changesetOf",{folderId,sha:detail.sha});
        return pin.changesetId?{...detail,restore:{changesetId:pin.changesetId,revision:pin.revision}}:detail;
      }
      if(rest==="/commits")return commits(dir,state,params);
      if(rest==="/branches")return branches(sessionId,folderId,dir);
      if(rest==="/tags")return tags(dir);
      if(rest==="/compare")return compare(dir,params.get("base"),params.get("head"));
      throw new AppError("notFound",404);
    });
    return send(value);
  };
}
