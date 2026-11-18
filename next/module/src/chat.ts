import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {appEnabled} from "./app-settings.ts";
import {storeBlob,releaseBlob,isBlobId} from "./blobs.ts";
import {chatLimits,chatAutoDownloadBytes,chatTimeKey,internationalDigits,type ChatContact,type ChatAccountState,type ChatView} from "../../shared/chat.ts";
import {discordIngest,discordServers,discordId,applyDiscordReaction} from "./chat-discord.ts";

export type Account=NonNullable<ReturnType<Ctx["db"]["chatAccounts"]["id"]["find"]>>;
export type Thread=NonNullable<ReturnType<Ctx["db"]["chatThreads"]["id"]["find"]>>;
export type Message=NonNullable<ReturnType<Ctx["db"]["chatMessages"]["id"]["find"]>>;
type Media=NonNullable<ReturnType<Ctx["db"]["chatMedia"]["id"]["find"]>>;

export function fail(code:string):never{throw new SenderError(code);}
export function text(value:unknown,max:number,allowEmpty=true){
  if(typeof value!=="string"||!allowEmpty&&!value.trim()||value.includes("\0")||value.length>max)fail("invalidInput");
  return value;
}
export const optional=(value:unknown,max:number)=>value===undefined||value===null?"":text(value,max);
export function keys(p:Record<string,unknown>,allowed:string[]){if(Object.keys(p).some(k=>!allowed.includes(k)))fail("invalidInput");}
export const num=(value:unknown)=>{if(typeof value!=="number"||!Number.isFinite(value))fail("invalidInput");return value;};
export const now=(ctx:Ctx)=>Number(ctx.timestamp.toMillis());
export const prefix=(value:string)=>new Range<string>({tag:"included",value:value+"/"},{tag:"excluded",value:value+"0"});
const after=(cursor:string,value:string)=>new Range<string>({tag:"excluded",value:cursor},{tag:"excluded",value:value+"0"});
const jidPattern=/^[0-9A-Za-z.:_-]{1,80}@(s\.whatsapp\.net|g\.us|lid)$/;
const jid=(value:unknown)=>{const v=text(value,120);if(!jidPattern.test(v))fail("invalidInput");return v;};
const messageIdPattern=/^[0-9A-Za-z._-]{1,128}$/;
export const messageId=(value:unknown)=>{const v=text(value,128);if(!messageIdPattern.test(v))fail("invalidInput");return v;};
export const messageKinds=new Set(["text","image","video","audio","voice","document","sticker","location","contact","poll","unsupported"]);
const statusRank:Record<string,number>={"":0,failed:0,pending:0,sent:1,delivered:2,read:3};
const states:ChatAccountState[]=["linking","connecting","connected","disconnected","loggedOut","failed"];
const keyPattern=/^[\x21-\x7e]{1,600}$/;

/** A chat ID on the account's platform: a WhatsApp JID or a Discord channel ID. */
const chatId=(a:Account,value:unknown)=>a.platform==="discord"?discordId(value):jid(value);
export function changed(ctx:Ctx,userId:string){
  const old=ctx.db.chatChanges.userId.find(userId),row={userId,revision:((old?.revision??0)+1)>>>0};
  if(old)ctx.db.chatChanges.userId.update(row);else ctx.db.chatChanges.insert(row);
}
const first=<T,>(rows:Iterable<T>):T|undefined=>{for(const row of rows)return row;return undefined;};
export const listKey=(accountId:string,at:number,id:string)=>`${accountId}/${chatTimeKey(at)}/${id}`;
export const threadKey=(threadId:string,at:number,id:string)=>`${threadId}/${chatTimeKey(at)}/${id}`;

function exposeThread(row:Thread,names?:ContactNames){
  const contact=row.kind==="direct"?names?.find(row.jid)??null:null,sender=row.previewSenderJid?names?.find(row.previewSenderJid):undefined;
  return {id:row.id,accountId:row.accountId,jid:row.jid,kind:row.kind,name:contact?.name??row.name,whatsappName:row.name,contact,at:row.at,preview:row.preview,previewKind:row.previewKind,previewFromMe:row.previewFromMe,
    previewSender:row.previewSender,previewStatus:row.previewStatus,unread:row.unread,archived:row.archived,pinned:row.pinned,mutedUntil:row.mutedUntil,
    serverId:row.serverId,section:row.section,order:row.order,topic:row.topic,mentions:row.mentions,
    ...sender?{previewSender:sender.name}:{}};
}
/** Dar contacts by phone number, read from every address book the person can see. Never sent to WhatsApp. */
type ContactNames=ReturnType<typeof contactNames>;
function contactNames(ctx:Ctx,userId:string,ownJid:string){
  const own=ownJid.split("@")[0],numbers=new Map<string,ChatContact>();let built=false;
  const build=()=>{
    built=true;let scanned=0;
    const books=[...ctx.db.pimCollections.ownerId.filter(userId)].filter(c=>c.kind==="contacts");
    for(const grant of ctx.db.pimGrants.userId.filter(userId)){const book=ctx.db.pimCollections.id.find(grant.collectionId);if(book?.kind==="contacts")books.push(book);}
    for(const book of books)for(const row of ctx.db.pimResources.collectionId.filter(book.id)){
      if(++scanned>20000)return;
      let data:{displayName?:unknown;phones?:{value?:unknown}[]};try{data=JSON.parse(row.projection);}catch{continue;}
      const name=typeof data.displayName==="string"?data.displayName.trim():"";if(!name||!Array.isArray(data.phones))continue;
      for(const phone of data.phones){const digits=internationalDigits(String(phone?.value??""),own);if(digits&&!numbers.has(digits))numbers.set(digits,{name,book:book.id,href:row.href});}
    }
  };
  return {find(jid:string):ChatContact|undefined{
    if(!jid.endsWith("@s.whatsapp.net"))return undefined;
    if(!built)build();return numbers.get(jid.split("@")[0]);
  }};
}
function exposeMessage(row:Message,media?:Media|null,reactions:ReturnType<typeof reactionsOf>=[]){
  return {id:row.id,messageId:row.messageId,sender:row.sender,senderName:row.senderName,fromMe:row.fromMe,at:row.at,kind:row.kind,text:row.deleted?"":row.text,fileName:row.fileName,
    quotedId:row.quotedId,quotedSender:row.quotedSender,quotedText:row.quotedText,status:row.status,edited:row.edited,deleted:row.deleted,
    media:media&&!row.deleted?{kind:media.kind,mime:media.mime,size:media.size,seconds:media.seconds,width:media.width,height:media.height,state:media.state}:null,
    reactions:row.deleted?[]:reactions};
}
const mediaOf=(ctx:Ctx,row:Message)=>ctx.db.chatMedia.id.find(row.id);
/** Reactions grouped by emoji, with who reacted and whether the account did. */
function reactionsOf(ctx:Ctx,rowId:string){
  const groups=new Map<string,{emoji:string;count:number;mine:boolean;names:string[]}>();
  for(const r of ctx.db.chatReactions.messageRowId.filter(rowId)){
    const group=groups.get(r.emoji)??{emoji:r.emoji,count:0,mine:false,names:[]};
    // Discord's "*" rows count everyone else without names.
    group.count+=r.count;if(r.fromMe)group.mine=true;else if(r.sender!=="*"&&group.names.length<20)group.names.push(r.senderName||r.sender.split("@")[0]);
    groups.set(r.emoji,group);
  }
  return [...groups.values()].sort((a,b)=>b.count-a.count);
}
/** One reaction per person; a newer change wins and an empty emoji removes it. */
function setReaction(ctx:Ctx,accountId:string,rowId:string,r:{sender:string;senderName:string;fromMe:boolean;emoji:string;at:number}){
  const id=`${rowId}|${r.fromMe?"me":r.sender}`,old=ctx.db.chatReactions.id.find(id);
  if(old&&old.at>r.at)return;
  if(!r.emoji){if(old)ctx.db.chatReactions.id.delete(id);return;}
  const row={id,messageRowId:rowId,accountId,sender:r.sender,senderName:r.senderName,fromMe:r.fromMe,emoji:r.emoji,at:r.at,count:1};
  if(old)ctx.db.chatReactions.id.update(row);else ctx.db.chatReactions.insert(row);
}
/** An edit or a deletion for everyone; a deletion takes the attachment and reactions with it. */
export function reviseMessage(ctx:Ctx,t:Thread,target:Message,change:{text?:string;deleted?:boolean}){
  const next=change.deleted?{...target,deleted:true,text:""}:{...target,text:change.text??target.text,edited:true};
  ctx.db.chatMessages.id.update(next);
  if(next.deleted){
    const media=ctx.db.chatMedia.id.find(target.id);if(media)dropMedia(ctx,media);
    for(const r of [...ctx.db.chatReactions.messageRowId.filter(target.id)])ctx.db.chatReactions.id.delete(r.id);
  }
  const current=ctx.db.chatThreads.id.find(t.id)!;
  if(current.at===target.at&&current.previewFromMe===target.fromMe)setThread(ctx,current,{...current,preview:next.deleted?"":next.text,previewKind:next.deleted?"deleted":current.previewKind});
  return next;
}
function exposeAccount(ctx:Ctx,row:Account){
  const counts={chats:0,unread:0,groups:0,archived:0};let unread=0;
  for(const thread of ctx.db.chatThreads.accountId.filter(row.id)){
    // Discord server channels count only when they mention the account.
    if(thread.serverId){if(thread.mentions>0)unread++;continue;}
    if(thread.archived){counts.archived++;continue;}
    counts.chats++;if(thread.kind==="group")counts.groups++;
    if(thread.unread>0){counts.unread++;if(thread.mutedUntil===0||thread.mutedUntil>0&&thread.mutedUntil<now(ctx))unread++;}
  }
  return {id:row.id,platform:row.platform,name:row.name,jid:row.jid,state:row.state,detail:row.detail,createdAt:row.createdAt,linkedAt:row.linkedAt,syncedAt:row.syncedAt,
    historyProgress:row.historyProgress,importing:row.importing,unread,counts};
}

function deleteChats(ctx:Ctx,accountId:string){
  for(const row of [...ctx.db.chatMessages.accountId.filter(accountId)])ctx.db.chatMessages.id.delete(row.id);
  for(const row of [...ctx.db.chatThreads.accountId.filter(accountId)])ctx.db.chatThreads.id.delete(row.id);
  for(const row of [...ctx.db.chatMedia.accountId.filter(accountId)])dropMedia(ctx,row);
  for(const row of [...ctx.db.chatReactions.accountId.filter(accountId)])ctx.db.chatReactions.id.delete(row.id);
  for(const row of [...ctx.db.chatServers.accountId.filter(accountId)])ctx.db.chatServers.id.delete(row.id);
}
/** Maintenance removes the blob once nothing else (another message, a file, a backup) uses it. */
export function dropMedia(ctx:Ctx,row:Media){ctx.db.chatMedia.id.delete(row.id);if(row.blobId)releaseBlob(ctx,row.blobId);}
function deleteAccountRows(ctx:Ctx,account:Account){
  deleteChats(ctx,account.id);
  clearKeys(ctx,account.id);
  ctx.db.chatAccounts.id.delete(account.id);
}
function clearKeys(ctx:Ctx,accountId:string){for(const row of [...ctx.db.chatKeys.scanKey.filter(prefix(accountId))])ctx.db.chatKeys.id.delete(row.id);}

export function setThread(ctx:Ctx,old:Thread,next:Thread){
  if(old.listKey!==next.listKey){ctx.db.chatThreads.id.delete(old.id);ctx.db.chatThreads.insert(next);}else ctx.db.chatThreads.id.update(next);
}
export function newThread(account:Account,id:string,kind:string,at:number):Thread{
  return {id:`${account.id}/${id}`,accountId:account.id,listKey:listKey(account.id,at,id),jid:id,kind,name:"",at,preview:"",previewKind:"",previewFromMe:false,previewSender:"",previewStatus:"",
    unread:0,archived:false,pinned:false,mutedUntil:0,previewSenderJid:"",serverId:"",section:"",order:"",topic:"",mentions:0};
}

/** Validated chat changes from the bridge. */
function validChat(value:unknown){
  if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");const c=value as Record<string,unknown>;
  keys(c,["id","kind","name","unread","archived","pinned","mutedUntil","at"]);
  const kind=c.kind==="group"?"group":c.kind==="direct"?"direct":fail("invalidInput");
  const flag=(v:unknown)=>v===undefined?undefined:typeof v==="boolean"?v:fail("invalidInput");
  return {id:jid(c.id),kind,name:optional(c.name,200).trim(),unread:c.unread===undefined?undefined:Math.min(1_000_000,Math.max(0,Math.floor(num(c.unread)))),
    archived:flag(c.archived),pinned:flag(c.pinned),mutedUntil:c.mutedUntil===undefined?undefined:num(c.mutedUntil),at:c.at===undefined?0:Math.max(0,num(c.at))};
}
function validMessage(value:unknown){
  if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");const m=value as Record<string,unknown>;
  keys(m,["id","chat","sender","senderName","fromMe","at","kind","text","fileName","target","quotedId","quotedSender","quotedText","status","media"]);
  const kind=text(m.kind,20);if(!messageKinds.has(kind)&&kind!=="edit"&&kind!=="revoke"&&kind!=="reaction")fail("invalidInput");
  const status=optional(m.status,20);if(!(status in statusRank))fail("invalidInput");
  if(typeof m.fromMe!=="boolean")fail("invalidInput");
  return {id:messageId(m.id),chat:jid(m.chat),sender:optional(m.sender,120),senderName:optional(m.senderName,200),fromMe:m.fromMe,at:Math.max(0,num(m.at)),kind,
    text:optional(m.text,chatLimits.text),fileName:optional(m.fileName,500),target:m.target===undefined?"":messageId(m.target),quotedId:m.quotedId===undefined?"":messageId(m.quotedId),
    quotedSender:optional(m.quotedSender,200),quotedText:optional(m.quotedText,2000),status,media:validMedia(m.media)};
}
export function validMedia(value:unknown){
  if(value===undefined||value===null)return undefined;
  if(typeof value!=="object"||Array.isArray(value))fail("invalidInput");const v=value as Record<string,unknown>;keys(v,["mime","size","seconds","width","height"]);
  const count=(n:unknown,max:number)=>n===undefined?0:Math.min(max,Math.max(0,Math.floor(num(n))));
  const mime=optional(v.mime,150).toLowerCase();// An odd type never blocks the message; the file is then only offered as a download.
  return {mime:/^[a-z0-9.+-]+\/[a-z0-9.+-]+$/.test(mime)?mime:"application/octet-stream",size:count(v.size,Number.MAX_SAFE_INTEGER),seconds:count(v.seconds,1e7),width:count(v.width,1e5),height:count(v.height,1e5)};
}

/** Stores chats and messages from the bridge; history keeps the phone's unread counts, live messages add to them. */
function ingest(ctx:Ctx,account:Account,p:Record<string,unknown>){
  const kind=text(p.kind,20),live=kind==="messages";
  const chats=Array.isArray(p.chats)?p.chats.map(validChat):[],messages=Array.isArray(p.messages)?p.messages.map(validMessage):[];
  if(chats.length>5000||messages.length>2000)fail("operationTooLarge");
  let stored=account.messages;const download:string[]=[];
  const thread=(id:string,kindOf:string,create:boolean)=>{
    const found=ctx.db.chatThreads.id.find(`${account.id}/${id}`);if(found||!create)return found;
    const row=newThread(account,id,kindOf,0);ctx.db.chatThreads.insert(row);return row;
  };
  for(const c of chats){
    const old=thread(c.id,c.kind,kind!=="chatState");if(!old)continue;
    const next={...old,kind:c.kind};
    if(c.name)next.name=c.name;
    if(c.unread!==undefined)next.unread=c.unread;
    if(c.archived!==undefined)next.archived=c.archived;
    if(c.pinned!==undefined)next.pinned=c.pinned;
    if(c.mutedUntil!==undefined)next.mutedUntil=c.mutedUntil;
    if(c.at>next.at){next.at=c.at;next.listKey=listKey(account.id,c.at,c.id);}
    setThread(ctx,old,next);
  }
  // Oldest first, so the newest message ends up as the preview.
  for(const m of messages.sort((a,b)=>a.at-b.at)){
    if(m.kind==="reaction"){
      // Kept even before the message itself arrives (history can bring it later).
      const t=thread(m.chat,"direct",false);if(t&&m.target)setReaction(ctx,account.id,`${t.id}/${m.target}`,{sender:m.sender,senderName:m.senderName,fromMe:m.fromMe,emoji:m.text.slice(0,32),at:m.at});
      continue;
    }
    const t=thread(m.chat,m.chat.endsWith("@g.us")?"group":"direct",true)!,current={...t};
    if(m.kind==="edit"||m.kind==="revoke"){
      const target=ctx.db.chatMessages.id.find(`${t.id}/${m.target}`);if(!target)continue;
      reviseMessage(ctx,t,target,m.kind==="edit"?{text:m.text}:{deleted:true});
      continue;
    }
    const id=`${t.id}/${m.id}`,old=ctx.db.chatMessages.id.find(id);
    if(old){
      // History repeats live messages; only a better delivery status changes them.
      if(old.fromMe&&statusRank[m.status]>statusRank[old.status])ctx.db.chatMessages.id.update({...old,status:m.status});
      continue;
    }
    if(stored>=chatLimits.messagesPerAccount)break;
    ctx.db.chatMessages.insert({id,accountId:account.id,threadKey:threadKey(t.id,m.at,m.id),messageId:m.id,sender:m.sender,senderName:m.senderName,fromMe:m.fromMe,at:m.at,kind:m.kind,
      text:m.text,fileName:m.fileName,quotedId:m.quotedId,quotedSender:m.quotedSender,quotedText:m.quotedText,status:m.fromMe?(m.status||"sent"):"",edited:false,deleted:false});
    stored++;
    if(m.media){
      ctx.db.chatMedia.insert({id,accountId:account.id,blobId:"",kind:m.kind,mime:m.media.mime,size:m.media.size,fileName:m.fileName,seconds:m.media.seconds,width:m.media.width,height:m.media.height,state:"remote",at:m.at});
      // New photos, stickers and voice notes download right away; the rest when opened.
      if(live&&["image","sticker","voice"].includes(m.kind)&&m.media.size<=chatAutoDownloadBytes)download.push(id);
    }
    const next={...current};
    if(m.at>=t.at){
      Object.assign(next,{at:m.at,listKey:listKey(account.id,m.at,t.jid),preview:m.text||m.fileName,previewKind:m.kind,previewFromMe:m.fromMe,previewSender:t.kind==="group"&&!m.fromMe?m.senderName:"",previewSenderJid:t.kind==="group"&&!m.fromMe?m.sender:"",
        previewStatus:m.fromMe?(m.status||"sent"):""});
    }
    if(live)next.unread=m.fromMe?0:Math.min(1_000_000,t.unread+1);
    setThread(ctx,t,next);
  }
  if(kind==="receipts"){
    const chat=jid(p.chat),status=text(p.status,20),ids=Array.isArray(p.ids)?p.ids.map(messageId):fail("invalidInput"),t=thread(chat,"direct",false);
    if(t&&ids.length<=1000){
      if(p.fromMe===true&&(status==="read"||status==="readSelf")){if(t.unread)setThread(ctx,t,{...t,unread:0});}
      else if(p.fromMe===false&&status in statusRank){
        for(const id of ids){
          const row=ctx.db.chatMessages.id.find(`${t.id}/${id}`);
          if(row?.fromMe&&statusRank[status]>statusRank[row.status]){
            ctx.db.chatMessages.id.update({...row,status});
            if(t.previewFromMe&&t.at===row.at)setThread(ctx,t,{...t,previewStatus:status});
          }
        }
      }
    }
  }
  const final=kind==="history"&&p.final===true;
  ctx.db.chatAccounts.id.update({...account,messages:stored,syncedAt:now(ctx),importing:kind==="history"?!final||Number(p.progress??100)<100:account.importing,
    historyProgress:final&&typeof p.progress==="number"?Math.min(100,Math.max(0,Math.floor(p.progress))):account.historyProgress});
  changed(ctx,account.ownerId);
  return {ok:true,download};
}

/** Pages a chat list newest first; pinned chats lead the first page of Chats. */
function threadPage(ctx:Ctx,account:Account,view:ChatView,cursor:string,limit:number,names:ContactNames){
  const matches=(row:Thread)=>row.serverId?false:view==="archived"?row.archived:!row.archived&&(view==="chats"?!row.pinned:view==="unread"?row.unread>0:row.kind==="group");
  const pinned=view==="chats"&&!cursor?[...ctx.db.chatThreads.accountId.filter(account.id)].filter(row=>row.pinned&&!row.archived&&!row.serverId).sort((a,b)=>b.at-a.at).map(row=>exposeThread(row,names)):[];
  const threads:Thread[]=[];let next:string|null=null;
  for(const row of ctx.db.chatThreads.listKey.filter(cursor?after(cursor,account.id):prefix(account.id))){
    if(!matches(row))continue;
    if(threads.length===limit){next=threads[threads.length-1].listKey;break;}
    threads.push(row);
  }
  return {pinned,threads:threads.map(row=>exposeThread(row,names)),cursor:next};
}
function messagePage(ctx:Ctx,thread:Thread,before:string,limit:number,names:ContactNames){
  const rows:Message[]=[];let more:string|null=null;
  for(const row of ctx.db.chatMessages.threadKey.filter(before?after(before,thread.id):prefix(thread.id))){
    if(rows.length===limit){more=rows[rows.length-1].threadKey;break;}
    rows.push(row);
  }
  // A Dar contact names group senders too.
  const named=(row:Message)=>{const contact=thread.kind==="group"&&!row.fromMe?names.find(row.sender):undefined;return contact?{...row,senderName:contact.name}:row;};
  return {thread:exposeThread(thread,names),messages:rows.reverse().map(row=>exposeMessage(named(row),mediaOf(ctx,row),reactionsOf(ctx,row.id))),before:more};
}

export {chatReadOperations} from "../../shared/read-operations.ts";
/** Account-scoped Chat operations for browser sessions. */
export function chatOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>){
  if(!appEnabled(ctx,"chat"))fail("notFound");
  const account=(value:unknown):Account=>{const row=ctx.db.chatAccounts.id.find(text(value,64));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const thread=(value:unknown):Thread=>{
    const id=text(value,300),row=ctx.db.chatThreads.id.find(id);if(!row)fail("notFound");account(row.accountId);return row;
  };
  const limit=(value:unknown,max:number)=>value===undefined?max:Math.min(max,Math.max(1,Math.floor(num(value))));
  if(operation==="access"){keys(p,[]);return {ok:true};}
  if(operation==="overview"){
    keys(p,[]);
    return {accounts:[...ctx.db.chatAccounts.ownerId.filter(userId)].sort((a,b)=>a.createdAt-b.createdAt).map(row=>exposeAccount(ctx,row))};
  }
  if(operation==="threads"){
    keys(p,["accountId","view","cursor","limit"]);
    const view=["chats","unread","groups","archived"].includes(String(p.view))?p.view as ChatView:fail("invalidInput");
    const a=account(p.accountId),cursor=optional(p.cursor,400);if(cursor&&!cursor.startsWith(a.id+"/"))fail("invalidInput");
    return threadPage(ctx,a,view,cursor,limit(p.limit,chatLimits.page),contactNames(ctx,userId,a.jid));
  }
  if(operation==="messages"){
    keys(p,["threadId","before","limit"]);
    const t=thread(p.threadId),before=optional(p.before,500);if(before&&!before.startsWith(t.id+"/"))fail("invalidInput");
    return messagePage(ctx,t,before,limit(p.limit,chatLimits.messagePage),contactNames(ctx,userId,account(t.accountId).jid));
  }
  if(operation==="markRead"){
    // Returns the newest incoming messages so the runtime can send read receipts.
    // Discord takes a read mark at the newest message instead.
    keys(p,["threadId"]);const t=thread(p.threadId),a=account(t.accountId),items:{id:string;sender:string}[]=[];let last="";
    if(t.unread>0||t.mentions>0){
      for(const row of ctx.db.chatMessages.threadKey.filter(prefix(t.id))){
        if(!last)last=row.messageId;
        if(items.length>=Math.min(t.unread,50)||a.platform==="discord")break;
        if(!row.fromMe&&!row.deleted)items.push({id:row.messageId,sender:row.sender});
      }
      ctx.db.chatThreads.id.update({...t,unread:0,mentions:0});changed(ctx,userId);
    }
    return {accountId:t.accountId,platform:a.platform,chat:t.jid,items:a.platform==="discord"?[]:items,last};
  }
  if(operation==="beginLink"){
    keys(p,["accountId","name","platform"]);
    const platform=p.platform===undefined?"whatsapp":p.platform==="whatsapp"||p.platform==="discord"?p.platform:fail("invalidInput"),label=platform==="discord"?"Discord":"WhatsApp";
    const name=text(p.name??label,chatLimits.name).trim()||label;
    if(p.accountId!==undefined){
      // Linking again keeps the account's history but starts with new device keys.
      const a=account(p.accountId);if(a.state!=="loggedOut"&&a.state!=="failed"&&a.state!=="linking"||p.platform!==undefined&&p.platform!==a.platform)fail("conflict");
      clearKeys(ctx,a.id);ctx.db.chatAccounts.id.update({...a,state:"linking",detail:""});changed(ctx,userId);return {accountId:a.id};
    }
    const mine=[...ctx.db.chatAccounts.ownerId.filter(userId)];
    if(mine.length>=chatLimits.accounts)fail("quotaExceeded");
    // An abandoned link attempt is replaced instead of piling up.
    for(const old of mine)if(old.state==="linking"&&!old.jid)deleteAccountRows(ctx,old);
    let id=ctx.newUuidV4().toString();while(ctx.db.chatAccounts.id.find(id))id=ctx.newUuidV4().toString();
    ctx.db.chatAccounts.insert({id,ownerId:userId,platform,name,jid:"",state:"linking",detail:"",createdAt:now(ctx),linkedAt:0,syncedAt:0,historyProgress:0,importing:false,messages:0});
    changed(ctx,userId);return {accountId:id};
  }
  if(operation==="rename"){
    keys(p,["accountId","name"]);const a=account(p.accountId),name=text(p.name,chatLimits.name,false).trim();
    ctx.db.chatAccounts.id.update({...a,name});changed(ctx,userId);return exposeAccount(ctx,{...a,name});
  }
  if(operation==="account"){keys(p,["accountId"]);const a=account(p.accountId);return {id:a.id,platform:a.platform,state:a.state,jid:a.jid};}
  if(operation==="servers"){
    keys(p,["accountId"]);const a=account(p.accountId);if(a.platform!=="discord")return {servers:[]};
    return {servers:discordServers(ctx,a,row=>exposeThread(row))};
  }
  if(operation==="find"){
    // A chat by its platform ID, after the bridge opened it.
    keys(p,["accountId","chat"]);const a=account(p.accountId),row=ctx.db.chatThreads.id.find(`${a.id}/${chatId(a,p.chat)}`);if(!row)fail("notFound");
    return exposeThread(row,contactNames(ctx,userId,a.jid));
  }
  if(operation==="deleteAccount"){keys(p,["accountId"]);deleteAccountRows(ctx,account(p.accountId));changed(ctx,userId);return {ok:true};}
  if(operation==="openDirect"){
    // A one-to-one chat by phone number, created if it is not in the list yet.
    keys(p,["accountId","phone"]);const a=account(p.accountId);if(a.platform!=="whatsapp")fail("invalidInput");const digits=text(p.phone,40).replace(/[^0-9]/g,"");
    if(digits.length<7||digits.length>15||digits.startsWith("0"))fail("invalidInput");
    const chat=`${digits}@s.whatsapp.net`,old=ctx.db.chatThreads.id.find(`${a.id}/${chat}`);if(old)return exposeThread(old,contactNames(ctx,userId,a.jid));
    const row={...newThread(a,chat,"direct",now(ctx)),name:`+${digits}`};ctx.db.chatThreads.insert(row);changed(ctx,userId);return exposeThread(row,contactNames(ctx,userId,a.jid));
  }
  if(operation==="prepareSend"){
    // Stores the outgoing message as pending under the ID the bridge will send it with.
    keys(p,["threadId","messageId","text","quotedId"]);
    const t=thread(p.threadId),a=account(t.accountId);if(a.state!=="connected"&&a.state!=="connecting"&&a.state!=="disconnected")fail("conflict");
    const body=text(p.text,chatLimits.text,false),id=messageId(p.messageId),at=now(ctx);
    let quoted:Message|undefined;if(p.quotedId!==undefined&&p.quotedId!==""){quoted=ctx.db.chatMessages.id.find(`${t.id}/${messageId(p.quotedId)}`)??undefined;if(!quoted)fail("notFound");}
    if(ctx.db.chatMessages.id.find(`${t.id}/${id}`))fail("alreadyExists");
    const row={id:`${t.id}/${id}`,accountId:a.id,threadKey:threadKey(t.id,at,id),messageId:id,sender:a.jid,senderName:"",fromMe:true,at,kind:"text",text:body,fileName:"",
      quotedId:quoted?.messageId??"",quotedSender:quoted?(quoted.fromMe?"You":quoted.senderName||t.name):"",quotedText:quoted?(quoted.text||quoted.fileName):"",status:"pending",edited:false,deleted:false};
    ctx.db.chatMessages.insert(row);
    setThread(ctx,t,{...t,at,listKey:listKey(a.id,at,t.jid),preview:body,previewKind:"text",previewFromMe:true,previewSender:"",previewSenderJid:"",previewStatus:"pending",unread:0});
    ctx.db.chatAccounts.id.update({...a,messages:a.messages+1});changed(ctx,userId);
    return {accountId:a.id,platform:a.platform,chat:t.jid,message:exposeMessage(row),quoted:quoted?{id:quoted.messageId,sender:quoted.fromMe?a.jid:quoted.sender,text:quoted.text}:null};
  }
  if(operation==="sendResult"){
    // Discord gives a sent message its own ID: the pending copy moves there, unless the gateway already did that.
    keys(p,["threadId","messageId","ok","id"]);const t=thread(p.threadId),realId=p.id===undefined?"":messageId(p.id);
    let row=ctx.db.chatMessages.id.find(`${t.id}/${messageId(p.messageId)}`);
    if(!row&&realId){const sent=ctx.db.chatMessages.id.find(`${t.id}/${realId}`);if(sent)return exposeMessage(sent);}
    if(!row)fail("notFound");
    const status=p.ok===true?"sent":"failed";if(row.status!=="pending")return exposeMessage(row);
    let next={...row,status};
    if(p.ok===true&&realId&&realId!==row.messageId){
      ctx.db.chatMessages.id.delete(row.id);next={...next,id:`${t.id}/${realId}`,messageId:realId,threadKey:threadKey(t.id,row.at,realId)};
      if(ctx.db.chatMessages.id.find(next.id)){ctx.db.chatAccounts.id.update({...account(t.accountId),messages:Math.max(0,account(t.accountId).messages-1)});changed(ctx,userId);return exposeMessage(ctx.db.chatMessages.id.find(next.id)!);}
      ctx.db.chatMessages.insert(next);
    }else ctx.db.chatMessages.id.update(next);
    if(t.previewFromMe&&t.at===row.at)ctx.db.chatThreads.id.update({...t,previewStatus:status});
    changed(ctx,userId);return exposeMessage(next);
  }
  if(operation==="retry"){
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),row=ctx.db.chatMessages.id.find(`${t.id}/${messageId(p.messageId)}`);
    if(!row||!row.fromMe||row.status!=="failed")fail("conflict");
    ctx.db.chatMessages.id.update({...row,status:"pending"});changed(ctx,userId);
    return {accountId:t.accountId,platform:account(t.accountId).platform,chat:t.jid,message:exposeMessage({...row,status:"pending"}),quoted:row.quotedId?{id:row.quotedId,sender:"",text:row.quotedText}:null};
  }
  if(operation==="media"){
    // What the runtime needs to serve, fetch or save one attachment.
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),row=ctx.db.chatMessages.id.find(`${t.id}/${messageId(p.messageId)}`),media=row&&!row.deleted?ctx.db.chatMedia.id.find(row.id):undefined;
    if(!row||!media)fail("notFound");
    return {id:media.id,accountId:t.accountId,chat:t.jid,messageId:row.messageId,kind:media.kind,mime:media.mime,size:media.size,fileName:media.fileName,blobId:media.blobId,state:media.state,at:media.at,chatName:t.name,platform:account(t.accountId).platform};
  }
  const message=(t:Thread,value:unknown)=>{const row=ctx.db.chatMessages.id.find(`${t.id}/${messageId(value)}`);if(!row)fail("notFound");return row;};
  if(operation==="target"){
    // What the bridge needs to act on a message: its chat and who sent it.
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),a=account(t.accountId),row=message(t,p.messageId);
    return {accountId:a.id,platform:a.platform,state:a.state,chat:t.jid,id:row.messageId,sender:row.fromMe?a.jid:row.sender,fromMe:row.fromMe,kind:row.kind,at:row.at,deleted:row.deleted};
  }
  if(operation==="applyDiscordReaction"){
    keys(p,["threadId","messageId","emoji","remove"]);const t=thread(p.threadId),a=account(t.accountId),row=message(t,p.messageId);if(a.platform!=="discord")fail("invalidInput");
    applyDiscordReaction(ctx,a.id,row.id,text(p.emoji,120,false),p.remove===true);changed(ctx,userId);return {ok:true};
  }
  if(operation==="applyReaction"){
    keys(p,["threadId","messageId","emoji"]);const t=thread(p.threadId),a=account(t.accountId),row=message(t,p.messageId);
    setReaction(ctx,a.id,row.id,{sender:a.jid,senderName:"",fromMe:true,emoji:text(p.emoji,32),at:now(ctx)});changed(ctx,userId);return {ok:true};
  }
  if(operation==="applyEdit"){
    keys(p,["threadId","messageId","text"]);const t=thread(p.threadId),row=message(t,p.messageId);if(!row.fromMe||row.deleted||row.kind!=="text")fail("conflict");
    reviseMessage(ctx,t,row,{text:text(p.text,chatLimits.text,false)});changed(ctx,userId);return {ok:true};
  }
  if(operation==="applyRevoke"){
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),row=message(t,p.messageId);if(!row.fromMe)fail("conflict");
    reviseMessage(ctx,t,row,{deleted:true});changed(ctx,userId);return {ok:true};
  }
  if(operation==="deleteForMe"){
    // Only from Dar; WhatsApp and the phone keep it.
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),row=message(t,p.messageId);
    const media=ctx.db.chatMedia.id.find(row.id);if(media)dropMedia(ctx,media);
    for(const r of [...ctx.db.chatReactions.messageRowId.filter(row.id)])ctx.db.chatReactions.id.delete(r.id);
    ctx.db.chatMessages.id.delete(row.id);
    const latest=first(ctx.db.chatMessages.threadKey.filter(prefix(t.id)));
    if(t.at===row.at)setThread(ctx,t,latest?{...t,preview:latest.deleted?"":latest.text||latest.fileName,previewKind:latest.deleted?"deleted":latest.kind,previewFromMe:latest.fromMe,
      previewSender:t.kind==="group"&&!latest.fromMe?latest.senderName:"",previewSenderJid:t.kind==="group"&&!latest.fromMe?latest.sender:"",previewStatus:latest.fromMe?latest.status:""}:{...t,preview:"",previewKind:"",previewSender:"",previewSenderJid:"",previewStatus:""});
    changed(ctx,userId);return {ok:true};
  }
  if(operation==="chatTarget"){
    // The chat and its newest message, which WhatsApp wants when archiving.
    keys(p,["threadId"]);const t=thread(p.threadId),a=account(t.accountId),latest=first(ctx.db.chatMessages.threadKey.filter(prefix(t.id)));
    return {accountId:a.id,platform:a.platform,serverId:t.serverId,kind:t.kind,state:a.state,chat:t.jid,lastAt:latest?.at??0,lastId:latest?.messageId??"",lastSender:latest&&!latest.fromMe?latest.sender:"",lastFromMe:latest?.fromMe??false};
  }
  if(operation==="applyChatSetting"){
    keys(p,["threadId","pinned","archived","mutedUntil"]);const t=thread(p.threadId),next={...t};
    if(typeof p.pinned==="boolean")next.pinned=p.pinned;
    if(typeof p.archived==="boolean"){next.archived=p.archived;if(p.archived)next.pinned=false;}
    if(p.mutedUntil!==undefined)next.mutedUntil=num(p.mutedUntil);
    setThread(ctx,t,next);changed(ctx,userId);return exposeThread(next,contactNames(ctx,userId,account(t.accountId).jid));
  }
  if(operation==="chatInfo"){
    // The chat with its recent attachments (newest first) and how many messages Dar keeps.
    keys(p,["threadId"]);const t=thread(p.threadId),a=account(t.accountId),media:{messageId:string;kind:string;mime:string;state:string;fileName:string;size:number;at:number}[]=[];let messages=0,scanned=0;
    for(const row of ctx.db.chatMessages.threadKey.filter(prefix(t.id))){
      messages++;if(++scanned>20000)break;
      const m=media.length<60?ctx.db.chatMedia.id.find(row.id):undefined;
      if(m&&!row.deleted)media.push({messageId:row.messageId,kind:m.kind,mime:m.mime,state:m.state,fileName:m.fileName,size:m.size,at:m.at});
    }
    const server=t.serverId?ctx.db.chatServers.id.find(`${a.id}/${t.serverId}`):undefined;
    return {thread:exposeThread(t,contactNames(ctx,userId,a.jid)),account:{id:a.id,name:a.name,state:a.state,platform:a.platform},server:server?{name:server.name}:null,media,messages,complete:scanned<=20000};
  }
  if(operation==="oldest"){
    // The oldest message Dar has, from which the phone can send earlier ones.
    keys(p,["threadId"]);const t=thread(p.threadId),a=account(t.accountId);let oldest:Message|undefined;
    for(const row of ctx.db.chatMessages.threadKey.filter(prefix(t.id)))oldest=row;
    return {accountId:a.id,platform:a.platform,state:a.state,chat:t.jid,message:oldest?{id:oldest.messageId,fromMe:oldest.fromMe,at:oldest.at}:null};
  }
  if(operation==="newest"){
    // The newest message Dar has (a pending one does not count), to catch up after it.
    keys(p,["threadId"]);const t=thread(p.threadId),a=account(t.accountId);let newest:Message|undefined;
    for(const row of ctx.db.chatMessages.threadKey.filter(prefix(t.id)))if(row.status!=="pending"&&row.status!=="failed"){newest=row;break;}
    return {accountId:a.id,platform:a.platform,state:a.state,chat:t.jid,message:newest?{id:newest.messageId,at:newest.at}:null};
  }
  if(operation==="names"){
    // Dar contact names for WhatsApp numbers (group members), keyed by JID.
    keys(p,["accountId","jids"]);const a=account(p.accountId);if(!Array.isArray(p.jids)||p.jids.length>2000)fail("invalidInput");
    const names=contactNames(ctx,userId,a.jid),result:Record<string,ChatContact>={};
    for(const value of p.jids){const id=text(value,120),found=names.find(id);if(found)result[id]=found;}
    return result;
  }
  if(operation==="contactSearch"){
    // Dar contacts with a phone number, for starting a chat; numbers come back international.
    keys(p,["accountId","q"]);const a=account(p.accountId),q=optional(p.q,100).normalize("NFC").toLocaleLowerCase().trim(),own=a.jid.split("@")[0],qDigits=q.replace(/\D/g,"");
    const books=[...ctx.db.pimCollections.ownerId.filter(userId)].filter(c=>c.kind==="contacts");
    for(const grant of ctx.db.pimGrants.userId.filter(userId)){const book=ctx.db.pimCollections.id.find(grant.collectionId);if(book?.kind==="contacts")books.push(book);}
    const found:{name:string;phone:string;label:string;book:string;href:string}[]=[];let scanned=0;
    outer:for(const book of books)for(const row of ctx.db.pimResources.collectionId.filter(book.id)){
      if(++scanned>20000)break outer;
      let data:{displayName?:unknown;phones?:{value?:unknown;label?:unknown}[]};try{data=JSON.parse(row.projection);}catch{continue;}
      const name=typeof data.displayName==="string"?data.displayName.trim():"";if(!name||!Array.isArray(data.phones))continue;
      for(const phone of data.phones){
        const digits=internationalDigits(String(phone?.value??""),own);if(!digits)continue;
        if(q&&!name.toLocaleLowerCase().includes(q)&&!(qDigits.length>=3&&digits.includes(qDigits)))continue;
        found.push({name,phone:`+${digits}`,label:typeof phone.label==="string"?phone.label:"",book:book.id,href:row.href});
      }
    }
    return {items:found.sort((x,y)=>x.name.localeCompare(y.name)).slice(0,30)};
  }
  if(operation==="deleteFailed"){
    keys(p,["threadId","messageId"]);const t=thread(p.threadId),row=ctx.db.chatMessages.id.find(`${t.id}/${messageId(p.messageId)}`);
    if(!row||!row.fromMe||row.status!=="failed")fail("conflict");
    ctx.db.chatMessages.id.delete(row.id);changed(ctx,userId);return {ok:true};
  }
  fail("invalidInput");
}

/** Bridge work: its key store, account states and incoming chats. */
export function chatWorker(ctx:Ctx,operation:string,p:Record<string,unknown>){
  const account=(value:unknown):Account=>{const row=ctx.db.chatAccounts.id.find(text(value,64));if(!row)fail("notFound");return row;};
  if(operation==="accounts"){
    keys(p,[]);
    return {accounts:[...ctx.db.chatAccounts.iter()].filter(row=>row.jid&&row.state!=="loggedOut").map(row=>({id:row.id,ownerId:row.ownerId,state:row.state,platform:row.platform}))};
  }
  if(operation==="kvLoad"){
    keys(p,["accountId","cursor"]);const a=account(p.accountId),cursor=optional(p.cursor,700),base=`${a.id}/c`;
    if(cursor&&!cursor.startsWith(base+"/"))fail("invalidInput");
    const entries:{key:string;value:string}[]=[];let next="";let size=0;
    for(const row of ctx.db.chatKeys.scanKey.filter(cursor?after(cursor,base):prefix(base))){
      if(entries.length>=2000||size>4*1024*1024){next=entries.length?`${base}/${entries[entries.length-1].key}`:"";break;}
      entries.push({key:row.scanKey.slice(base.length+1),value:row.value});size+=row.value.length;
    }
    return {entries,next};
  }
  if(operation==="kvGet"){
    keys(p,["accountId","keys"]);const a=account(p.accountId);
    if(!Array.isArray(p.keys)||p.keys.length>500)fail("invalidInput");
    const values:Record<string,string>={};
    for(const key of p.keys){const k=text(key,600);if(!keyPattern.test(k))fail("invalidInput");const row=ctx.db.chatKeys.id.find(`${a.id}/${k}`);if(row)values[k]=row.value;}
    return {values};
  }
  if(operation==="kvCommit"){
    // One transaction: whatsmeow relies on its decryption steps landing together.
    keys(p,["accountId","puts","deletes"]);const a=account(p.accountId);
    if(!Array.isArray(p.puts)||!Array.isArray(p.deletes)||p.puts.length+p.deletes.length>20000)fail("invalidInput");
    for(const key of p.deletes){const k=text(key,600);if(!keyPattern.test(k))fail("invalidInput");ctx.db.chatKeys.id.delete(`${a.id}/${k}`);}
    for(const put of p.puts){
      if(!put||typeof put!=="object")fail("invalidInput");const e=put as Record<string,unknown>;keys(e,["key","value","lazy","expiresAt"]);
      const k=text(e.key,600);if(!keyPattern.test(k))fail("invalidInput");const value=text(e.value,8*1024*1024);
      const expiresAt=e.expiresAt===undefined?0:num(e.expiresAt),id=`${a.id}/${k}`;
      const row={id,scanKey:`${a.id}/${e.lazy===true?"l":"c"}/${k}`,expiryKey:expiresAt>0?`${String(Math.floor(expiresAt)).padStart(15,"0")}/${id}`:"",value};
      if(ctx.db.chatKeys.id.find(id))ctx.db.chatKeys.id.update(row);else ctx.db.chatKeys.insert(row);
    }
    return {ok:true};
  }
  if(operation==="kvClear"){keys(p,["accountId"]);clearKeys(ctx,account(p.accountId).id);return {ok:true};}
  if(operation==="purge"){
    // Expired event buffers and retry copies; bounded so one call stays small.
    keys(p,[]);let removed=0;const end=String(now(ctx)).padStart(15,"0");
    for(const row of [...ctx.db.chatKeys.expiryKey.filter(new Range<string>({tag:"included",value:"0"},{tag:"excluded",value:end}))]){if(removed>=5000)break;ctx.db.chatKeys.id.delete(row.id);removed++;}
    return {removed};
  }
  if(operation==="state"){
    keys(p,["accountId","state","jid","detail"]);const a=account(p.accountId);
    const state=states.includes(p.state as ChatAccountState)?p.state as ChatAccountState:fail("invalidInput");
    const next={...a,state,detail:optional(p.detail,200)};
    if(p.jid!==undefined){
      next.jid=chatId(a,p.jid);next.linkedAt=now(ctx);
      // Linking a different WhatsApp number or Discord user to this account starts its chats afresh.
      if(a.jid&&a.jid!==next.jid){deleteChats(ctx,a.id);next.messages=0;next.historyProgress=0;}
    }
    if(state==="loggedOut")next.importing=false;
    ctx.db.chatAccounts.id.update(next);changed(ctx,a.ownerId);return {ok:true,ownerId:a.ownerId};
  }
  if(operation==="mediaStored"){
    keys(p,["accountId","id","blobId","size"]);const a=account(p.accountId),row=ctx.db.chatMedia.id.find(text(p.id,400));
    if(!row||row.accountId!==a.id)fail("notFound");const blobId=text(p.blobId,64),size=num(p.size);if(!isBlobId(blobId))fail("invalidInput");
    storeBlob(ctx,blobId,size);
    if(row.blobId&&row.blobId!==blobId)releaseBlob(ctx,row.blobId);
    ctx.db.chatMedia.id.update({...row,blobId,size,state:"stored"});changed(ctx,a.ownerId);return {ok:true};
  }
  if(operation==="mediaState"){
    keys(p,["accountId","id","state"]);const a=account(p.accountId),row=ctx.db.chatMedia.id.find(text(p.id,400));
    if(!row||row.accountId!==a.id)fail("notFound");const state=["remote","retrying","expired","failed"].includes(String(p.state))?String(p.state):fail("invalidInput");
    if(row.state!=="stored"){ctx.db.chatMedia.id.update({...row,state});changed(ctx,a.ownerId);}return {ok:true};
  }
  if(operation==="mediaFind"){
    // The phone sent expired media again: which attachment, so the runtime downloads it.
    keys(p,["accountId","chat","messageId"]);const a=account(p.accountId),row=ctx.db.chatMedia.id.find(`${a.id}/${chatId(a,p.chat)}/${messageId(p.messageId)}`);
    return row?{id:row.id,state:row.state,chat:chatId(a,p.chat),messageId:String(p.messageId)}:{id:""};
  }
  if(operation==="ingest"){
    const a=account(p.accountId);if(!a.jid)fail("conflict");
    const {accountId:_,...rest}=p;return a.platform==="discord"?discordIngest(ctx,a,rest):ingest(ctx,a,rest);
  }
  fail("invalidInput");
}

/** Removes a deleted Dar account's Chat data. The runtime unlinks its devices first when it can. */
export function deleteChatAccount(ctx:Ctx,ownerId:string){for(const row of [...ctx.db.chatAccounts.ownerId.filter(ownerId)])deleteAccountRows(ctx,row);}

/** Chats, messages and downloaded attachments for backups. Keys are left out: a restored account
 * comes back unlinked, so two servers never use the same WhatsApp device. */
export function chatSnapshot(ctx:Ctx){
  return {accounts:[...ctx.db.chatAccounts.iter()],threads:[...ctx.db.chatThreads.iter()],messages:[...ctx.db.chatMessages.iter()],
    media:[...ctx.db.chatMedia.iter()].map(row=>row.state==="stored"?row:{...row,blobId:"",state:row.state==="retrying"?"remote":row.state}),
    reactions:[...ctx.db.chatReactions.iter()],servers:[...ctx.db.chatServers.iter()]};
}
export function chatBlobIds(ctx:Ctx){return [...new Set([...ctx.db.chatMedia.iter()].filter(row=>row.blobId&&row.state==="stored").map(row=>row.blobId))];}
export function restoreChat(ctx:Ctx,value?:ReturnType<typeof chatSnapshot>){
  if(!value)return;
  for(const row of value.accounts)ctx.db.chatAccounts.insert({...row,state:row.jid?"loggedOut":"failed",detail:"restored",importing:false});
  // Backups from before Discord lack its columns.
  for(const row of value.threads)ctx.db.chatThreads.insert(Object.assign({serverId:"",section:"",order:"",topic:"",mentions:0},row));
  for(const row of value.messages)ctx.db.chatMessages.insert(row);
  for(const row of value.media)ctx.db.chatMedia.insert(row.blobId&&!ctx.db.fileBlobs.id.find(row.blobId)?{...row,blobId:"",state:"expired"}:row);
  for(const row of value.reactions??[])ctx.db.chatReactions.insert(Object.assign({count:1},row));
  for(const row of value.servers??[])ctx.db.chatServers.insert(row);
}

/**
 * Chat in the shared search: chat names and message text, newest chats and messages first, at
 * most three messages per chat. Returns false when it stopped at its own work limit.
 */
export function chatSearch(ctx:Ctx,userId:string,q:string,matches:(value:string)=>boolean,add:(result:{id:string;title:string;detail:string;href:string})=>void){
  let budget=40000;
  for(const a of ctx.db.chatAccounts.ownerId.filter(userId)){
    const names=contactNames(ctx,userId,a.jid);
    const servers=new Map([...ctx.db.chatServers.accountId.filter(a.id)].map(s=>[s.serverId,s.name]));
    for(const row of ctx.db.chatThreads.listKey.filter(prefix(a.id))){
      const thread=exposeThread(row,names),href=`/chat?account=${encodeURIComponent(a.id)}&chat=${encodeURIComponent(row.id)}`;
      const title=row.serverId?`#${row.name}`:thread.name||thread.whatsappName,kind=row.serverId?servers.get(row.serverId)??"Server":row.kind==="group"?"Group":"Chat";
      if(matches(thread.name)||matches(thread.whatsappName))add({id:row.id,title,detail:`${kind} · ${a.name}`,href});
      let hits=0;
      for(const m of ctx.db.chatMessages.threadKey.filter(prefix(row.id))){
        if(--budget<0)return false;
        if(m.deleted||!m.text||!matches(m.text))continue;
        const at=m.text.normalize("NFC").toLowerCase().indexOf(q),start=Math.max(0,at-40),end=Math.min(m.text.length,at+q.length+60);
        const snippet=`${start>0?"…":""}${m.text.slice(start,end).replace(/\s+/g," ").trim()}${end<m.text.length?"…":""}`;
        const who=m.fromMe?"You":row.kind!=="direct"?(names.find(m.sender)?.name??m.senderName):"";
        add({id:m.id,title,detail:`${who?who+": ":""}${snippet}`,href:`${href}&message=${encodeURIComponent(m.messageId)}`});
        if(++hits>=3)break;
      }
    }
  }
  return true;
}
