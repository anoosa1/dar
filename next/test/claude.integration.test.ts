import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,mkdir,writeFile,readFile,unlink} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync} from "node:child_process";
import {startServer} from "../runtime/server.ts";
import {login,connect,upload} from "./integration-helpers.ts";
import type {AgentRun,AgentHistory,AgentProvider,AgentLimits} from "../shared/agent.ts";

test("Claude Code CLI runs through stream-json with resume, questions, declines, steering and limits",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:120000},async()=>{
 const root=await mkdtemp("/tmp/dar-claude-acceptance-"),name="dar-claude-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
 const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(published.status,0,published.stdout+published.stderr);
 const executable=path.join(root,"claude-fixture"),home=path.join(root,"claude-home");await mkdir(home,{mode:0o700});await writeFile(executable,await readFile(new URL("./fixtures/claude-fixture.mjs",import.meta.url)),{mode:0o700});
 const claudeFile=path.join(root,"claude.json");
 const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,codexFile:path.join(root,"codex.json"),claudeFile};
 const runtime=await startServer(config);
 try{
  const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Claude fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});assert.equal(setup.status,200);
  const account=await setup.json() as {user:{id:string}};
  const session=await login(base),headers={...session.headers,"content-type":"application/json"};
  const call=async<T>(route:string,value?:unknown,match?:number):Promise<T>=>{const response=await fetch(base+"/api/v1/agent"+route,{headers:{...headers,...match!==undefined?{"if-match":`"${match}"`}:{}},...value!==undefined?{method:"POST",body:JSON.stringify(value)}:{}});assert.ok(response.ok,await response.clone().text());return response.json() as Promise<T>;};
  const before=await call<AgentProvider>("/provider");assert.equal(before.configured,false);assert.deepEqual(before.providers!.map(item=>[item.id,item.configured]),[["codex",false],["claude",false]]);
  await writeFile(claudeFile,JSON.stringify({executable,home,userId:randomUUID(),gitHosts:[]}));assert.match((await call<AgentProvider>("/provider")).providers![1].message,/different Dar account/);
  await writeFile(claudeFile,JSON.stringify({executable,home,userId:account.user.id,gitHosts:[]}));
  const provider=await call<AgentProvider>("/provider");assert.equal(provider.connected,true);assert.ok(!JSON.stringify(provider).includes("fixture@example.test"));
  assert.deepEqual(provider.models.map(model=>[model.id,model.provider,model.isDefault]),[["claude/fixture","claude",true]]);assert.deepEqual(provider.models[0].efforts,["low","high"]);
  assert.equal(provider.providers![1].connected,true);assert.match(provider.providers![1].message,/Claude Code login \(max\)/);
  await writeFile(path.join(home,"logged-out"),"");assert.equal((await call<AgentProvider>("/provider")).connected,false);await unlink(path.join(home,"logged-out"));
  const folderId=randomUUID(),connection=await connect(session);try{await connection.reducers.createFolder({id:folderId,parentId:"",name:"Source"});}finally{connection.disconnect();}
  const file=await upload(base,session,"greeting.txt","Original source.\n",folderId),projectId=randomUUID();await call("/projects",{id:projectId,name:"Files code",sourceType:"files",sourceRef:folderId});
  async function thread(title:string){const id=randomUUID();await call("/threads",{id,projectId,title});return id;}
  async function wait(id:string,expected:string,ready?:(run:AgentRun)=>boolean){const deadline=Date.now()+15000;let result:AgentRun;do{result=await call<AgentRun>(`/runs/${id}`);if(result.status===expected&&(!ready||ready(result)))return result;if(["failed","interrupted","cancelled"].includes(result.status)&&result.status!==expected)throw Error(JSON.stringify(result));await new Promise(r=>setTimeout(r,100));}while(Date.now()<deadline);throw Error(`Run did not become ${expected}: ${JSON.stringify(result!)}`);}
  const revision=async(id:string)=>(await call<AgentHistory>(`/threads/${id}/messages`)).thread.revision;
  const start=async(id:string,content:string,extra:Record<string,unknown>={})=>call<AgentRun>(`/threads/${id}/runs`,{id:randomUUID(),messageId:randomUUID(),model:"claude/fixture",content,...extra},await revision(id));
  // An edit run: typed steps, usage, context window and a reviewable Files change.
  const editThread=await thread("Edit source"),done=await wait((await start(editThread,"Update the greeting.",{effort:"high"})).id,"completed");
  assert.equal(done.document.output,"The edit is ready.");assert.equal(done.document.effort,"high");assert.ok(done.document.claudeSession);
  assert.deepEqual(done.document.steps!.map(step=>[step.kind,step.status]),[["reasoning","completed"],["plan","running"],["edit","completed"],["tool","failed"]]);
  assert.equal(done.document.steps![2].label,"greeting.txt");assert.deepEqual(done.document.steps![1].items,[{text:"Read greeting",done:true},{text:"Update greeting",done:false}]);
  assert.deepEqual(done.document.usage,{input:15,cached:4,output:5});assert.equal(done.document.contextWindow,200000);assert.equal(done.document.contextUsed,18);
  assert.equal(done.document.changes.length,1);assert.equal(done.document.changes[0].fileId,file.fileId);assert.equal(done.document.changes[0].content,"Updated by Claude fixture.\n");
  assert.deepEqual((await call<AgentHistory>(`/threads/${editThread}/messages`)).items.map(m=>m.role),["user","assistant"]);
  const badEffort=await fetch(`${base}/api/v1/agent/threads/${editThread}/runs`,{method:"POST",headers:{...headers,"if-match":`"${await revision(editThread)}"`},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"claude/fixture",effort:"max",content:"Effort check."})});assert.equal(badEffort.status,400);
  // Review is read-only and receives Dar's diff; compaction is left to Claude Code.
  assert.equal((await wait((await start(editThread,"Review the uncommitted changes in this working copy.",{mode:"review"})).id,"completed")).document.output,"Review: greeting.txt changed; no issues found.");
  const compact=await fetch(`${base}/api/v1/agent/threads/${editThread}/runs`,{method:"POST",headers:{...headers,"if-match":`"${await revision(editThread)}"`},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"claude/fixture",mode:"compact",content:"Compact."})});assert.equal(compact.status,409);
  // Resumed sessions receive only unseen messages; a missing session starts over with the transcript.
  const resumeThread=await thread("Resume");assert.equal((await wait((await start(resumeThread,"RESUME_CHECK first")).id,"completed")).document.output,"Turns: 1. Input: User: RESUME_CHECK first");
  await call(`/threads/${resumeThread}/messages`,{id:randomUUID(),content:"Saved note"},await revision(resumeThread));
  assert.equal((await wait((await start(resumeThread,"RESUME_CHECK second")).id,"completed")).document.output,"Turns: 2. Input: User: Saved note User: RESUME_CHECK second");
  await writeFile(path.join(home,"fixture-sessions.json"),"{}");
  assert.match((await wait((await start(resumeThread,"RESUME_CHECK third")).id,"completed")).document.output,/^Turns: 1\. Input: User: RESUME_CHECK first .*User: RESUME_CHECK third$/);
  // Questions use the answer card; escalations are declined; plan mode never edits.
  const question=await start(await thread("Question"),"ASK_QUESTION"),asking=await wait(question.id,"running",run=>!!run.document.pending);assert.equal(asking.document.pending!.questions[0].options[0].label,"Blue");
  assert.equal((await fetch(`${base}/api/v1/agent/runs/${question.id}/answer`,{method:"POST",headers,body:JSON.stringify({requestId:asking.document.pending!.id,answers:{"0":["Blue"]}})})).status,200);
  const answered=await wait(question.id,"completed");assert.equal(answered.document.output,"You chose Blue.");assert.deepEqual(answered.document.steps!.find(step=>step.kind==="question")!.items,[{text:"Blue",done:true}]);
  const network=await wait((await start(await thread("Network"),"REQUEST_NETWORK")).id,"completed");assert.ok(network.document.steps!.some(step=>step.label==="Declined: network access to example.test"));
  const planned=await wait((await start(await thread("Plan"),"PLAN_ONLY",{mode:"plan"})).id,"completed");assert.equal(planned.document.output,"1. Read greeting\n2. Update greeting\n\nPlanned without editing.");
  // Steering is written into the running turn once and saved as a user message.
  const steerThread=await thread("Steer"),steer=await start(steerThread,"WAIT_FOR_STEER");await wait(steer.id,"running",run=>!!run.document.steps?.length);
  const guidance={id:randomUUID(),content:"Use blue"};for(let attempt=0;attempt<2;attempt++)assert.equal((await fetch(`${base}/api/v1/agent/runs/${steer.id}/steer`,{method:"POST",headers,body:JSON.stringify(guidance)})).status,201);
  const steered=await wait(steer.id,"completed");assert.equal(steered.document.output,"Steered: Use blue");assert.deepEqual(steered.document.steered,[guidance.id]);
  // Images are sent as base64 content blocks from the private store.
  const imageThread=await thread("Image"),imageId=randomUUID(),png=Buffer.from("89504e470d0a1a0a0000000d49484452","hex");
  assert.equal((await fetch(`${base}/api/v1/agent/threads/${imageThread}/attachments/${imageId}?name=pixel.png`,{method:"PUT",headers:{...session.headers,"content-type":"image/png"},body:png})).status,201);
  assert.equal((await wait((await start(imageThread,"DESCRIBE_IMAGE",{attachments:[imageId]})).id,"completed")).document.output,"Saw 1 image.");
  // Cancellation interrupts the CLI; failures keep the provider's reason.
  const waiting=await start(await thread("Cancel"),"WAIT_FOR_CANCEL");await wait(waiting.id,"running",run=>!!run.document.steps?.length);await call(`/runs/${waiting.id}/cancel`,{});await wait(waiting.id,"cancelled");
  assert.match((await wait((await start(await thread("Fail"),"FAIL_CLI")).id,"failed")).document.error,/Fixture failure/);
  assert.match((await wait((await start(await thread("Exit"),"EXIT_WITHOUT_RESULT")).id,"failed")).document.error,/exited before completing/);
  // Usage windows come from the CLI's own report.
  const limits=await call<AgentLimits>("/provider/limits");assert.equal(limits.available,true);assert.equal(limits.plan,"max");
  assert.deepEqual(limits.windows.map(item=>[item.label,item.usedPercent,item.windowMinutes]),[["Claude Code · 5 hour",42,300],["Claude Code · Weekly",7,10080]]);
  // Codex models are refused while only Claude Code is connected.
  const codexRun=await fetch(`${base}/api/v1/agent/threads/${imageThread}/runs`,{method:"POST",headers:{...headers,"if-match":`"${await revision(imageThread)}"`},body:JSON.stringify({id:randomUUID(),messageId:randomUUID(),model:"fixture-model",content:"Codex"})});assert.equal(codexRun.status,409);
 }finally{await runtime.close();}
});
