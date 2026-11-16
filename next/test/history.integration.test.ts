import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import {mkdtemp,mkdir,writeFile,rm} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {connect,login,upload} from "./integration-helpers.ts";
import type {HistoryOverview,HistoryDetail,HistoryBranch} from "../shared/history.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;

test("folder history: track, commit, rename, restore, retention, quota, export, sharing and agent branches",{skip:!origin,timeout:90_000},async()=>{
  const base=origin!,session=await login(base),db=await connect(session),headers={...session.headers,"content-type":"application/json"};
  const route=(folderId:string,rest="")=>`${base}/api/v1/files/history/${folderId}${rest}`;
  const overview=async(folderId:string,who=session)=>{const response=await fetch(route(folderId),{headers:who.headers});assert.equal(response.status,200,await response.clone().text());return await response.json() as HistoryOverview;};
  const post=(folderId:string,rest:string,value:unknown,who=headers)=>fetch(route(folderId,rest),{method:"POST",headers:who,body:JSON.stringify(value)});
  const used=()=>[...db.db.myUsage.iter()][0].used;
  const text=async(url:string)=>{const response=await fetch(url,{headers:session.headers});assert.equal(response.status,200,url);return response.text();};
  try{
    const folderId=randomUUID(),childId=randomUUID();
    await db.reducers.createFolder({id:folderId,parentId:"",name:`History ${randomUUID()}`});
    await db.reducers.createFolder({id:childId,parentId:folderId,name:"docs"});
    const readme=await upload(base,session,"README.md","# One\n",folderId),notes=await upload(base,session,"notes.txt","first\n",childId);

    let state=await overview(folderId);assert.equal(state.tracked,false);assert.equal(state.owner,true);
    assert.equal((await post(folderId,"",{action:"track"})).status,200);
    assert.equal((await post(childId,"",{action:"track"})).status,403,"a folder inside a tracked folder cannot be tracked");
    state=await overview(folderId);
    assert.deepEqual(state.status!.changes.map(change=>[change.kind,change.path]),[["added","README.md"],["added","docs/notes.txt"]]);
    const before=used();
    assert.equal((await post(folderId,"/commits",{message:"",token:state.status!.token})).status,400,"a commit needs a message");
    assert.equal((await post(folderId,"/commits",{message:"Initial",token:"0000000000000000"})).status,409,"a stale status is a conflict");
    const initial=await post(folderId,"/commits",{message:"Initial",token:state.status!.token});assert.equal(initial.status,201,await initial.clone().text());
    assert.equal(used(),before,"committing adds no bytes");
    state=await overview(folderId);assert.equal(state.status!.changes.length,0);assert.equal(state.changesets[0].message,"Initial");
    assert.equal((await post(folderId,"/commits",{snapshot:true,token:state.status!.token})).status,409,"nothing to commit");

    // Modify, rename and add; commit only part of it.
    await upload(base,session,"README.md","# Two\n",folderId,readme.fileId,1);
    await db.reducers.moveEntry({id:notes.fileId,parentId:folderId,expectedRevision:1,name:"notes.md"});
    await upload(base,session,"extra.txt","extra\n",folderId);
    state=await overview(folderId);
    assert.deepEqual(state.status!.changes.map(change=>[change.kind,change.path,change.previousPath??""]),[["modified","README.md",""],["added","extra.txt",""],["renamed","notes.md","docs/notes.txt"]]);
    const partial=await post(folderId,"/commits",{message:"Edit readme",token:state.status!.token,paths:["README.md","notes.md"]});assert.equal(partial.status,201);
    state=await overview(folderId);assert.deepEqual(state.status!.changes.map(change=>change.path),["extra.txt"]);
    const detail=await(await fetch(route(folderId,`/commits/${state.changesets[0].id}`),{headers:session.headers})).json() as HistoryDetail;
    const modified=detail.changes.find(change=>change.kind==="modified")!;
    assert.equal(await text(route(folderId,`/versions/${modified.previousVersionId}/content`)),"# One\n");
    assert.equal(await text(route(folderId,`/versions/${modified.versionId}/content`)),"# Two\n");
    assert.equal((await fetch(route(folderId,`/versions/${randomUUID()}/content`),{headers:session.headers})).status,404);

    // Restore needs a clean folder, then brings back the first state and commits it.
    const initialId=state.changesets.at(-1)!.id;
    assert.equal((await post(folderId,"/restore",{id:initialId,revision:state.repo!.revision})).status,409);
    assert.equal((await post(folderId,"/commits",{snapshot:true,token:state.status!.token})).status,201);
    state=await overview(folderId);
    const restored=await post(folderId,"/restore",{id:initialId,revision:state.repo!.revision});assert.equal(restored.status,201,await restored.clone().text());
    state=await overview(folderId);assert.equal(state.changesets[0].kind,"restore");assert.equal(state.status!.changes.length,0);
    assert.equal(await text(`${base}/api/v1/files/${readme.fileId}/content`),"# One\n");
    await db.reducers.browse({parentId:childId,section:"files",query:"",cursor:""});
    const back=[...db.db.myFiles.iter()].find(row=>row.name==="notes.txt"&&row.parentId===childId)!;assert.equal(await text(`${base}/api/v1/files/${back.id}/content`),"first\n");

    // Deleting a file for good keeps the versions history uses, still counted.
    await db.reducers.browse({parentId:folderId,section:"files",query:"",cursor:""});
    const extra=[...db.db.myFiles.iter()].find(row=>row.name==="extra.txt"&&row.parentId===folderId);
    assert.equal(extra,undefined,"restore moved the later file to deleted files");
    await db.reducers.browse({parentId:"",section:"trash",query:"extra.txt",cursor:""});
    const trashed=[...db.db.myFiles.iter()].find(row=>row.name==="extra.txt")!;
    const kept=used();await db.reducers.deleteEntry({id:trashed.id,expectedRevision:trashed.revision});assert.equal(used(),kept);
    const snapshotDetail=await(await fetch(route(folderId,`/commits/${state.changesets[1].id}`),{headers:session.headers})).json() as HistoryDetail;
    assert.equal(await text(route(folderId,`/versions/${snapshotDetail.changes.find(change=>change.path==="extra.txt")!.versionId}/content`)),"extra\n");

    // The Git bundle clones into the same history.
    const bundle=await fetch(route(folderId,"/bundle"),{headers:session.headers});assert.equal(bundle.status,200);
    const work=await mkdtemp(path.join(tmpdir(),"dar-history-test-"));
    try{
      await writeFile(path.join(work,"history.bundle"),Buffer.from(await bundle.arrayBuffer()));
      const env={...process.env,GIT_CONFIG_GLOBAL:"/dev/null",GIT_CONFIG_NOSYSTEM:"1",HOME:work};
      assert.equal(spawnSync("git",["clone","-q","history.bundle","clone"],{cwd:work,env}).status,0);
      const log=spawnSync("git",["log","--format=%s"],{cwd:path.join(work,"clone"),env,encoding:"utf8"}).stdout.trim().split("\n");
      assert.deepEqual(log,["Restore to “Initial”","Snapshot","Edit readme","Initial"]);
      assert.equal(spawnSync("git",["show","HEAD~1:extra.txt"],{cwd:path.join(work,"clone"),env,encoding:"utf8"}).stdout,"extra\n");
    }finally{await rm(work,{recursive:true,force:true});}

    // Readers see history but cannot change it.
    const username=`history-${randomUUID().slice(0,8)}`;
    assert.equal((await fetch(`${base}/api/v1/auth/users`,{method:"POST",headers,body:JSON.stringify({username,displayName:"History reader",password:"a long isolated test password"})})).status,201);
    const reader=await login(base,username),readerHeaders={...reader.headers,"content-type":"application/json"};
    assert.equal((await fetch(route(folderId),{headers:reader.headers})).status,404);
    const folder=[...db.db.myFiles.iter()].find(row=>row.id===folderId)??(await db.reducers.browse({parentId:"",section:"files",query:"",cursor:""}),[...db.db.myFiles.iter()].find(row=>row.id===folderId))!;
    await db.reducers.grantAccess({id:randomUUID(),fileId:folderId,username,role:"read",expectedRevision:folder.revision});
    const seen=await overview(folderId,reader);assert.equal(seen.canWrite,false);assert.equal(seen.changesets.length,4);
    assert.equal((await post(folderId,"/commits",{snapshot:true,token:seen.status!.token},readerHeaders)).status,403);

    // An Agent thread on the folder proposes a branch; merge applies and commits it.
    const projectId=randomUUID(),threadId=randomUUID(),agent=(rest:string,value?:unknown)=>fetch(`${base}/api/v1/agent${rest}`,{method:value===undefined?"GET":"POST",headers,...value===undefined?{}:{body:JSON.stringify(value)}});
    assert.equal((await agent("/projects",{id:projectId,name:"History project",sourceType:"files",sourceRef:folderId})).status,201);
    assert.equal((await agent("/threads",{id:threadId,projectId,title:"Tidy notes"})).status,201);
    assert.equal((await agent(`/threads/${threadId}/branch`)).status,200);
    const me=[...db.db.me.iter()][0].id,thread=path.join(process.env.DAR_TEST_ROOT!,"agent-workspaces",me,threadId);
    const current=await overview(folderId);await db.reducers.browse({parentId:folderId,section:"files",query:"",cursor:""});
    const readmeRow=[...db.db.myFiles.iter()].find(row=>row.id===readme.fileId)!;
    await mkdir(path.join(thread,"work","docs"),{recursive:true});
    await writeFile(path.join(thread,"work","README.md"),"# Agent\n");await writeFile(path.join(thread,"work","docs","new.md"),"added by agent\n");await writeFile(path.join(thread,"work","docs","notes.txt"),"first\n");
    await writeFile(path.join(thread,"baseline.json"),JSON.stringify({source:{project:{id:projectId,ownerId:me,name:"History project",sourceType:"files",sourceRef:folderId,createdAt:0},files:[{id:readme.fileId,path:"README.md",revision:readmeRow.revision,size:6,mediaType:"text/plain"},{id:back.id,path:"docs/notes.txt",revision:back.revision,size:6,mediaType:"text/plain"}]},
      files:[{path:"README.md",content:Buffer.from("# One\n").toString("base64")},{path:"docs/notes.txt",content:Buffer.from("first\n").toString("base64")}]}));
    const staged=await agent(`/threads/${threadId}/branch`,{});assert.equal(staged.status,201,await staged.clone().text());
    const branch=(await staged.json() as {branch:HistoryBranch}).branch;
    assert.deepEqual(branch.changes.map(change=>[change.kind,change.path]),[["modified","README.md"],["added","docs/new.md"]]);
    assert.ok(used()>kept,"staged content counts toward quota");
    state=await overview(folderId);assert.equal(state.branches.length,1);assert.equal(state.status!.changes.length,0,"a branch leaves the folder unchanged");
    assert.equal(state.repo!.revision,current.repo!.revision+1);
    const merged=await post(folderId,"/branches/merge",{name:branch.name,revision:state.repo!.revision});assert.equal(merged.status,200,await merged.clone().text());
    assert.equal((await merged.json() as {merged:boolean}).merged,true);
    assert.equal(await text(`${base}/api/v1/files/${readme.fileId}/content`),"# Agent\n");
    state=await overview(folderId);assert.equal(state.branches.length,0);assert.equal(state.changesets[0].kind,"merge");assert.equal(state.status!.changes.length,0);

    // A second proposal against a changed file conflicts; discarding releases its staged bytes.
    const again=await agent(`/threads/${threadId}/branch`,{});assert.equal(again.status,201);
    state=await overview(folderId);
    const conflict=await(await post(folderId,"/branches/merge",{name:branch.name,revision:state.repo!.revision})).json() as {merged:boolean;conflicts:string[]};
    assert.equal(conflict.merged,false);assert.ok(conflict.conflicts.includes("README.md"));
    const withBranch=used();
    assert.equal((await post(folderId,"/branches/discard",{name:branch.name,revision:state.repo!.revision})).status,200);
    assert.ok(used()<withBranch);

    // Stopping history keeps live files' versions and releases what only history held.
    state=await overview(folderId);const tracked=used();
    assert.equal((await post(folderId,"",{action:"untrack",revision:state.repo!.revision},readerHeaders)).status,403);
    assert.equal((await post(folderId,"",{action:"untrack",revision:state.repo!.revision})).status,200);
    assert.equal(used(),tracked-Buffer.byteLength("extra\n"));
    assert.equal((await overview(folderId)).tracked,false);
  }finally{db.disconnect();}
});
