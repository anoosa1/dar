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
import type {ChatInfo,ChatLinkState,ChatMessage,ChatMessagePage,ChatOverview,ChatServer,ChatThread,ChatThreadPage} from "../shared/chat.ts";

const fake=path.resolve(import.meta.dirname,"fixtures/fake-chat-bridge.mjs");
const sleep=(ms:number)=>new Promise(r=>setTimeout(r,ms));
async function until<T>(read:()=>Promise<T>,ok:(value:T)=>boolean,what:string,timeout=15000):Promise<T>{
  const deadline=Date.now()+timeout;let last:T|undefined;
  while(Date.now()<deadline){last=await read();if(ok(last))return last;await sleep(100);}
  throw new Error(`Timed out waiting for ${what}: ${JSON.stringify(last)}`);
}

test("Chat Discord: QR sign-in, servers and channels, DMs, send with nonce, reactions, live changes, history, mute, voice, restart, backups and sign-out",{skip:!process.env.DAR_INTEGRATION_ORIGIN,timeout:240000},async()=>{
  const root=await mkdtemp("/tmp/dar-chat-discord-"),control=await mkdtemp("/tmp/dar-chat-discord-control-"),name="dar-discord-"+randomUUID().slice(0,8),listener=createServer();
  await new Promise<void>(r=>listener.listen(0,"127.0.0.1",r));const port=(listener.address() as import("node:net").AddressInfo).port;await new Promise<void>(r=>listener.close(()=>r()));const base=`http://127.0.0.1:${port}`;
  const publish=(db:string)=>spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"publish",db,"--js-path",path.resolve(import.meta.dirname,"../module/dist/bundle.js"),"--server",process.env.DAR_DATABASE_URL!,"--yes","--no-config"],{env:{...process.env,DAR_ISSUER:base+"/auth/issuer"},encoding:"utf8",timeout:30000});
  const published=publish(name);assert.equal(published.status,0,published.stdout+published.stderr);
  const sql=(query:string,db=name)=>{const r=spawnSync(process.env.DAR_SPACETIME_BIN!,["--root-dir",process.env.DAR_SPACETIME_CLI_ROOT!,"sql",db,query,"--server",process.env.DAR_DATABASE_URL!,"--yes"],{encoding:"utf8",timeout:30000});assert.equal(r.status,0,r.stderr);return r.stdout;};
  const wrapper=path.join(control,"bridge.sh");await writeFile(wrapper,`#!/bin/sh\nDAR_FAKE_CHAT_CONTROL=${JSON.stringify(control)} exec node ${JSON.stringify(fake)}\n`);await chmod(wrapper,0o755);
  const config={host:"127.0.0.1",port,origin:base,root:path.join(root,"state"),databaseUrl:process.env.DAR_DATABASE_URL!,databaseName:name,serviceTokenFile:process.env.DAR_DATABASE_TOKEN_FILE!,bootstrapTokenFile:process.env.DAR_BOOTSTRAP_TOKEN_FILE!,development:true,chatBridge:wrapper};
  let runtime=await startServer(config);
  const bridgeLog=async()=>(await readFile(path.join(control,"log.jsonl"),"utf8").catch(()=>"")).split("\n").filter(Boolean).map(line=>JSON.parse(line) as Record<string,unknown>);
  const controlFile=(accountId:string,action:string,data?:unknown)=>writeFile(path.join(control,`${accountId}.${action}${data===undefined?"":".json"}`),data===undefined?"":JSON.stringify(data));
  try{
    const setup=await fetch(base+"/api/v1/auth/setup",{method:"POST",headers:{origin:base,"content-type":"application/json"},body:JSON.stringify({username:"admin",displayName:"Discord fixture",password:"a long isolated test password",bootstrapToken:await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!,"utf8")})});
    assert.equal(setup.status,200,await setup.text());
    let session=await login(base);
    const raw=(route:string,init:RequestInit={})=>fetch(base+"/api/v1/chat"+route,{...init,headers:{...session.headers,"content-type":"application/json",...init.headers}});
    const call=async<T>(route:string,body?:unknown,method=body===undefined?"GET":"POST"):Promise<T>=>{const response=await raw(route,{method,...body!==undefined?{body:JSON.stringify(body)}:{}});assert.ok(response.ok,`${method} ${route}: ${response.status} ${await response.clone().text()}`);return await response.json() as T;};
    const overview=()=>call<ChatOverview>("/overview");
    await until(overview,o=>o.bridge.available,"the bridge");

    // QR sign-in: Discord's code, the scan, then the phone's confirmation.
    const {accountId}=await call<{accountId:string}>("/accounts/link",{platform:"discord"});
    const qr=await until(()=>call<ChatLinkState>(`/accounts/${accountId}/link`),s=>s.state==="qr","a Discord QR code");
    assert.match(qr.qr!,/^https:\/\/discord\.com\/ra\//);assert.ok(qr.expiresAt!-Date.now()>60_000);
    let account=(await overview()).accounts[0];assert.equal(account.platform,"discord");assert.equal(account.name,"Discord");
    await controlFile(accountId,"scan");
    const scanned=await until(()=>call<ChatLinkState>(`/accounts/${accountId}/link`),s=>s.state==="scanned","the scan");assert.equal(scanned.name,"anas");
    await controlFile(accountId,"pair");
    await until(()=>call<ChatLinkState>(`/accounts/${accountId}/link`),s=>s.state==="paired","the sign-in");
    account=(await until(overview,o=>o.accounts[0]?.state==="connected","connected")).accounts[0];
    assert.equal(account.jid,"100");
    // The token is stored encrypted.
    assert.doesNotMatch(sql("SELECT value FROM chat_keys"),new RegExp(`fake-discord-token|${Buffer.from("fake-discord-token").toString("base64")}`));
    assert.match(sql("SELECT scan_key FROM chat_keys"),new RegExp(`${accountId}/c/discord/token`));

    // Servers with their channels in Discord's order; server channels stay out of the DM list.
    const servers=async()=>(await call<{servers:ChatServer[]}>(`/servers?accountId=${accountId}`)).servers;
    let tree=await until(servers,list=>list.length===2,"servers");
    assert.deepEqual(tree.map(s=>s.name),["Homelab Club","Kernel newbies"]);
    assert.deepEqual(tree[0].channels.map(c=>c.name),["general","nixos","off-topic","Lounge"]);
    const general=tree[0].channels[0],lounge=tree[0].channels[3];
    assert.equal(general.section,"Text channels");assert.equal(general.topic,"Say hello");assert.equal(general.mentions,1);assert.equal(lounge.kind,"voice");
    assert.equal(tree[0].mentions,1);assert.equal(tree[1].mutedUntil,-1);
    const dms=await call<ChatThreadPage>(`/threads?accountId=${accountId}&view=chats`);
    assert.deepEqual(dms.threads.map(t=>t.name),["Lina K"]);
    account=(await overview()).accounts[0];
    assert.equal(account.counts.chats,1);assert.equal(account.unread,2,"an unread DM and a channel that mentions the account");

    // Messages: Discord reactions are counts, the account's own marked.
    const messagesOf=(threadId:string)=>call<ChatMessagePage>(`/messages?threadId=${encodeURIComponent(threadId)}`);
    let page=await messagesOf(general.id);
    assert.deepEqual(page.messages.map(m=>m.text||m.fileName),["Anyone moved to the new caddy module?","Yes, painless","rack.png"]);
    const painless=page.messages[1];
    assert.deepEqual(painless.reactions.map(r=>[r.emoji,r.count,r.mine]),[["👍",3,true],["party:55",1,false]]);
    // Several reactions per person; removing one keeps the others.
    await call("/react",{threadId:general.id,messageId:painless.messageId,emoji:"👍",remove:true});
    await call("/react",{threadId:general.id,messageId:painless.messageId,emoji:"🎉"});
    page=await messagesOf(general.id);
    assert.deepEqual(page.messages[1].reactions.map(r=>[r.emoji,r.count,r.mine]).sort(),[["party:55",1,false],["👍",2,false],["🎉",1,true]].sort());
    const reacted=(await bridgeLog()).filter(e=>e.method==="react");
    assert.deepEqual(reacted.map(e=>[e.emoji,e.remove??false,e.chat,e.id]),[["👍",true,"911",painless.messageId],["🎉",false,"911",painless.messageId]]);
    assert.equal((await raw("/react",{method:"POST",body:JSON.stringify({threadId:general.id,messageId:painless.messageId,emoji:""})})).status,400);

    // The attachment downloads under its file name.
    const media=await raw(`/media?threadId=${encodeURIComponent(general.id)}&messageId=1300000000000000013&download=1`);
    assert.equal(media.status,200);assert.match(media.headers.get("content-disposition")??"",/rack\.png/);assert.equal((await media.arrayBuffer()).byteLength>0,true);

    // Sending: the pending copy is stored under a nonce and moves to Discord's ID, whichever arrives first.
    const sent=await call<ChatMessage>("/send",{threadId:general.id,text:"hello there",quotedId:painless.messageId});
    const sentLog=(await bridgeLog()).filter(e=>e.method==="send").at(-1)!;
    assert.match(String(sentLog.id),/^\d{17,20}$/);assert.equal(sentLog.chat,"911");assert.equal(sentLog.quotedId,painless.messageId);
    assert.equal(sent.messageId,String(BigInt(String(sentLog.id))+7n));assert.equal(sent.status,"sent");
    await call<ChatMessage>("/send",{threadId:general.id,text:"echo first"});
    page=await messagesOf(general.id);
    assert.equal(page.messages.filter(m=>m.text==="echo first").length,1,"the gateway's copy and the answer make one message");
    assert.equal(page.messages.filter(m=>m.text==="hello there").length,1);
    assert.ok(page.messages.every(m=>m.status!=="pending"));
    assert.equal((await raw("/send",{method:"POST",body:JSON.stringify({threadId:general.id,text:"x".repeat(4001)})})).status,400);
    const failed=await call<ChatMessage>("/send",{threadId:general.id,text:"this will fail"});assert.equal(failed.status,"failed");
    await call("/discard",{threadId:general.id,messageId:failed.messageId});

    // Editing has no time limit on Discord; deleting removes it for everyone.
    await call("/edit",{threadId:general.id,messageId:sent.messageId,text:"hello again"});
    await call("/revoke",{threadId:general.id,messageId:sent.messageId});
    page=await messagesOf(general.id);
    assert.equal(page.messages.find(m=>m.messageId===sent.messageId)?.deleted,true);
    assert.ok((await bridgeLog()).some(e=>e.method==="edit"&&e.text==="hello again"&&e.chat==="911"));

    // Live: a mention adds to the channel's mentions; edits, deletions and reactions from others follow.
    const at=Date.now();
    await controlFile(accountId,"ingest",{kind:"messages",messages:[
      {id:"1300000000000000101",chat:"912",sender:"200",senderName:"lina.k",fromMe:false,at,kind:"text",text:"@Anas can you look?",mention:true},
      {id:"1300000000000000102",chat:"912",sender:"300",senderName:"tarek",fromMe:false,at:at+1,kind:"text",text:"typo hree"}]});
    tree=await until(servers,list=>list[0].channels[1].mentions===1,"a mention");
    const nixos=tree[0].channels[1];assert.equal(nixos.unread,2);assert.equal(nixos.preview,"typo hree");
    await controlFile(accountId,"ingest",{kind:"messages",messages:[{id:"1300000000000000102-edit",chat:"912",sender:"300",fromMe:false,at:at+2,kind:"edit",text:"typo here",target:"1300000000000000102"},
      {id:"1300000000000000101-delete",chat:"912",fromMe:false,at:at+3,kind:"revoke",target:"1300000000000000101"}]});
    page=await until(()=>messagesOf(nixos.id),p=>p.messages.some(m=>m.text==="typo here")&&p.messages.some(m=>m.deleted),"the edit and the deletion");
    await writeFile(path.join(control,`${accountId}.ingest.json`),JSON.stringify({kind:"reactions",items:[{chat:"912",id:"1300000000000000102",emoji:"😂",me:false,delta:1},{chat:"912",id:"1300000000000000102",emoji:"😂",me:false,delta:1}]}));
    page=await until(()=>messagesOf(nixos.id),p=>p.messages.find(m=>m.messageId==="1300000000000000102")?.reactions[0]?.count===2,"reactions from others");
    // Read on another device clears it here too.
    await writeFile(path.join(control,`${accountId}.ingest.json`),JSON.stringify({kind:"read",chat:"912"}));
    await until(servers,list=>list[0].channels[1].unread===0&&list[0].channels[1].mentions===0,"read elsewhere");

    // Opening a channel marks it read at the newest message.
    await writeFile(path.join(control,`${accountId}.ingest.json`),JSON.stringify({kind:"messages",messages:[{id:"1300000000000000103",chat:"911",sender:"200",senderName:"lina.k",fromMe:false,at:Date.now(),kind:"text",text:"ping <@100>",mention:true}]}));
    await until(servers,list=>list[0].channels[0].mentions===1,"a new mention in general");
    await call("/read",{threadId:general.id});
    const read=await until(bridgeLog,log=>log.some(e=>e.method==="markRead"),"the read mark");
    const mark=read.find(e=>e.method==="markRead")!;assert.equal(mark.chat,"911");assert.match(String(mark.last),/^\d+$/);assert.equal(mark.items,undefined);
    assert.equal((await servers())[0].channels[0].mentions,0);

    // Catching up asks from the newest message; earlier messages come straight from Discord.
    await call("/catchUp",{threadId:general.id});
    const caught=(await bridgeLog()).filter(e=>e.method==="requestHistory").at(-1)!;assert.match(String(caught.after),/^\d+$/);assert.equal(caught.before,undefined);
    const earlier=await call<{count:number;more:boolean}>("/history",{threadId:general.id});
    assert.deepEqual(earlier,{ok:true,count:1,more:false});
    assert.equal((await messagesOf(general.id)).messages[0].text,"Welcome to the channel");

    // Mute follows Discord's channel settings; pinning is WhatsApp's.
    await call("/chatSetting",{threadId:general.id,mutedUntil:-1});
    const muted=(await bridgeLog()).filter(e=>e.method==="chatSetting").at(-1)!;assert.deepEqual([muted.chat,muted.serverId,muted.mutedUntil],["911","900",-1]);
    assert.equal((await servers())[0].channels[0].mutedUntil,-1);
    assert.equal((await raw("/chatSetting",{method:"POST",body:JSON.stringify({threadId:general.id,pinned:true})})).status,400);
    const info=await call<ChatInfo>(`/info?threadId=${encodeURIComponent(general.id)}`);
    assert.equal(info.server?.name,"Homelab Club");assert.equal(info.account.platform,"discord");assert.equal(info.group,null);

    // A new direct message with a friend.
    const friends=await call<{friends:{id:string;name:string}[]}>(`/friends?accountId=${accountId}`);assert.deepEqual(friends.friends.map(f=>f.name),["Lina K","tarek"]);
    const opened=await call<ChatThread>("/chats",{accountId,user:"300"});assert.equal(opened.name,"tarek");assert.equal(opened.kind,"direct");
    assert.equal((await raw("/chats",{method:"POST",body:JSON.stringify({accountId,phone:"+15550009999"})})).status,400);

    // Voice: a live socket per account brings who is in each voice channel and the call. It needs
    // the session and this origin; the socket that owns the call carries its audio both ways.
    const liveUrl=`ws://127.0.0.1:${port}/api/v1/chat/live?accountId=${accountId}`;
    const refused=(headers:Record<string,string>,url=liveUrl)=>new Promise<boolean>(resolve=>{const ws=new WebSocket(url,{headers} as never);ws.onopen=()=>{ws.close();resolve(false);};ws.onerror=()=>resolve(true);});
    assert.ok(await refused({origin:base}),"a socket without the session");
    assert.ok(await refused({cookie:session.headers.cookie,origin:"http://127.0.0.1:1"}),"a socket from another origin");
    assert.ok(await refused({cookie:session.headers.cookie,origin:base},liveUrl.replace(accountId,randomUUID())),"a socket for someone else's account");
    type Live=Record<string,unknown>&{type:string};
    const openLive=async()=>{
      const ws=new WebSocket(liveUrl,{headers:{cookie:session.headers.cookie,origin:base}} as never),texts:Live[]=[],audio:Uint8Array[]=[];
      ws.binaryType="arraybuffer";
      ws.onmessage=event=>{if(event.data instanceof ArrayBuffer)audio.push(new Uint8Array(event.data));else texts.push(JSON.parse(String(event.data)));};
      await new Promise<void>((resolve,reject)=>{ws.onopen=()=>resolve();ws.onerror=()=>reject(new Error("the live socket was refused"));});
      return {ws,texts,audio,last:(type:string)=>texts.filter(t=>t.type===type).at(-1)};
    };
    const tab=await openLive(),other=await openLive();
    const hello=await until(async()=>tab.last("hello"),h=>!!h,"hello") as unknown as {servers:Record<string,Record<string,{id:string}[]>>;call:unknown};
    assert.deepEqual(hello.servers["900"]["921"].map(p=>p.id),["200","300"]);assert.equal(hello.call,null);
    assert.equal((await raw("/voice/join",{method:"POST",body:JSON.stringify({threadId:general.id})})).status,400,"a text channel cannot be joined");
    await call("/voice/join",{threadId:lounge.id,muted:false});
    assert.deepEqual((await bridgeLog()).find(e=>e.method==="voiceJoin"),{method:"voiceJoin",account:accountId,chat:"921",muted:false,deafened:false});
    tab.ws.send(JSON.stringify({type:"claim"}));
    const connectedCall=await until(async()=>tab.last("call"),c=>(c?.call as {state?:string}|null)?.state==="connected","the call to connect") as unknown as {call:{channelId:string;e2ee:boolean}};
    assert.equal(connectedCall.call.channelId,"921");assert.equal(connectedCall.call.e2ee,true);
    await until(async()=>other.last("owner"),o=>o?.elsewhere===true,"the other tab to see the call is elsewhere");
    assert.equal(tab.last("owner")!.here,true);
    await until(async()=>tab.last("voice"),v=>(v?.servers as Record<string,Record<string,unknown[]>>|undefined)?.["900"]?.["921"]?.length===3,"me in Lounge");
    // Microphone frames from the owning tab reach the bridge; another tab's are ignored. Lina's audio comes back as binary.
    const opus=new Uint8Array([2,0xfc,0xff,0xfe,1,2,3]);
    other.ws.send(opus);await sleep(200);assert.ok(!(await bridgeLog()).some(e=>e.method==="voiceAudio"),"audio from a tab that does not own the call");
    for(let i=0;i<3;i++)tab.ws.send(opus);
    await until(bridgeLog,log=>log.some(e=>e.method==="voiceAudio"&&e.bytes===6),"microphone audio at the bridge");
    await until(async()=>tab.audio.length,n=>n>=3,"Lina's audio");
    const frame=tab.audio[0];assert.equal(frame[0],1);assert.equal(new DataView(frame.buffer).getBigUint64(1),200n);assert.deepEqual([...frame.subarray(9)],[...opus.subarray(1)]);
    assert.equal(other.audio.length,0,"only the owning tab hears the call");
    // Mute and deafen go to Discord; the call code shows for an encrypted call.
    await call("/voice/set",{accountId,muted:true,deafened:false});
    await until(async()=>tab.last("call"),c=>(c?.call as {selfMute?:boolean}|null)?.selfMute===true,"muted");
    assert.equal((await call<{code:string}>(`/voice/code?accountId=${accountId}`)).code.length,30);
    assert.equal((await raw("/voice/set",{method:"POST",body:JSON.stringify({accountId,muted:"yes",deafened:false})})).status,400);
    // Discord ending the call says why; leaving ends it too.
    await controlFile(accountId,"voiceend");
    const ended=await until(async()=>tab.last("call"),c=>c?.call===null,"the call to end") as unknown as {ended:{reason:string;message:string}};
    assert.equal(ended.ended.reason,"disconnected");assert.match(ended.ended.message,/disconnected/);
    await call("/voice/join",{threadId:lounge.id,muted:true});tab.ws.send(JSON.stringify({type:"claim"}));
    await until(async()=>tab.last("call"),c=>(c?.call as {state?:string}|null)?.state==="connected","joining again");
    await call("/voice/leave",{accountId});
    await until(async()=>tab.last("call"),c=>c?.call===null,"leaving");
    // A call nobody hears any more is left after a while, as when Discord's app closes.
    await call("/voice/join",{threadId:lounge.id,muted:true});tab.ws.send(JSON.stringify({type:"claim"}));
    await until(async()=>tab.last("owner"),o=>o?.here===true,"owning the call");
    tab.ws.close();
    await until(bridgeLog,log=>log.filter(e=>e.method==="voiceLeave").length===2,"the orphaned call to be left",25000);
    other.ws.close();

    // Search finds channel messages under the channel's name.
    type Found={items:{group:string;title:string;detail:string;href:string}[]};
    const found=await (await fetch(base+"/api/v1/search?q=caddy",{headers:session.headers})).json() as Found;
    const hit=found.items.filter(i=>i.group==="Chat");assert.equal(hit.length,1);assert.equal(hit[0].title,"#general");assert.match(hit[0].detail,/lina\.k: Anyone moved/);

    // Channels and servers the account no longer sees go, with their messages.
    await writeFile(path.join(control,`${accountId}.ingest.json`),JSON.stringify({kind:"channels",serverId:"900",complete:true,chats:[general,nixos,lounge].map(c=>({id:c.jid,kind:c.kind,name:c.name,serverId:"900",section:c.section,order:c.order}))}));
    await until(servers,list=>list[0].channels.length===3,"a removed channel");
    await writeFile(path.join(control,`${accountId}.ingest.json`),JSON.stringify({kind:"servers",complete:true,servers:[{id:"900",name:"Homelab Club",order:"00000"}]}));
    await until(servers,list=>list.length===1,"a server left");
    assert.doesNotMatch(sql("SELECT id FROM chat_threads"),/\/951/);

    // Backups keep servers; one made before Discord (no new columns) still restores.
    const service=await database(process.env.DAR_DATABASE_URL!,name,process.env.DAR_DATABASE_TOKEN_FILE!),pin=randomUUID();
    const snapshot=await service.call<{chat:{servers:unknown[];threads:Record<string,unknown>[];reactions:Record<string,unknown>[]}}&Record<string,unknown>>("service_backup",[pin]);
    await service.call("service_release_backup",[pin]);
    assert.equal(snapshot.chat.servers.length,1);assert.doesNotMatch(JSON.stringify(snapshot),/fake-discord-token/);
    const old=structuredClone(snapshot);delete (old.chat as {servers?:unknown}).servers;
    for(const t of old.chat.threads)for(const key of ["serverId","section","order","topic","mentions"])delete t[key];
    for(const r of old.chat.reactions)delete r.count;
    const second=name+"-restore";assert.equal(publish(second).status,0);
    await (await database(process.env.DAR_DATABASE_URL!,second,process.env.DAR_DATABASE_TOKEN_FILE!)).call("service_restore",[JSON.stringify(old)]);
    assert.match(sql("SELECT state FROM chat_accounts",second),/loggedOut/);

    // After a restart the bridge reconnects with the stored token.
    await runtime.close();runtime=await startServer(config);session=await login(base);
    await until(overview,o=>o.bridge.available&&o.accounts[0]?.state==="connected","reconnecting after a restart",20000);
    const reconnect=(await bridgeLog()).find(e=>e.method==="connect")!;assert.equal(reconnect.platform,"discord");assert.equal(reconnect.token,"fake-discord-token");

    // Signing out keeps the messages, read-only, and deletes the token.
    const out=await call<{confirmed:boolean}>(`/accounts/${accountId}/unlink`,{});assert.equal(out.confirmed,true);
    assert.equal((await overview()).accounts[0].state,"loggedOut");
    assert.doesNotMatch(sql("SELECT id FROM chat_keys"),new RegExp(accountId));
    assert.ok((await messagesOf(general.id)).messages.length>0);
    await call(`/accounts/${accountId}`,undefined,"DELETE");
    assert.doesNotMatch(sql("SELECT id FROM chat_servers"),new RegExp(accountId));
  }finally{await runtime.close();}
});
