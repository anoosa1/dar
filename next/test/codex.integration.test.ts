import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,mkdir,writeFile,readFile,unlink,stat} from "node:fs/promises";
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
 const codexFile=path.join(root,"codex.json");
 const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,codexFile};
 let runtime=await startServer(config);
 try{
  const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Codex fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
  const account=await setup.json() as {user:{id:string}};
  const session=await login(base),headers={...session.headers,"content-type":"application/json"};
  const call=async<T>(route:string,value?:unknown,match?:number):Promise<T>=>{const response=await fetch(base+"/api/v1/agent"+route,{headers:{...headers,...match!==undefined?{"if-match":`"${match}"`}:{}},...value!==undefined?{method:"POST",body:JSON.stringify(value)}:{}});assert.ok(response.ok,await response.clone().text());return response.json() as Promise<T>;};
  assert.equal((await call<AgentProvider>("/provider")).configured,false);
  await writeFile(codexFile,JSON.stringify({executable,home,userId:account.user.id,gitHosts:[]}));
  const provider=await call<AgentProvider>("/provider");assert.equal(provider.connected,true);assert.equal(provider.models[0].id,"fixture-model");assert.ok(!JSON.stringify(provider).includes("fixture@example.test"));
  await writeFile(path.join(home,"logged-out"),"");assert.equal((await call<AgentProvider>("/provider")).connected,false);
  await unlink(path.join(home,"logged-out"));assert.equal((await call<AgentProvider>("/provider")).connected,true);
  await writeFile(codexFile,"invalid JSON");assert.equal((await call<AgentProvider>("/provider")).configured,false);
  await writeFile(codexFile,JSON.stringify({executable,home,userId:account.user.id,gitHosts:[]}));assert.equal((await call<AgentProvider>("/provider")).connected,true);
  const folderId=randomUUID(),connection=await connect(session);try{await connection.reducers.createFolder({id:folderId,parentId:"",name:"Source"});}finally{connection.disconnect();}
  const file=await upload(base,session,"greeting.txt","Original source.\n",folderId),projectId=randomUUID();await call("/projects",{id:projectId,name:"Files code",sourceType:"files",sourceRef:folderId});
  async function thread(title:string){const id=randomUUID();await call("/threads",{id,projectId,title});return id;}
  async function wait(id:string,expected:string,ready?:(run:AgentRun)=>boolean){const deadline=Date.now()+15000;let result:AgentRun;do{result=await call<AgentRun>(`/runs/${id}`);if(result.status===expected&&(!ready||ready(result)))return result;if(["failed","interrupted","cancelled"].includes(result.status)&&result.status!==expected)throw Error(JSON.stringify(result));await new Promise(r=>setTimeout(r,100));}while(Date.now()<deadline);throw Error(`Run did not become ${expected}: ${JSON.stringify(result!)}`);}
  const threadId=await thread("Edit source"),runRequest={id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"Update the greeting."};
  const run=await call<AgentRun>(`/threads/${threadId}/runs`,runRequest,1);assert.equal(run.status,"queued");assert.equal((await call<AgentRun>(`/threads/${threadId}/runs`,runRequest,1)).id,run.id);
  const done=await wait(run.id,"completed");assert.equal(done.document.output,"The edit is ready.");assert.ok(done.document.terminal.includes("fixture check passed"));assert.equal(done.document.changes.length,1);assert.equal(done.document.changes[0].fileId,file.fileId);assert.equal(done.document.changes[0].content,"Updated by Codex fixture.\n");
  const history=await call<AgentHistory>(`/threads/${threadId}/messages`);assert.deepEqual(history.items.map(m=>m.role),["user","assistant"]);assert.equal(history.items.at(-1)!.content,"The edit is ready.");
  assert.deepEqual(provider.models[0].efforts,["low","medium","high"]);assert.equal(provider.models[0].defaultEffort,"medium");
  assert.deepEqual(done.document.steps!.map(step=>step.kind),["reasoning","plan","edit","command"]);assert.equal(done.document.steps!.at(-1)!.exitCode,0);assert.equal(done.document.steps!.at(-1)!.output,"fixture check passed\n");assert.deepEqual(done.document.usage,{input:10,cached:4,output:5});
  const listed=await call<{source:string;items:string[]}>(`/threads/${threadId}/files?query=greet`);assert.equal(listed.source,"workspace");assert.deepEqual(listed.items,["greeting.txt"]);
  const effortThread=await thread("Effort"),badEffort=await fetch(`${base}/api/v1/agent/threads/${effortThread}/runs`,{method:"POST",headers:{...headers,"if-match":'"1"'},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"fixture-model",effort:"extreme",content:"Effort check."})});assert.equal(badEffort.status,400);
  const effortRun=await call<AgentRun>(`/threads/${effortThread}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",effort:"high",content:"Effort check."},1);assert.equal(effortRun.document.effort,"high");assert.equal((await wait(effortRun.id,"completed")).document.effort,"high");
  // App-server runner: resumed sessions receive only unseen messages.
  const revision=async(id:string)=>(await call<AgentHistory>(`/threads/${id}/messages`)).thread.revision;
  const start=async(id:string,content:string,extra:Record<string,unknown>={})=>call<AgentRun>(`/threads/${id}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content,...extra},await revision(id));
  const resumeThread=await thread("Resume");assert.equal((await wait((await start(resumeThread,"RESUME_CHECK first")).id,"completed")).document.output,"Turns: 1. Input: User: RESUME_CHECK first");
  await call(`/threads/${resumeThread}/messages`,{id:randomUUID(),content:"Saved note"},await revision(resumeThread));
  const resumed=await wait((await start(resumeThread,"RESUME_CHECK second")).id,"completed");assert.equal(resumed.document.output,"Turns: 2. Input: User: Saved note User: RESUME_CHECK second");assert.ok(resumed.document.codexThread);
  // Blocking questions wait for the owner's answer; stale request IDs are rejected.
  const questionThread=await thread("Question"),question=await start(questionThread,"ASK_QUESTION");
  const asking=await wait(question.id,"running",run=>!!run.document.pending);assert.equal(asking.document.pending!.questions[0].options[0].label,"Blue");
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${question.id}/answer`,{method:"POST",headers,body:JSON.stringify({requestId:"stale",answers:{color:["Red"]}})})).status,409);
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${question.id}/answer`,{method:"POST",headers,body:JSON.stringify({requestId:asking.document.pending!.id,answers:{color:["Blue"]}})})).status,200);
  const answered=await wait(question.id,"completed");assert.equal(answered.document.output,"You chose Blue.");assert.equal(answered.document.pending,null);assert.deepEqual(answered.document.steps!.find(step=>step.kind==="question")!.items,[{text:"Blue",done:true}]);
  // Escalation requests are declined without user involvement.
  const approval=await wait((await start(await thread("Approval"),"REQUEST_APPROVAL")).id,"completed");assert.equal(approval.document.output,"Approval was declined.");assert.ok(approval.document.steps!.some(step=>step.label.startsWith("Declined: curl")));
  // Steering reaches the running turn once and is saved as a user message.
  const steerThread=await thread("Steer"),steer=await start(steerThread,"WAIT_FOR_STEER");await wait(steer.id,"running",run=>!!run.document.steps?.some(step=>step.id==="steer-wait"));
  const guidance={id:randomUUID(),content:"Use blue"};for(let attempt=0;attempt<2;attempt++)assert.equal((await fetch(`${base}/api/v1/agent/runs/${steer.id}/steer`,{method:"POST",headers,body:JSON.stringify(guidance)})).status,201);
  const steeredRun=await wait(steer.id,"completed");assert.equal(steeredRun.document.output,"Steered: Use blue");assert.deepEqual(steeredRun.document.steered,[guidance.id]);
  assert.deepEqual((await call<AgentHistory>(`/threads/${steerThread}/messages`)).items.map(item=>item.content),["WAIT_FOR_STEER","Use blue","Steered: Use blue"]);
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${steer.id}/steer`,{method:"POST",headers,body:JSON.stringify({id:randomUUID(),content:"Too late"})})).status,409);
  // Plan mode is forwarded as the collaboration mode.
  const planned=await wait((await start(await thread("Plan"),"PLAN_ONLY",{mode:"plan"})).id,"completed");assert.equal(planned.document.output,"Planned without editing.");assert.equal(planned.document.mode,"plan");
  // Images are stored privately per thread, served inert, and passed as local images.
  const imageThread=await thread("Image"),imageId=randomUUID(),png=Buffer.from("89504e470d0a1a0a0000000d49484452","hex"),imageUrl=`${base}/api/v1/agent/threads/${imageThread}/attachments/${imageId}`;
  assert.equal((await fetch(imageUrl+"?name=pixel.png",{method:"PUT",headers:{...session.headers,"content-type":"text/html"},body:png})).status,415);
  assert.equal((await fetch(imageUrl+"?name=pixel.png",{method:"PUT",headers:{...session.headers,"content-type":"image/png"},body:png})).status,201);
  const served=await fetch(imageUrl,{headers:session.headers});assert.equal(served.headers.get("content-type"),"image/png");assert.equal(served.headers.get("x-content-type-options"),"nosniff");assert.deepEqual(Buffer.from(await served.arrayBuffer()),png);
  const pictured=await wait((await start(imageThread,"DESCRIBE_IMAGE",{attachments:[imageId]})).id,"completed");assert.equal(pictured.document.output,"Saw 1 image.");assert.equal(pictured.document.attachments![0].name,"pixel.png");
  assert.equal((await fetch(`${base}/api/v1/agent/threads/${imageThread}/runs`,{method:"POST",headers:{...headers,"if-match":`"${await revision(imageThread)}"`},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"Missing",attachments:[randomUUID()]})})).status,404);
  // Rule files in the Codex home would act outside Dar's control; runs refuse until removed.
  await mkdir(path.join(home,"rules"));await writeFile(path.join(home,"rules","default.rules"),"");
  const ruled=await start(await thread("Rules"),"RESUME_CHECK rules"),refused=await wait(ruled.id,"failed");assert.match(refused.document.error,/execution-policy rules/);
  await unlink(path.join(home,"rules","default.rules"));
  // Terminal: a PTY in the working copy through app-server command/exec, read by offset.
  // Local Git through Dar: status, branch and commit; no remotes are contacted.
  const gitUrl=`${base}/api/v1/agent/threads/${threadId}/git`,gitStatus=await(await fetch(gitUrl,{headers})).json() as {available:boolean;branch:string;changed:number;commits:{subject:string}[]};
  assert.equal(gitStatus.available,true);assert.equal(gitStatus.branch,"agent");assert.ok(gitStatus.changed>0);
  assert.equal((await fetch(gitUrl,{method:"POST",headers,body:JSON.stringify({action:"branch",name:"-bad"})})).status,400);
  assert.equal((await(await fetch(gitUrl,{method:"POST",headers,body:JSON.stringify({action:"branch",name:"feature/greeting"})})).json()).branch,"feature/greeting");
  const committed=await(await fetch(gitUrl,{method:"POST",headers,body:JSON.stringify({action:"commit",message:"Update greeting"})})).json() as typeof gitStatus;assert.equal(committed.changed,0);assert.equal(committed.commits[0].subject,"Update greeting");
  const shell=`${base}/api/v1/agent/threads/${threadId}/terminal`,post=(url:string,value:unknown)=>fetch(url,{method:"POST",headers,body:JSON.stringify(value)});
  assert.equal((await post(`${base}/api/v1/agent/threads/${await thread("No workspace")}/terminal`,{rows:24,cols:80})).status,409);
  assert.equal((await post(shell,{rows:24,cols:80})).status,200);
  let screen="",cursor=0;const until=async(check:(text:string,value:{exited:boolean})=>boolean)=>{for(let n=0;n<40;n++){const value=await(await fetch(`${shell}?after=${cursor}`,{headers})).json() as {end:number;data:string;exited:boolean};screen+=Buffer.from(value.data,"base64").toString();cursor=value.end;if(check(screen,value))return value;}throw Error("Terminal output: "+screen);};
  await until(text=>text.includes("$ "));assert.equal((await post(shell+"/input",{data:"echo terminal works\r"})).status,200);await until(text=>text.includes("terminal works\r\n$ "));
  assert.equal((await post(shell+"/resize",{rows:30,cols:100})).status,200);await post(shell+"/input",{data:"exit\r"});await until((_text,value)=>value.exited);
  const renamed=await fetch(`${base}/api/v1/agent/threads/${effortThread}`,{method:"PATCH",headers,body:JSON.stringify({title:"Renamed effort thread"})});assert.equal(renamed.status,200);assert.equal((await call<{items:{id:string;title:string;revision:number}[]}>("/threads")).items.find(row=>row.id===effortThread)!.title,"Renamed effort thread");
  const removed=await fetch(`${base}/api/v1/agent/threads/${effortThread}`,{method:"DELETE",headers});assert.equal(removed.status,200);assert.equal((await call<{items:{id:string}[]}>("/threads")).items.some(row=>row.id===effortThread),false);
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${effortRun.id}`,{headers})).status,404);await assert.rejects(stat(path.join(config.root,"agent-workspaces",account.user.id,effortThread)));
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
  assert.equal((await fetch(base+"/api/v1/agent/provider/connect",{method:"POST",headers:other.headers})).status,403);
  const browser=await promisify(execFile)(process.execPath,["test/agent-codex-browser.mjs"],{cwd:path.resolve(import.meta.dirname,".."),env:{...process.env,DAR_INTEGRATION_ORIGIN:base,DAR_TEST_ROOT:root,DAR_CODEX_PROJECT:projectId},encoding:"utf8",timeout:45000});console.log(browser.stdout.trim());
 }finally{await runtime.close();}
});
