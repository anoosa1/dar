#!/usr/bin/env node
// A stand-in for dar-chat-bridge (next/bridge) for tests: same stdio protocol, no WhatsApp.
// The test drives it through files in DAR_FAKE_CHAT_CONTROL and reads what it did from
// <control>/log.jsonl. It never touches the network.
import {createInterface} from "node:readline";
import {appendFileSync,existsSync,readFileSync,readdirSync,renameSync,writeFileSync} from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const control=process.env.DAR_FAKE_CHAT_CONTROL;
if(!control)throw new Error("DAR_FAKE_CHAT_CONTROL is required");
const write=frame=>process.stdout.write(JSON.stringify(frame)+"\n");
const log=entry=>appendFileSync(path.join(control,"log.jsonl"),JSON.stringify(entry)+"\n");
const pending=new Map();let next=0;
const call=(method,params)=>new Promise((resolve,reject)=>{const id=`g${++next}`;pending.set(id,{resolve,reject});write({id,method,params});});
const emit=(account,event,data={})=>write({event,account,data});
const b64=text=>Buffer.from(text).toString("base64");
const linking=new Set(),connected=new Set(),resent=new Set();
// Attachment bytes by message ID; a real 4×3 PNG so the browser test can render it.
function makePng(width,height){
  const chunk=(type,data)=>{const body=Buffer.concat([Buffer.from(type),data]),length=Buffer.alloc(4),crc=Buffer.alloc(4);length.writeUInt32BE(data.length);crc.writeUInt32BE(zlib.crc32(body));return Buffer.concat([length,body,crc]);};
  const header=Buffer.alloc(13);header.writeUInt32BE(width,0);header.writeUInt32BE(height,4);header[8]=8;header[9]=2;
  const rows=Buffer.concat(Array.from({length:height},()=>Buffer.concat([Buffer.from([0]),Buffer.alloc(width*3,0x9a)])));
  return Buffer.concat([Buffer.from("89504e470d0a1a0a","hex"),chunk("IHDR",header),chunk("IDAT",zlib.deflateSync(rows)),chunk("IEND",Buffer.alloc(0))]);
}
const png=makePng(4,3);
const files={H3:png,V1:Buffer.from("OggS fake voice note"),D1:Buffer.from("%PDF-1.4 fake menu"),X1:Buffer.from("<script>alert(1)</script>"),EXP1:png};
const jid="15550001111@s.whatsapp.net";

// Discord: the same protocol with snowflake IDs, a server with channels and a direct message.
const discord=new Set(),me="100",dcAt=Date.UTC(2026,10,17,9,0);
const dcFiles={"1300000000000000013":png};
async function discordPair(account){
  linking.delete(account);
  await call("kv.commit",{account,puts:[{key:"discord/token",value:b64("fake-discord-token")},{key:"discord/user",value:b64(JSON.stringify({id:me,username:"anas"}))}],deletes:[]});
  emit(account,"paired",{jid:me,name:"Anas"});
  connected.add(account);
  await discordReady(account);
  emit(account,"connected",{name:"Anas"});
}
async function discordReady(account){
  await call("ingest",{account,kind:"servers",complete:true,servers:[{id:"900",name:"Homelab Club",order:"00000",mutedUntil:0},{id:"950",name:"Kernel newbies",order:"00001",mutedUntil:-1}]});
  await call("ingest",{account,kind:"channels",serverId:"900",complete:true,chats:[
    {id:"911",kind:"channel",name:"general",serverId:"900",section:"Text channels",order:"000001/0/000001/911",topic:"Say hello",unread:1,mentions:1,mutedUntil:0,at:dcAt+60_000},
    {id:"912",kind:"channel",name:"nixos",serverId:"900",section:"Text channels",order:"000001/0/000002/912",topic:"",unread:0,mentions:0,mutedUntil:0,at:dcAt},
    {id:"913",kind:"channel",name:"off-topic",serverId:"900",section:"Text channels",order:"000001/0/000003/913",topic:"",unread:1,mentions:0,mutedUntil:-1,at:dcAt},
    {id:"921",kind:"voice",name:"Lounge",serverId:"900",section:"Voice channels",order:"000002/1/000000/921",topic:"",unread:0,mentions:0,mutedUntil:0,at:0}]});
  await call("ingest",{account,kind:"channels",serverId:"950",complete:true,chats:[{id:"951",kind:"channel",name:"welcome",serverId:"950",section:"",order:"000000/0/000000/951",unread:0,mentions:0,mutedUntil:0,at:dcAt}]});
  await call("ingest",{account,kind:"chats",chats:[{id:"700",kind:"direct",name:"Lina K",unread:1,mentions:1,mutedUntil:0,at:dcAt+120_000}]});
  voiceStates(account);
  await call("ingest",{account,kind:"history",messages:[
    {id:"1300000000000000011",chat:"911",sender:"200",senderName:"lina.k",fromMe:false,at:dcAt,kind:"text",text:"Anyone moved to the new caddy module?"},
    {id:"1300000000000000012",chat:"911",sender:"300",senderName:"tarek",fromMe:false,at:dcAt+30_000,kind:"text",text:"Yes, painless",reactions:[{emoji:"👍",count:3,me:true},{emoji:"party:55",count:1,me:false}]},
    {id:"1300000000000000013",chat:"911",sender:"300",senderName:"tarek",fromMe:false,at:dcAt+60_000,kind:"image",text:"",fileName:"rack.png",media:{mime:"image/png",size:png.length,width:4,height:3}},
    {id:"1300000000000000021",chat:"700",sender:"200",senderName:"Lina K",fromMe:false,at:dcAt+120_000,kind:"text",text:"Hopping into Lounge later?"}]});
}
const dcError=(message,code)=>Object.assign(new Error(message),{code});
// Voice: Lina and tarek are in Lounge; a call echoes the microphone back as Lina speaking.
const lounge=[{id:"200",name:"Lina K"},{id:"300",name:"tarek",muted:true}],calls=new Map();
function voiceStates(account){
  const call=calls.get(account),people=call?[...lounge,{id:me,name:"Anas",...call.muted?{muted:true}:{},...call.deafened?{deafened:true}:{}}]:lounge;
  emit(account,"voiceStates",{servers:{"900":{"921":people}},complete:true});
}
function callState(account){const c=calls.get(account);emit(account,"voiceCall",{state:c.state,serverId:"900",channelId:c.chat,selfMute:c.muted,selfDeaf:c.deafened,canSpeak:true,e2ee:c.state==="connected",protocol:1});}

async function pair(account){
  linking.delete(account);
  // Device keys go through the runtime: a cached key and a lazy key that expires.
  await call("kv.commit",{account,puts:[{key:"device",value:b64("fake-device-secret")},{key:"session/1555:0",value:b64("session-secret")},{key:"evbuf/aa",value:b64("buffered"),lazy:true,expiresAt:Date.now()+60_000}],deletes:[]});
  emit(account,"paired",{jid,platform:"android"});
  connected.add(account);emit(account,"connected",{pushName:"Me"});
  const at=Date.UTC(2026,10,16,9,0);
  await call("ingest",{account,kind:"history",syncType:"INITIAL_BOOTSTRAP",final:false,
    chats:[{id:"15550002222@s.whatsapp.net",kind:"direct",name:"Sara Haddad",unread:1,pinned:true,archived:false,mutedUntil:0,at:at+60_000},
      {id:"120363000000000001@g.us",kind:"group",name:"Family",unread:2,pinned:false,archived:false,mutedUntil:-1,at:at+120_000},
      {id:"15550003333@s.whatsapp.net",kind:"direct",name:"Old friend",unread:0,pinned:false,archived:true,mutedUntil:0,at:at}],
    messages:[{id:"H1",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at,kind:"text",text:"Are we still on for dinner?"},
      {id:"H2",chat:"15550002222@s.whatsapp.net",sender:jid,fromMe:true,at:at+30_000,kind:"text",text:"Yes!",status:"read"},
      {id:"H3",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:at+60_000,kind:"image",text:"The place",quotedId:"H2",quotedSender:"You",quotedText:"Yes!",media:{mime:"image/png",size:files.H3.length,width:1200,height:900}},
      {id:"D1",chat:"120363000000000001@g.us",sender:"15550004444@s.whatsapp.net",senderName:"Mama",fromMe:false,at:at+80_000,kind:"document",fileName:"Eid lunch menu.pdf",media:{mime:"application/pdf",size:files.D1.length}},
      {id:"X1",chat:"120363000000000001@g.us",sender:"15550004444@s.whatsapp.net",senderName:"Mama",fromMe:false,at:at+85_000,kind:"document",fileName:"page.html",media:{mime:"text/html",size:files.X1.length}},
      {id:"EXP1",chat:"15550003333@s.whatsapp.net",sender:"15550003333@s.whatsapp.net",fromMe:false,at:at+1000,kind:"image",media:{mime:"image/png",size:files.EXP1.length,width:1,height:1}},
      {id:"G1",chat:"120363000000000001@g.us",sender:"15550004444@s.whatsapp.net",senderName:"Mama",fromMe:false,at:at+90_000,kind:"text",text:"Eid mubarak"},
      {id:"G2",chat:"120363000000000001@g.us",sender:"15550005555@s.whatsapp.net",senderName:"Baba",fromMe:false,at:at+120_000,kind:"voice"},
      {id:"O1",chat:"15550003333@s.whatsapp.net",sender:"15550003333@s.whatsapp.net",fromMe:false,at,kind:"text",text:"Long time"}]});
  await call("ingest",{account,kind:"history",syncType:"INITIAL_BOOTSTRAP",final:true,progress:100,chats:[],messages:[]});
  const lazy=await call("kv.get",{account,keys:["evbuf/aa","evbuf/missing"]});
  log({method:"lazyCheck",account,ok:lazy.values["evbuf/aa"]===b64("buffered")&&!("evbuf/missing" in lazy.values)});
}

const handlers={
  hello:()=>({protocol:1,platforms:["whatsapp","discord"]}),
  async link({account,phone,platform}){
    linking.add(account);
    if(platform==="discord"){discord.add(account);setTimeout(()=>emit(account,"linkCode",{code:"https://discord.com/ra/fakeFingerprint"+account.slice(0,4),expiresIn:110}),50);return {};}
    discord.delete(account);
    setTimeout(()=>{emit(account,"linkCode",{code:"2@fakeQrCode,"+account.slice(0,8)+",abc=",expiresIn:20});if(phone)emit(account,"pairingCode",{code:"K7QM2X9D"});},50);
    return {};
  },
  async connect({account,platform}){
    const page=await call("kv.load",{account,cursor:""});
    if(platform==="discord"){
      discord.add(account);const token=page.entries.find(e=>e.key==="discord/token");
      if(!token)throw Object.assign(new Error("This account is not linked."),{code:"notLinked"});
      log({method:"connect",account,platform,token:Buffer.from(token.value,"base64").toString()});
      connected.add(account);setTimeout(()=>{void discordReady(account).then(()=>emit(account,"connected",{}));},20);
      return {jid:me};
    }
    const device=page.entries.find(e=>e.key==="device");
    if(!device)throw Object.assign(new Error("This account is not linked."),{code:"notLinked"});
    log({method:"connect",account,device:Buffer.from(device.value,"base64").toString(),keys:page.entries.map(e=>e.key).sort()});
    const lazy=await call("kv.get",{account,keys:["evbuf/aa"]});
    log({method:"lazyAfterRestart",account,ok:lazy.values["evbuf/aa"]===b64("buffered")});
    connected.add(account);setTimeout(()=>emit(account,"connected",{}),20);
    return {jid};
  },
  disconnect({account}){linking.delete(account);connected.delete(account);return {};},
  logout({account}){if(!connected.has(account))throw Object.assign(new Error("WhatsApp is not connected."),{code:"notConnected"});connected.delete(account);log({method:"logout",account});return {};},
  async send(params){
    log({method:"send",...params});
    if(discord.has(params.account)){
      if(!connected.has(params.account))throw dcError("Discord is not connected.","notConnected");
      if(params.text.includes("fail"))throw dcError("Discord did not accept this.","invalidInput");
      // Discord gives the message its own ID; with "echo" the gateway's copy arrives before the answer.
      const id=String(BigInt(params.id)+7n);
      if(params.text.includes("echo"))await call("ingest",{account:params.account,kind:"messages",messages:[{id,chat:params.chat,sender:me,senderName:"Anas",fromMe:true,at:Date.now(),kind:"text",text:params.text,nonce:params.id,
        ...params.quotedId?{quotedId:params.quotedId}:{}}]});
      return {id,at:Date.now()};
    }
    if(!connected.has(params.account))throw Object.assign(new Error("WhatsApp is not connected."),{code:"notConnected"});
    if(params.text.includes("fail"))throw Object.assign(new Error("WhatsApp did not accept the message."),{code:"sendFailed"});
    return {id:params.id,at:Date.now()};
  },
  markRead(params){log({method:"markRead",...params});return {};},
  react(params){log({method:"react",...params});return {id:"R"+Date.now(),at:Date.now()};},
  edit(params){log({method:"edit",...params});return {id:"E"+Date.now(),at:Date.now()};},
  revoke(params){log({method:"revoke",...params});return {id:"X"+Date.now(),at:Date.now()};},
  chatSetting(params){log({method:"chatSetting",...params});return {};},
  groupInfo({chat}){return {name:"Family",topic:"Eid plans",created:Date.UTC(2020,0,1),members:[{jid:jid,name:"Me",admin:true},{jid:"15550004444@s.whatsapp.net",name:"Mama",admin:true},{jid:"15550005555@s.whatsapp.net",name:"Baba"}]};},
  async requestHistory(params){
    log({method:"requestHistory",...params});
    if(discord.has(params.account)){
      if(params.before){
        await call("ingest",{account:params.account,kind:"history",messages:[{id:"1299999999999999990",chat:params.chat,sender:"200",senderName:"lina.k",fromMe:false,at:dcAt-86_400_000,kind:"text",text:"Welcome to the channel"}]});
        return {count:1,more:false};
      }
      if(params.after)return {count:0};
      return {count:0};
    }
    // The phone answers with up to 50 earlier messages, as on-demand history.
    setTimeout(()=>{void call("ingest",{account:params.account,kind:"history",syncType:"ON_DEMAND",final:true,progress:100,chats:[],
      messages:[{id:"H0",chat:params.chat,sender:params.chat,fromMe:false,at:params.at-86_400_000,kind:"text",text:"Hello from last week"}]});},50);
    return {};
  },
  voiceJoin({account,chat,muted,deafened}){
    log({method:"voiceJoin",account,chat,muted,deafened});
    if(!connected.has(account))throw dcError("Discord is not connected.","notConnected");
    if(chat!=="921")throw dcError("This voice channel no longer exists.","notFound");
    calls.set(account,{chat,muted,deafened,state:"connecting",frames:0});callState(account);
    setTimeout(()=>{const c=calls.get(account);if(!c)return;c.state="connected";callState(account);voiceStates(account);},60);
    return {};
  },
  voiceSet({account,muted,deafened}){
    log({method:"voiceSet",account,muted,deafened});const c=calls.get(account);if(!c)throw dcError("You are not in a call.","conflict");
    c.muted=muted;c.deafened=deafened;callState(account);voiceStates(account);return {};
  },
  voiceLeave({account}){
    log({method:"voiceLeave",account});const c=calls.get(account);if(!c)return {};
    calls.delete(account);emit(account,"voiceCall",{state:"ended",serverId:"900",channelId:c.chat,reason:"left"});voiceStates(account);return {};
  },
  voiceAudio({account,frames}){
    const c=calls.get(account);if(!c||c.muted)return;
    for(const frame of frames){
      if(++c.frames===1||c.frames%25===0)log({method:"voiceAudio",account,frames:c.frames,bytes:Buffer.from(frame,"base64").length});
      if(!c.deafened)emit(account,"voiceAudio",{user:"200",seq:c.frames,data:frame});
    }
  },
  voiceCode({account}){if(!calls.get(account))throw dcError("You are not in a call.","conflict");return {code:"483122207906411751234567890123"};},
  friends({account}){if(!discord.has(account))throw dcError("This is not available for this account.","unsupported");return {friends:[{id:"200",name:"Lina K",username:"lina"},{id:"300",name:"tarek",username:"tarek"}]};},
  async openDirect({account,user}){
    log({method:"openDirect",account,user});
    await call("ingest",{account,kind:"chats",chats:[{id:"7300",kind:"direct",name:"tarek"}]});
    return {chat:"7300"};
  },
  download({account,chat,id,path:file}){
    log({method:"download",account,chat,id});
    if(discord.has(account)){
      if(!dcFiles[id])throw dcError("This attachment cannot be downloaded.","noMedia");
      writeFileSync(file,dcFiles[id],{flag:"wx",mode:0o600});return {size:dcFiles[id].length};
    }
    if(!connected.has(account))throw Object.assign(new Error("WhatsApp is not connected."),{code:"notConnected"});
    if(path.dirname(file)!==process.env.DAR_CHAT_DOWNLOADS||!/^[0-9a-f-]{36}\.part$/.test(path.basename(file)))throw Object.assign(new Error("bad path"),{code:"invalidInput"});
    if(id==="EXP1"&&!resent.has(id))throw Object.assign(new Error("WhatsApp no longer has this file; your phone was asked to send it again."),{code:"retrying"});
    if(!files[id])throw Object.assign(new Error("This attachment cannot be downloaded."),{code:"noMedia"});
    writeFileSync(file,files[id],{flag:"wx",mode:0o600});return {size:files[id].length};
  },
};

createInterface({input:process.stdin}).on("line",async line=>{
  const frame=JSON.parse(line);
  if(!frame.method){const item=pending.get(frame.id);if(!item)return;pending.delete(frame.id);if(frame.error)item.reject(new Error(frame.error.message));else item.resolve(frame.result);return;}
  // Notifications (microphone audio) carry no id and get no answer.
  if(!frame.id){try{await handlers[frame.method]?.(frame.params??{});}catch{}return;}
  try{write({id:frame.id,result:await handlers[frame.method](frame.params??{})});}
  catch(error){write({id:frame.id,error:{code:error.code??"bridgeError",message:error.message}});}
}).on("close",()=>process.exit(0));

// Control files: <account>.<action>[.json]; each is handled once.
setInterval(async()=>{
  for(const name of readdirSync(control)){
    const match=/^([0-9a-f-]{36})\.(pair|incoming|receipt|loggedout|resend|scan|ingest|voiceend)(\.json)?$/.exec(name);if(!match)continue;
    const file=path.join(control,name),done=file+".done";renameSync(file,done);
    const [,account,action]=match,data=match[3]?JSON.parse(readFileSync(done,"utf8")):{};
    try{
      if(action==="scan"&&linking.has(account))emit(account,"linkScanned",{name:"anas"});
      if(action==="pair"&&linking.has(account))await (discord.has(account)?discordPair(account):pair(account));
      if(action==="ingest")await call("ingest",{account,...data});
      if(action==="incoming")await call("ingest",{account,kind:"messages",chats:data.chats??[],messages:data.messages});
      if(action==="receipt")await call("ingest",{account,kind:"receipts",...data});
      if(action==="resend"){resent.add(data.id);emit(account,"mediaRetried",{chat:data.chat,id:data.id,ok:true});}
      if(action==="voiceend"&&calls.has(account)){const c=calls.get(account);calls.delete(account);emit(account,"voiceCall",{state:"ended",serverId:"900",channelId:c.chat,reason:"disconnected"});voiceStates(account);}
      if(action==="loggedout"){connected.delete(account);emit(account,"loggedOut",{reason:"logged out from another device"});}
      log({method:"control",action,account,ok:true});
    }catch(error){log({method:"control",action,account,ok:false,error:error.message});}
  }
},100);
if(!existsSync(control))process.exit(1);
