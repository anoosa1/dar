import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,mkdir,writeFile,readFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync,execFile} from "node:child_process";
import {promisify} from "node:util";
import {startServer} from "../runtime/server.ts";
import {database} from "../runtime/database.ts";
import {login,connect,upload} from "./integration-helpers.ts";
import type {AgentRun,AgentHistory,AgentProvider} from "../shared/agent.ts";

test("direct Codex CLI preserves login, run state, cancellation, isolation and Files revisions",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:120000},async()=>{
 const root=await mkdtemp("/tmp/dar-codex-acceptance-"),name="dar-codex-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
 const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
 const executable=path.join(root,"codex-fixture"),home=path.join(root,"codex-home");await mkdir(home,{mode:0o700});await writeFile(executable,await readFile(new URL("./fixtures/codex-fixture.mjs",import.meta.url)),{mode:0o700});
 const codexFile=path.join(root,"codex.json");await writeFile(codexFile,JSON.stringify({executable,home,userId:randomUUID(),gitHosts:[]}));
 const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,codexFile};
 let runtime=await startServer(config);
 try{
  const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Codex fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
  const account=await setup.json() as {user:{id:string}};await writeFile(codexFile,JSON.stringify({executable,home,userId:account.user.id,gitHosts:[]}));await runtime.close();runtime=await startServer(config);
  const session=await login(base),headers={...session.headers,"content-type":"application/json"};
  const call=async<T>(route:string,value?:unknown,match?:number):Promise<T>=>{const response=await fetch(base+"/api/v1/agent"+route,{headers:{...headers,...match!==undefined?{"if-match":`"${match}"`}:{}},...value!==undefined?{method:"POST",body:JSON.stringify(value)}:{}});assert.ok(response.ok,await response.clone().text());return response.json() as Promise<T>;};
  const provider=await call<AgentProvider>("/provider");assert.equal(provider.connected,true);assert.equal(provider.models[0].id,"fixture-model");assert.ok(!JSON.stringify(provider).includes("fixture@example.test"));
  const folderId=randomUUID(),connection=await connect(session);try{await connection.reducers.createFolder({id:folderId,parentId:"",name:"Source"});}finally{connection.disconnect();}
  const file=await upload(base,session,"greeting.txt","Original source.\n",folderId),projectId=randomUUID();await call("/projects",{id:projectId,name:"Files code",sourceType:"files",sourceRef:folderId});
  async function thread(title:string){const id=randomUUID();await call("/threads",{id,projectId,title});return id;}
  async function wait(id:string,expected:string,ready?:(run:AgentRun)=>boolean){const deadline=Date.now()+15000;let result:AgentRun;do{result=await call<AgentRun>(`/runs/${id}`);if(result.status===expected&&(!ready||ready(result)))return result;if(["failed","interrupted","cancelled"].includes(result.status)&&result.status!==expected)throw Error(JSON.stringify(result));await new Promise(r=>setTimeout(r,100));}while(Date.now()<deadline);throw Error(`Run did not become ${expected}: ${JSON.stringify(result!)}`);}
  const threadId=await thread("Edit source"),runRequest={id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"Update the greeting."};
  const run=await call<AgentRun>(`/threads/${threadId}/runs`,runRequest,1);assert.equal(run.status,"queued");assert.equal((await call<AgentRun>(`/threads/${threadId}/runs`,runRequest,1)).id,run.id);
  const done=await wait(run.id,"completed");assert.equal(done.document.output,"The edit is ready.");assert.ok(done.document.terminal.includes("fixture check passed"));assert.equal(done.document.changes.length,1);assert.equal(done.document.changes[0].fileId,file.fileId);assert.equal(done.document.changes[0].content,"Updated by Codex fixture.\n");
  const history=await call<AgentHistory>(`/threads/${threadId}/messages`);assert.deepEqual(history.items.map(m=>m.role),["user","assistant"]);assert.equal(history.items.at(-1)!.content,"The edit is ready.");
  assert.equal(await(await fetch(`${base}/api/v1/files/${file.fileId}/content`,{headers})).text(),"Original source.\n");
  const change=done.document.changes[0];await upload(base,session,"greeting.txt",change.content!,folderId,file.fileId,change.expectedRevision);assert.equal(await(await fetch(`${base}/api/v1/files/${file.fileId}/content`,{headers})).text(),"Updated by Codex fixture.\n");
  assert.equal(change.mediaType,"text/plain");
  const stale=await fetch(base+"/api/v1/files/uploads",{method:"POST",headers,body:JSON.stringify({fileId:file.fileId,expectedRevision:1,name:"greeting.txt",size:3,mediaType:"text/plain"})});assert.equal(stale.status,409);
  const waitingId=await thread("Cancellation"),waiting=await call<AgentRun>(`/threads/${waitingId}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"WAIT_FOR_CANCEL"},1);await wait(waiting.id,"running");
  const competing=await fetch(`${base}/api/v1/agent/threads/${threadId}/runs`,{method:"POST",headers:{...headers,"if-match":`"${history.thread.revision}"`},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"Concurrent run"})});assert.equal(competing.status,409);
  await call(`/runs/${waiting.id}/cancel`,{});await wait(waiting.id,"cancelled");
  const revokedId=await thread("Source access ended"),revoked=await call<AgentRun>(`/threads/${revokedId}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"WAIT_FOR_CANCEL"},1);await wait(revoked.id,"running",row=>row.document.activity.includes("fixture waiting for cancellation"));
  const sourceConnection=await connect(session);try{const revision=[...sourceConnection.db.myFiles.iter()].find(row=>row.id===folderId)!.revision;await sourceConnection.reducers.trashEntry({id:folderId,expectedRevision:revision});await wait(revoked.id,"interrupted");await sourceConnection.reducers.restoreEntry({id:folderId,expectedRevision:revision+1});}finally{sourceConnection.disconnect();}
  for(const content of ["FAIL_CLI","EXIT_WITHOUT_TURN"]){const failedId=await thread(content),failed=await call<AgentRun>(`/threads/${failedId}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content},1);await wait(failed.id,"failed");assert.equal((await call<AgentHistory>(`/threads/${failedId}/messages`)).items.length,1);}
  const restartId=await thread("Interrupted"),restart=await call<AgentRun>(`/threads/${restartId}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"WAIT_FOR_CANCEL"},1);await wait(restart.id,"running");
  await runtime.close();runtime=await startServer(config);await wait(restart.id,"interrupted");assert.equal((await call<AgentHistory>(`/threads/${restartId}/messages`)).items.length,1);
  const db=await database(process.env.DAR_DATABASE_URL!,name,process.env.DAR_DATABASE_TOKEN_FILE!);await assert.rejects(db.call("service_agent_worker",[run.id,randomUUID(),"finish",JSON.stringify({status:"completed",document:done.document})]),/conflict/);
  const username="other";assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username,displayName:"Other",password:"a long isolated test password"})})).status,201);const other=await login(base,username);
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${run.id}`,{headers:other.headers})).status,404);assert.equal((await(await fetch(base+"/api/v1/agent/provider",{headers:other.headers})).json() as AgentProvider).configured,false);
  const browser=await promisify(execFile)(process.execPath,["test/agent-codex-browser.mjs"],{cwd:path.resolve(import.meta.dirname,".."),env:{...process.env,DAR_INTEGRATION_ORIGIN:base,DAR_TEST_ROOT:root,DAR_CODEX_PROJECT:projectId},encoding:"utf8",timeout:45000});console.log(browser.stdout.trim());
 }finally{await runtime.close();}
});
