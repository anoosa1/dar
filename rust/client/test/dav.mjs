// WebDAV, CardDAV and CalDAV through the Rust runtime with app passwords: folders, uploads,
// listing, ranges, moves, contacts and events, and what the browser sees of them.
// Usage: DAR_RUST_ORIGIN=… node dav.mjs
import assert from "node:assert/strict";
const origin=process.env.DAR_RUST_ORIGIN;
const step=async(name,fn)=>{process.stdout.write(name+" … ");await fn();console.log("ok");};
const login=await fetch(origin+"/api/v1/auth/login",{method:"POST",headers:{origin,"content-type":"application/json"},body:JSON.stringify({username:"amina",password:"a long isolated test password"})});
const cookie=login.headers.getSetCookie()[0].split(";")[0],{csrfToken}=await login.json();
const created=await (await fetch(origin+"/api/v1/auth/app-passwords",{method:"POST",headers:{origin,cookie,"x-csrf-token":csrfToken,"content-type":"application/json"},body:JSON.stringify({label:"dav test",scope:"v2 files:read files:write"})})).json();
const auth="Basic "+Buffer.from(`amina:${created.secret}`).toString("base64"),base=created.filesDavUrl,run=Date.now().toString(36);
const dav=(method,path,init={})=>fetch(base+path,{...init,method,headers:{authorization:auth,...init.headers}});
const text=`hello from ${run}\n`;
await step("MKCOL and PUT",async()=>{assert.equal((await dav("MKCOL",`dav-${run}/`)).status,201);assert.equal((await dav("PUT",`dav-${run}/note.txt`,{body:text})).status,201);});
await step("GET returns the same bytes",async()=>{assert.equal(await (await dav("GET",`dav-${run}/note.txt`)).text(),text);});
await step("byte ranges",async()=>{const r=await dav("GET",`dav-${run}/note.txt`,{headers:{range:"bytes=0-4"}});assert.equal(r.status,206);assert.equal(await r.text(),"hello");});
await step("PROPFIND lists the folder",async()=>{const r=await dav("PROPFIND",`dav-${run}/`,{headers:{depth:"1"}});assert.equal(r.status,207);assert.match(await r.text(),new RegExp(`dav-${run}/note.txt`));});
await step("MOVE renames",async()=>{assert.equal((await dav("MOVE",`dav-${run}/note.txt`,{headers:{destination:`${base}dav-${run}/moved.txt`}})).status,201);assert.equal((await dav("GET",`dav-${run}/note.txt`)).status,404);});
await step("the browser's search sees it",async()=>{const found=await (await fetch(`${origin}/api/v1/search?q=moved`,{headers:{cookie}})).json();assert.ok(found.items.some(i=>i.title==="moved.txt"),JSON.stringify(found));});
await step("a wrong password is refused",async()=>{const r=await fetch(base,{method:"PROPFIND",headers:{authorization:"Basic "+Buffer.from(`amina:${created.secret.slice(0,-1)}x`).toString("base64"),depth:"0"}});assert.equal(r.status,401);});
await step("DELETE",async()=>{assert.equal((await dav("DELETE",`dav-${run}/moved.txt`)).status,204);});

// CardDAV and CalDAV with a contacts-and-calendar app password, on a fresh address book and
// calendar made through the browser API (and deleted at the end).
const api=async(method,path,body,etag)=>{const r=await fetch(origin+path,{method,headers:{origin,cookie,"x-csrf-token":csrfToken,"content-type":"application/json",...etag?{"if-match":etag}:{}},body:body&&JSON.stringify(body)});const value=await r.json();if(!r.ok)throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(value)}`);return value;};
const pimPassword=await (await fetch(origin+"/api/v1/auth/app-passwords",{method:"POST",headers:{origin,cookie,"x-csrf-token":csrfToken,"content-type":"application/json"},body:JSON.stringify({label:"pim test",scope:"v2 contacts:read contacts:write calendar:read calendar:write"})})).json();
const pimAuth="Basic "+Buffer.from(`amina:${pimPassword.secret}`).toString("base64");
const pim=(method,path,init={})=>fetch(origin+path,{...init,method,headers:{authorization:pimAuth,...init.headers}});
const book=await api("POST","/api/v1/contacts/books",{name:`DAV book ${run}`,color:"#2f7d8c"}),calendar=await api("POST","/api/v1/calendar/calendars",{name:`DAV calendar ${run}`,color:"#73396f"});
const books="/dav/addressbooks/amina/",calendars="/dav/calendars/amina/";
const vcard=["BEGIN:VCARD","VERSION:3.0",`UID:dav-${run}`,`FN:Dav Person ${run}`,"N:Person;Dav;;;","EMAIL;TYPE=home:dav@example.com","END:VCARD",""].join("\r\n");
const event=["BEGIN:VCALENDAR","VERSION:2.0","PRODID:-//Dar test//EN","BEGIN:VEVENT",`UID:dav-${run}`,"DTSTAMP:20261101T000000Z","DTSTART:20261203T100000Z","DTEND:20261203T110000Z",`SUMMARY:DAV meeting ${run}`,"RRULE:FREQ=DAILY;COUNT=3","END:VEVENT","END:VCALENDAR",""].join("\r\n");
let cardTag="";
await step("CardDAV lists the address book",async()=>{const r=await pim("PROPFIND",books,{headers:{depth:"1"}});assert.equal(r.status,207);assert.match(await r.text(),new RegExp(`${books}${book.id}/`));});
await step("CardDAV PUT creates a contact",async()=>{const r=await pim("PUT",`${books}${book.id}/dav-${run}.vcf`,{headers:{"content-type":"text/vcard","if-none-match":"*"},body:vcard});assert.equal(r.status,201);cardTag=r.headers.get("etag");assert.ok(cardTag);});
await step("the browser API sees it",async()=>{const found=await (await fetch(`${origin}/api/v1/contacts/people?book=${book.id}`,{headers:{cookie}})).json();assert.ok(found.items.some(i=>i.data.displayName===`Dav Person ${run}`),JSON.stringify(found));});
await step("CardDAV refuses a stale If-Match",async()=>{const r=await pim("PUT",`${books}${book.id}/dav-${run}.vcf`,{headers:{"content-type":"text/vcard","if-match":'"stale:0"'},body:vcard});assert.equal(r.status,412);});
await step("CardDAV GET returns the card",async()=>{const r=await pim("GET",`${books}${book.id}/dav-${run}.vcf`);assert.equal(r.status,200);assert.equal(r.headers.get("etag"),cardTag);assert.match(await r.text(),/EMAIL;TYPE=home:dav@example.com/);});
await step("CalDAV PUT creates a repeating event",async()=>{const r=await pim("PUT",`${calendars}${calendar.id}/dav-${run}.ics`,{headers:{"content-type":"text/calendar","if-none-match":"*"},body:event});assert.equal(r.status,201);});
await step("the browser API expands it",async()=>{const found=await (await fetch(`${origin}/api/v1/calendar/calendars/${calendar.id}/occurrences?from=2026-12-01T00:00:00Z&to=2026-12-10T00:00:00Z&timezone=UTC`,{headers:{cookie}})).json();assert.equal(found.items.filter(i=>i.title===`DAV meeting ${run}`).length,3,JSON.stringify(found));});
await step("CalDAV calendar-query by time range",async()=>{const query=`<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="20261204T000000Z" end="20261205T000000Z"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>`;
  const hit=await pim("REPORT",`${calendars}${calendar.id}/`,{headers:{depth:"1","content-type":"application/xml"},body:query});assert.equal(hit.status,207);assert.match(await hit.text(),new RegExp(`dav-${run}.ics`));
  const miss=await pim("REPORT",`${calendars}${calendar.id}/`,{headers:{depth:"1","content-type":"application/xml"},body:query.replace("20261204T000000Z","20261210T000000Z").replace("20261205T000000Z","20261211T000000Z")});assert.equal(miss.status,207);assert.doesNotMatch(await miss.text(),new RegExp(`dav-${run}.ics`));});
await step("a files-only password cannot read calendars",async()=>{const r=await fetch(origin+calendars,{method:"PROPFIND",headers:{authorization:auth,depth:"0"}});assert.ok([401,403].includes(r.status),String(r.status));});
await step("DELETE needs If-Match, then removes",async()=>{for(const path of [`${calendars}${calendar.id}/dav-${run}.ics`,`${books}${book.id}/dav-${run}.vcf`]){assert.equal((await pim("DELETE",path)).status,428);const tag=(await pim("GET",path)).headers.get("etag");assert.equal((await pim("DELETE",path,{headers:{"if-match":tag}})).status,204);}assert.equal((await pim("GET",`${books}${book.id}/dav-${run}.vcf`)).status,404);});
const etag=c=>`"${c.id}:${c.revision}"`;
await api("DELETE",`/api/v1/contacts/books/${book.id}`,undefined,etag(await api("GET",`/api/v1/contacts/books/${book.id}`)));
await api("DELETE",`/api/v1/calendar/calendars/${calendar.id}`,undefined,etag(await api("GET",`/api/v1/calendar/calendars/${calendar.id}`)));
