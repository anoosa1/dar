import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile} from "node:fs/promises";
import path from "node:path";
import http from "node:http";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync} from "node:child_process";
import {publishModule,startInstance} from "./instance.ts";
import {splitSubscription} from "../runtime/pim/subscriptions.ts";
import {summarize} from "../runtime/notes/library.ts";
import {login,upload} from "./integration-helpers.ts";
import type {Occurrence,PimCollection} from "../shared/pim.ts";
import type {NotesLibrary} from "../shared/workspace.ts";

const feed=(withRetro:boolean)=>["BEGIN:VCALENDAR","VERSION:2.0","PRODID:-//Test//EN",
  "BEGIN:VTIMEZONE","TZID:Europe/Berlin","BEGIN:STANDARD","DTSTART:19701025T030000","TZOFFSETFROM:+0200","TZOFFSETTO:+0100","RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU","END:STANDARD","END:VTIMEZONE",
  "BEGIN:VEVENT","UID:offsite@test","DTSTAMP:20261101T000000Z","DTSTART;VALUE=DATE:20261117","DTEND;VALUE=DATE:20261118","SUMMARY:Company offsite","END:VEVENT",
  "BEGIN:VEVENT","UID:standup@test","DTSTAMP:20261101T000000Z","DTSTART;TZID=Europe/Berlin:20261116T100000","DTEND;TZID=Europe/Berlin:20261116T103000","RRULE:FREQ=DAILY;COUNT=3","SUMMARY:Standup","END:VEVENT",
  "BEGIN:VEVENT","UID:standup@test","DTSTAMP:20261101T000000Z","RECURRENCE-ID;TZID=Europe/Berlin:20261117T100000","DTSTART;TZID=Europe/Berlin:20261117T110000","DTEND;TZID=Europe/Berlin:20261117T113000","SUMMARY:Standup (moved)","END:VEVENT",
  "BEGIN:VEVENT","DTSTAMP:20261101T000000Z","DTSTART:20261118T090000Z","SUMMARY:No UID","END:VEVENT",
  ...withRetro?["BEGIN:VEVENT","UID:retro@test","DTSTAMP:20261101T000000Z","DTSTART:20261118T170000Z","DTEND:20261118T180000Z","SUMMARY:Retro","END:VEVENT"]:[],
  "END:VCALENDAR",""].join("\r\n");

test("subscription feeds split by UID with overrides and timezones; note summaries read headings and prose",()=>{
  const {items,skipped}=splitSubscription(feed(true));
  assert.deepEqual(items.map(item=>item.uid).sort(),["offsite@test","retro@test","standup@test"]);assert.equal(skipped,1);
  const standup=items.find(item=>item.uid==="standup@test")!;assert.match(standup.content,/RECURRENCE-ID/);assert.match(standup.content,/BEGIN:VTIMEZONE/);assert.match(standup.href,/^[a-f0-9]{40}\.ics$/);
  assert.throws(()=>splitSubscription("<html>not a calendar</html>"),/iCalendar/);
  assert.deepEqual(summarize("Plan.md","---\ntitle: Weekly plan\ntags: [a]\n---\n# Ignored heading\n\n- [ ] **Buy** milk\n> [Link](https://x) and [[Page|alias]]"),{title:"Weekly plan",excerpt:"Buy milk Link and alias"});
  assert.deepEqual(summarize("Loose thoughts.md","Just text, no heading."),{title:"Loose thoughts",excerpt:"Just text, no heading."});
});

test("calendar subscriptions sync by link, stay read-only and show dated tasks; Notes live in Files",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:120000},async()=>{
  const root=await mkdtemp("/tmp/dar-subscriptions-acceptance-"),name="dar-subs-"+randomUUID().slice(0,8),listener=createServer();await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  publishModule(name, base);
  let retro=true,requests=0;const source=http.createServer((_req,res)=>{requests++;res.writeHead(200,{"content-type":"text/calendar"});res.end(feed(retro));});await new Promise<void>(r=>source.listen(0,"127.0.0.1",r));
  const feedUrl=`webcal://127.0.0.1:${(source.address() as import("node:net").AddressInfo).port}/team.ics`;
  const runtime=await startInstance({port,origin:base,root:path.join(root,"state"),databaseName:name,development:true,newsAllowPrivateNetworks:true});
  try{
    assert.equal((await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Subscriptions",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})})).status,200);
    const session=await login(base),headers={...session.headers,"content-type":"application/json"};
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST"):Promise<T>=>{const response=await fetch(base+route,{method,headers,...body!==undefined?{body:JSON.stringify(body)}:{}});assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return response.json() as Promise<T>;};
    const calendars=async()=>(await call<{items:PimCollection[]}>("/api/v1/calendar/calendars")).items;
    const occurrences=async(id:string,tasks=false)=>(await call<{items:Occurrence[]}>(`/api/v1/calendar/calendars/${id}/occurrences?${new URLSearchParams({from:"2026-11-15T00:00:00Z",to:"2026-11-22T00:00:00Z",timezone:"UTC",includeTasks:tasks?"1":"0"})}`)).items;
    async function until(check:()=>Promise<boolean>){const deadline=Date.now()+15000;while(!await check()){if(Date.now()>deadline)throw Error("timed out");await new Promise(r=>setTimeout(r,200));}}

    // Subscribe: the first refresh runs right away and lists every UID once (overrides included).
    const created=await call<PimCollection>("/api/v1/calendar/subscriptions",{name:"Team",color:"#2f7d8c",url:feedUrl,refreshMinutes:60});
    await until(async()=>(await calendars()).find(c=>c.id===created.id)?.subscription?.lastSyncAt!==0);
    const subscribed=(await calendars()).find(c=>c.id===created.id)!;assert.equal(subscribed.subscription!.url,feedUrl);assert.equal(subscribed.subscription!.lastError,"");
    let titles=(await occurrences(created.id)).map(o=>o.title).sort();assert.deepEqual(titles,["Company offsite","Retro","Standup","Standup","Standup (moved)"]);
    // Read-only everywhere; invalid links and intervals are refused.
    const write=await fetch(`${base}/api/v1/calendar/calendars/${created.id}/items`,{method:"POST",headers,body:JSON.stringify({data:{title:"Mine",start:"2026-11-19T10:00",end:"2026-11-19T11:00",timezone:"UTC",allDay:false}})});assert.equal(write.status,403);
    assert.equal((await fetch(base+"/api/v1/calendar/subscriptions",{method:"POST",headers,body:JSON.stringify({name:"Bad",url:"ftp://example.test/a.ics",refreshMinutes:60})})).status,400);
    assert.equal((await fetch(base+"/api/v1/calendar/subscriptions",{method:"POST",headers,body:JSON.stringify({name:"Bad",url:feedUrl,refreshMinutes:7})})).status,400);
    assert.equal((await call<{items:PimCollection[]}>("/api/v1/tasks/lists")).items.some(c=>c.id===created.id),false);
    // Refresh now: events the source dropped are removed.
    retro=false;const before=requests;await call(`/api/v1/calendar/calendars/${created.id}/refresh`,{});
    await until(async()=>requests>before&&!(await occurrences(created.id)).some(o=>o.title==="Retro"));
    // A broken link reports an error on the calendar and keeps the synced events.
    await call(`/api/v1/calendar/calendars/${created.id}/subscription`,{url:"http://127.0.0.1:9/missing.ics"},"PUT");
    await until(async()=>!!(await calendars()).find(c=>c.id===created.id)?.subscription?.lastError);
    titles=(await occurrences(created.id)).map(o=>o.title);assert.ok(titles.includes("Company offsite"));

    // Dated tasks appear in Calendar occurrences only when asked for.
    const list=await call<PimCollection>("/api/v1/tasks/lists",{name:"Errands",color:"#7c3aed"});
    await call("/api/v1/tasks/items",{collectionId:list.id,data:{title:"File taxes",status:"NEEDS-ACTION",allDay:true,start:"",due:"2026-11-19",timezone:"UTC"}});
    await call("/api/v1/tasks/items",{collectionId:list.id,data:{title:"Someday",status:"NEEDS-ACTION",allDay:true,start:"",due:""}});
    assert.deepEqual(await occurrences(list.id),[]);
    const tasks=await occurrences(list.id,true);assert.deepEqual(tasks.map(o=>[o.title,o.task,o.startAt.slice(0,10)]),[["File taxes",true,"2026-11-19"]]);

    // Notes: the library is the Markdown files in Files › docs/notes.
    const library=await call<NotesLibrary>("/api/v1/notes/library");assert.equal(library.path,"docs/notes");assert.deepEqual(library.notes,[]);
    await upload(base,session,"Groceries.md","# Groceries\n\nMilk and bread.\n",library.rootId);await upload(base,session,"photo.png","not a note",library.rootId);
    const notes=await call<NotesLibrary>("/api/v1/notes/library");assert.deepEqual(notes.notes.map(n=>[n.name,n.title,n.excerpt]),[["Groceries.md","Groceries","Milk and bread."]]);
    assert.deepEqual((await call<NotesLibrary>("/api/v1/notes/library?q=bread")).notes.length,1);assert.deepEqual((await call<NotesLibrary>("/api/v1/notes/library?q=absent")).notes,[]);
  }finally{await runtime.close();source.close();}
});
