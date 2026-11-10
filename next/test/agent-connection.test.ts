import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,mkdir,readFile,writeFile,rm} from "node:fs/promises";
import {tmpdir} from "node:os";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createAgentRuntime} from "../runtime/agent/runtime.ts";
import type {Database} from "../runtime/database.ts";

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
