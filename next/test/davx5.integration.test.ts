import {test} from "node:test";
import assert from "node:assert/strict";
import {randomUUID} from "node:crypto";
import {login} from "./integration-helpers.ts";
const base=process.env.DAR_INTEGRATION_ORIGIN;

test("DAVx5-style discovery and ETag synchronization use one scoped credential for both homes",{skip:!base,timeout:30000},async()=>{
  const session=await login(base!);
  async function post(path:string,value:object){const response=await fetch(base+path,{method:"POST",headers:{...session.headers,"content-type":"application/json"},body:JSON.stringify(value)});assert.equal(response.status,201,await response.clone().text());return response.json() as Promise<any>;}
  const credential=await post("/api/v1/auth/app-passwords",{label:"DAVx5 proof",scope:"pim:write"}),auth={authorization:`Basic ${Buffer.from(`admin:${credential.secret}`).toString("base64")}`};
  assert.equal(credential.davUrl,base+"/dav/");
  for(const path of["/dav","/.well-known/carddav","/.well-known/caldav"]){const response=await fetch(base+path,{redirect:"manual"});assert.equal(response.status,308);assert.equal(response.headers.get("location"),"/dav/");}
  const denied=await fetch(base+"/dav/",{method:"PROPFIND",headers:{depth:"0"}});assert.equal(denied.status,401);assert.match(denied.headers.get("www-authenticate")!,/Basic/);
  const options=await fetch(base+"/dav/",{method:"OPTIONS"});assert.equal(options.status,204);assert.match(options.headers.get("dav")!,/calendar-access/);assert.match(options.headers.get("dav")!,/addressbook/);
  const discovery='<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="urn:ietf:params:xml:ns:carddav"><d:prop><d:current-user-principal/><c:calendar-home-set/><a:addressbook-home-set/></d:prop></d:propfind>';
  for(const path of["/dav/","/dav/principals/admin/"]){const response=await fetch(base+path,{method:"PROPFIND",headers:{...auth,depth:"0","content-type":"application/xml"},body:discovery});assert.equal(response.status,207);const text=await response.text();assert.match(text,/\/dav\/calendars\/admin\//);assert.match(text,/\/dav\/addressbooks\/admin\//);}
  for(const kind of["contacts","calendar"]){
    const api=`/api/v1/${kind}/${kind==="contacts"?"books":"calendars"}`,collection=await post(api,{name:"DAVx5 "+kind,color:"#1e66f5"}),home=`/dav/${kind==="contacts"?"addressbooks":"calendars"}/admin/`,href=home+collection.id+"/";
    const list=await fetch(base+home,{method:"PROPFIND",headers:{...auth,depth:"1"}});assert.equal(list.status,207);const listing=await list.text();assert.ok(listing.includes(collection.id),listing);
    const before=await fetch(base+href,{method:"PROPFIND",headers:{...auth,depth:"0"}});const initialTag=(await before.text()).match(/<cs:getctag>(.*?)<\/cs:getctag>/)![1];
    const uid=randomUUID(),content=kind==="contacts"?`BEGIN:VCARD\r\nVERSION:3.0\r\nUID:${uid}\r\nFN:DAVx5 contact\r\nEND:VCARD\r\n`:`BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:${uid}\r\nDTSTART:20261109T120000Z\r\nDTEND:20261109T130000Z\r\nSUMMARY:DAVx5 event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`,resource=href+uid+(kind==="contacts"?".vcf":".ics");
    let response=await fetch(base+resource,{method:"PUT",headers:{...auth,"if-none-match":"*"},body:content});assert.equal(response.status,201,await response.clone().text());const etag=response.headers.get("etag")!;
    response=await fetch(base+resource,{headers:auth});assert.equal(response.status,200);assert.match(await response.text(),new RegExp(uid));
    const changed=await fetch(base+href,{method:"PROPFIND",headers:{...auth,depth:"0"}});assert.notEqual((await changed.text()).match(/<cs:getctag>(.*?)<\/cs:getctag>/)![1],initialTag);
    assert.equal((await fetch(base+resource,{method:"PUT",headers:{...auth,"if-match":"\"stale\""},body:content})).status,412);
    const read=await post("/api/v1/auth/app-passwords",{label:"Read only DAVx5",scope:"pim:read"}),readAuth={authorization:`Basic ${Buffer.from(`admin:${read.secret}`).toString("base64")}`};
    assert.equal((await fetch(base+resource,{headers:readAuth})).status,200);assert.equal((await fetch(base+resource,{method:"DELETE",headers:{...readAuth,"if-match":etag}})).status,403);
    assert.equal((await fetch(base+resource,{method:"DELETE",headers:{...auth,"if-match":etag}})).status,204);assert.equal((await fetch(base+resource,{headers:auth})).status,404);
  }
  const unrelated=await fetch(base+"/dav/files/admin/",{method:"PROPFIND",headers:{...auth,depth:"0"}});assert.ok([401,403].includes(unrelated.status));
  assert.equal((await fetch(base+"/api/v1/auth/app-passwords/"+credential.id,{method:"DELETE",headers:session.headers})).status,200);
  assert.equal((await fetch(base+"/dav/",{method:"PROPFIND",headers:{...auth,depth:"0"}})).status,401);
});
