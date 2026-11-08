import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {login,upload,connect} from "./integration-helpers.ts";
import type {SearchResults} from "../shared/search.ts";
const base=process.env.DAR_INTEGRATION_ORIGIN;
test("global search spans app data, respects private/granted scopes and revocation, and bounds results",{skip:!base,timeout:60000},async()=>{
  const admin=await login(base!),marker="search-"+randomUUID().slice(0,8),otherName="search-owner-"+randomUUID().slice(0,6);
  async function post(session:typeof admin,url:string,value:unknown){const response=await fetch(base+url,{method:"POST",headers:{...session.headers,"content-type":"application/json"},body:JSON.stringify(value)});assert.equal(response.status,201,await response.clone().text());return response.json();}
  await post(admin,"/api/v1/auth/users",{username:otherName,displayName:"Private search owner",password:"a long isolated test password"});const other=await login(base!,otherName);
  await upload(base!,admin,marker+" plan.txt","A file");await upload(base!,admin,"."+marker+" hidden.txt","Hidden");
  const notebook=await post(admin,"/api/v1/notes/notebooks",{name:"Search notebook"});
  const note=await post(admin,"/api/v1/notes/items",{collectionId:notebook.id,title:"Body match",content:marker+" inside Markdown"});
  const calendar=await post(admin,"/api/v1/calendar/calendars",{name:"Search calendar",color:"#1e66f5"});
  await post(admin,`/api/v1/calendar/calendars/${calendar.id}/items`,{data:{title:marker+" event",start:"2026-11-08T10:00",end:"2026-11-08T11:00",timezone:"UTC",allDay:false}});
  await post(admin,"/api/v1/tasks/items",{collectionId:calendar.id,data:{title:marker+" task",status:"NEEDS-ACTION",allDay:true,start:"",due:""}});
  const book=await post(admin,"/api/v1/contacts/books",{name:"Search book",color:"#1e66f5"});
  await post(admin,`/api/v1/contacts/books/${book.id}/items`,{content:`BEGIN:VCARD\r\nVERSION:4.0\r\nUID:${randomUUID()}\r\nFN:${marker} Contact\r\nEND:VCARD\r\n`});
  const key="search-"+randomUUID();assert.equal((await fetch(base+`/api/v1/apps/org.dar.prayer-times/userData/${key}`,{method:"PUT",headers:{...admin.headers,"content-type":"application/json","if-match":'"0"'},body:JSON.stringify({schemaVersion:1,value:{label:marker}})})).status,200);
  const search=async(session:typeof admin)=>(await(await fetch(base+`/api/v1/search?q=${marker}`,{headers:session.headers})).json()) as SearchResults;
  const result=await search(admin);for(const group of ["Files","Contacts","Calendar","Notes","Tasks","Applications"])assert.ok(result.items.some(item=>item.group===group),group);
  assert.ok(result.items.some(item=>item.href===`/notes?item=${note.id}`));assert.equal(result.items.some(item=>item.title.startsWith(".")),false);assert.deepEqual((await search(other)).items,[]);
  const owner=await connect(other),folder=randomUUID(),grant=randomUUID();
  try{await owner.reducers.createFolder({id:folder,parentId:"",name:marker+" shared"});await upload(base!,other,marker+" nested.txt","Shared file",folder);await owner.reducers.grantAccess({id:grant,fileId:folder,username:"admin",role:"read",expectedRevision:(JSON.parse(await owner.procedures.fileDetails({id:folder})) as {entry:{revision:number}}).entry.revision});assert.ok((await search(admin)).items.some(item=>item.title===marker+" nested.txt"));await owner.reducers.revokeGrant({id:grant});assert.equal((await search(admin)).items.some(item=>item.title===marker+" nested.txt"),false);}finally{owner.disconnect();}
  for(let i=0;i<10;i++)await post(admin,"/api/v1/notes/items",{collectionId:notebook.id,title:marker+" note "+i,content:""});
  const limited=await search(admin);assert.equal(limited.items.filter(item=>item.group==="Notes").length,8);assert.equal(limited.truncated,true);
  assert.equal((await fetch(base+"/api/v1/search?q=test")).status,401);assert.equal((await fetch(base+"/api/v1/search?q="+"x".repeat(201),{headers:admin.headers})).status,400);
});
