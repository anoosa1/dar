import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readFile, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { login } from "./integration-helpers.ts";
const origin=process.env.DAR_INTEGRATION_ORIGIN;
if(!origin)throw new Error("An isolated instance is required.");
const root=await mkdtemp(path.join(tmpdir(),"dar-pim-client-")),session=await login(origin);
async function post(url,value){const r=await fetch(origin+url,{method:"POST",headers:{...session.headers,"content-type":"application/json"},body:JSON.stringify(value)});assert.equal(r.status,201,await r.clone().text());return r.json();}
let config=`[general]\nstatus_path = ${JSON.stringify(path.join(root,"status"))}\n`;
const cases=[];
for(const kind of ["contacts","calendar"]){
  const base=`/api/v1/${kind}/${kind==="contacts"?"books":"calendars"}`,collection=await post(base,{name:`Real client ${kind}`,color:"#2563eb"}),password=await post("/api/v1/auth/app-passwords",{label:`Real client ${kind}`,scope:`${kind}:write`}),suffix=kind==="contacts"?".vcf":".ics";
  const local=path.join(root,kind,collection.id);await mkdir(local,{recursive:true});
  const content=kind==="contacts"?"BEGIN:VCARD\r\nVERSION:3.0\r\nUID:real-client-contact\r\nFN:Real client contact\r\nN:contact;Real client;;;\r\nEND:VCARD\r\n":"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Client//EN\r\nBEGIN:VEVENT\r\nUID:real-client-event\r\nDTSTART:20261109T140000Z\r\nDTEND:20261109T150000Z\r\nSUMMARY:Real client event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
  const task="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:real-client-task\r\nSUMMARY:Real client task\r\nDUE;VALUE=DATE:20261110\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";if(kind==="calendar")await writeFile(path.join(local,"task.ics"),task);await writeFile(path.join(local,"external"+suffix),content);await writeFile(path.join(local,"anchor"+suffix),content.replace(/UID:([^\r]+)/,"UID:$1-anchor").replace(/(FN|SUMMARY):[^\r]+/,"$1:Retained anchor"));
  config+=`\n[pair ${kind}]\na = "${kind}_local"\nb = "${kind}_remote"\ncollections = [${JSON.stringify(collection.id)}]\n\n[storage ${kind}_local]\ntype = "filesystem"\npath = ${JSON.stringify(path.join(root,kind))}\nfileext = ${JSON.stringify(suffix)}\n\n[storage ${kind}_remote]\ntype = "${kind==="contacts"?"carddav":"caldav"}"\nurl = ${JSON.stringify(password.davUrl)}\nusername = "admin"\npassword = ${JSON.stringify(password.secret)}\n${kind==="calendar"?'item_types = ["VEVENT", "VTODO"]\n':""}`;
  cases.push({kind,base,collection,password,local,suffix});
}
const filename=path.join(root,"config");await writeFile(filename,config,{mode:0o600});
function client(...args){const result=spawnSync(process.env.DAR_DAV_CLIENT??"vdirsyncer",["--config",filename,...args],{encoding:"utf8",timeout:30000,env:{...process.env,HOME:root,NO_PROXY:"127.0.0.1,localhost"},maxBuffer:1024*1024});if(result.error)throw result.error;assert.equal(result.status,0,`vdirsyncer ${args[0]}: ${result.stdout}\n${result.stderr}`);return result.stdout+result.stderr;}
client("discover");client("sync");
for(const c of cases){const r=await fetch(`${origin}${c.base}/${c.collection.id}/items`,{headers:session.headers});assert.equal(r.status,200);const {items}=await r.json();assert.equal(items.length,c.kind==="calendar"?3:2);if(c.kind==="calendar"){const task=items.find(i=>i.uid==="real-client-task");assert.ok(task);const changed=await fetch(`${origin}${c.base}/${c.collection.id}/items/${task.href}`,{method:"PUT",headers:{...session.headers,"content-type":"application/json","if-match":`"${task.id}:${task.revision}"`},body:JSON.stringify({component:"task",data:{title:"Changed native task",status:"COMPLETED"}})});assert.equal(changed.status,200,await changed.clone().text());}const row=items.find(item=>item.uid.endsWith(c.kind==="contacts"?"contact":"event"));assert.ok(row);const content=row.content??await(await fetch(c.password.davUrl+c.collection.id+"/"+row.href,{headers:{authorization:`Basic ${Buffer.from(`admin:${c.password.secret}`).toString("base64")}`}})).text();const changed=await fetch(`${origin}${c.base}/${c.collection.id}/items/${row.href}`,{method:"PUT",headers:{...session.headers,"content-type":"application/json","if-match":`"${row.id}:${row.revision}"`},body:JSON.stringify({content:content.replace(/(FN|SUMMARY):[^\r]+/,"$1:Changed in native app")})});assert.equal(changed.status,200,await changed.clone().text());}
client("sync");
for(const c of cases){const files=await readdir(c.local);let changed=false;for(const file of files){if((await readFile(path.join(c.local,file),"utf8")).includes("Changed in native app"))changed=true;}assert.ok(changed,`${c.kind}: server change did not reach independent client`);if(c.kind==="calendar"){const contents=await Promise.all(files.map(f=>readFile(path.join(c.local,f),"utf8")));assert.ok(contents.some(s=>s.includes("Changed native task")&&s.includes("STATUS:COMPLETED")));}}
console.info("PASS vdirsyncer 0.20.0 discovers both homes, uploads contacts/events/tasks and receives native edits with conditional round trips.");
