#!/usr/bin/env node
// A stand-in for dar-chat-bridge (next/bridge) for tests: same stdio protocol, no WhatsApp.
// The test drives it through files in DAR_FAKE_CHAT_CONTROL and reads what it did from
// <control>/log.jsonl. It never touches the network.
import {createInterface} from "node:readline";
import {appendFileSync,existsSync,readFileSync,readdirSync,renameSync} from "node:fs";
import path from "node:path";

const control=process.env.DAR_FAKE_CHAT_CONTROL;
if(!control)throw new Error("DAR_FAKE_CHAT_CONTROL is required");
const write=frame=>process.stdout.write(JSON.stringify(frame)+"\n");
const log=entry=>appendFileSync(path.join(control,"log.jsonl"),JSON.stringify(entry)+"\n");
const pending=new Map();let next=0;
const call=(method,params)=>new Promise((resolve,reject)=>{const id=`g${++next}`;pending.set(id,{resolve,reject});write({id,method,params});});
const emit=(account,event,data={})=>write({event,account,data});
const b64=text=>Buffer.from(text).toString("base64");
const linking=new Set(),connected=new Set();
const jid="15550001111@s.whatsapp.net";

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
      {id:"H3",chat:"15550002222@s.whatsapp.net",sender:"15550002222@s.whatsapp.net",fromMe:false,at:at+60_000,kind:"image",text:"The place",quotedId:"H2",quotedSender:"You",quotedText:"Yes!"},
      {id:"G1",chat:"120363000000000001@g.us",sender:"15550004444@s.whatsapp.net",senderName:"Mama",fromMe:false,at:at+90_000,kind:"text",text:"Eid mubarak"},
      {id:"G2",chat:"120363000000000001@g.us",sender:"15550005555@s.whatsapp.net",senderName:"Baba",fromMe:false,at:at+120_000,kind:"voice"},
      {id:"O1",chat:"15550003333@s.whatsapp.net",sender:"15550003333@s.whatsapp.net",fromMe:false,at,kind:"text",text:"Long time"}]});
  await call("ingest",{account,kind:"history",syncType:"INITIAL_BOOTSTRAP",final:true,progress:100,chats:[],messages:[]});
  const lazy=await call("kv.get",{account,keys:["evbuf/aa","evbuf/missing"]});
  log({method:"lazyCheck",account,ok:lazy.values["evbuf/aa"]===b64("buffered")&&!("evbuf/missing" in lazy.values)});
}

const handlers={
  hello:()=>({protocol:1,platforms:["whatsapp"]}),
  async link({account,phone}){
    linking.add(account);
    setTimeout(()=>{emit(account,"linkCode",{code:"2@fakeQrCode,"+account.slice(0,8)+",abc=",expiresIn:20});if(phone)emit(account,"pairingCode",{code:"K7QM2X9D"});},50);
    return {};
  },
  async connect({account}){
    const page=await call("kv.load",{account,cursor:""});
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
  send(params){
    log({method:"send",...params});
    if(!connected.has(params.account))throw Object.assign(new Error("WhatsApp is not connected."),{code:"notConnected"});
    if(params.text.includes("fail"))throw Object.assign(new Error("WhatsApp did not accept the message."),{code:"sendFailed"});
    return {id:params.id,at:Date.now()};
  },
  markRead(params){log({method:"markRead",...params});return {};},
};

createInterface({input:process.stdin}).on("line",async line=>{
  const frame=JSON.parse(line);
  if(!frame.method){const item=pending.get(frame.id);if(!item)return;pending.delete(frame.id);if(frame.error)item.reject(new Error(frame.error.message));else item.resolve(frame.result);return;}
  try{write({id:frame.id,result:await handlers[frame.method](frame.params??{})});}
  catch(error){write({id:frame.id,error:{code:error.code??"bridgeError",message:error.message}});}
}).on("close",()=>process.exit(0));

// Control files: <account>.<action>[.json]; each is handled once.
setInterval(async()=>{
  for(const name of readdirSync(control)){
    const match=/^([0-9a-f-]{36})\.(pair|incoming|receipt|loggedout)(\.json)?$/.exec(name);if(!match)continue;
    const file=path.join(control,name),done=file+".done";renameSync(file,done);
    const [,account,action]=match,data=match[3]?JSON.parse(readFileSync(done,"utf8")):{};
    try{
      if(action==="pair"&&linking.has(account))await pair(account);
      if(action==="incoming")await call("ingest",{account,kind:"messages",chats:data.chats??[],messages:data.messages});
      if(action==="receipt")await call("ingest",{account,kind:"receipts",...data});
      if(action==="loggedout"){connected.delete(account);emit(account,"loggedOut",{reason:"logged out from another device"});}
      log({method:"control",action,account,ok:true});
    }catch(error){log({method:"control",action,account,ok:false,error:error.message});}
  }
},100);
if(!existsSync(control))process.exit(1);
