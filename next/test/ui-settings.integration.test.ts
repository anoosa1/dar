import {test} from "node:test";
import assert from "node:assert/strict";
import {randomUUID} from "node:crypto";
import {login,upload,connect} from "./integration-helpers.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;
async function account(base:string){const admin=await login(base),username=`ux-${randomUUID().slice(0,8)}`,response=await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username,displayName:"UX settings",password:"a long isolated test password"})});assert.equal(response.status,201);return login(base,username);}
test("App preferences preserve other apps, reject stale/invalid writes, scope users and filter hidden file windows",{skip:!origin,timeout:20000},async()=>{
  const base=origin!,user=await account(base),other=await account(base),headers={...user.headers,"content-type":"application/json"},endpoint=base+"/api/v1/pim/preferences";
  const initial=await fetch(endpoint,{headers}),etag=initial.headers.get("etag")!;
  let response=await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":etag},body:JSON.stringify({filesView:"grid",filesShowHidden:false,notesEditorMode:"preview",tasksView:"all",appearance:"mocha"})});assert.equal(response.status,200);let value=await response.json() as any;
  assert.equal((await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":etag},body:JSON.stringify({filesView:"list"})})).status,412);
  response=await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"preferences:${value.revision}"`},body:JSON.stringify({timezone:"America/Toronto",weekStart:0})});assert.equal(response.status,200);value=await response.json();assert.equal(value.filesView,"grid");assert.equal(value.appearance,"mocha");assert.equal(value.notesEditorMode,"preview");assert.equal(value.tasksView,"all");
  assert.equal((await (await fetch(endpoint,{headers:other.headers})).json() as any).filesView,undefined);
  for(const invalid of [{appearance:"invalid"},{filesView:"broken"},{filesShowHidden:"yes"},{tasksView:"unknown"},{notesEditorMode:"html"}])assert.equal((await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"preferences:${value.revision}"`},body:JSON.stringify(invalid)})).status,400);
  const hidden=await upload(base,user,".hidden-proof.txt","hidden"),visible=await upload(base,user,"visible-proof.txt","visible"),live=await connect(user);
  try{assert.equal([...live.db.myFiles.iter()].some(r=>r.id===hidden.fileId),false);assert.equal([...live.db.myFiles.iter()].some(r=>r.id===visible.fileId),true);
    const changed=new Promise<void>(resolve=>live.db.myFiles.onInsert((_ctx,row)=>{if(row.id===hidden.fileId)resolve();}));
    response=await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"preferences:${value.revision}"`},body:JSON.stringify({filesShowHidden:true})});assert.equal(response.status,200);await changed;assert.ok([...live.db.myFiles.iter()].some(r=>r.id===hidden.fileId));
  }finally{live.disconnect();}
});
test("Personal profile edits preserve identity/privileges and enforce revisions and user scope",{skip:!origin,timeout:20000},async()=>{
  const base=origin!,user=await account(base),other=await account(base),endpoint=base+"/api/v1/account/profile",headers={...user.headers,"content-type":"application/json"};
  let response=await fetch(endpoint,{headers}),profile=await response.json() as any;assert.equal(response.status,200);const id=profile.id,username=profile.userName,etag=response.headers.get("etag")!;
  response=await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":etag},body:JSON.stringify({displayName:"Changed display",givenName:"Test",familyName:"Person",email:"profile@example.com"})});assert.equal(response.status,200);profile=await response.json();assert.equal(profile.id,id);assert.equal(profile.userName,username);assert.equal(profile.administrator,false);assert.equal(profile.active,true);assert.equal(profile.name.givenName,"Test");assert.equal(profile.emails[0].value,"profile@example.com");
  assert.equal((await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":etag},body:JSON.stringify({displayName:"stale"})})).status,412);
  for(const privileged of [{administrator:true},{userName:"admin"},{active:false}])assert.equal((await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"${id}:${profile.revision}"`},body:JSON.stringify({displayName:"Rejected",...privileged})})).status,400);
  assert.equal((await fetch(endpoint,{method:"PUT",headers,body:JSON.stringify({displayName:"missing revision"})})).status,428);
  assert.notEqual((await (await fetch(endpoint,{headers:other.headers})).json() as any).id,id);
  const session=await (await fetch(base+"/api/v1/auth/session",{headers})).json() as any;assert.equal(session.user.displayName,"Changed display");
  response=await fetch(endpoint,{method:"PUT",headers:{...headers,"if-match":`"${id}:${profile.revision}"`},body:JSON.stringify({displayName:"",email:"bad"})});assert.equal(response.status,400);
});
