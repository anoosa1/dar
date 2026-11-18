import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp,readFile,writeFile,chmod} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {createServer} from "node:net";
import {spawnSync} from "node:child_process";
import {startServer} from "../runtime/server.ts";
import {login} from "./integration-helpers.ts";
import {database} from "../runtime/database.ts";
import type {ChatAccount,ChatLinkState,ChatMessage,ChatMessagePage,ChatOverview,ChatThread,ChatThreadPage} from "../shared/chat.ts";

const fake=path.resolve(import.meta.dirname,"fixtures/fake-chat-bridge.mjs");
const sleep=(ms:number)=>new Promise(r=>setTimeout(r,ms));
async function until<T>(read:()=>Promise<T>,ok:(value:T)=>boolean,what:string,timeout=15000):Promise<T>{
  const deadline=Date.now()+timeout;let last:T|undefined;
  while(Date.now()<deadline){last=await read();if(ok(last))return last;await sleep(100);}
  throw new Error(`Timed out waiting for ${what}: ${JSON.stringify(last)}`);
}

test("Chat: link, import, send, receive, receipts, encrypted keys, restart, isolation, unlink and delete",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:240000},async()=>{
  const root=await mkdtemp("/tmp/dar-chat-acceptance-"),control=await mkdtemp("/tmp/dar-chat-control-"),name="dar-chat-"+randomUUID().slice(0,8),listener=createServer();
  await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const published=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",name,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});
  assert.equal(published.status,0,published.stdout+published.stderr);
  const sql=(query:string)=>{const r=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"sql",name,query,"--server",process.env.DAR_DATABASE_URL!,"--yes"],{encoding:"utf8",timeout:30000});assert.equal(r.status,0,r.stderr);return r.stdout;};
  // The runtime gives the bridge a minimal environment, so a wrapper tells the fake where its control files are.
  const wrapper=path.join(control,"bridge.sh");await writeFile(wrapper,`#!/bin/sh\nDAR_FAKE_CHAT_CONTROL=${JSON.stringify(control)} exec node ${JSON.stringify(fake)}\n`);await chmod(wrapper,0o755);
  const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,chatBridge:wrapper};
  let runtime=await startServer(config);
  const bridgeLog=async()=>(await readFile(path.join(control,"log.jsonl"),"utf8").catch(()=>"")).split("\n").filter(Boolean).map(line=>JSON.parse(line) as Record<string,unknown>);
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Chat fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});
    assert.equal(setup.status,200,await setup.text());
    let session=await login(base);
    const raw=(route:string,init:RequestInit={})=>fetch(base+"/api/v1/chat"+route,{...init,headers:{...session.headers,"content-type":"application/json",...init.headers}});
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST"):Promise<T>=>{const response=await raw(route,{method,...body!==undefined?{body:JSON.stringify(body)}:{}});assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return await response.json() as T;};
    const overview=()=>call<ChatOverview>("/overview");

    // The bridge says hello before Chat offers linking.
    const empty=await until(overview,o=>o.bridge.available,"the bridge");
    assert.deepEqual(empty.accounts,[]);

    // Link with a QR code; the fake "scans" when the test says so.
    const {accountId}=await call<{accountId:string}>("/accounts/link",{name:"Personal"});
    const qr=await until(()=>call<ChatLinkState>(`/accounts/${accountId}/link`),s=>s.state==="qr","a QR code");
    assert.match(qr.qr!,/^2@fakeQrCode,/);assert.ok(qr.expiresAt!>Date.now());
    let account=(await overview()).accounts[0];assert.equal(account.state,"linking");assert.equal(account.name,"Personal");
    await writeFile(path.join(control,`${accountId}.pair`),"");
    await until(()=>call<ChatLinkState>(`/accounts/${accountId}/link`),s=>s.state==="paired","pairing");
    account=(await until(overview,o=>o.accounts[0]?.state==="connected"&&!o.accounts[0].importing&&o.accounts[0].historyProgress===100,"the import")).accounts[0];
    assert.equal(account.jid,"15550001111@s.whatsapp.net");
    assert.deepEqual(account.counts,{chats:2,unread:2,groups:1,archived:1});
    assert.equal(account.unread,1,"a chat muted forever does not add to the account's unread count");
    assert.ok((await bridgeLog()).some(e=>e.method==="lazyCheck"&&e.ok===true),"lazy keys round-trip through the runtime");

    // Keys are stored encrypted, bound to their account and key.
    const stored=sql("SELECT value FROM chat_keys");
    assert.doesNotMatch(stored,new RegExp(Buffer.from("fake-device-secret").toString("base64")));assert.doesNotMatch(stored,/fake-device-secret|session-secret/);
    assert.match(sql("SELECT scan_key FROM chat_keys"),new RegExp(`${accountId}/l/evbuf/aa`));

    // Chat list: pinned first, then newest first; views filter.
    const chats=await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=chats`);
    assert.deepEqual(chats.pinned.map(t=>t.name),["Sara Haddad"]);assert.deepEqual(chats.threads.map(t=>t.name),["Family"]);
    const sara=chats.pinned[0],family=chats.threads[0];
    assert.equal(sara.preview,"The place");assert.equal(sara.previewKind,"image");assert.equal(sara.unread,1);
    assert.equal(family.previewKind,"voice");assert.equal(family.previewSender,"Baba");assert.equal(family.mutedUntil,-1);
    assert.deepEqual((await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=archived`)).threads.map(t=>t.name),["Old friend"]);
    assert.deepEqual((await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=groups`)).threads.map(t=>t.name),["Family"]);
    assert.deepEqual((await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=unread`)).threads.map(t=>t.name).sort(),["Family","Sara Haddad"]);

    // A conversation reads oldest to newest, with quotes and statuses; opening it marks it read on WhatsApp.
    let page=await call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(sara.id)}`);
    assert.deepEqual(page.messages.map(m=>m.messageId),["H1","H2","H3"]);
    assert.equal(page.messages[1].status,"read");assert.equal(page.messages[2].quotedText,"Yes!");
    await call("/read",{threadId:sara.id});
    assert.equal((await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=chats`)).pinned[0].unread,0);
    const read=await until(bridgeLog,log=>log.some(e=>e.method==="markRead"),"read receipts");
    assert.deepEqual(read.find(e=>e.method==="markRead"),{method:"markRead",account:accountId,chat:"15550002222@s.whatsapp.net",items:[{id:"H3",sender:"15550002222@s.whatsapp.net"}]});

    // Sending: the message is stored before it goes out, under the ID WhatsApp receives.
    const sent=await call<ChatMessage>("/send",{threadId:sara.id,text:"See you at 7",quotedId:"H1"},"POST");
    assert.equal(sent.status,"sent");assert.match(sent.messageId,/^3EB0[0-9A-F]{18}$/);assert.equal(sent.quotedText,"Are we still on for dinner?");
    const out=(await bridgeLog()).find(e=>e.method==="send")!;
    assert.equal(out.id,sent.messageId);assert.equal(out.chat,"15550002222@s.whatsapp.net");assert.equal(out.quotedId,"H1");assert.equal(out.quotedSender,"15550002222@s.whatsapp.net");
    const failed=await call<ChatMessage>("/send",{threadId:sara.id,text:"this will fail"},"POST");
    assert.equal(failed.status,"failed");
    assert.equal((await call<ChatMessage>("/retry",{threadId:sara.id,messageId:failed.messageId})).status,"failed");
    await call("/discard",{threadId:sara.id,messageId:failed.messageId});
    page=await call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(sara.id)}`);
    assert.deepEqual(page.messages.map(m=>m.messageId),["H1","H2","H3",sent.messageId]);
    assert.equal((await raw("/send",{method:"POST",body:JSON.stringify({threadId:sara.id,text:"  "})})).status,400);

    // Receipts upgrade our messages; incoming messages count as unread; edits and deletions apply.
    await writeFile(path.join(control,`${accountId}.receipt.json`),JSON.stringify({chat:"15550002222@s.whatsapp.net",ids:[sent.messageId],status:"delivered",fromMe:false}));
    await writeFile(path.join(control,`${accountId}.incoming.json`),JSON.stringify({chats:[{id:"15550002222@s.whatsapp.net",kind:"direct",name:"Sara Haddad"}],
      messages:[{id:"L1",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:Date.now(),kind:"text",text:"Perfect, see you at 7 then"}]}));
    const after=await until(()=>call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(sara.id)}`),p=>p.messages.length===5&&p.messages[3].status==="delivered","the receipt and incoming message");
    assert.equal(after.thread.unread,1);assert.equal(after.thread.preview,"Perfect, see you at 7 then");
    await sleep(300);
    await writeFile(path.join(control,`${accountId}.receipt.json`),JSON.stringify({chat:"15550002222@s.whatsapp.net",ids:[sent.messageId],status:"read",fromMe:false}));
    await writeFile(path.join(control,`${accountId}.incoming.json`),JSON.stringify({messages:[
      {id:"E1",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:Date.now(),kind:"edit",target:"L1",text:"Perfect, see you at 7:30 then"},
      {id:"R1",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:Date.now(),kind:"revoke",target:"H1"}]}));
    const changed=await until(()=>call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(sara.id)}`),p=>p.messages[3].status==="read"&&p.messages[4].edited&&p.messages[0].deleted,"the edit and deletion");
    assert.equal(changed.messages[4].text,"Perfect, see you at 7:30 then");assert.equal(changed.messages[0].text,"");assert.equal(changed.messages.length,5,"edits and deletions are not messages");
    assert.equal(changed.thread.preview,"Perfect, see you at 7:30 then");

    // Attachments download through the bridge on first use, once, into the blob store.
    const messagesOf=(threadId:string)=>call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(threadId)}`);
    const media=(threadId:string,messageId:string,extra="")=>raw(`/media?${new URLSearchParams({threadId,messageId})}${extra}`);
    const photo=(await messagesOf(sara.id)).messages.find(m=>m.messageId==="H3")!;
    assert.deepEqual(photo.media,{kind:"image",mime:"image/png",size:photo.media!.size,seconds:0,width:1200,height:900,state:"remote"});
    let response=await media(sara.id,"H3");assert.equal(response.status,200);assert.equal(response.headers.get("content-type"),"image/png");
    assert.equal(Buffer.from(await response.arrayBuffer()).subarray(1,4).toString(),"PNG");
    assert.equal((await messagesOf(sara.id)).messages.find(m=>m.messageId==="H3")!.media!.state,"stored");
    assert.match(sql("SELECT blob_id FROM chat_media"),/[a-f0-9]{64}/);
    assert.equal((await media(sara.id,"H3")).status,200);assert.equal((await bridgeLog()).filter(e=>e.method==="download"&&e.id==="H3").length,1,"a stored attachment is not downloaded again");
    // Documents download as attachments; HTML never renders inline.
    response=await media(family.id,"D1","&download=1");assert.equal(response.status,200);await response.arrayBuffer();
    assert.match(response.headers.get("content-disposition")!,/^attachment; filename\*=UTF-8''Eid%20lunch%20menu\.pdf/);
    response=await media(family.id,"X1");assert.equal(response.headers.get("content-type"),"application/octet-stream");assert.match(response.headers.get("content-disposition")!,/^attachment/);await response.arrayBuffer();
    // Expired media: the phone is asked to send it again, then it downloads.
    const old=(await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=archived`)).threads[0];
    response=await media(old.id,"EXP1");assert.equal(response.status,409);assert.match((await response.json() as {error:{message:string}}).error.message,/asked to send it again/);
    assert.equal((await messagesOf(old.id)).messages.find(m=>m.messageId==="EXP1")!.media!.state,"retrying");
    await writeFile(path.join(control,`${accountId}.resend.json`),JSON.stringify({chat:"15550003333@s.whatsapp.net",id:"EXP1"}));
    await until(()=>messagesOf(old.id),p=>p.messages.find(m=>m.messageId==="EXP1")!.media!.state==="stored","the resent attachment");
    // Save to Files; a second copy gets a numbered name.
    const saved=await call<{fileId:string;name:string}>("/save",{threadId:family.id,messageId:"D1",folderId:""});assert.equal(saved.name,"Eid lunch menu.pdf");
    assert.equal((await call<{name:string}>("/save",{threadId:family.id,messageId:"D1",folderId:""})).name,"Eid lunch menu (2).pdf");
    assert.equal(await (await fetch(`${base}/api/v1/files/${saved.fileId}/content`,{headers:session.headers})).text(),"%PDF-1.4 fake menu");
    // New voice notes download as they arrive; deleting a message for everyone removes its attachment.
    await writeFile(path.join(control,`${accountId}.incoming.json`),JSON.stringify({messages:[{id:"V1",chat:"120363000000000001@g.us",sender:"15550005555@s.whatsapp.net",senderName:"Baba",fromMe:false,at:Date.now(),kind:"voice",media:{mime:"audio/ogg",size:20,seconds:3}}]}));
    await until(()=>messagesOf(family.id),p=>p.messages.find(m=>m.messageId==="V1")?.media?.state==="stored","the automatic voice note download");
    await writeFile(path.join(control,`${accountId}.incoming.json`),JSON.stringify({messages:[{id:"R2",chat:"120363000000000001@g.us",sender:"15550004444@s.whatsapp.net",fromMe:false,at:Date.now(),kind:"revoke",target:"X1"}]}));
    await until(()=>messagesOf(family.id),p=>p.messages.find(m=>m.messageId==="X1")!.deleted,"the deletion");
    assert.equal((await messagesOf(family.id)).messages.find(m=>m.messageId==="X1")!.media,null);assert.doesNotMatch(sql("SELECT id FROM chat_media"),/\/X1/);

    // A new chat by phone number.
    const direct=await call<ChatThread>("/chats",{accountId,phone:"+1 555 000 9999"});
    assert.equal(direct.jid,"15550009999@s.whatsapp.net");assert.equal(direct.name,"+15550009999");
    assert.equal((await raw("/chats",{method:"POST",body:JSON.stringify({accountId,phone:"0555"})})).status,400);

    // After a restart the bridge reconnects from the stored, encrypted keys.
    await runtime.close();runtime=await startServer(config);session=await login(base);
    await until(overview,o=>o.bridge.available&&o.accounts[0]?.state==="connected","reconnecting after a restart",20000);
    const reconnect=(await bridgeLog()).find(e=>e.method==="connect")!;
    assert.equal(reconnect.device,"fake-device-secret");assert.deepEqual(reconnect.keys,["device","session/1555:0"],"lazy keys are not loaded at start");
    assert.ok((await bridgeLog()).some(e=>e.method==="lazyAfterRestart"&&e.ok===true));

    // Another person sees none of it.
    assert.equal((await fetch(base+"/api/v1/auth/users",{method:"POST",headers:{...session.headers,"content-type":"application/json"},body:JSON.stringify({username:"other",displayName:"Other",password:"a long isolated test password"})})).status,201);
    {
      const other=await login(base,"other"),as=(route:string,init:RequestInit={})=>fetch(base+"/api/v1/chat"+route,{...init,headers:{...other.headers,"content-type":"application/json"}});
      assert.deepEqual((await (await as("/overview")).json() as ChatOverview).accounts,[]);
      assert.equal((await as(`/threads?accountId=${accountId}&view=chats`)).status,404);
      assert.equal((await as(`/messages?threadId=${encodeURIComponent(sara.id)}`)).status,404);
      assert.equal((await as("/send",{method:"POST",body:JSON.stringify({threadId:sara.id,text:"hi"})})).status,404);
      assert.equal((await as(`/accounts/${accountId}`,{method:"DELETE"})).status,404);
    }

    // Backups carry chats, messages and attachments but no keys; a restored account comes back unlinked.
    const service=await database(process.env.DAR_DATABASE_URL!,name,process.env.DAR_DATABASE_TOKEN_FILE!),pin=randomUUID();
    const snapshot=await service.call<{format:string;chat:{accounts:unknown[];messages:{id:string}[];media:{blobId:string}[]};blobs:{id:string}[]}>("service_backup",[pin]);
    await service.call("service_release_backup",[pin]);
    assert.equal(snapshot.format,"dar-backup-v12");assert.equal(snapshot.chat.accounts.length,1);assert.ok(snapshot.chat.messages.length>=9);
    const kept=snapshot.chat.media.filter(m=>m.blobId).map(m=>m.blobId);assert.ok(kept.length>=3);for(const id of kept)assert.ok(snapshot.blobs.some(b=>b.id===id),"attachment blobs are in the backup");
    assert.doesNotMatch(JSON.stringify(snapshot),/fake-device-secret|ZmFrZS1kZXZpY2Ut/);
    const second=name+"-restore",republished=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",second,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});
    assert.equal(republished.status,0,republished.stderr);
    const restored=await database(process.env.DAR_DATABASE_URL!,second,process.env.DAR_DATABASE_TOKEN_FILE!);await restored.call("service_restore",[JSON.stringify(snapshot)]);
    const restoredSql=(query:string)=>spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"sql",second,query,"--server",process.env.DAR_DATABASE_URL!,"--yes"],{encoding:"utf8"}).stdout;
    assert.match(restoredSql("SELECT state, detail FROM chat_accounts"),/loggedOut.*restored/);assert.doesNotMatch(restoredSql("SELECT id FROM chat_keys"),new RegExp(accountId));
    assert.match(restoredSql("SELECT blob_id FROM chat_media"),new RegExp(kept[0]));

    // Unlinking keeps history, read-only, and removes the keys.
    const unlinked=await call<{confirmed:boolean}>(`/accounts/${accountId}/unlink`,{});
    assert.equal(unlinked.confirmed,true);
    account=(await overview()).accounts[0];assert.equal(account.state,"loggedOut");
    assert.equal((await call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(sara.id)}`)).messages.length,5);
    assert.equal((await raw("/send",{method:"POST",body:JSON.stringify({threadId:sara.id,text:"hello?"})})).status,409);
    assert.doesNotMatch(sql("SELECT id FROM chat_keys"),new RegExp(accountId));

    // An unlinked phone (WhatsApp logs Dar out) is reported the same way.
    // Disabling Chat hides it; deleting the account removes everything.
    const settings=await (await fetch(base+"/api/v1/admin/installedApps",{headers:session.headers})).json() as {revision:number};
    const toggle=(enabled:boolean,revision:number)=>fetch(base+"/api/v1/admin/installedApps",{method:"PUT",headers:{...session.headers,"content-type":"application/json","if-match":`"apps:${revision}"`},body:JSON.stringify({id:"chat",enabled,revision})});
    const disabled=await toggle(false,settings.revision);assert.equal(disabled.status,200,await disabled.clone().text());
    assert.equal((await raw("/overview")).status,404);
    assert.equal((await toggle(true,(await disabled.json() as {revision:number}).revision)).status,200);
    await call(`/accounts/${accountId}`,undefined,"DELETE");
    assert.deepEqual((await overview()).accounts,[]);
    assert.doesNotMatch(sql("SELECT id FROM chat_threads"),new RegExp(accountId));assert.doesNotMatch(sql("SELECT id FROM chat_messages"),new RegExp(accountId));
    const linkedAgain=await call<{accountId:string}>("/accounts/link",{name:"Work",phone:"+966551230142"});
    const code=await until(()=>call<ChatLinkState>(`/accounts/${linkedAgain.accountId}/link`),s=>s.state==="code","a pairing code");
    assert.equal(code.code,"K7QM2X9D");
    await call(`/accounts/${linkedAgain.accountId}/link`,undefined,"DELETE");
    assert.deepEqual((await overview()).accounts as ChatAccount[],[],"a cancelled link leaves no account behind");
  }finally{await runtime.close();}
});
