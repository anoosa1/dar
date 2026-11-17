import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,writeFile,rm,symlink,chmod,readFile,readlink,stat} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import type {HistoryOverview} from "../shared/history.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

test("git: clone, fetch and push a tracked folder over HTTPS",{skip:!origin,timeout:240_000},async()=>{
  const base=origin!,session=await login(base),db=await connect(session),headers={...session.headers,"content-type":"application/json"};
  const work=await mkdtemp(path.join(tmpdir(),"dar-git-"));
  const history=(folderId:string,rest="")=>`${base}/api/v1/files/history/${folderId}${rest}`;
  const overview=async(folderId:string)=>{const response=await fetch(history(folderId),{headers:session.headers});assert.equal(response.status,200);return await response.json() as HistoryOverview;};
  const password=async(scope:string)=>{const response=await fetch(`${base}/api/v1/auth/app-passwords`,{method:"POST",headers,body:JSON.stringify({label:`Git ${scope}`,scope})});assert.equal(response.status,201);return (await response.json() as {secret:string}).secret;};
  const used=()=>[...db.db.myUsage.iter()][0].used;
  // Subscription updates arrive after a blocking git command returns.
  const until=async(check:()=>boolean,message:string)=>{for(let i=0;i<100;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,50));}assert.fail(message);};
  const live=async(fileId:string)=>{const response=await fetch(`${base}/api/v1/files/${fileId}/content`,{headers:session.headers});assert.equal(response.status,200);return response.text();};
  const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work,GIT_TERMINAL_PROMPT:"0",GIT_AUTHOR_NAME:"Ada Lovelace",GIT_AUTHOR_EMAIL:"ada@example.com",GIT_COMMITTER_NAME:"Ada Lovelace",GIT_COMMITTER_EMAIL:"ada@example.com"};
  const run=(cwd:string,secret:string,args:string[],ok=true)=>{
    const result=spawnSync("git",["-c",`http.extraHeader=Authorization: Basic ${Buffer.from(`admin:${secret}`).toString("base64")}`,"-c","init.defaultBranch=main",...args],{cwd,env,encoding:"utf8"});
    if(ok)assert.equal(result.status,0,`git ${args.join(" ")}\n${result.stderr}`);else assert.notEqual(result.status,0,`git ${args.join(" ")} should fail`);
    return {out:result.stdout.trim(),err:result.stderr};
  };
  try{
    const folderId=randomUUID(),docsId=randomUUID(),url=`${base}/dav/git/${folderId}.git`;
    await db.reducers.createFolder({id:folderId,parentId:"",name:`Git ${randomUUID().slice(0,6)}`});
    await db.reducers.createFolder({id:docsId,parentId:folderId,name:"docs"});
    const readme=await upload(base,session,"README.md","# Project\n",folderId),notes=await upload(base,session,"a.txt","alpha\n",docsId);
    assert.equal((await fetch(history(folderId),{method:"POST",headers,body:JSON.stringify({action:"track"})})).status,200);
    let state=await overview(folderId);
    assert.equal((await fetch(history(folderId,"/commits"),{method:"POST",headers,body:JSON.stringify({message:"Initial",token:state.status!.token})})).status,201);
    const write=await password("write"),read=await password("read");

    // Git signs in with Basic authentication: no credentials asks for them.
    const anonymous=await fetch(`${url}/info/refs?service=git-upload-pack`);
    assert.equal(anonymous.status,401);assert.match(anonymous.headers.get("www-authenticate")??"",/^Basic /);
    assert.equal((await fetch(`${url}/info/refs`,{headers:{authorization:`Basic ${Buffer.from(`admin:${read}`).toString("base64")}`}})).status,403,"dumb HTTP is refused");

    // Clone: Dar's commit becomes a Git commit, the same one every time.
    run(work,read,["clone","-q",url,"one"]);const one=path.join(work,"one");
    assert.equal(await readFile(path.join(one,"README.md"),"utf8"),"# Project\n");
    assert.equal(run(one,read,["log","-1","--format=%s|%an"]).out,"Initial|Test Administrator");
    const initial=run(one,read,["rev-parse","HEAD"]).out;
    run(work,read,["-c","protocol.version=0","clone","-q",url,"two"]);const two=path.join(work,"two");
    assert.equal(run(two,read,["rev-parse","HEAD"]).out,initial,"protocol v0 clones the same commit");
    state=await overview(folderId);assert.equal(state.changesets[0].sha,initial,"the History tab shows the Git ID");

    // Push: modes, a symbolic link and a rename reach Dar; the folder changes with them.
    await writeFile(path.join(one,"README.md"),"# Project\n\nFrom Git\n");
    await writeFile(path.join(one,"run.sh"),"#!/bin/sh\necho hi\n");await chmod(path.join(one,"run.sh"),0o755);
    await symlink("README.md",path.join(one,"link"));
    run(one,write,["mv","docs/a.txt","docs/b.txt"]);run(one,write,["add","-A"]);run(one,write,["commit","-q","-m","Edit from Git\n\nWith a body."]);
    const before=used(),pushed=run(one,write,["push","origin","main"]);
    assert.match(pushed.err,/the folder now matches main/);
    const edited=run(one,write,["rev-parse","HEAD"]).out;
    assert.equal(await live(readme.fileId),"# Project\n\nFrom Git\n");
    state=await overview(folderId);
    assert.equal(state.changesets[0].sha,edited);assert.equal(state.changesets[0].author,"Ada Lovelace");assert.equal(state.changesets[0].message,"Edit from Git\n\nWith a body.");
    assert.equal(state.status!.changes.length,0,"the folder matches main after a push");
    const detail=await (await fetch(history(folderId,`/commits/${state.changesets[0].id}`),{headers:session.headers})).json() as {changes:{kind:string;path:string;previousPath?:string;fileId?:string}[]};
    assert.deepEqual(detail.changes.map(change=>[change.kind,change.path]),[["modified","README.md"],["renamed","docs/b.txt"],["added","link"],["added","run.sh"]]);
    assert.equal(detail.changes[1].fileId,notes.fileId,"a renamed file keeps its ID");assert.equal(await live(notes.fileId),"alpha\n");
    await until(()=>used()>before,"pushed content counts against the quota");
    run(one,read,["push","origin","main"],false);

    // A commit made in Dar arrives with a fetch; modes and the link survive the round trip.
    await upload(base,session,"README.md","# Project\n\nFrom Dar\n",folderId,readme.fileId,2);
    state=await overview(folderId);
    assert.equal((await fetch(history(folderId,"/commits"),{method:"POST",headers,body:JSON.stringify({message:"Edit in Dar",token:state.status!.token})})).status,201);
    run(two,read,["pull","-q","--ff-only"]);
    assert.equal(run(two,read,["log","-1","--format=%s"]).out,"Edit in Dar");
    assert.equal(run(two,read,["rev-parse","HEAD~1"]).out,edited);
    assert.match(run(two,read,["ls-tree","HEAD","run.sh","link"]).out,/^120000 blob .*\tlink\n100755 blob .*\trun\.sh$/);
    assert.equal(await readlink(path.join(two,"link")),"README.md");
    assert.equal((await stat(path.join(two,"run.sh"))).mode&0o111,0o111);
    const fromDar=run(two,read,["rev-parse","HEAD"]).out;

    // The repository is only a cache: rebuilt from the database, every ID is the same.
    await rm(path.join(process.env.DAR_TEST_ROOT!,"git",`${folderId}.git`),{recursive:true,force:true});
    run(work,read,["clone","-q","--mirror",url,"three.git"]);
    assert.equal(run(path.join(work,"three.git"),read,["rev-parse","main"]).out,fromDar);
    run(path.join(work,"three.git"),read,["fsck","--strict","--no-dangling"]);

    // Branches and tags are kept without touching the folder; deleting one releases its content.
    run(one,write,["pull","-q","--ff-only"]);
    run(one,write,["checkout","-q","-b","feature"]);await writeFile(path.join(one,"feature.bin"),Buffer.alloc(4096,7));
    run(one,write,["add","-A"]);run(one,write,["commit","-q","-m","Feature work"]);
    await new Promise(resolve=>setTimeout(resolve,500));const quota=used();run(one,write,["push","-q","origin","feature"]);
    await until(()=>used()>=quota+4096,"branch content counts");
    run(one,write,["tag","-a","v1","-m","Version 1","main"]);run(one,write,["push","-q","origin","v1"]);
    state=await overview(folderId);
    assert.deepEqual(state.gitRefs.map(ref=>ref.name),["refs/heads/feature","refs/tags/v1"]);
    assert.equal(state.status!.changes.length,0);
    const remote=run(two,read,["ls-remote",url]).out;
    assert.match(remote,/refs\/heads\/feature/);assert.match(remote,/refs\/tags\/v1\^\{\}/);
    run(one,write,["push","-q","origin","--delete","feature"]);
    await until(()=>used()<=quota,"a deleted branch's content is released");
    run(one,write,["push","origin","--delete","main"],false);

    // Like Git: a stale main is refused, and so is a push over uncommitted Dar changes.
    run(one,write,["checkout","-q","main"]);
    await writeFile(path.join(two,"stale.txt"),"stale\n");run(two,write,["add","-A"]);run(two,write,["commit","-q","-m","Stale"]);
    run(one,write,["commit","-q","--allow-empty","-m","Ahead"]);run(one,write,["push","-q","origin","main"]);
    assert.match(run(two,write,["push","origin","main"],false).err,/rejected/);
    await upload(base,session,"README.md","# Dirty in Dar\n",folderId,readme.fileId,3);
    await writeFile(path.join(one,"README.md"),"# Conflicting\n");run(one,write,["commit","-qam","Conflict"]);
    assert.match(run(one,write,["push","origin","main"],false).err,/uncommitted changes in Dar/);
    run(one,write,["reset","-q","--hard","origin/main"]);
    await writeFile(path.join(one,"docs","b.txt"),"beta\n");run(one,write,["commit","-qam","Elsewhere"]);
    run(one,write,["push","-q","origin","main"]);
    state=await overview(folderId);
    assert.deepEqual(state.status!.changes.map(change=>[change.kind,change.path]),[["modified","README.md"]],"Dar's own edit stays uncommitted");
    assert.equal(await live(notes.fileId),"beta\n");

    // A merge commit, and a shallow clone.
    run(one,write,["checkout","-q","-b","side"]);await writeFile(path.join(one,"side.txt"),"side\n");run(one,write,["add","-A"]);run(one,write,["commit","-q","-m","Side"]);
    run(one,write,["checkout","-q","main"]);await writeFile(path.join(one,"main.txt"),"main\n");run(one,write,["add","-A"]);run(one,write,["commit","-q","-m","Main"]);
    run(one,write,["merge","-q","--no-ff","-m","Merge side","side"]);run(one,write,["push","-q","origin","main"]);
    state=await overview(folderId);assert.equal(state.changesets[0].kind,"merge");assert.equal(state.changesets[0].sha,run(one,read,["rev-parse","HEAD"]).out);
    run(work,read,["clone","-q","--depth","1",url,"shallow"]);
    assert.equal(run(path.join(work,"shallow"),read,["rev-parse","HEAD"]).out,state.changesets[0].sha);

    // The History tab's bundle has the same commits as a clone.
    const bundle=await fetch(history(folderId,"/bundle"),{headers:session.headers});assert.equal(bundle.status,200);
    await writeFile(path.join(work,"history.bundle"),Buffer.from(await bundle.arrayBuffer()));
    run(work,read,["clone","-q",path.join(work,"history.bundle"),"four"]);
    assert.equal(run(path.join(work,"four"),read,["rev-parse","HEAD"]).out,state.changesets[0].sha);
  }finally{db.disconnect();await rm(work,{recursive:true,force:true});}
});
