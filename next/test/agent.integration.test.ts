import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {spawnSync} from "node:child_process";
import path from "node:path";
import {login,connect} from "./integration-helpers.ts";
import {database} from "../runtime/database.ts";
import {digestToken} from "../runtime/auth/session.ts";
import type {AgentProject,AgentThread,AgentMessage,AgentHistory} from "../shared/agent.ts";

test("Agent sources, account isolation, atomic revisions, replay, pagination, disable and restore",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:60000},async()=>{
  const base=process.env.DAR_INTEGRATION_ORIGIN!,admin=await login(base),db=await database(process.env.DAR_DATABASE_URL!,process.env.DAR_DATABASE_NAME!,process.env.DAR_DATABASE_TOKEN_FILE!);
  const headers={...admin.headers,"content-type":"application/json"};
  const post=(route:string,value:unknown,extra:Record<string,string>={})=>fetch(base+"/api/v1/agent"+route,{method:"POST",headers:{...headers,...extra},body:JSON.stringify(value)});
  const project={id:randomUUID(),name:"Source proof "+randomUUID(),sourceType:"git",sourceRef:"https://example.test/you/project.git"};
  assert.equal((await post("/projects",project)).status,201);
  assert.equal((await post("/projects",project)).status,201);
  assert.equal((await post("/projects",{...project,id:randomUUID(),sourceRef:"https://secret@example.test/repo.git"})).status,400);
  assert.equal((await post("/projects",{...project,id:randomUUID(),sourceType:"host",sourceRef:"/var/lib/dar"})).status,400);
  assert.equal((await post("/projects",{...project,id:randomUUID(),ownerId:randomUUID()})).status,400);
  assert.equal((await fetch(base+"/api/v1/agent/projects")).status,401);
  assert.equal((await fetch(base+"/api/v1/agent/projects",{method:"POST",headers:{cookie:admin.headers.cookie,origin:base,"content-type":"application/json"},body:JSON.stringify(project)})).status,403);
  const folderId=randomUUID(),connection=await connect(admin);
  try{await connection.reducers.createFolder({id:folderId,parentId:"",name:"Agent files "+randomUUID()});}finally{connection.disconnect();}
  const filesProject={id:randomUUID(),name:"Files project",sourceType:"files",sourceRef:folderId};
  assert.equal((await post("/projects",filesProject)).status,201);
  const folders=await(await fetch(base+"/api/v1/agent/folders",{headers})).json() as {items:{id:string}[]};assert.ok(folders.items.some(f=>f.id===folderId));
  const username="agent-"+randomUUID().slice(0,8);
  assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username,displayName:"Agent isolation",password:"a long isolated test password"})})).status,201);
  const other=await login(base,username),otherProjects=await(await fetch(base+"/api/v1/agent/projects",{headers:other.headers})).json() as {items:AgentProject[]};assert.deepEqual(otherProjects.items,[]);
  assert.equal((await fetch(`${base}/api/v1/agent/folders?parentId=${folderId}`,{headers:other.headers})).status,404);
  const picker=await(await fetch(`${base}/api/v1/files/picker?parentId=${folderId}`,{headers})).json() as {folder:{id:string};items:unknown[]};assert.equal(picker.folder.id,folderId);
  assert.equal((await fetch(`${base}/api/v1/files/picker?parentId=${folderId}`,{headers:other.headers})).status,404);
  assert.equal((await fetch(`${base}/api/v1/files/picker?kind=invalid`,{headers})).status,400);
  const autoId=randomUUID();assert.equal((await post("/threads",{id:autoId,projectId:project.id})).status,201);
  const auto=await post(`/threads/${autoId}/messages`,{id:randomUUID(),content:"Fix the project chooser\nKeep it keyboard accessible."},{"if-match":'"1"'});assert.equal(auto.status,201);assert.equal((await auto.json() as {thread:AgentThread}).thread.title,"Fix the project chooser Keep it keyboard accessible.".slice(0,50));
  assert.equal((await post("/threads",{id:autoId,projectId:project.id})).status,201);
  const unicodeId=randomUUID();assert.equal((await post("/threads",{id:unicodeId,projectId:project.id})).status,201);
  const unicode=await post(`/threads/${unicodeId}/messages`,{id:randomUUID(),content:"🌸".repeat(80)},{"if-match":'"1"'});assert.equal(unicode.status,201);assert.equal(Buffer.byteLength((await unicode.json() as {thread:AgentThread}).thread.title),200);
  const all=await(await fetch(base+"/api/v1/agent/threads",{headers:other.headers})).json() as {items:AgentThread[]};assert.deepEqual(all.items,[]);
  assert.equal((await fetch(base+"/api/v1/agent/projects",{method:"POST",headers:{...other.headers,"content-type":"application/json"},body:JSON.stringify({...filesProject,id:randomUUID()})})).status,404);
  const threadId=randomUUID(),threadResponse=await post("/threads",{id:threadId,projectId:project.id,title:"Revision proof"});assert.equal(threadResponse.status,201);let thread=await threadResponse.json() as AgentThread;
  assert.equal((await fetch(`${base}/api/v1/agent/threads?projectId=${project.id}`,{headers:other.headers})).status,404);
  assert.equal((await fetch(`${base}/api/v1/agent/threads/${threadId}/messages`,{headers:other.headers})).status,404);
  const message={id:randomUUID(),content:"Preserve exact Markdown.\n\n<script>untrusted</script>"};
  const route=`/threads/${threadId}/messages`,match={"if-match":'"1"'};
  assert.equal((await post(route,message)).status,428);
  const writes=await Promise.all([post(route,message,match),post(route,{id:randomUUID(),content:"Competing write"},match)]);
  assert.deepEqual(writes.map(r=>r.status).sort(),[201,409]);
  const current=await(await fetch(base+"/api/v1/agent"+route,{headers})).json() as AgentHistory;assert.equal(current.items.length,1);thread=current.thread;
  const saved=current.items[0];
  const replay=await post(route,{id:saved.id,content:saved.content},match);assert.equal(replay.status,201);assert.equal((await replay.json() as {message:AgentMessage}).message.id,saved.id);
  assert.equal((await post(route,{id:saved.id,content:"Changed payload"},match)).status,409);
  assert.equal((await post(route,{id:randomUUID(),content:"x".repeat(16385)},{"if-match":`"${thread.revision}"`})).status,400);
  for(let i=0;i<52;i++){const response=await post(route,{id:randomUUID(),content:`History ${i}`},{"if-match":`"${thread.revision}"`});assert.equal(response.status,201);thread=(await response.json() as {thread:AgentThread}).thread;}
  const latest=await(await fetch(base+"/api/v1/agent"+route,{headers})).json() as AgentHistory;assert.equal(latest.items.length,50);assert.ok(latest.nextBefore);
  const earlier=await(await fetch(base+"/api/v1/agent"+route+`?before=${latest.nextBefore}`,{headers})).json() as AgentHistory;assert.equal(earlier.items.length,3);assert.equal(earlier.nextBefore,null);assert.equal(earlier.items[0].id,saved.id);
  assert.equal((await fetch(base+"/api/v1/agent"+route+"?before=NaN",{headers})).status,400);
  const escapedThread=await(await post("/threads",{id:randomUUID(),projectId:project.id,title:"JSON expansion boundary"})).json() as AgentThread;
  assert.equal((await post(`/threads/${escapedThread.id}/messages`,{id:randomUUID(),content:"\t".repeat(16383)+"x"},{"if-match":'"1"'})).status,201);
  const sid=(await db.call<{session:{id:string}}>("service_session",[digestToken(admin.headers.cookie.split("=")[1])])).session.id;
  const settings=await db.call<{revision:number}>("service_app_settings",["","",false,0]);
  const disabled=await db.call<{revision:number}>("service_app_settings",[sid,"agent",false,settings.revision]);
  try{
    assert.equal((await fetch(base+"/api/v1/agent/projects",{headers})).status,404);
    const search=await(await fetch(base+"/api/v1/search?q="+encodeURIComponent(project.name),{headers})).json() as {items:{id:string}[]};assert.ok(!search.items.some(r=>r.id===project.id));
  }finally{await db.call("service_app_settings",[sid,"agent",true,disabled.revision]);}
  const pin=randomUUID(),snapshot=await db.call<{format:string;agent:{projects:AgentProject[];threads:AgentThread[];messages:AgentMessage[]};accounts:{id:string;username:string;verifier:string}[]}>("service_backup",[pin]);await db.call("service_release_backup",[pin]);assert.equal(snapshot.format,"dar-backup-v7");
  const publish=async()=>{
    const name="dar-agent-restore-"+randomUUID().slice(0,8),result=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});assert.equal(result.status,0,result.stdout+result.stderr);return database(process.env.DAR_DATABASE_URL!,name,process.env.DAR_DATABASE_TOKEN_FILE!);
  };
  const restored=await publish();await restored.call("service_restore",[JSON.stringify(snapshot)]);
  const restoredPin=randomUUID(),recovered=await restored.call<typeof snapshot>("service_backup",[restoredPin]);await restored.call("service_release_backup",[restoredPin]);assert.deepEqual(recovered.agent,snapshot.agent);
  const malformed=await publish();await assert.rejects(malformed.call("service_restore",[JSON.stringify({...snapshot,agent:{...snapshot.agent,messages:snapshot.agent.messages.map((row,i)=>i===0?{...row,ownerId:randomUUID()}:row)}})]),/invalidInput/);
  const legacy=await publish();const {agent:_,...old}=snapshot;await legacy.call("service_restore",[JSON.stringify({...old,format:"dar-backup-v5"})]);
  const oldPin=randomUUID(),oldRecovered=await legacy.call<typeof snapshot>("service_backup",[oldPin]);await legacy.call("service_release_backup",[oldPin]);assert.deepEqual(oldRecovered.agent,{projects:[],threads:[],messages:[],runs:[],shares:[]});
});
