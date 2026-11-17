import type {IncomingMessage,ServerResponse} from "node:http";
import {spawn,type ChildProcess} from "node:child_process";
import {createHash,randomUUID} from "node:crypto";
import {constants} from "node:fs";
import {access,mkdir,mkdtemp,open,readFile,readdir,rename,rm,stat,writeFile} from "node:fs/promises";
import {createDeflate,createGunzip,deflateSync} from "node:zlib";
import {once} from "node:events";
import {isUtf8} from "node:buffer";
import path from "node:path";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {digestToken} from "../auth/session.ts";
import {blobPath,type blobStore} from "./blobs.ts";
import {mediaTypeOf} from "./media-types.ts";
import {gitReadOperations} from "../../shared/read-operations.ts";
import {gitLimits,historyLimits} from "../../shared/history.ts";
import {maxFileBytes} from "../../shared/limits.ts";

/**
 * Git over HTTPS for tracked folders (docs/spacetimedb/folder-history.md#git). The database keeps the
 * history; each tracked folder has a bare repository under <root>/git as a cache, rebuilt from the
 * database when missing. Clones and fetches run git upload-pack on it. Pushes are received into a
 * quarantine, read with Git, and applied by the database in one transaction before Git reports success.
 */
type Blobs=ReturnType<typeof blobStore>;
export const zero="0".repeat(40),emptyTree="4b825dc642cb6eb9a060e54bf8d69288fbee4904",mainRef="refs/heads/main";

// ---- Git processes --------------------------------------------------------------------------

/** Git with no user or system configuration, hooks or network. */
export function gitProcess(home:string,args:string[],env:Record<string,string>={}):ChildProcess{
  return spawn("git",["-c","core.hooksPath=/dev/null","-c","protocol.allow=never",...args],{stdio:["pipe","pipe","pipe"],
    env:{PATH:process.env.PATH??"",HOME:home,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",GIT_TERMINAL_PROMPT:"0",...env}});
}
/** Runs Git to completion; resolves its output, or rejects with its error text kept out of responses. */
export function git(home:string,args:string[],options:{env?:Record<string,string>;input?:string|Buffer;timeout?:number;allowFailure?:boolean}={}):Promise<{stdout:Buffer;code:number}>{
  const child=gitProcess(home,args,options.env),out:Buffer[]=[],timer=setTimeout(()=>child.kill("SIGKILL"),options.timeout??300_000);
  let stderr="";
  child.stdout!.on("data",chunk=>out.push(chunk));child.stderr!.on("data",chunk=>{if(stderr.length<4096)stderr+=chunk;});
  child.stdin!.on("error",()=>{});child.stdin!.end(options.input??"");
  return new Promise((resolve,reject)=>{
    child.once("error",()=>{clearTimeout(timer);reject(new AppError("gitUnavailable",503,"Git is not available on this server."));});
    child.once("close",code=>{
      clearTimeout(timer);
      if(code===0||options.allowFailure)resolve({stdout:Buffer.concat(out),code:code??-1});
      else reject(Object.assign(new AppError("gitFailed",502,"Git could not complete the request."),{detail:stderr}));
    });
  });
}
export const lines=(value:Buffer)=>value.toString("utf8").split("\n").filter(Boolean);

// ---- Objects --------------------------------------------------------------------------------

/** Writes loose objects; existing ones (loose or packed) are left alone. */
class ObjectWriter {
  private dir:string;private root:string;
  constructor(dir:string,root:string){this.dir=dir;this.root=root;}
  private file(sha:string){return path.join(this.dir,"objects",sha.slice(0,2),sha.slice(2));}
  async exists(sha:string){try{await access(this.file(sha));return true;}catch{return false;}}
  async write(type:string,content:Buffer){
    const header=Buffer.from(`${type} ${content.length}\0`),sha=createHash("sha1").update(header).update(content).digest("hex");
    if(!await this.exists(sha))await this.place(sha,deflateSync(Buffer.concat([header,content])));
    return sha;
  }
  /** Streams a stored blob into a Git blob, returning its ID. */
  async writeBlob(blobId:string,size:number){
    const source=await open(blobPath(this.root,blobId),constants.O_RDONLY|constants.O_NOFOLLOW);
    const temporary=path.join(this.dir,"objects",`tmp-${randomUUID()}`);
    try{
      if((await source.stat()).size!==size)throw new AppError("contentUnavailable",503);
      const hash=createHash("sha1"),deflate=createDeflate(),target=await open(temporary,constants.O_CREAT|constants.O_EXCL|constants.O_WRONLY,0o600);
      const written=(async()=>{for await(const chunk of deflate)await target.write(chunk as Buffer);})();
      const header=Buffer.from(`blob ${size}\0`);hash.update(header);deflate.write(header);
      for await(const chunk of source.createReadStream({autoClose:false})){hash.update(chunk as Buffer);if(!deflate.write(chunk))await once(deflate,"drain");}
      deflate.end();await written;await target.close();
      const sha=hash.digest("hex");
      if(await this.exists(sha))await rm(temporary,{force:true});else await this.place(sha,undefined,temporary);
      return sha;
    }finally{await source.close();await rm(temporary,{force:true});}
  }
  private async place(sha:string,data?:Buffer,from?:string){
    const file=this.file(sha);await mkdir(path.dirname(file),{recursive:true});
    if(from){await rename(from,file);return;}
    const temporary=`${file}.${randomUUID()}.tmp`;await writeFile(temporary,data!,{mode:0o444,flag:"wx"});await rename(temporary,file);
  }
}

/** A tree being built; unchanged subtrees keep their IDs. */
interface Leaf {kind:"blob";mode:string;sha:string}
interface Tree {kind:"tree";entries:Map<string,Leaf|Tree>;sha?:string}
const emptyNode=():Tree=>({kind:"tree",entries:new Map(),sha:emptyTree});
function setPath(node:Tree,parts:string[],leaf:Leaf|undefined):Tree{
  const entries=new Map(node.entries),[name,...rest]=parts;
  if(!rest.length){if(leaf)entries.set(name,leaf);else entries.delete(name);}
  else{const child=entries.get(name),next=setPath(child?.kind==="tree"?child:emptyNode(),rest,leaf);if(next.entries.size)entries.set(name,next);else entries.delete(name);}
  return {kind:"tree",entries};
}
/** Git's tree order: names compared as bytes, a folder as if its name ended in "/". */
async function hashTree(node:Tree,objects:ObjectWriter):Promise<string>{
  if(node.sha)return node.sha;
  const items=await Promise.all([...node.entries].map(async([name,child])=>({name,key:Buffer.from(child.kind==="tree"?name+"/":name),mode:child.kind==="tree"?"40000":child.mode||"100644",sha:child.kind==="tree"?await hashTree(child,objects):child.sha})));
  items.sort((a,b)=>Buffer.compare(a.key,b.key));
  node.sha=await objects.write("tree",Buffer.concat(items.flatMap(item=>[Buffer.from(`${item.mode} ${item.name}\0`),Buffer.from(item.sha,"hex")])));
  return node.sha;
}
/** A commit's tree, read back from the repository. */
async function loadTree(dir:string,commit:string):Promise<Tree>{
  const {stdout}=await git(dir,["--git-dir",dir,"ls-tree","-r","-t","-z","--full-tree",commit]),root:Tree={kind:"tree",entries:new Map()};
  const trees=new Map<string,Tree>([["",root]]);
  for(const record of stdout.toString("utf8").split("\0").filter(Boolean)){
    const tab=record.indexOf("\t"),[mode,type,sha]=record.slice(0,tab).split(" "),file=record.slice(tab+1),slash=file.lastIndexOf("/");
    const parent=trees.get(slash<0?"":file.slice(0,slash))!,name=file.slice(slash+1);
    if(type==="tree"){const node:Tree={kind:"tree",entries:new Map(),sha};trees.set(file,node);parent.entries.set(name,node);}
    else parent.entries.set(name,{kind:"blob",mode:mode==="100644"?"":mode,sha});
  }
  // Entries were added after their tree's ID was read; that ID still describes them.
  return root;
}

// ---- History from the database --------------------------------------------------------------

interface ExportChange {path:string;previousPath:string;kind:string;mode:string;blobId?:string;size?:number;sha?:string}
interface ExportCommit {id:string;parentId:string;main:boolean;message:string;createdAt:number;author:{name:string;username:string};sha?:string;raw?:string;changes:ExportChange[]}
interface GitState {revision:number;generation:number;main:string|null;base:string|null;pending:ExportCommit[];refs:{name:string;sha:string}[]}
interface GitExport {revision:number;generation:number;main:string|null;headId:string;commits:ExportCommit[];tags:{sha:string;raw:string}[];refs:{name:string;sha:string}[]}
interface LocalState {generation:number;revision:number}
interface Pin {id:string;sha:string;raw:string;parents:string[];author:string}

const clean=(value:string)=>value.replace(/[<>\n\r\0]/g,"").trim();
/** The Git commit for a Dar commit: same tree, author and time every time it is made. */
function nativeCommit(tree:string,parent:string|undefined,commit:ExportCommit){
  const who=`${clean(commit.author.name)||clean(commit.author.username)||"Dar"} <${commit.author.username.replace(/[<>\s]/g,"")||"dar"}@dar.invalid> ${Math.floor(commit.createdAt/1000)} +0000`;
  return Buffer.from(`tree ${tree}\n${parent?`parent ${parent}\n`:""}author ${who}\ncommitter ${who}\n\n${commit.message}\n`);
}
function headerOf(raw:Buffer){const end=raw.indexOf("\n\n");return (end<0?raw:raw.subarray(0,end)).toString("utf8").split("\n");}

/** Per-folder repositories under <root>/git, one operation at a time per folder. */
export function gitRepositories(db:Database,root:string){
  const base=path.join(root,"git"),locks=new Map<string,Promise<unknown>>();
  // Folders whose repository may have new objects since the last maintenance.
  const touched=new Set<string>();
  const dirOf=(folderId:string)=>path.join(base,`${folderId}.git`);
  function locked<T>(folderId:string,work:()=>Promise<T>):Promise<T>{
    touched.add(folderId);
    const previous=locks.get(folderId)??Promise.resolve(),next=previous.catch(()=>{}).then(work);
    const settled=next.catch(()=>{});locks.set(folderId,settled);void settled.then(()=>{if(locks.get(folderId)===settled)locks.delete(folderId);});
    return next;
  }
  const call=<T>(sessionId:string,operation:string,document:Record<string,unknown>)=>db.operation<T>("service_git",operation,gitReadOperations,[sessionId,operation,JSON.stringify(document)]);

  async function prepare(dir:string,generation:number):Promise<LocalState>{
    try{const state=JSON.parse(await readFile(path.join(dir,"dar-state.json"),"utf8")) as LocalState;if(state.generation===generation)return state;}catch{}
    await rm(dir,{recursive:true,force:true});await mkdir(base,{recursive:true,mode:0o700});
    await git(base,["init","--bare","-q","-b","main",dir]);
    for(const [key,value] of [["uploadpack.allowFilter","true"],["uploadpack.allowReachableSHA1InWant","true"],["gc.autoDetach","false"],["core.logAllRefUpdates","false"],["receive.denyDeletes","true"]])
      await git(base,["config","--file",path.join(dir,"config"),key,value]);
    const state={generation,revision:-1};await saveState(dir,state);return state;
  }
  const saveState=(dir:string,state:LocalState)=>writeFile(path.join(dir,"dar-state.json"),JSON.stringify(state),{mode:0o600});
  async function missing(dir:string,shas:string[]){
    if(!shas.length)return [];
    const {stdout}=await git(dir,["--git-dir",dir,"cat-file","--batch-check"],{input:shas.join("\n")+"\n"});
    return lines(stdout).filter(line=>line.endsWith(" missing")).map(line=>line.split(" ")[0]);
  }

  /**
   * Writes Git objects for commits (in base-first order) from their bases' trees, and the Git
   * commits Dar's own commits get. Returns the pins to record.
   */
  async function materialize(dir:string,commits:ExportCommit[],trees:Map<string,Tree>,shas:Map<string,string>){
    const objects=new ObjectWriter(dir,root),pins:Pin[]=[],blobPins=new Map<string,{sha:string;blobId:string;size:number}>(),hashed=new Map<string,string>();
    const children=new Map<string,number>();for(const commit of commits)children.set(commit.parentId,(children.get(commit.parentId)??0)+1);
    for(const commit of commits){
      let tree=commit.parentId?trees.get(commit.parentId):emptyNode();
      if(!tree)throw new AppError("historyUnavailable",503,"This folder's history could not be read.");
      for(const change of commit.changes)if(change.kind==="deleted"||change.kind==="renamed")tree=setPath(tree,(change.kind==="renamed"?change.previousPath:change.path).split("/"),undefined);
      for(const change of commit.changes){
        if(change.kind==="deleted")continue;
        let sha=change.sha||hashed.get(change.blobId!);
        if(!sha||!await objects.exists(sha)){
          const written=await objects.writeBlob(change.blobId!,change.size!);
          if(sha&&sha!==written)throw new AppError("historyMismatch",500,"This folder's history does not match its Git objects.");
          sha=written;if(!change.sha)blobPins.set(sha,{sha,blobId:change.blobId!,size:change.size!});
        }
        hashed.set(change.blobId!,sha);tree=setPath(tree,change.path.split("/"),{kind:"blob",mode:change.mode,sha});
      }
      const treeSha=await hashTree(tree,objects);
      if(commit.raw){
        const raw=Buffer.from(commit.raw,"base64");
        if(headerOf(raw)[0]!==`tree ${treeSha}`)throw new AppError("historyMismatch",500,"This folder's history does not match its Git objects.");
        if(await objects.write("commit",raw)!==commit.sha)throw new AppError("historyMismatch",500,"This folder's history does not match its Git objects.");
      }else{
        const parent=commit.parentId?shas.get(commit.parentId):undefined;
        if(commit.parentId&&!parent)throw new AppError("historyUnavailable",503,"This folder's history could not be read.");
        const raw=nativeCommit(treeSha,parent,commit);commit.sha=await objects.write("commit",raw);
        pins.push({id:commit.id,sha:commit.sha,raw:raw.toString("base64"),parents:parent?[parent]:[],author:clean(commit.author.name)||commit.author.username});
      }
      shas.set(commit.id,commit.sha!);trees.set(commit.id,tree);
      // A tree is kept only while commits based on it are still to come.
      const left=(children.get(commit.parentId)??1)-1;children.set(commit.parentId,left);if(!left)trees.delete(commit.parentId);
      if(!children.get(commit.id))trees.delete(commit.id);
    }
    return {pins,blobs:[...blobPins.values()]};
  }
  async function pin(sessionId:string,folderId:string,result:{pins:Pin[];blobs:{sha:string;blobId:string;size:number}[]}){
    for(let i=0;i<Math.max(result.pins.length,result.blobs.length);i+=1000){
      const {conflict}=await call<{conflict:boolean}>(sessionId,"pin",{folderId,commits:result.pins.slice(i,i+1000),blobs:result.blobs.slice(i,i+1000)});
      if(conflict)throw new AppError("conflict",409,"The folder's history changed; try again.");
    }
  }
  async function setRefs(dir:string,wanted:Map<string,string>){
    const {stdout}=await git(dir,["--git-dir",dir,"for-each-ref","--format=%(objectname) %(refname)"]),have=new Map(lines(stdout).map(line=>{const [sha,name]=line.split(" ");return [name,sha] as const;}));
    let input="";
    for(const [name,sha] of wanted)if(have.get(name)!==sha)input+=`update ${name} ${sha}\n`;
    for(const name of have.keys())if(!wanted.has(name))input+=`delete ${name}\n`;
    if(input)await git(dir,["--git-dir",dir,"update-ref","--stdin"],{input});
  }
  /** Brings a folder's repository up to date with its history and returns its path. Caller holds the lock. */
  async function syncLocked(sessionId:string,folderId:string){
    const state=await call<GitState>(sessionId,"state",{folderId}),dir=dirOf(folderId),local=await prepare(dir,state.generation);
    if(local.revision===state.revision&&!state.pending.length)return {dir,state};
    const shas=new Map<string,string>();let main=state.main;
    const needed=[...new Set([state.main,state.base,...state.refs.map(ref=>ref.sha)].filter((sha):sha is string=>!!sha))];
    if((await missing(dir,needed)).length){
      // The cache is new or incomplete: rebuild every commit from the database.
      const all=await call<GitExport>(sessionId,"export",{folderId}),byId=new Map(all.commits.map(commit=>[commit.id,commit])),ordered:ExportCommit[]=[],done=new Set<string>([""]);
      const visit=(commit:ExportCommit)=>{const stack=[commit];while(stack.length){const top=stack.at(-1)!;if(done.has(top.id)){stack.pop();continue;}const parent=byId.get(top.parentId);if(top.parentId&&!parent)throw new AppError("historyUnavailable",503,"This folder's history could not be read.");if(parent&&!done.has(parent.id)){stack.push(parent);continue;}done.add(top.id);ordered.push(top);stack.pop();}};
      for(const commit of all.commits)visit(commit);
      for(const commit of ordered)if(commit.sha)shas.set(commit.id,commit.sha);
      await pin(sessionId,folderId,await materialize(dir,ordered,new Map(),shas));
      const objects=new ObjectWriter(dir,root);for(const tag of all.tags)await objects.write("tag",Buffer.from(tag.raw,"base64"));
      main=all.headId?shas.get(all.headId)??null:null;
    }else if(state.pending.length){
      const first=state.pending[0],trees=new Map<string,Tree>();
      if(first.parentId){trees.set(first.parentId,await loadTree(dir,state.base!));shas.set(first.parentId,state.base!);}
      await pin(sessionId,folderId,await materialize(dir,state.pending,trees,shas));
      main=shas.get(state.pending.at(-1)!.id)!;
    }
    const wanted=new Map(state.refs.map(ref=>[ref.name,ref.sha]));if(main)wanted.set(mainRef,main);
    await setRefs(dir,wanted);
    await saveState(dir,{generation:state.generation,revision:state.revision});
    return {dir,state:{...state,main}};
  }
  const sync=(sessionId:string,folderId:string)=>locked(folderId,()=>syncLocked(sessionId,folderId));
  /**
   * git gc --auto with low thresholds: a few hundred loose objects become a pack, and 8 packs one
   * pack with a bitmap; each writes a commit graph. Unreachable objects (refused pushes, the Git
   * app's merge checks) go at once: the repository is a cache, and the caller holds the lock.
   */
  async function compact(dir:string){
    try{await access(dir);}catch{return;}
    await git(dir,["--git-dir",dir,"-c","gc.auto=250","-c","gc.autoPackLimit=8","-c","gc.autoDetach=false","-c","gc.pruneExpire=now",
      "-c","repack.writeBitmaps=true","-c","gc.writeCommitGraph=true","gc","--auto","--quiet"],{timeout:1_800_000});
  }

  return {
    sync,syncLocked,
    /**
     * Removes repositories of folders that no longer keep history, and packs the ones used since
     * last time once loose objects or packs pile up: Dar's own commits and a rebuilt repository
     * are loose objects, and each push adds a pack, which makes every clone and fetch slower.
     */
    async maintain(){
      let names:string[];try{names=await readdir(base);}catch{return;}
      const repos=new Set((await db.read<{id:string}[]>("service_git_repos")).map(repo=>`${repo.id}.git`));
      for(const name of names)if(/^[a-f0-9-]{36}\.git$/.test(name)&&!repos.has(name))await locked(name.slice(0,36),()=>rm(path.join(base,name),{recursive:true,force:true}));
      const folders=[...touched];touched.clear();
      for(const folderId of folders)if(repos.has(`${folderId}.git`))await locked(folderId,()=>compact(dirOf(folderId))).catch(()=>{});
      // locked() marks each folder it runs for; packing is not a change.
      for(const folderId of folders)touched.delete(folderId);
    },
    /** main and every branch and tag as a Git bundle, for the History tab's download. */
    async bundle(sessionId:string,folderId:string,res:ServerResponse,name:string){
      const {dir,state}=await sync(sessionId,folderId);
      if(!state.main&&!state.refs.length)throw new AppError("nothingToExport",409,"Commit something before exporting.");
      const work=await mkdtemp(path.join(base,"bundle-"));
      try{
        await git(dir,["--git-dir",dir,"bundle","create","-q",path.join(work,"history.bundle"),"--all"]);
        const file=path.join(work,"history.bundle"),size=(await stat(file)).size,filename=`${name.replace(/[^\w .-]+/g,"").trim()||"history"}.bundle`;
        res.writeHead(200,{"content-type":"application/octet-stream","content-length":size,"cache-control":"private, no-store","content-disposition":`attachment; filename*=UTF-8''${encodeURIComponent(filename)}`});
        const handle=await open(file,constants.O_RDONLY);try{for await(const chunk of handle.createReadStream({autoClose:false}))if(!res.write(chunk))await once(res,"drain");}finally{await handle.close();}
        res.end();
      }finally{await rm(work,{recursive:true,force:true});}
    },
    locked,call,dirOf,
  };
}
export type GitRepositories=ReturnType<typeof gitRepositories>;

// ---- Smart HTTP -----------------------------------------------------------------------------

const pkt=(payload:string|Buffer)=>{const data=Buffer.from(payload);return Buffer.concat([Buffer.from((data.length+4).toString(16).padStart(4,"0")),data]);};
const flush=Buffer.from("0000");

/** Reads pkt-lines from a request, then hands over the rest (the pack). */
class PacketReader {
  private buffer=Buffer.alloc(0);private done=false;private iterator:AsyncIterator<Buffer>;
  constructor(source:AsyncIterable<Buffer>){this.iterator=source[Symbol.asyncIterator]();}
  private async fill(size:number){
    while(this.buffer.length<size&&!this.done){const next=await this.iterator.next();if(next.done)this.done=true;else this.buffer=Buffer.concat([this.buffer,next.value]);}
    return this.buffer.length>=size;
  }
  /** A packet's payload, or null for a flush. */
  async packet():Promise<Buffer|null>{
    if(!await this.fill(4))throw new AppError("invalidRequest",400,"The Git request ended early.");
    const length=Number.parseInt(this.buffer.subarray(0,4).toString("ascii"),16);
    if(!Number.isInteger(length)||length===1||length===2||length===3||length>65520)throw new AppError("invalidRequest",400,"The Git request is malformed.");
    if(length===0){this.buffer=this.buffer.subarray(4);return null;}
    if(!await this.fill(length))throw new AppError("invalidRequest",400,"The Git request ended early.");
    const payload=this.buffer.subarray(4,length);this.buffer=this.buffer.subarray(length);return payload;
  }
  async *rest(){if(this.buffer.length)yield this.buffer;this.buffer=Buffer.alloc(0);while(true){const next=await this.iterator.next();if(next.done)return;yield next.value;}}
}

/** Reads `git cat-file --batch` output object by object. */
class BatchReader {
  private buffer=Buffer.alloc(0);private ended=false;private iterator:AsyncIterator<Buffer>;
  constructor(stream:NodeJS.ReadableStream){this.iterator=(stream as AsyncIterable<Buffer>)[Symbol.asyncIterator]();}
  private async more(){const next=await this.iterator.next();if(next.done){this.ended=true;return false;}this.buffer=Buffer.concat([this.buffer,next.value]);return true;}
  async header(){
    let end:number;while((end=this.buffer.indexOf(10))<0)if(!await this.more())throw new AppError("gitFailed",502,"Git could not read the push.");
    const line=this.buffer.subarray(0,end).toString("utf8");this.buffer=this.buffer.subarray(end+1);
    const [sha,type,size]=line.split(" ");if(type==="missing"||!size)throw new AppError("invalidRequest",400,"The push is missing objects.");
    return {sha,type,size:Number(size)};
  }
  async read(size:number){while(this.buffer.length<size+1)if(!await this.more())throw new AppError("gitFailed",502,"Git could not read the push.");const data=this.buffer.subarray(0,size);this.buffer=this.buffer.subarray(size+1);return Buffer.from(data);}
  /** The next object's content as chunks, without holding all of it. */
  async *stream(size:number){
    let left=size;
    while(left>0){if(!this.buffer.length&&!await this.more())throw new AppError("gitFailed",502,"Git could not read the push.");const part=this.buffer.subarray(0,Math.min(left,this.buffer.length));this.buffer=this.buffer.subarray(part.length);left-=part.length;yield Buffer.from(part);}
    while(!this.buffer.length&&!this.ended)await this.more();this.buffer=this.buffer.subarray(1);
  }
}

interface Command {old:string;new:string;name:string}
interface Change {path:string;previousPath:string;kind:"added"|"modified"|"deleted"|"renamed";mode:string;previousMode:string;blob:string;previousBlob:string;mediaType:string}
const modeOf=(value:string)=>value==="100644"?"":value;

/** Parses `git diff-tree --stdin -r -z --raw --always` output into each commit's changes, in input order. */
function parseDiff(output:Buffer):Change[][]{
  const tokens=output.toString("utf8").split("\0"),result:Change[][]=[];let current:Change[]|undefined;
  for(let i=0;i<tokens.length;i++){
    const token=tokens[i];if(!token)continue;
    if(!token.startsWith(":")){current=[];result.push(current);continue;}
    const [oldMode,newMode,oldSha,newSha,status]=token.slice(1).split(" ");
    if([oldMode,newMode].includes("160000"))throw new AppError("unsupportedSubmodule",400,"Dar does not keep Git submodules.");
    for(const mode of [oldMode,newMode])if(!["000000","100644","100755","120000"].includes(mode))throw new AppError("unsupportedMode",400,`Dar cannot keep files with Git mode ${mode}.`);
    const first=tokens[++i],kind=status[0];
    if(kind==="R"||kind==="C"){const second=tokens[++i];current!.push(kind==="R"?{path:second,previousPath:first,kind:"renamed",mode:modeOf(newMode),previousMode:modeOf(oldMode),blob:newSha,previousBlob:oldSha,mediaType:""}:{path:second,previousPath:"",kind:"added",mode:modeOf(newMode),previousMode:"",blob:newSha,previousBlob:"",mediaType:""});}
    else if(kind==="A")current!.push({path:first,previousPath:"",kind:"added",mode:modeOf(newMode),previousMode:"",blob:newSha,previousBlob:"",mediaType:""});
    else if(kind==="D")current!.push({path:first,previousPath:"",kind:"deleted",mode:"",previousMode:modeOf(oldMode),blob:"",previousBlob:oldSha,mediaType:""});
    else current!.push({path:first,previousPath:"",kind:"modified",mode:modeOf(newMode),previousMode:modeOf(oldMode),blob:newSha,previousBlob:oldSha,mediaType:""});
  }
  return result;
}

/** Reads a request body, gunzipping it when Git compressed it. */
function bodyOf(req:IncomingMessage):AsyncIterable<Buffer>{
  const encoding=req.headers["content-encoding"];
  if(encoding==="gzip"||encoding==="x-gzip")return req.pipe(createGunzip());
  if(encoding&&encoding!=="identity")throw new AppError("unsupportedEncoding",415);
  return req;
}

/** /dav/git/<folderId>.git: Git's smart HTTP protocol, signed in with an app password with Files access. */
export async function handleGitHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,repositories:GitRepositories,blobs:Blobs,origin:string){
  // A folder by ID, which never changes, or by alias: /dav/git/<username>/<path from their root>.git.
  const match=/^\/dav\/git\/(?:([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})(?:\.git)?|([^/]+)\/(.+)\.git)(\/info\/refs|\/git-upload-pack|\/git-receive-pack)$/.exec(url.pathname);
  if(!match)throw new AppError("notFound",404);
  let alias:{user:string;path:string}|undefined;
  if(!match[1])try{alias={user:decodeURIComponent(match[2]),path:match[3].split("/").map(decodeURIComponent).join("/")};}catch{throw new AppError("notFound",404);}
  if(req.headers.origin&&req.headers.origin!==origin)throw new AppError("invalidOrigin",403);
  const route=match[4],method=req.method??"GET";
  const service=route==="/info/refs"?url.searchParams.get("service"):route.slice(1);
  if(service!=="git-upload-pack"&&service!=="git-receive-pack")throw new AppError("smartHttpRequired",403,"Use a Git client that speaks the smart HTTP protocol.");
  if(route==="/info/refs"?method!=="GET":method!=="POST"||req.headers["content-type"]!==`application/x-${service}-request`)throw new AppError("notFound",404);
  const write=service==="git-receive-pack";
  const authorization=req.headers.authorization;
  if(!authorization?.startsWith("Basic ")||authorization.length>2048)throw new AppError("unauthenticated",401);
  const decoded=Buffer.from(authorization.slice(6),"base64").toString("utf8"),separator=decoded.indexOf(":");
  if(separator<1)throw new AppError("unauthenticated",401);
  const secret=decoded.slice(separator+1);
  if(!/^[A-Za-z0-9_-]{43}$/.test(secret))throw new AppError("unauthenticated",401);
  const {sessionId}=await db.read<{sessionId:string}>("service_protocol_authenticate",[decoded.slice(0,separator),digestToken(secret),write]);
  if(alias&&alias.user.toLowerCase()!==decoded.slice(0,separator).toLowerCase())throw new AppError("notFound",404);
  const folderId=match[1]??(await repositories.call<{folderId:string}>(sessionId,"resolve",{path:alias!.path})).folderId;
  res.setHeader("cache-control","no-cache, max-age=0, must-revalidate");
  const protocol=typeof req.headers["git-protocol"]==="string"&&/^[\w.=:-]{1,256}$/.test(req.headers["git-protocol"])?req.headers["git-protocol"]:"";

  if(service==="git-upload-pack"){
    const {dir}=await repositories.sync(sessionId,folderId);
    const advertise=route==="/info/refs",child=gitProcess(dir,["upload-pack","--stateless-rpc",...advertise?["--http-backend-info-refs"]:[],dir],protocol?{GIT_PROTOCOL:protocol}:{});
    const closed=new Promise<number|null>(resolve=>{child.once("close",resolve);child.once("error",()=>resolve(-1));});
    child.stderr!.resume();
    res.on("close",()=>{if(child.exitCode===null)child.kill("SIGKILL");});
    res.writeHead(200,{"content-type":`application/x-git-upload-pack-${advertise?"advertisement":"result"}`});
    if(advertise&&!/version=2/.test(protocol)){res.write(pkt("# service=git-upload-pack\n"));res.write(flush);}
    if(advertise)child.stdin!.end();
    else void (async()=>{try{for await(const chunk of bodyOf(req))if(!child.stdin!.write(chunk))await once(child.stdin!,"drain");child.stdin!.end();}catch{child.kill("SIGKILL");}})();
    child.stdin!.on("error",()=>{});
    for await(const chunk of child.stdout!)if(!res.write(chunk))await once(res,"drain");
    if(await closed!==0){res.destroy();return;}
    res.end();return;
  }
  if(route==="/info/refs"){
    // receive-pack's advertisement: the refs as the database has them.
    const {state}=await repositories.sync(sessionId,folderId);
    const refs=[...state.main?[[mainRef,state.main] as const]:[],...state.refs.map(ref=>[ref.name,ref.sha] as const)];
    const capabilities="report-status delete-refs side-band-64k quiet atomic ofs-delta object-format=sha1 agent=dar";
    res.writeHead(200,{"content-type":"application/x-git-receive-pack-advertisement"});
    res.write(pkt("# service=git-receive-pack\n"));res.write(flush);
    if(!refs.length)res.write(pkt(`${zero} capabilities^{}\0${capabilities}\n`));
    refs.forEach(([name,sha],index)=>res.write(pkt(`${sha} ${name}${index?"":`\0${capabilities}`}\n`)));
    res.end(flush);return;
  }
  return receivePack(req,res,db,repositories,blobs,sessionId,folderId);
}

/** git-receive-pack: quarantine the pack, read what it adds, let the database decide, then report per ref. */
async function receivePack(req:IncomingMessage,res:ServerResponse,db:Database,repositories:GitRepositories,blobs:Blobs,sessionId:string,folderId:string){
  const reader=new PacketReader(bodyOf(req)),commands:Command[]=[];let capabilities=new Set<string>();
  for(let packet=await reader.packet();packet;packet=await reader.packet()){
    let text=packet.toString("utf8");
    const nul=text.indexOf("\0");if(nul>=0){if(!commands.length)capabilities=new Set(text.slice(nul+1).trim().split(" "));text=text.slice(0,nul);}
    if(text.startsWith("shallow "))throw new AppError("shallowPush",400,"Dar cannot take a push from a shallow clone.");
    const [old,next,name]=text.replace(/\n$/,"").split(" ");
    if(!/^[0-9a-f]{40}$/.test(old)||!/^[0-9a-f]{40}$/.test(next)||!name||commands.length>=1000)throw new AppError("invalidRequest",400,"The Git request is malformed.");
    commands.push({old,new:next,name});
  }
  const sideBand=capabilities.has("side-band-64k"),report=capabilities.has("report-status");
  const {dir}=await repositories.sync(sessionId,folderId);
  const quarantine=path.join(dir,"objects",`incoming-${randomUUID()}`),env={GIT_DIR:dir,GIT_OBJECT_DIRECTORY:quarantine,GIT_ALTERNATE_OBJECT_DIRECTORIES:path.join(dir,"objects"),GIT_QUARANTINE_PATH:quarantine};
  let unpack="ok";const results=new Map<string,string|undefined>(),messages:string[]=[];
  // Checking and storing a large push can take a while with nothing to say: once the pack is in, an
  // empty side-band packet every few seconds keeps proxies (nginx waits 120 s) from giving up.
  let keepAlive:ReturnType<typeof setInterval>|undefined;
  const keep=pkt(Buffer.from([1])),startKeepAlive=()=>{
    if(!sideBand||!report||res.headersSent)return;
    res.writeHead(200,{"content-type":"application/x-git-receive-pack-result"});res.write(keep);
    keepAlive=setInterval(()=>{if(!res.destroyed)res.write(keep);},10_000);keepAlive.unref();
  };
  try{
    await mkdir(path.join(quarantine,"pack"),{recursive:true});
    if(commands.some(command=>command.new!==zero)){
      // Thin packs are completed from the repository; --strict checks every object (and refuses .git paths).
      const child=gitProcess(dir,["index-pack","--stdin","--fix-thin",`--strict=${fsckIgnored}`,`--max-input-size=${gitLimits.packBytes}`],env);
      let stderr="";child.stderr!.on("data",chunk=>{if(stderr.length<4096)stderr+=chunk;});child.stdout!.resume();child.stdin!.on("error",()=>{});
      const closed=new Promise<number|null>(resolve=>{child.once("close",resolve);child.once("error",()=>resolve(-1));});
      try{for await(const chunk of reader.rest())if(!child.stdin!.write(chunk))await once(child.stdin!,"drain");}finally{child.stdin!.end();}
      if(await closed!==0)unpack=/fatal: (.*)/.exec(stderr)?.[1]?.slice(0,200)??"the pack could not be read";
    }else for await(const _ of reader.rest()){/* No pack follows deletions. */}
    if(unpack!=="ok")for(const command of commands)results.set(command.name,"unpacker error");
    else{
      startKeepAlive();
      const outcome=await repositories.locked(folderId,()=>applyPush(db,repositories,blobs,sessionId,folderId,dir,env,commands,capabilities.has("atomic"),quarantine));
      for(const [name,error] of outcome.results)results.set(name,error);messages.push(...outcome.messages);
    }
  }catch(error){
    const message=error instanceof AppError&&error.status<500?pushFailures[error.code]??error.message:error instanceof AppError&&error.code==="quotaExceeded"?pushFailures.quotaExceeded:"the push could not be stored";
    for(const command of commands)results.set(command.name,message);messages.push(`Dar: ${message}`);
  }finally{clearInterval(keepAlive);await rm(quarantine,{recursive:true,force:true});}

  if(!res.headersSent)res.writeHead(200,{"content-type":"application/x-git-receive-pack-result"});
  const status=Buffer.concat([pkt(`unpack ${unpack}\n`),...commands.map(command=>{const error=results.get(command.name);return pkt(error?`ng ${command.name} ${error.replace(/\n/g," ")}\n`:`ok ${command.name}\n`);}),flush]);
  if(!report){res.end();return;}
  if(!sideBand){res.end(status);return;}
  for(const message of messages)res.write(pkt(Buffer.concat([Buffer.from([2]),Buffer.from(message+"\n")])));
  for(let i=0;i<status.length;i+=65515)res.write(pkt(Buffer.concat([Buffer.from([1]),status.subarray(i,i+65515)])));
  res.end(flush);
}
/** Database refusals as the pusher sees them (Git prints them after "remote:" or beside the ref). */
const pushFailures:Record<string,string>={quotaExceeded:"not enough storage left in Dar",conflict:"the folder changed during the push; fetch and try again",
  operationTooLarge:"the push is too large for Dar; push fewer commits at a time",forbidden:"you can read this folder but not push to it",notFound:"this folder does not keep history",
  invalidInput:"Dar could not read the push",contentUnavailable:"some stored content is missing"};
/** Commit and tag header problems Git itself accepts by default; tree problems stay errors. */
const fsckIgnored=["badDate","badDateOverflow","badEmail","badName","badTimezone","missingAuthor","missingCommitter","missingEmail","missingNameBeforeEmail","missingSpaceBeforeDate","missingSpaceBeforeEmail","zeroPaddedDate","multipleAuthors","missingTaggerEntry","badTagName"].map(id=>`${id}=ignore`).join(",");

interface ParsedCommit {sha:string;raw:Buffer;parents:string[];author:string;message:string;time:number;changes:Change[]}
function parseCommit(sha:string,raw:Buffer):ParsedCommit{
  if(raw.length>gitLimits.objectBytes)throw new AppError("objectTooLarge",413,`Commit ${sha.slice(0,7)} is too large for Dar.`);
  const header=headerOf(raw),end=raw.indexOf("\n\n"),parents:string[]=[];let author="",time=0;
  for(const line of header){
    if(line.startsWith("parent "))parents.push(line.slice(7));
    else if(line.startsWith("author ")){author=line.slice(7).replace(/\s*<[^>]*>.*$/,"").trim();}
    else if(line.startsWith("committer ")){const stamp=/>\s*(\d+)\s*[+-]\d{4}\s*$/.exec(line);time=stamp?Number(stamp[1])*1000:0;}
  }
  const message=(end<0?"":raw.subarray(end+2).toString("utf8")).replace(/�/g,"").trim()||"(no message)";
  return {sha,raw,parents,author:author.slice(0,256)||"Unknown",message:message.slice(0,historyLimits.messageBytes/2),time:time||Date.now(),changes:[]};
}

/** Reads the pushed objects with Git and hands the push to the database; returns each ref's error, if any. */
async function applyPush(db:Database,repositories:GitRepositories,blobs:Blobs,sessionId:string,folderId:string,dir:string,env:Record<string,string>,commands:Command[],atomic:boolean,quarantine:string){
  const {state}=await repositories.syncLocked(sessionId,folderId);
  const results=new Map<string,string|undefined>(),messages:string[]=[];
  const tips=[...new Set(commands.filter(command=>command.new!==zero).map(command=>command.new))];
  const types=new Map<string,string>();
  if(tips.length){const {stdout}=await git(dir,["cat-file","--batch-check"],{env,input:tips.join("\n")+"\n"});for(const line of lines(stdout)){const [sha,type]=line.split(" ");types.set(sha,type);}}
  // Annotated tags: keep each tag object down to the commit it names.
  const tags:{sha:string;raw:string;target:string;author:string}[]=[],commitTips=new Set<string>();
  for(const tip of tips){
    let sha=tip,type=types.get(tip);
    for(let depth=0;type==="tag"&&depth<16;depth++){
      const {stdout:raw}=await git(dir,["cat-file","tag",sha],{env});
      if(raw.length>gitLimits.objectBytes)throw new AppError("objectTooLarge",413,"A tag is too large for Dar.");
      const header=headerOf(raw),target=header.find(line=>line.startsWith("object "))?.slice(7)??"",targetType=header.find(line=>line.startsWith("type "))?.slice(5)??"";
      tags.push({sha,raw:raw.toString("base64"),target,author:(header.find(line=>line.startsWith("tagger "))?.slice(7).replace(/\s*<[^>]*>.*$/,"")??"").trim().slice(0,256)});
      sha=target;type=targetType;
    }
    if(type==="commit")commitTips.add(sha);
  }
  // New commits: everything the tips reach that no ref here reaches yet, parents first.
  const fresh=commitTips.size?lines((await git(dir,["rev-list","--topo-order","--reverse",...commitTips,"--not","--all"],{env})).stdout):[];
  if(fresh.length>gitLimits.commits)throw new AppError("pushTooLarge",413,`Push at most ${gitLimits.commits} new commits at a time.`);
  const commits=new Map<string,ParsedCommit>();
  if(fresh.length){
    const child=gitProcess(dir,["cat-file","--batch"],env),batch=new BatchReader(child.stdout!);child.stderr!.resume();child.stdin!.end(fresh.join("\n")+"\n");
    for(const sha of fresh){const header=await batch.header();if(header.type!=="commit")throw new AppError("invalidRequest",400,"The push is malformed.");commits.set(sha,parseCommit(sha,await batch.read(header.size)));}
  }
  // main's new commits, oldest first, and their changes against main's previous commit.
  const mainCommand=commands.find(command=>command.name===mainRef&&command.new!==zero&&command.old===(state.main??zero)&&types.get(command.new)==="commit");
  const chain=mainCommand?lines((await git(dir,["rev-list","--first-parent","--reverse",mainCommand.new,...state.main?["--not",state.main]:[]],{env})).stdout):[];
  if(chain.length>gitLimits.commits)throw new AppError("pushTooLarge",413,`Push at most ${gitLimits.commits} new commits at a time.`);
  const diffs:string[]=[],targets:{kind:"commit"|"chain";sha:string}[]=[];
  for(const commit of commits.values()){diffs.push(commit.parents.length?`${commit.sha} ${commit.parents[0]}`:commit.sha);targets.push({kind:"commit",sha:commit.sha});}
  chain.forEach((sha,index)=>{
    const base=index?chain[index-1]:state.main,commit=commits.get(sha);
    if(commit&&commit.parents[0]===base||commit&&!base&&!commit.parents.length)return;
    diffs.push(base?`${sha} ${base}`:sha);targets.push({kind:"chain",sha});
  });
  const chainChanges=new Map<string,Change[]>();
  if(diffs.length){
    const parsed=parseDiff((await git(dir,["diff-tree","--stdin","-r","-z","-M","--raw","--root","--always"],{env,input:diffs.join("\n")+"\n"})).stdout);
    if(parsed.length!==targets.length)throw new AppError("gitFailed",502,"Git could not read the push.");
    targets.forEach((target,index)=>{if(target.kind==="commit")commits.get(target.sha)!.changes=parsed[index];else chainChanges.set(target.sha,parsed[index]);});
  }
  const allChanges=[...[...commits.values()].flatMap(commit=>commit.changes),...[...chainChanges.values()].flat()];
  if(allChanges.length>gitLimits.changes)throw new AppError("pushTooLarge",413,"This push changes too many files at once; push fewer commits at a time.");
  // Content: sizes from Git; the database says which it already has.
  const wantedBlobs=[...new Set(allChanges.filter(change=>change.kind!=="deleted").map(change=>change.blob))];
  const sizes=new Map<string,number>();
  if(wantedBlobs.length)for(const line of lines((await git(dir,["cat-file","--batch-check"],{env,input:wantedBlobs.join("\n")+"\n"})).stdout)){
    const [sha,type,size]=line.split(" ");if(type!=="blob")throw new AppError("invalidRequest",400,"The push is missing objects.");
    if(Number(size)>maxFileBytes)throw new AppError("uploadTooLarge",413,"A file in this push is larger than Dar allows.");sizes.set(sha,Number(size));
  }
  const begin=await repositories.call<{blobs:({sha:string;blobId:string}|{sha:string;uploadId:string})[]}>(sessionId,"pushBegin",{folderId,blobs:wantedBlobs.map(sha=>({sha,size:sizes.get(sha)}))});
  const text=new Map<string,boolean>(),stored:Record<string,unknown>[]=[];
  const uploads=begin.blobs.filter((row):row is {sha:string;uploadId:string}=>"uploadId" in row);
  for(const row of begin.blobs)if("blobId" in row)stored.push({sha:row.sha,size:sizes.get(row.sha),blobId:row.blobId});
  if(uploads.length){
    // One batch: the files are flushed to disk together rather than one after another.
    const child=gitProcess(dir,["cat-file","--batch"],env),batch=new BatchReader(child.stdout!);child.stderr!.resume();child.stdin!.end(uploads.map(row=>row.sha).join("\n")+"\n");
    try{
      const digests=await blobs.receiveBatch((async function*(){
        for(const row of uploads){
          const header=await batch.header(),size=sizes.get(row.sha)!;
          if(header.sha!==row.sha||header.size!==size)throw new AppError("gitFailed",502,"Git could not read the push.");
          let sample:Buffer|undefined;
          yield {id:row.uploadId,size,source:(async function*(){for await(const chunk of batch.stream(size)){if(!sample)sample=chunk.subarray(0,8192);yield chunk;}})()};
          // UTF-8 at the start (cut at a character boundary) makes an unknown extension plain text.
          text.set(row.sha,!!sample&&(isUtf8(sample)||sample.length===8192&&[1,2,3].some(cut=>isUtf8(sample!.subarray(0,8192-cut)))));
        }
      })());
      for(const row of uploads)stored.push({sha:row.sha,size:sizes.get(row.sha),uploadId:row.uploadId,digest:digests.get(row.uploadId)});
    }finally{if(child.exitCode===null)child.kill("SIGKILL");}
  }
  // By extension; else plain text for UTF-8 content; else (""), the type stored content already has.
  const typed=(change:Change)=>{if(change.kind==="deleted")return change;const guess=mediaTypeOf(change.path);return {...change,mediaType:guess!=="application/octet-stream"?guess:text.get(change.blob)?"text/plain":""};};
  const outcome=await repositories.call<{results:{name:string;error?:string}[]}>(sessionId,"push",{folderId,atomic,updates:commands,
    commits:[...commits.values()].map(commit=>({sha:commit.sha,raw:commit.raw.toString("base64"),parents:commit.parents,author:commit.author,message:commit.message,time:commit.time,changes:commit.changes.map(typed)})),
    tags,...mainCommand?{main:{chain:chain.map(sha=>({sha,...chainChanges.has(sha)?{changes:chainChanges.get(sha)!.map(typed)}:{}}))}}:{},blobs:stored});
  for(const row of outcome.results)results.set(row.name,row.error);
  // Keep the objects: refused refs leave them unreachable, which is harmless.
  const pack=path.join(quarantine,"pack");
  const names=(await readdir(pack)).sort((a,b)=>Number(a.endsWith(".idx"))-Number(b.endsWith(".idx")));
  await mkdir(path.join(dir,"objects","pack"),{recursive:true});
  for(const name of names)await rename(path.join(pack,name),path.join(dir,"objects","pack",name));
  await repositories.syncLocked(sessionId,folderId).catch(()=>{});
  if(mainCommand&&!results.get(mainRef))messages.push("Dar: the folder now matches main.");
  return {results,messages};
}

/**
 * A ref change made in Dar, such as a merge from the Git app: its objects are already in the
 * repository, and it is checked and applied exactly like a push of the same refs, atomically.
 * The caller holds the folder's lock.
 */
export async function pushFromDar(db:Database,repositories:GitRepositories,blobs:Blobs,sessionId:string,folderId:string,dir:string,commands:Command[]){
  const quarantine=path.join(dir,"objects",`incoming-${randomUUID()}`);
  try{
    await mkdir(path.join(quarantine,"pack"),{recursive:true});
    return await applyPush(db,repositories,blobs,sessionId,folderId,dir,{GIT_DIR:dir},commands,true,quarantine);
  }finally{await rm(quarantine,{recursive:true,force:true});}
}
export type {Blobs as GitBlobs};
