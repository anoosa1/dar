import assert from "node:assert/strict";
import {test} from "node:test";
import {randomUUID} from "node:crypto";
import {login} from "./integration-helpers.ts";

const base=process.env.DAR_INTEGRATION_ORIGIN;
test("instance branding is public, admin-only, revisioned and resettable", {skip:!base},async()=>{
  const admin=await login(base!),name="appearance-"+randomUUID().slice(0,8);
  await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({username:name,displayName:"Appearance test",password:"an isolated appearance password"})});
  const user=await login(base!,name,"an isolated appearance password");
  const current=await (await fetch(base+"/api/v1/appearance")).json() as {revision:number};
  const logo="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=";
  const save=(headers:Record<string,string>,revision:number,image=logo,mediaType="image/png")=>fetch(base+"/api/v1/appearance",{method:"PUT",headers:{...headers,"content-type":"application/json","if-match":`"appearance:${revision}"`},body:JSON.stringify({revision,logo:image,mediaType})});
  assert.equal((await save(user.headers,current.revision)).status,403);
  assert.equal((await save(admin.headers,current.revision,"PHN2Zz4=","image/svg+xml")).status,400);
  assert.equal((await save(admin.headers,current.revision)).status,200);
  assert.equal((await save(admin.headers,current.revision)).status,409);
  const saved=await (await fetch(base+"/api/v1/appearance")).json() as {revision:number;logoUrl:string};
  const image=await fetch(base+saved.logoUrl);
  assert.equal(image.headers.get("content-type"),"image/png");
  assert.equal(Buffer.from(await image.arrayBuffer()).toString("base64"),logo);
  assert.equal((await save(admin.headers,saved.revision,"","")).status,200);
  assert.equal((await fetch(base+"/api/v1/appearance/logo")).status,404);
});

test("mixed app permissions independently enforce read and write over DAV",{skip:!base},async()=>{
  const admin=await login(base!);
  const create=async(scope:string)=>{
    const response=await fetch(base+"/api/v1/auth/app-passwords",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({label:"Mixed scopes",scope})});
    assert.equal(response.status,201,await response.clone().text());
    const value=await response.json() as {id:string;secret:string;davUrl:string};
    return {...value,headers:{authorization:"Basic "+Buffer.from("admin:"+value.secret).toString("base64")}};
  };
  for(const scope of ["v2 ","v2 files:admin","v2 files:read files:read"]){
    assert.equal((await fetch(base+"/api/v1/auth/app-passwords",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({label:"Invalid",scope})})).status,400);
  }
  const mixed=await create("v2 files:write contacts:read calendar:write"),read=await create("v2 files:read calendar:read");
  assert.equal(mixed.davUrl,base+"/dav/");
  const file=base+"/dav/files/admin/scope-"+randomUUID()+".txt";
  const put=await fetch(file,{method:"PUT",headers:mixed.headers,body:"private contents"});
  assert.equal(put.status,201,await put.clone().text());
  assert.equal((await fetch(file,{headers:mixed.headers})).status,403);
  assert.equal((await fetch(file,{method:"PROPFIND",headers:{...mixed.headers,depth:"0"}})).status,403);
  assert.equal(await (await fetch(file,{headers:read.headers})).text(),"private contents");
  assert.equal((await fetch(file,{method:"PUT",headers:read.headers,body:"denied"})).status,403);
  const book=await (await fetch(base+"/api/v1/contacts/books",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({name:"Scope book",color:"#2563eb"})})).json() as {id:string};
  const contact=base+"/dav/addressbooks/admin/"+book.id+"/denied.vcf";
  assert.equal((await fetch(contact,{method:"PUT",headers:mixed.headers,body:"denied"})).status,403);
  const cal=await (await fetch(base+"/api/v1/calendar/calendars",{method:"POST",headers:{...admin.headers,"content-type":"application/json"},body:JSON.stringify({name:"Scope calendar",color:"#2563eb"})})).json() as {id:string};
  const event=base+"/dav/calendars/admin/"+cal.id+"/test.ics";
  const content="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Test//EN\r\nBEGIN:VEVENT\r\nUID:"+randomUUID()+"\r\nDTSTAMP:20261107T120000Z\r\nDTSTART:20261107T120000Z\r\nDTEND:20261107T130000Z\r\nSUMMARY:Scope test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
  const created=await fetch(event,{method:"PUT",headers:{...mixed.headers,"if-none-match":"*"},body:content});
  assert.equal(created.status,201,await created.clone().text());
  assert.equal((await fetch(event,{headers:mixed.headers})).status,403);
  assert.match(await (await fetch(event,{headers:read.headers})).text(),/Scope test/);
  assert.equal((await fetch(event,{method:"DELETE",headers:{...mixed.headers,"if-match":created.headers.get("etag")!}})).status,204);
  await fetch(base+"/api/v1/auth/app-passwords/"+mixed.id,{method:"DELETE",headers:admin.headers});
  assert.equal((await fetch(file,{method:"DELETE",headers:{...mixed.headers,"if-match":put.headers.get("etag")!}})).status,401);
});
