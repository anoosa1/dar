import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,mkdir,readFile,writeFile,rm,symlink,lstat} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createAgentRuntime} from "../runtime/agent/runtime.ts";
import {codexSearchDirectories,findCodexExecutable} from "../runtime/agent/discovery.ts";
import {prepareWorkspace,type Source} from "../runtime/agent/workspace.ts";
import type {Database} from "../runtime/database.ts";

test("cached workspaces repair a missing Codex deny root before sandbox startup",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-cached-workspace-")),owner=randomUUID(),thread=randomUUID(),directory=path.join(root,"agent-workspaces",owner,thread),workspace=path.join(directory,"work");
  const source:Source={project:{id:randomUUID(),ownerId:owner,name:"Fixture",sourceType:"files",sourceRef:randomUUID(),createdAt:0},files:[]};
  try{
    await mkdir(workspace,{recursive:true});await writeFile(path.join(workspace,"keep.txt"),"Retain existing work");
    await writeFile(path.join(directory,"baseline.json"),JSON.stringify({source,files:[]}));
    await prepareWorkspace(root,owner,thread,source,{executable:"/unused",home:"/unused",userId:owner,gitHosts:[]},{} as Database,"fixture",new AbortController().signal);
    assert.equal((await lstat(path.join(workspace,".codex"))).isDirectory(),true);
    assert.equal(await readFile(path.join(workspace,"keep.txt"),"utf8"),"Retain existing work");
  }finally{await rm(root,{recursive:true,force:true});}
});

test("Codex discovery includes NixOS profiles and skips unavailable or non-executable candidates",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-codex-discovery-"));
  try{
    const directories=codexSearchDirectories("/service/bin:relative:/service/bin",root);
    assert.equal(directories[0],"/service/bin");assert.equal(directories.filter(d=>d==="/service/bin").length,1);
    assert.ok(directories.includes("/run/current-system/sw/bin"));assert.ok(directories.includes(path.join(root,".nix-profile/bin")));assert.ok(!directories.includes("relative"));
    const blocked=path.join(root,"blocked"),profile=path.join(root,"profile"),target=path.join(root,"installed-codex");
    await mkdir(blocked);await mkdir(profile);await writeFile(path.join(blocked,"codex"),"",{mode:0o600});
    await writeFile(target,"#!/bin/sh\nexit 0\n",{mode:0o700});await symlink(target,path.join(profile,"codex"));
    assert.equal(await findCodexExecutable([path.join(root,"missing"),blocked,profile]),path.join(profile,"codex"));
    assert.equal(await findCodexExecutable([blocked]),undefined);
  }finally{await rm(root,{recursive:true,force:true});}
});

test("connecting the discovered CLI binds its existing login without replacing an account binding",async()=>{
  const root=await mkdtemp(path.join(tmpdir(),"dar-agent-connection-")),home=path.join(root,"home"),bin=path.join(root,"bin"),filename=path.join(root,"codex.json");
  await mkdir(home);await mkdir(bin);await writeFile(path.join(bin,"codex"),await readFile(new URL("fixtures/codex-fixture.mjs",import.meta.url)),{mode:0o700});
  const previousPath=process.env.PATH,previousHome=process.env.CODEX_HOME;
  process.env.PATH=bin+path.delimiter+previousPath;process.env.CODEX_HOME=home;
  const db={call:async()=>undefined} as unknown as Database,runtime=await createAgentRuntime(db,root,filename),owner=randomUUID();
  try{
    assert.equal((await runtime.status(owner)).configured,false);
    assert.equal((await runtime.connect(owner)).connected,true);
    const binding=JSON.parse(await readFile(filename,"utf8"));assert.equal(binding.userId,owner);assert.equal(binding.home,home);assert.equal(binding.executable,path.join(bin,"codex"));
    assert.equal((await runtime.connect(owner)).connected,true);
    await assert.rejects(runtime.connect(randomUUID()),/already linked/);
    assert.equal(JSON.parse(await readFile(filename,"utf8")).userId,owner);
  }finally{await runtime.close();if(previousPath===undefined)delete process.env.PATH;else process.env.PATH=previousPath;if(previousHome===undefined)delete process.env.CODEX_HOME;else process.env.CODEX_HOME=previousHome;await rm(root,{recursive:true,force:true});}
});
