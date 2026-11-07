import {test} from "node:test";
import assert from "node:assert/strict";
import {randomUUID} from "node:crypto";
import {login} from "./integration-helpers.ts";
import {database} from "../runtime/database.ts";
import {deliverReminders} from "../runtime/pim/reminders.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;
test("Notebook export/import and Unicode search paginate without losing source or metadata",{skip:!origin,timeout:30000},async()=>{
  const base=origin!,session=await login(base),headers={...session.headers,"content-type":"application/json"};
  const post=async(path:string,body:unknown)=>fetch(base+"/api/v1/notes"+path,{method:"POST",headers,body:JSON.stringify(body)});
  const book=await (await post("/notebooks",{name:"Notebook pagination"})).json() as any,items=Array.from({length:205},(_,i)=>({title:`Zoë ${String(i).padStart(3,"0")}`,content:`# Exact ${i}\r\n\r\nSource with Unicode é.\n`,tags:["proof"],favorite:true}));
  const imported=await post(`/notebooks/${book.id}/import`,{items});assert.equal(imported.status,200);assert.equal((await imported.json() as any).items.filter((i:any)=>i.id).length,205);
  const endpoint=`${base}/api/v1/notes/items?collection=${book.id}&q=zoë&tag=proof&view=favorites&sort=title`,first=await (await fetch(endpoint,{headers})).json() as any,second=await (await fetch(endpoint+"&cursor="+encodeURIComponent(first.nextCursor),{headers})).json() as any;
  assert.equal(first.items.length,200);assert.equal(second.items.length,5);assert.equal(second.nextCursor,"");assert.equal(new Set([...first.items,...second.items].map(i=>i.id)).size,205);
  const exported=await (await fetch(`${base}/api/v1/notes/notebooks/${book.id}/export`,{headers})).json() as any;assert.equal(exported.format,"dar-notebook-v1");assert.deepEqual(exported.items.sort((a:any,b:any)=>a.title.localeCompare(b.title)),items.map(i=>({kind:"note",...i})));
  const target=await (await post("/notebooks",{name:"Notebook imported copy"})).json() as any;const copied=await post(`/notebooks/${target.id}/import`,exported);assert.equal(copied.status,200);assert.equal((await copied.json() as any).items.filter((i:any)=>i.id).length,205);
});
test("Due-only task alarms deliver once and completion invalidates its prior inbox entry",{skip:!origin,timeout:15000},async()=>{
  const base=origin!,session=await login(base),headers={...session.headers,"content-type":"application/json"},db=await database(process.env.DAR_DATABASE_URL!,process.env.DAR_DATABASE_NAME!,process.env.DAR_DATABASE_TOKEN_FILE!);
  const list=await (await fetch(base+"/api/v1/tasks/lists",{method:"POST",headers,body:JSON.stringify({name:"Task alarm proof"})})).json() as any,due=new Date(Date.now()+3000).toISOString().slice(0,19);
  const created=await fetch(base+"/api/v1/tasks/items",{method:"POST",headers,body:JSON.stringify({collectionId:list.id,data:{title:"Due task alarm",due,timezone:"UTC",reminderMinutes:0}})});assert.equal(created.status,201);const task=await created.json() as any;
  await new Promise(resolve=>setTimeout(resolve,Math.max(0,Date.parse(due+"Z")-Date.now()+25)));await deliverReminders(db);await deliverReminders(db);
  let inbox=await (await fetch(base+"/api/v1/pim/reminders",{headers})).json() as any;assert.equal(inbox.items.filter((i:any)=>i.resourceId===task.id).length,1);
  const completed=await fetch(`${base}/api/v1/tasks/items/${task.id}`,{method:"PUT",headers:{...headers,"if-match":created.headers.get("etag")!},body:JSON.stringify({data:{status:"COMPLETED"}})});assert.equal(completed.status,200);await deliverReminders(db);
  inbox=await (await fetch(base+"/api/v1/pim/reminders",{headers})).json() as any;assert.equal(inbox.items.some((i:any)=>i.resourceId===task.id),false);
});
test("VTODO conditional DAV edits, multiget/date filters, scoped credentials and child-first import",{skip:!origin,timeout:40000},async()=>{
  const base=origin!,session=await login(base),headers={...session.headers,"content-type":"application/json"};
  async function post(path:string,value:unknown){const r=await fetch(base+path,{method:"POST",headers,body:JSON.stringify(value)});assert.equal(r.status,201,await r.clone().text());return r.json() as Promise<any>;}
  const list=await post("/api/v1/tasks/lists",{name:"DAV tasks"});
  const password=await post("/api/v1/auth/app-passwords",{label:"Task DAV",scope:"calendar:write"}),read=await post("/api/v1/auth/app-passwords",{label:"Task read",scope:"calendar:read"}),files=await post("/api/v1/auth/app-passwords",{label:"Files isolation",scope:"write"});
  const auth=(secret:string)=>({authorization:`Basic ${Buffer.from("admin:"+secret).toString("base64")}`}),home=`${base}/dav/calendars/admin/${list.id}/`,href=home+"external.ics";
  const content="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:dav-task-proof\r\nDUE:20261107T010000Z\r\nSUMMARY:External task\r\nSTATUS:NEEDS-ACTION\r\nX-PRIVATE:retain\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
  let r=await fetch(href,{method:"PUT",headers:{...auth(password.secret),"if-none-match":"*"},body:content});assert.equal(r.status,201,await r.clone().text());
  r=await fetch(href,{headers:auth(read.secret)});assert.equal(r.status,200);const etag=r.headers.get("etag")!;assert.match(await r.text(),/X-PRIVATE:retain/);
  assert.equal((await fetch(href,{method:"PUT",headers:{...auth(read.secret),"if-match":etag},body:content})).status,403);
  assert.equal((await fetch(href,{headers:auth(files.secret)})).status,403);
  r=await fetch(href,{method:"PUT",headers:{...auth(password.secret),"if-match":etag},body:content.replace("NEEDS-ACTION","COMPLETED")});assert.equal(r.status,204,await r.clone().text());
  assert.equal((await fetch(href,{method:"DELETE",headers:{...auth(password.secret),"if-match":etag}})).status,412);
  const report=async(body:string)=>fetch(home,{method:"REPORT",headers:{...auth(read.secret),"content-type":"application/xml"},body});
  r=await report(`<c:calendar-multiget xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:d="DAV:"><d:prop><d:getetag/><c:calendar-data/></d:prop><d:href>/dav/calendars/admin/${list.id}/external.ics</d:href></c:calendar-multiget>`);assert.equal(r.status,207);assert.match(await r.text(),/STATUS:COMPLETED/);
  const query=(start:string,end:string)=>`<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:d="DAV:"><d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"><c:time-range start="${start}" end="${end}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>`;
  r=await report(query("20261107T000000Z","20261107T020000Z"));assert.equal(r.status,207);assert.match(await r.text(),/External task/);
  r=await report(query("20261108T000000Z","20261108T020000Z"));assert.equal(r.status,207);assert.doesNotMatch(await r.text(),/External task/);
  const imported=content.replace(/BEGIN:VTODO[\s\S]*END:VTODO/,"BEGIN:VTODO\r\nUID:import-child\r\nSUMMARY:Imported child\r\nRELATED-TO;RELTYPE=PARENT:import-parent\r\nEND:VTODO\r\nBEGIN:VTODO\r\nUID:import-parent\r\nSUMMARY:Imported parent\r\nEND:VTODO");
  r=await fetch(`${base}/api/v1/tasks/lists/${list.id}/import`,{method:"POST",headers,body:JSON.stringify({content:imported})});assert.equal(r.status,200);assert.equal((await r.json() as any).imported,2);
  const current=await fetch(href,{headers:auth(password.secret)});assert.equal((await fetch(href,{method:"DELETE",headers:{...auth(password.secret),"if-match":current.headers.get("etag")!}})).status,204);
});
test("Notes atomic revisions, Markdown fidelity, empty edits, move/copy, private trash and shared revocation",{skip:!origin,timeout:40000},async()=>{
  const base=origin!,admin=await login(base),headers={...admin.headers,"content-type":"application/json"};
  async function api(path:string,method="GET",body?:unknown,etag?:string,auth=headers){return fetch(base+"/api/v1/notes"+path,{method,headers:{...auth,...etag?{"if-match":etag}:{}},body:body===undefined?undefined:JSON.stringify(body)});}
  let r=await api("/notebooks","POST",{name:"Notes proof"});assert.equal(r.status,201);const book=await r.json() as any;r=await api("/notebooks","POST",{name:"Other notebook"});const other=await r.json() as any;
  const content="# Exact source\r\n\r\n<script>not executed</script>\n";r=await api("/items","POST",{collectionId:book.id,title:"A note",content:""});assert.equal(r.status,201,await r.clone().text());let note=await r.json() as any;const uid=note.uid,id=note.id,tag=r.headers.get("etag")!;
  r=await api(`/items/${id}`,"PUT",{title:"Edited",content,tags:["project"],favorite:true},tag);assert.equal(r.status,200,await r.clone().text());note=await r.json();assert.equal(note.uid,uid);assert.equal(note.content,content);assert.equal(await (await api(`/items/${id}/export`)).text(),content);
  assert.equal((await api(`/items/${id}`,"PUT",{title:"lost update",content:"wrong"},tag)).status,412);
  const search=await (await api("/items?q=not%20executed&view=favorites")).json() as any;assert.ok(search.items.some((n:any)=>n.id===id));
  r=await api(`/items/${id}/copy`,"POST",{collectionId:other.id},`"${id}:${note.revision}"`);assert.equal(r.status,201);const copy=await r.json() as any;assert.notEqual(copy.id,id);assert.notEqual(copy.uid,uid);
  r=await api(`/items/${id}/move`,"POST",{collectionId:other.id},`"${id}:${note.revision}"`);assert.equal(r.status,200,await r.clone().text());note=await (await api(`/items/${id}`)).json();assert.equal(note.collectionId,other.id);assert.equal(note.uid,uid);
  const username=`notes-reader-${randomUUID().slice(0,6)}`;r=await fetch(base+"/api/v1/auth/users",{method:"POST",headers,body:JSON.stringify({username,displayName:"Notes reader",password:"a long isolated test password"})});assert.equal(r.status,201);const reader=await login(base,username),readerHeaders={...reader.headers,"content-type":"application/json"};
  const c=await (await api(`/notebooks/${other.id}`)).json() as any;r=await api(`/notebooks/${other.id}/grants`,"POST",{username,role:"read"},`"${c.id}:${c.revision}"`);assert.equal(r.status,200);
  assert.equal((await api(`/items/${id}`,"GET",undefined,undefined,readerHeaders)).status,200);assert.equal((await api(`/items/${id}`,"PUT",{title:"blocked",content:"x"},`"${id}:${note.revision}"`,readerHeaders)).status,403);
  const grants=await (await api(`/notebooks/${other.id}/grants`)).json() as any,currentBook=await (await api(`/notebooks/${other.id}`)).json() as any;
  r=await api(`/notebooks/${other.id}/grants`,"DELETE",{id:grants.items[0].id},`"${other.id}:${currentBook.revision}"`);assert.equal(r.status,200);assert.equal((await api(`/items/${id}`,"GET",undefined,undefined,readerHeaders)).status,404);
  const revoked=await (await api(`/notebooks/${other.id}`)).json() as any;r=await api(`/notebooks/${other.id}/grants`,"POST",{username,role:"read"},`"${other.id}:${revoked.revision}"`);assert.equal(r.status,200);
  assert.equal((await api(`/items/${id}`,"DELETE",undefined,`"${id}:${note.revision}"`)).status,200);assert.equal((await api(`/items/${id}`,"GET",undefined,undefined,readerHeaders)).status,404);assert.equal((await (await api("/trash","GET",undefined,undefined,readerHeaders)).json() as any).items.length,0);
  r=await api(`/trash/${id}/restore`,"POST",{collectionId:book.id},`"${id}:${note.revision}"`);assert.equal(r.status,200,await r.clone().text());note=await (await api(`/items/${id}`)).json();assert.equal(note.content,content);assert.equal(note.uid,uid);assert.equal(note.collectionId,book.id);
  assert.equal((await api(`/items/${id}`,"DELETE",undefined,`"${id}:${note.revision}"`)).status,200);assert.equal((await api(`/trash/${id}`,"DELETE",undefined,`"${id}:${note.revision}"`)).status,200);assert.equal((await api(`/trash/${id}`)).status,404);
});
test("Tasks hierarchy, cycles, completion, subtree copy/move, mixed Calendar and CalDAV VTODO",{skip:!origin,timeout:40000},async()=>{
  const base=origin!,admin=await login(base),headers={...admin.headers,"content-type":"application/json"};async function api(path:string,method="GET",body?:unknown,etag?:string){return fetch(base+"/api/v1/tasks"+path,{method,headers:{...headers,...etag?{"if-match":etag}:{}},body:body===undefined?undefined:JSON.stringify(body)});}
  let r=await api("/lists","POST",{name:"Tasks proof"});assert.equal(r.status,201);const list=await r.json() as any;r=await api("/lists","POST",{name:"Tasks destination"});const dest=await r.json() as any;
  r=await api("/items","POST",{collectionId:list.id,data:{title:"Parent",start:"2026-11-06",due:"2026-11-07",allDay:true,priority:1}});assert.equal(r.status,201,await r.clone().text());let parent=await r.json() as any;
  r=await api("/items","POST",{collectionId:list.id,data:{title:"Child",parentUid:parent.uid}});assert.equal(r.status,201,await r.clone().text());const child=await r.json() as any;
  assert.equal((await api(`/items/${parent.id}`,"PUT",{data:{...parent.data,parentUid:child.uid}},`"${parent.id}:${parent.revision}"`)).status,400);
  assert.equal((await api(`/items/${parent.id}`,"DELETE",undefined,`"${parent.id}:${parent.revision}"`)).status,409);
  r=await api(`/items/${parent.id}/copy`,"POST",{collectionId:dest.id},`"${parent.id}:${parent.revision}"`);assert.equal(r.status,200,await r.clone().text());const copies=await r.json() as any;assert.equal(copies.items.length,2);assert.notEqual(copies.items[0].uid,parent.uid);assert.equal(JSON.parse(copies.items[1].projection).parentUid,copies.items[0].uid);
  r=await api(`/items/${parent.id}/move`,"POST",{collectionId:dest.id},`"${parent.id}:${parent.revision}"`);assert.equal(r.status,200,await r.clone().text());parent=await (await api(`/items/${parent.id}`)).json();assert.equal(parent.collectionId,dest.id);assert.equal((await (await api(`/items/${child.id}`)).json() as any).collectionId,dest.id);
  r=await api(`/items/${child.id}`,"GET");let current=await r.json() as any;r=await api(`/items/${child.id}`,"PUT",{data:{...current.data,status:"COMPLETED"}},r.headers.get("etag")!);assert.equal(r.status,200);current=await r.json();assert.equal(current.data.progress,100);assert.ok(current.data.completedAt);
  const exported=await (await api(`/lists/${dest.id}/export`)).text();assert.match(exported,/BEGIN:VTODO/);const events=await fetch(`${base}/api/v1/calendar/calendars/${dest.id}/occurrences?from=2026-11-01&to=2026-11-15&timezone=UTC`,{headers});assert.equal(events.status,200);assert.deepEqual((await events.json() as any).items,[]);
  r=await fetch(base+"/api/v1/auth/app-passwords",{method:"POST",headers,body:JSON.stringify({label:"VTODO proof",scope:"calendar:write"})});const password=await r.json() as any,auth={authorization:`Basic ${Buffer.from("admin:"+password.secret).toString("base64")}`,"content-type":"application/xml"};
  const query='<?xml version="1.0"?><c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:d="DAV:"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"/></c:comp-filter></c:filter></c:calendar-query>';
  r=await fetch(`${base}/dav/calendars/admin/${dest.id}/`,{method:"REPORT",headers:auth,body:query});assert.equal(r.status,207,await r.clone().text());assert.match(await r.text(),/BEGIN:VTODO/);
});
