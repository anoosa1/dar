import type {Ctx} from "./index.ts";
import {chatLimits,chatAutoDownloadBytes} from "../../shared/chat.ts";
import {type Account,type Thread,type Message,fail,text,optional,keys,num,now,prefix,changed,listKey,threadKey,setThread,newThread,validMedia,reviseMessage,dropMedia,messageId,messageKinds} from "./chat.ts";

const snowflake=/^[0-9]{1,25}$/;
/** A Discord ID: a server, channel, user or message. */
export function discordId(value:unknown){const v=text(value,25);if(!snowflake.test(v))fail("invalidInput");return v;}
const chatKinds=new Set(["direct","group","channel","voice"]);
const orderPattern=/^[0-9a-z/]{0,80}$/;

function record(value:unknown){if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");return value as Record<string,unknown>;}
const count=(value:unknown)=>value===undefined?undefined:Math.min(1_000_000,Math.max(0,Math.floor(num(value))));

function validServer(value:unknown){
  const s=record(value);keys(s,["id","name","order","mutedUntil"]);
  const order=optional(s.order,80);if(!orderPattern.test(order))fail("invalidInput");
  return {id:discordId(s.id),name:optional(s.name,200).trim()||"Server",order,mutedUntil:s.mutedUntil===undefined?0:num(s.mutedUntil)};
}
function validChat(value:unknown){
  const c=record(value);keys(c,["id","kind","name","serverId","section","order","topic","unread","mentions","mutedUntil","at"]);
  const kind=text(c.kind,20);if(!chatKinds.has(kind))fail("invalidInput");
  const order=optional(c.order,80);if(!orderPattern.test(order))fail("invalidInput");
  return {id:discordId(c.id),kind,name:optional(c.name,200).trim(),serverId:c.serverId===undefined||c.serverId===""?"":discordId(c.serverId),section:optional(c.section,200),order,
    topic:optional(c.topic,2048),unread:count(c.unread),mentions:count(c.mentions),mutedUntil:c.mutedUntil===undefined?undefined:num(c.mutedUntil),at:c.at===undefined?0:Math.max(0,num(c.at))};
}
function validReactions(value:unknown){
  if(value===undefined)return undefined;if(!Array.isArray(value)||value.length>100)fail("invalidInput");
  return value.map(item=>{const r=record(item);keys(r,["emoji","count","me"]);return {emoji:text(r.emoji,120,false),count:count(r.count)??0,me:r.me===true};});
}
function validMessage(value:unknown){
  const m=record(value);
  keys(m,["id","chat","sender","senderName","fromMe","at","kind","text","fileName","target","quotedId","quotedSender","quotedText","status","media","nonce","mention","reactions"]);
  const kind=text(m.kind,20);if(!messageKinds.has(kind)&&kind!=="edit"&&kind!=="revoke")fail("invalidInput");
  if(typeof m.fromMe!=="boolean")fail("invalidInput");
  return {id:messageId(m.id),chat:discordId(m.chat),sender:m.sender===undefined||m.sender===""?"":discordId(m.sender),senderName:optional(m.senderName,200),fromMe:m.fromMe,at:Math.max(0,num(m.at)),kind,
    text:optional(m.text,chatLimits.text),fileName:optional(m.fileName,500),target:m.target===undefined?"":discordId(m.target),quotedId:m.quotedId===undefined||m.quotedId===""?"":discordId(m.quotedId),
    quotedSender:optional(m.quotedSender,200),quotedText:optional(m.quotedText,2000),media:validMedia(m.media),nonce:m.nonce===undefined||m.nonce===""?"":messageId(m.nonce),
    mention:m.mention===true,reactions:validReactions(m.reactions)};
}

/** Removes a chat with its messages, attachments and reactions; returns how many messages went. */
function deleteThread(ctx:Ctx,t:Thread){
  let removed=0;
  for(const m of [...ctx.db.chatMessages.threadKey.filter(prefix(t.id))]){
    const media=ctx.db.chatMedia.id.find(m.id);if(media)dropMedia(ctx,media);
    for(const r of [...ctx.db.chatReactions.messageRowId.filter(m.id)])ctx.db.chatReactions.id.delete(r.id);
    ctx.db.chatMessages.id.delete(m.id);removed++;
  }
  ctx.db.chatThreads.id.delete(t.id);
  return removed;
}

function setRow<T extends {id:string}>(table:{id:{find:(id:string)=>T|null|undefined;update:(row:T)=>unknown};insert:(row:T)=>unknown},row:T){if(table.id.find(row.id))table.id.update(row);else table.insert(row);}

/** Discord reactions: the account's own per emoji, and a count for everyone else. */
function setDiscordReaction(ctx:Ctx,accountId:string,rowId:string,emoji:string,who:"me"|"*",count:number,at:number){
  const id=`${rowId}|${who}|${emoji}`;
  if(count<=0){if(ctx.db.chatReactions.id.find(id))ctx.db.chatReactions.id.delete(id);return;}
  setRow(ctx.db.chatReactions,{id,messageRowId:rowId,accountId,sender:who,senderName:"",fromMe:who==="me",emoji,at,count});
}
function replaceReactions(ctx:Ctx,accountId:string,rowId:string,list:{emoji:string;count:number;me:boolean}[],at:number){
  for(const r of [...ctx.db.chatReactions.messageRowId.filter(rowId)])ctx.db.chatReactions.id.delete(r.id);
  for(const r of list){if(r.me)setDiscordReaction(ctx,accountId,rowId,r.emoji,"me",1,at);setDiscordReaction(ctx,accountId,rowId,r.emoji,"*",r.count-(r.me?1:0),at);}
}
/** The account's own reaction, after Discord accepted it. */
export function applyDiscordReaction(ctx:Ctx,accountId:string,rowId:string,emoji:string,remove:boolean){setDiscordReaction(ctx,accountId,rowId,emoji,"me",remove?0:1,now(ctx));}

/**
 * Stores what the bridge sends for a Discord account: servers, each server's channels (a
 * complete list removes channels the account no longer sees, with their messages), direct
 * messages, messages, reactions and read marks from other devices.
 */
export function discordIngest(ctx:Ctx,account:Account,p:Record<string,unknown>){
  const kind=text(p.kind,20),live=kind==="messages";let stored=account.messages;const download:string[]=[];
  const chats=Array.isArray(p.chats)?p.chats.map(validChat):[];if(chats.length>5000)fail("operationTooLarge");
  const upsert=(c:ReturnType<typeof validChat>,create:boolean)=>{
    const id=`${account.id}/${c.id}`,old=ctx.db.chatThreads.id.find(id);if(!old&&!create)return;
    const base=old??newThread(account,c.id,c.kind,0),next:Thread={...base,kind:c.kind,serverId:c.serverId,section:c.section,order:c.order,topic:c.topic};
    if(c.name)next.name=c.name;
    if(c.unread!==undefined)next.unread=c.unread;
    if(c.mentions!==undefined)next.mentions=c.mentions;
    if(c.mutedUntil!==undefined)next.mutedUntil=c.mutedUntil;
    if(c.at>next.at){next.at=c.at;next.listKey=listKey(account.id,c.at,c.id);}
    if(old)setThread(ctx,old,next);else ctx.db.chatThreads.insert(next);
  };
  if(kind==="servers"){
    const servers=Array.isArray(p.servers)?p.servers.map(validServer):fail("invalidInput");if(servers.length>1000)fail("operationTooLarge");
    for(const s of servers)setRow(ctx.db.chatServers,{id:`${account.id}/${s.id}`,accountId:account.id,serverId:s.id,name:s.name,order:s.order,mutedUntil:s.mutedUntil});
    if(p.complete===true){
      // Servers the account left go with their channels.
      const keep=new Set(servers.map(s=>s.id));
      for(const row of [...ctx.db.chatServers.accountId.filter(account.id)])if(!keep.has(row.serverId))ctx.db.chatServers.id.delete(row.id);
      for(const t of [...ctx.db.chatThreads.accountId.filter(account.id)])if(t.serverId&&!keep.has(t.serverId))stored-=deleteThread(ctx,t);
    }
  }else if(kind==="channels"){
    const serverId=discordId(p.serverId);
    for(const c of chats){if(c.serverId!==serverId||c.kind!=="channel"&&c.kind!=="voice")fail("invalidInput");upsert(c,true);}
    if(p.complete===true){
      const keep=new Set(chats.map(c=>c.id));
      for(const t of [...ctx.db.chatThreads.accountId.filter(account.id)])if(t.serverId===serverId&&!keep.has(t.jid))stored-=deleteThread(ctx,t);
    }
  }else if(kind==="chats"){
    for(const c of chats){if(c.serverId||c.kind!=="direct"&&c.kind!=="group")fail("invalidInput");upsert(c,true);}
  }else if(kind==="messages"||kind==="history"){
    // A new direct message brings its chat along; server channels must already be known.
    for(const c of chats){if(c.serverId||c.kind!=="direct"&&c.kind!=="group")fail("invalidInput");upsert(c,true);}
    const messages=Array.isArray(p.messages)?p.messages.map(validMessage):[];if(messages.length>2000)fail("operationTooLarge");
    for(const m of messages.sort((a,b)=>a.at-b.at)){
      const t=ctx.db.chatThreads.id.find(`${account.id}/${m.chat}`);if(!t)continue;
      if(m.kind==="edit"||m.kind==="revoke"){
        const target=ctx.db.chatMessages.id.find(`${t.id}/${m.target}`);
        if(target&&!target.deleted&&(m.kind==="revoke"||target.text!==m.text))reviseMessage(ctx,t,target,m.kind==="edit"?{text:m.text}:{deleted:true});
        continue;
      }
      const id=`${t.id}/${m.id}`;
      // Our own message coming back under its real ID replaces the pending copy Dar stored under the nonce.
      if(m.fromMe&&m.nonce&&m.nonce!==m.id){const pending=ctx.db.chatMessages.id.find(`${t.id}/${m.nonce}`);if(pending){ctx.db.chatMessages.id.delete(pending.id);stored--;}}
      const old=ctx.db.chatMessages.id.find(id);
      if(old){
        // Fetched again: reaction counts may have changed meanwhile.
        if(m.reactions&&!old.deleted)replaceReactions(ctx,account.id,old.id,m.reactions,m.at);
        continue;
      }
      if(stored>=chatLimits.messagesPerAccount)break;
      const row:Message={id,accountId:account.id,threadKey:threadKey(t.id,m.at,m.id),messageId:m.id,sender:m.sender,senderName:m.senderName,fromMe:m.fromMe,at:m.at,kind:m.kind,
        text:m.text,fileName:m.fileName,quotedId:m.quotedId,quotedSender:m.quotedSender,quotedText:m.quotedText,status:m.fromMe?"sent":"",edited:false,deleted:false};
      ctx.db.chatMessages.insert(row);stored++;
      if(m.reactions?.length)replaceReactions(ctx,account.id,id,m.reactions,m.at);
      if(m.media){
        ctx.db.chatMedia.insert({id,accountId:account.id,blobId:"",kind:m.kind,mime:m.media.mime,size:m.media.size,fileName:m.fileName,seconds:m.media.seconds,width:m.media.width,height:m.media.height,state:"remote",at:m.at});
        if(live&&["image","sticker","voice"].includes(m.kind)&&m.media.size<=chatAutoDownloadBytes)download.push(id);
      }
      const current=ctx.db.chatThreads.id.find(t.id)!,next={...current};
      if(m.at>=current.at)Object.assign(next,{at:m.at,listKey:listKey(account.id,m.at,current.jid),preview:m.text||m.fileName,previewKind:m.kind,previewFromMe:m.fromMe,
        previewSender:current.kind!=="direct"&&!m.fromMe?m.senderName:"",previewSenderJid:"",previewStatus:m.fromMe?"sent":""});
      if(live){
        // Sending from another device means the chat was read there.
        if(m.fromMe){next.unread=0;next.mentions=0;}
        else{next.unread=Math.min(1_000_000,current.unread+1);if(m.mention)next.mentions=Math.min(1_000_000,current.mentions+1);}
      }
      setThread(ctx,current,next);
    }
  }else if(kind==="reactions"){
    const items=Array.isArray(p.items)?p.items:fail("invalidInput");if(items.length>1000)fail("operationTooLarge");
    for(const item of items){
      const r=record(item);keys(r,["chat","id","emoji","me","delta","clear"]);
      const rowId=`${account.id}/${discordId(r.chat)}/${discordId(r.id)}`,emoji=optional(r.emoji,120);
      const row=ctx.db.chatMessages.id.find(rowId);if(!row||row.deleted)continue;
      if(r.clear===true){for(const old of [...ctx.db.chatReactions.messageRowId.filter(rowId)])if(!emoji||old.emoji===emoji)ctx.db.chatReactions.id.delete(old.id);continue;}
      if(!emoji)fail("invalidInput");const delta=Math.sign(num(r.delta));
      if(r.me===true)setDiscordReaction(ctx,account.id,rowId,emoji,"me",delta>0?1:0,now(ctx));
      else{const others=ctx.db.chatReactions.id.find(`${rowId}|*|${emoji}`);setDiscordReaction(ctx,account.id,rowId,emoji,"*",(others?.count??0)+delta,now(ctx));}
    }
  }else if(kind==="read"){
    const t=ctx.db.chatThreads.id.find(`${account.id}/${discordId(p.chat)}`);
    if(t&&(t.unread||t.mentions))setThread(ctx,t,{...t,unread:0,mentions:0});
  }else fail("invalidInput");
  ctx.db.chatAccounts.id.update({...account,messages:Math.max(0,stored),syncedAt:now(ctx)});
  changed(ctx,account.ownerId);
  return {ok:true,download};
}

/** The servers of a Discord account with their channels, for the sidebar. */
export function discordServers(ctx:Ctx,account:Account,expose:(row:Thread)=>unknown){
  const channels=new Map<string,Thread[]>();
  for(const t of ctx.db.chatThreads.accountId.filter(account.id))if(t.serverId){const list=channels.get(t.serverId)??[];list.push(t);channels.set(t.serverId,list);}
  const at=now(ctx),quiet=(until:number)=>until===-1||until>at;
  return [...ctx.db.chatServers.accountId.filter(account.id)].sort((a,b)=>a.order.localeCompare(b.order)||a.name.localeCompare(b.name)).map(s=>{
    const list=(channels.get(s.serverId)??[]).sort((a,b)=>a.order.localeCompare(b.order));
    return {id:s.id,serverId:s.serverId,name:s.name,mutedUntil:s.mutedUntil,
      unread:!quiet(s.mutedUntil)&&list.some(t=>t.unread>0&&!quiet(t.mutedUntil)),mentions:list.reduce((n,t)=>n+t.mentions,0),channels:list.map(expose)};
  });
}
