import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,writeFile,rm,mkdir,readFile} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import type {HistoryOverview} from "../shared/history.ts";
import type {GitBranches,GitCommitDetail,GitCommitPage,GitCompare,GitRepository,GitTag,GitTree} from "../shared/git.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

/**
 * Git features beyond everyday clone and push, with the real git client: force pushes, lightweight
 * tags, notes, partial clones, atomic pushes, pruning, signed commits, unusual names and content,
 * every refusal, and the Git app's merge, restore and source filter.
 */
test("git interop: the rest of Git, refusals, web merges and restore",{skip:!origin,timeout:600_000},async()=>{
  const base=origin!,session=await login(base),db=await connect(session),headers={...session.headers,"content-type":"application/json"};
  const work=await mkdtemp(path.join(tmpdir(),"dar-git-interop-"));
  const api=async<T>(route:string,init:RequestInit={},status=200)=>{const response=await fetch(`${base}/api/v1/git${route}`,{...init,headers:init.body?headers:session.headers});const text=await response.text();assert.equal(response.status,status,`${route}: ${text}`);return (text?JSON.parse(text):undefined) as T;};
  const history=(folderId:string,rest="")=>`${base}/api/v1/files/history/${folderId}${rest}`;
  const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Ada Lovelace",GIT_AUTHOR_EMAIL:"ada@example.com",GIT_COMMITTER_NAME:"Ada Lovelace",GIT_COMMITTER_EMAIL:"ada@example.com"};
  let secret="";
  const git=(cwd:string,args:string[],ok=true,input?:string)=>{
    const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`admin:${secret}`).toString("base64")}`,"-c","init.defaultBranch=main","-c","protocol.version=2",...args],{cwd,env,encoding:"utf8",input,maxBuffer:64*1024*1024});
    if(ok)assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);else assert.notEqual(result.status,0,`git ${args.join(" ")} should fail\n${result.stdout}${result.stderr}`);
    return {out:result.stdout.trim(),err:result.stderr};
  };
  const until=async(check:()=>boolean|Promise<boolean>,what:string)=>{for(let i=0;i<100;i++){if(await check())return;await new Promise(resolve=>setTimeout(resolve,50));}assert.fail(what);};
  // A folder's entries through the picker API (the live subscription only follows the open folder).
  const children=async(parentId:string)=>{const items:{id:string;name:string;kind:string}[]=[];let cursor="";do{const page=await (await fetch(`${base}/api/v1/files/picker?${new URLSearchParams({parentId,cursor,kind:"file"})}`,{headers:session.headers})).json() as {items:typeof items;nextCursor:string|null};items.push(...page.items);cursor=page.nextCursor??"";}while(cursor);return items;};
  const child=async(parentId:string,name:string)=>(await children(parentId)).find(item=>item.name===name);
  try{
    const folderId=randomUUID(),name=`Interop ${randomUUID().slice(0,6)}`,repoApi=`/repositories/${folderId}`;
    await db.reducers.createFolder({id:folderId,parentId:"",name});
    const readme=await upload(base,session,"README.md","# Interop\n",folderId);
    assert.equal((await fetch(history(folderId),{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
    let state=await (await fetch(history(folderId),{headers:session.headers})).json() as HistoryOverview;
    assert.equal((await fetch(history(folderId,"/commits"),{method:"POST",headers,body:JSON.stringify({message:"Start",token:state.status!.token})})).status,201);
    secret=(await (await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:"Interop",scope:"write"})})).json() as {secret:string}).secret;
    const url=(await api<GitRepository>(repoApi)).clone.alias;
    git(work,["clone","-q",url,"a"]);const a=path.join(work,"a");
    git(work,["clone","-q",url,"b"]);const b=path.join(work,"b");
    const remote=()=>git(a,["ls-remote",url]).out;

    // Force pushes: refused without --force, accepted with it.
    git(a,["checkout","-q","-b","topic"]);await writeFile(path.join(a,"topic.txt"),"one\n");git(a,["add","-A"]);git(a,["commit","-qm","Topic one"]);git(a,["push","-q","origin","topic"]);
    git(a,["commit","-q","--amend","-m","Topic rewritten"]);
    assert.match(git(a,["push","origin","topic"],false).err,/rejected|non-fast-forward/);
    git(a,["push","-q","--force","origin","topic"]);
    assert.match(remote(),new RegExp(`${git(a,["rev-parse","topic"]).out}\\s+refs/heads/topic`));

    // Lightweight tags push and delete.
    git(a,["tag","light","topic"]);git(a,["push","-q","origin","light"]);
    assert.deepEqual((await api<GitTag[]>(`${repoApi}/tags`)).map(tag=>[tag.name,tag.annotated]),[["light",false]]);
    git(a,["push","-q","origin","--delete","light"]);assert.doesNotMatch(remote(),/refs\/tags\/light/);

    // Notes travel with refs/notes.
    git(a,["notes","add","-m","Reviewed","topic"]);git(a,["push","-q","origin","refs/notes/commits"]);
    git(b,["fetch","-q","origin","refs/notes/commits:refs/notes/commits","topic:topic"]);
    assert.equal(git(b,["notes","show","topic"]).out,"Reviewed");

    // Partial clone: blobs arrive when checked out or diffed.
    git(work,["clone","-q","--filter=blob:none",url,"partial"]);
    assert.equal(await readFile(path.join(work,"partial","README.md"),"utf8"),"# Interop\n");
    assert.match(git(path.join(work,"partial"),["log","-p","--all"]).out,/Topic one|one/);

    // Atomic pushes: one bad ref stops them all.
    git(a,["checkout","-q","main"]);git(a,["checkout","-q","-b","atomic-new"]);git(a,["checkout","-q","main"]);
    git(a,["commit","-q","--allow-empty","-m","Diverge"]);git(b,["commit","-q","--allow-empty","-m","Ahead elsewhere"]);git(b,["push","-q","origin","main"]);
    git(a,["push","--atomic","origin","main","atomic-new"],false);
    assert.doesNotMatch(remote(),/atomic-new/);
    git(a,["reset","-q","--hard","origin/main"]);git(a,["pull","-q","--ff-only"]);

    // fetch --prune drops branches deleted on Dar.
    git(b,["fetch","-q","origin"]);assert.match(git(b,["branch","-r"]).out,/origin\/topic/);
    git(a,["push","-q","origin","--delete","topic"]);git(b,["fetch","-q","--prune","origin"]);
    assert.doesNotMatch(git(b,["branch","-r"]).out,/origin\/topic/);

    // Refusals: submodules, names differing only by case, shallow pushes, oversized pushes.
    const refused=async(setup:()=>Promise<void>,pattern:RegExp)=>{
      git(a,["checkout","-q","-B","refused","main"]);await setup();
      assert.match(git(a,["push","origin","refused:main"],false).err,pattern);
      git(a,["checkout","-q","main"]);git(a,["branch","-q","-D","refused"]);
    };
    await refused(async()=>{git(a,["update-index","--add","--cacheinfo",`160000,${git(a,["rev-parse","HEAD"]).out},lib`]);git(a,["commit","-qm","Submodule"]);},/submodule/i);
    await refused(async()=>{await writeFile(path.join(a,"Notes.txt"),"1\n");await writeFile(path.join(a,"notes.txt"),"2\n");git(a,["add","-A"]);git(a,["commit","-qm","Case"]);},/only by case/);
    await rm(path.join(a,"Notes.txt"),{force:true});await rm(path.join(a,"notes.txt"),{force:true});
    git(work,["clone","-q","--depth","1",url,"shallow"]);const shallow=path.join(work,"shallow");
    await writeFile(path.join(shallow,"from-shallow.txt"),"x\n");git(shallow,["add","-A"]);git(shallow,["commit","-qm","From a shallow clone"]);
    git(shallow,["push","origin","main"],false);
    const stream=["blob","mark :1","data 2","x",""];
    for(let i=0;i<5001;i++)stream.push("commit refs/heads/huge",`committer Ada <ada@example.com> ${1700000000+i} +0000`,"data 2","c",...i?[]:["from refs/heads/main"],`M 100644 :1 n${i%7}.txt`,"");
    git(a,["fast-import","--quiet"],true,stream.join("\n")+"\n");
    assert.match(git(a,["push","origin","huge"],false).err,/at most 5000/);
    git(a,["branch","-q","-D","huge"]);
    assert.equal((await api<GitRepository>(repoApi)).branches,1,"nothing refused was kept");

    // Signed commits keep their exact bytes: the signature still verifies after a fresh clone.
    await mkdir(path.join(work,"keys"));spawnSync("ssh-keygen",["-q","-t","ed25519","-N","","-C","ada","-f",path.join(work,"keys","id")]);
    await writeFile(path.join(work,"keys","allowed"),`ada@example.com ${(await readFile(path.join(work,"keys","id.pub"),"utf8")).trim()}\n`);
    await writeFile(path.join(a,"signed.txt"),"signed\n");git(a,["add","-A"]);
    git(a,["-c","gpg.format=ssh","-c",`user.signingkey=${path.join(work,"keys","id")}`,"commit","-q","-S","-m","Signed change"]);
    git(a,["push","-q","origin","main"]);const signed=git(a,["rev-parse","HEAD"]).out;
    await rm(path.join(process.env.DAR_TEST_ROOT!,"git",`${folderId}.git`),{recursive:true,force:true});
    git(work,["clone","-q",url,"c"]);const c=path.join(work,"c");
    assert.equal(git(c,["rev-parse","HEAD"]).out,signed);
    git(c,["-c",`gpg.ssh.allowedSignersFile=${path.join(work,"keys","allowed")}`,"verify-commit","HEAD"]);

    // Names and content: accents, binary files and diffs too large to show.
    await mkdir(path.join(a,"café"));await writeFile(path.join(a,"café","naïve résumé.txt"),"bonjour\n");
    const binary=Buffer.from(Array.from({length:3000},(_,i)=>(i*37)%256));await writeFile(path.join(a,"image.bin"),binary);
    await writeFile(path.join(a,"big.txt"),Array.from({length:6000},(_,i)=>`line ${i}`).join("\n")+"\n");
    git(a,["add","-A"]);git(a,["commit","-qm","Unusual content"]);git(a,["push","-q","origin","main"]);
    await until(async()=>{const cafe=await child(folderId,"café");return !!cafe&&!!await child(cafe.id,"naïve résumé.txt");},"accented names reach Files");
    assert.deepEqual((await api<GitTree>(`${repoApi}/tree?path=caf%C3%A9`)).entries.map(entry=>entry.name),["naïve résumé.txt"]);
    const detail=await api<GitCommitDetail>(`${repoApi}/commits/${git(a,["rev-parse","HEAD"]).out}`);
    assert.equal(detail.files.find(file=>file.path==="image.bin")?.binary,true);
    assert.equal(detail.files.find(file=>file.path==="big.txt")?.truncated,true);
    assert.equal(detail.files.find(file=>file.path==="café/naïve résumé.txt")?.patch,"@@ -0,0 +1 @@\n+bonjour");
    assert.deepEqual(Buffer.from(await (await fetch(`${base}/api/v1/git${repoApi}/blob?path=image.bin`,{headers:session.headers})).arrayBuffer()),binary);

    // A branch pushed to main from the command line fast-forwards it and changes the folder.
    git(a,["checkout","-q","-b","promote"]);await writeFile(path.join(a,"promoted.txt"),"promoted\n");git(a,["add","-A"]);git(a,["commit","-qm","Promote me"]);
    git(a,["push","-q","origin","promote"]);git(a,["push","-q","origin","promote:main"]);
    await until(async()=>!!await child(folderId,"promoted.txt"),"the promoted commit changes the folder");
    git(a,["checkout","-q","main"]);git(a,["pull","-q","--ff-only"]);git(a,["push","-q","origin","--delete","promote"]);

    // Web merges: fast-forward, a merge commit, conflicts, stale views and uncommitted Dar edits.
    const merge=(body:object,status=200)=>api<{sha:string;fastForward:boolean;deleted:boolean}>(`${repoApi}/merge`,{method:"POST",body:JSON.stringify(body)},status);
    const compare=(head:string)=>api<GitCompare>(`${repoApi}/compare?base=main&head=${encodeURIComponent(head)}`);
    git(a,["checkout","-q","-b","ff"]);await writeFile(path.join(a,"ff.txt"),"fast\n");git(a,["add","-A"]);git(a,["commit","-qm","Fast-forward me"]);git(a,["push","-q","origin","ff"]);
    let view=await compare("ff");assert.deepEqual([view.fastForward,view.mergeable],[true,true]);
    let result=await merge({base:"main",head:"ff",baseSha:view.baseSha,headSha:view.headSha,mode:"fast-forward",deleteBranch:true});
    assert.deepEqual([result.sha,result.fastForward,result.deleted],[view.headSha,true,true]);
    await until(async()=>!!await child(folderId,"ff.txt"),"a fast-forward changes the folder");
    assert.doesNotMatch(remote(),/refs\/heads\/ff/);
    git(a,["checkout","-q","main"]);git(a,["pull","-q","--ff-only"]);
    await writeFile(path.join(a,"shared.txt"),"top\nmiddle\nbottom\n");git(a,["add","-A"]);git(a,["commit","-qm","Shared file"]);git(a,["push","-q","origin","main"]);
    git(a,["checkout","-q","-b","side"]);await writeFile(path.join(a,"shared.txt"),"TOP\nmiddle\nbottom\n");git(a,["commit","-qam","Side edits the top"]);git(a,["push","-q","origin","side"]);
    git(a,["checkout","-q","main"]);await writeFile(path.join(a,"shared.txt"),"top\nmiddle\nBOTTOM\n");git(a,["commit","-qam","Main edits the bottom"]);git(a,["push","-q","origin","main"]);
    view=await compare("side");assert.deepEqual([view.fastForward,view.mergeable,view.behind],[false,true,1]);
    await merge({base:"main",head:"side",baseSha:view.baseSha,headSha:view.headSha,mode:"fast-forward"},409);
    await merge({base:"main",head:"side",baseSha:"0".repeat(40),headSha:view.headSha,mode:"merge"},409);
    result=await merge({base:"main",head:"side",baseSha:view.baseSha,headSha:view.headSha,mode:"merge",message:"Merge side from the web"});
    assert.equal(result.fastForward,false);
    git(a,["pull","-q","--ff-only"]);
    assert.equal(git(a,["log","-1","--format=%s|%P|%an|%ae"]).out,`Merge side from the web|${view.baseSha} ${view.headSha}|Test Administrator|admin@dar.invalid`);
    const shared=(await child(folderId,"shared.txt"))!;
    await until(async()=>await (await fetch(`${base}/api/v1/files/${shared.id}/content`,{headers:session.headers})).text()==="TOP\nmiddle\nBOTTOM\n","a merged file holds both sides");
    git(a,["checkout","-q","-B","clash","main"]);await writeFile(path.join(a,"shared.txt"),"CLASH\nmiddle\nBOTTOM\n");git(a,["commit","-qam","Clash"]);git(a,["push","-q","origin","clash"]);
    git(a,["checkout","-q","main"]);await writeFile(path.join(a,"shared.txt"),"OTHER\nmiddle\nBOTTOM\n");git(a,["commit","-qam","Other"]);git(a,["push","-q","origin","main"]);
    view=await compare("clash");assert.equal(view.mergeable,false);
    const clash=await fetch(`${base}/api/v1/git${repoApi}/merge`,{method:"POST",headers,body:JSON.stringify({base:"main",head:"clash",baseSha:view.baseSha,headSha:view.headSha,mode:"merge"})});
    assert.equal(clash.status,409);assert.match(await clash.text(),/shared\.txt/);
    git(a,["push","-q","origin","--delete","clash"]);
    git(a,["checkout","-q","-b","dirty"]);await writeFile(path.join(a,"README.md"),"# From the branch\n");git(a,["commit","-qam","Branch README"]);git(a,["push","-q","origin","dirty"]);git(a,["checkout","-q","main"]);
    await upload(base,session,"README.md","# Edited in Dar\n",folderId,readme.fileId,1);
    view=await compare("dirty");
    const dirty=await fetch(`${base}/api/v1/git${repoApi}/merge`,{method:"POST",headers,body:JSON.stringify({base:"main",head:"dirty",baseSha:view.baseSha,headSha:view.headSha,mode:"fast-forward"})});
    assert.equal(dirty.status,409);assert.match(await dirty.text(),/uncommitted/);
    assert.equal(await (await fetch(`${base}/api/v1/files/${readme.fileId}/content`,{headers:session.headers})).text(),"# Edited in Dar\n","the Dar edit is untouched");

    // Restore from a commit's details: refused while Dar has edits, then a new restore commit.
    const start=(await api<GitCommitPage>(`${repoApi}/commits?ref=main&source=dar`)).commits.at(-1)!;
    const startDetail=await api<GitCommitDetail>(`${repoApi}/commits/${start.sha}`);assert.ok(startDetail.restore,"main commits can be restored");
    const restore=()=>fetch(history(folderId,"/restore"),{method:"POST",headers,body:JSON.stringify({id:startDetail.restore!.changesetId,revision:startDetail.restore!.revision})});
    assert.ok((await restore()).status>=400,"restoring over uncommitted edits is refused");
    state=await (await fetch(history(folderId),{headers:session.headers})).json() as HistoryOverview;
    assert.equal((await fetch(history(folderId,"/commits"),{method:"POST",headers,body:JSON.stringify({message:"Keep the Dar edit",token:state.status!.token})})).status,201);
    const fresh=await api<GitCommitDetail>(`${repoApi}/commits/${start.sha}`);
    assert.equal((await fetch(history(folderId,"/restore"),{method:"POST",headers,body:JSON.stringify({id:fresh.restore!.changesetId,revision:fresh.restore!.revision})})).status,201);
    assert.equal(await (await fetch(`${base}/api/v1/files/${readme.fileId}/content`,{headers:session.headers})).text(),"# Interop\n");
    assert.equal((await api<GitCommitDetail>(`${repoApi}/commits/${git(a,["rev-parse","origin/dirty"]).out}`)).restore,undefined,"branch commits are not restore points");

    // The source filter covers the whole history and the three sources add up.
    const count=async(query:string)=>(await api<GitCommitPage>(`${repoApi}/commits?ref=all${query}`)).total;
    const [all,pushed,dar,agent]=[await count(""),await count("&source=git"),await count("&source=dar"),await count("&source=agent")];
    assert.equal(pushed+dar+agent,all);assert.ok(dar>=3&&pushed>=8);
    const darPage=await api<GitCommitPage>(`${repoApi}/commits?ref=all&source=dar&limit=1&skip=1`);
    assert.equal(darPage.commits.length,1);assert.equal(darPage.commits[0].source,"dar");assert.equal(darPage.next,2);

    // Turning the app off hides the Git app; Git over HTTPS belongs to Files and keeps working.
    const settings=async()=>await (await fetch(`${base}/api/v1/admin/installedApps`,{headers:session.headers})).json() as {revision:number};
    const toggle=async(enabled:boolean)=>{const revision=(await settings()).revision;assert.equal((await fetch(`${base}/api/v1/admin/installedApps`,{method:"PUT",headers:{...headers,"if-match":`"apps:${revision}"`},body:JSON.stringify({id:"git",enabled,revision})})).status,200);};
    await toggle(false);
    try{await api(`/repositories`,{},404);git(work,["clone","-q",url,"while-off"]);}
    finally{await toggle(true);}
    assert.ok((await api<GitBranches>(`${repoApi}/branches`)).main);
  }finally{db.disconnect();await rm(work,{recursive:true,force:true});}
});
