import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, writeFile, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { database } from "../runtime/database.ts";
import { hashPassword } from "../runtime/auth/password.ts";

const databaseUrl=process.env.DAR_DATABASE_URL;
test("auth/Files module upgrades in place; old backup imports and unknown formats fail closed",{skip:!databaseUrl,timeout:60000},async()=>{
  const project=path.resolve(import.meta.dirname,".."),root=await mkdtemp(path.join(tmpdir(),"dar-module-upgrade-")),old=path.join(root,"old"),issuer=process.env.DAR_INTEGRATION_ORIGIN+"/auth/issuer";
  await mkdir(path.join(old,"src"),{recursive:true});
  // This immutable commit is the independently verified auth/Files baseline.
  const source=execFileSync("git",["show","bf782c0:next/module/src/index.ts"],{cwd:project,encoding:"utf8"});
  await writeFile(path.join(old,"src/index.ts"),source);await writeFile(path.join(old,"package.json"),JSON.stringify({type:"module",dependencies:{spacetimedb:"2.14.1"}}));await symlink(path.join(project,"node_modules"),path.join(old,"node_modules"));
  await writeFile(path.join(old,"tsconfig.json"),execFileSync("git",["show","bf782c0:next/module/tsconfig.json"],{cwd:project}));
  const cli=process.env.DAR_SPACETIME_BIN!,prefix=["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!],env={...process.env,DAR_ISSUER:issuer};
  function run(args:string[]){const result=spawnSync(cli,[...prefix,...args],{env,encoding:"utf8",timeout:30000,maxBuffer:1024*1024});assert.equal(result.status,0,result.stdout+result.stderr);}
  run(["build","--module-path",old]);
  const name=`dar-upgrade-${randomUUID().slice(0,8)}`;
  run(["publish",name,"--js-path",path.join(old,"dist/bundle.js"),"--server",databaseUrl!,"--yes","--no-config"]);
  const db=await database(databaseUrl!,name,process.env.DAR_DATABASE_TOKEN_FILE!),userId=randomUUID(),sessionId=randomUUID(),verifier=await hashPassword("old module proof password");
  await db.call("service_create_account",[userId,"upgrade-user","Upgrade user",verifier,true]);
  await db.call("service_create_session",[sessionId,userId,"upgrade-test-digest","",Date.now()+60000,verifier]);
  const uploadId=randomUUID(),fileId=randomUUID();
  await db.call("service_begin_upload",[sessionId,uploadId,fileId,"","Existing.txt",0,"text/plain",0]);
  await db.call("service_finish_upload",[sessionId,uploadId,"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"]);
  const pin=randomUUID(),snapshot=await db.call<{format:string;files:{id:string}[]}>("service_backup",[pin]);assert.equal(snapshot.format,"dar-backup-v1");await db.call("service_release_backup",[pin]);
  run(["publish",name,"--js-path",path.join(project,"module/dist/bundle.js"),"--server",databaseUrl!,"--yes","--no-config"]);
  const existing=await db.call<{id:string}>("service_download",[sessionId,fileId,""]);assert.equal(existing.id,fileId);
  const created=await db.call<{id:string}>("service_pim",[sessionId,"createCollection",JSON.stringify({id:randomUUID(),kind:"contacts",name:"After upgrade",color:"#2563eb"})]);assert.ok(created.id);
  const restoredName=`dar-old-restore-${randomUUID().slice(0,8)}`;
  run(["publish",restoredName,"--js-path",path.join(project,"module/dist/bundle.js"),"--server",databaseUrl!,"--yes","--no-config"]);
  const restored=await database(databaseUrl!,restoredName,process.env.DAR_DATABASE_TOKEN_FILE!);
  await assert.rejects(restored.call("service_restore",[JSON.stringify({...snapshot,format:"dar-backup-v999"})]));
  await restored.call("service_restore",[JSON.stringify(snapshot)]);
  const account=await restored.call<{id:string;verifier:string}>("service_account",["upgrade-user"]);assert.equal(account.id,userId);assert.equal(account.verifier,verifier);
  const recovered=await restored.call<{format:string;files:{id:string}[];pim:{collections:unknown[]}}>("service_backup",[randomUUID()]);assert.equal(recovered.format,"dar-backup-v4");assert.ok(recovered.files.some(row=>row.id===fileId));assert.equal(recovered.pim.collections.length,0);
});
