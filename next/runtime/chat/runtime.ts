import path from "node:path";
import {randomBytes} from "node:crypto";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {keyringBox} from "../auth/keyring.ts";
import {startBridge} from "./bridge.ts";
import {createChatMedia} from "./media.ts";
import type {blobStore} from "../files/blobs.ts";
import type {IncomingMessage,ServerResponse} from "node:http";
import {chatReadOperations} from "../../shared/read-operations.ts";
import {chatPlatformNames,type ChatLinkState,type ChatMessage,type ChatOverview,type ChatPlatform} from "../../shared/chat.ts";

interface Entry {key:string;value:string;lazy?:boolean;expiresAt?:number}
interface SendTarget {accountId:string;platform:ChatPlatform;chat:string;message:ChatMessage;quoted:{id:string;sender:string;text:string}|null}
const accountPattern=/^[0-9a-f-]{36}$/;
const reasons:Record<string,string>={
  timeout:"The code expired before it was used. Start again to get a new one.",
  invalidPhone:"Enter the full number with its country code, without a leading 0.",
  pairingFailed:"WhatsApp could not link this device. Start again.",
  passkey:"This WhatsApp account asks for a passkey to link a device, which Dar does not support yet.",
  outdated:"WhatsApp no longer accepts this version of the bridge. Update Dar.",
  multidevice:"Turn on linked devices in WhatsApp, then scan again.",
  cancelled:"Sign-in was cancelled on the phone. Start again to get a new code.",
  captcha:"Discord asked for a check that Dar cannot show. Sign in to Discord in its app or website first, then try again.",
  connectFailed:"Dar could not reach the service. Check the server's internet connection.",
};
/** A Discord nonce: a snowflake for now, which Discord uses to recognise a resent message. */
function discordNonce(){return ((BigInt(Date.now()-1420070400000)<<22n)|BigInt(randomBytes(3).readUIntBE(0,3)&0x3fffff)).toString();}

/**
 * Owns Chat's effects: the bridge process, its encrypted key store, account states,
 * linking and sending. The database checks every account and thread belongs to the caller.
 */
export async function createChatRuntime(db:Database,root:string,{executable,enabled,blobs}:{executable?:string;enabled:boolean;blobs:ReturnType<typeof blobStore>}){
  const box=await keyringBox(path.join(root,"secrets"),"chat-keys");
  const links=new Map<string,ChatLinkState>(),platforms=new Map<string,ChatPlatform>();
  let closing=false,active=enabled,purgeTimer:ReturnType<typeof setInterval>|undefined;
  const worker=<T>(operation:string,value:object)=>db.operation<T>("service_chat_worker",operation,chatReadOperations,[operation,JSON.stringify(value)]);
  const call=<T>(sessionId:string,operation:string,value:object)=>db.operation<T>("service_chat",operation,chatReadOperations,[sessionId,operation,JSON.stringify(value)]);
  const seal=(accountId:string,key:string,value:string)=>box.encrypt({a:accountId,k:key,v:value});
  const open=(accountId:string,key:string,sealed:string)=>{
    const value=box.decrypt<{a:string;k:string;v:string}>(sealed);
    if(value.a!==accountId||value.k!==key)throw new Error("A chat key does not belong where it is stored.");
    return value.v;
  };
  const account=(params:Record<string,unknown>)=>{const id=String(params.account??"");if(!accountPattern.test(id))throw new AppError("invalidInput");return id;};

  // State changes for one account are written in the order the bridge reported them.
  const queues=new Map<string,Promise<unknown>>();
  function setState(accountId:string,state:string,extra:{jid?:string;detail?:string}={}){
    const run=(queues.get(accountId)??Promise.resolve()).catch(()=>{}).then(()=>worker("state",{accountId,state,detail:extra.detail??"",...extra.jid?{jid:extra.jid}:{}}));
    queues.set(accountId,run);
    return run.catch(error=>{if(error instanceof AppError&&error.code==="notFound")void bridge.call("disconnect",{account:accountId}).catch(()=>{});});
  }

  const bridge=startBridge(executable,{
    async request(method,params){
      const accountId=account(params);
      try{
        if(method==="kv.load"){
          const page=await worker<{entries:{key:string;value:string}[];next:string}>("kvLoad",{accountId,cursor:String(params.cursor??"")});
          return {entries:page.entries.map(e=>({key:e.key,value:open(accountId,e.key,e.value)})),next:page.next};
        }
        if(method==="kv.get"){
          const keys=Array.isArray(params.keys)?params.keys.map(String):[];
          const values:Record<string,string>={};
          for(let i=0;i<keys.length;i+=500){
            const found=await worker<{values:Record<string,string>}>("kvGet",{accountId,keys:keys.slice(i,i+500)});
            for(const [key,sealed] of Object.entries(found.values))values[key]=open(accountId,key,sealed);
          }
          return {values};
        }
        if(method==="kv.commit"){
          const puts=(Array.isArray(params.puts)?params.puts:[]) as Entry[],deletes=(Array.isArray(params.deletes)?params.deletes:[]).map(String);
          await worker("kvCommit",{accountId,deletes,puts:puts.map(e=>({key:String(e.key),value:seal(accountId,String(e.key),String(e.value??"")),...e.lazy?{lazy:true}:{},...e.expiresAt?{expiresAt:Number(e.expiresAt)}:{}}))});
          return {};
        }
        if(method==="ingest"){
          // History can follow pairing within milliseconds; store the link first.
          await queues.get(accountId)?.catch(()=>{});
          const {account:_,...rest}=params,result=await worker<{ok:boolean;download?:string[]}>("ingest",{accountId,...rest});
          if(result.download?.length)media.queue(result.download);
          return {ok:true};
        }
      }catch(error){
        // The account was deleted; let the bridge stop instead of retrying forever.
        if(error instanceof AppError&&error.code==="notFound")setTimeout(()=>{void bridge.call("disconnect",{account:accountId}).catch(()=>{});},0);
        throw error;
      }
      throw new AppError("invalidInput");
    },
    event(accountId,event,data){
      if(!accountPattern.test(accountId))return;
      const link=links.get(accountId);
      if(event==="linkCode"){links.set(accountId,{accountId,state:"qr",qr:String(data.code),expiresAt:Date.now()+Number(data.expiresIn??20)*1000});return;}
      if(event==="pairingCode"){links.set(accountId,{accountId,state:"code",code:String(data.code)});return;}
      if(event==="linkScanned"){links.set(accountId,{accountId,state:"scanned",name:String(data.name??"").slice(0,64)});return;}
      if(event==="linkFailed"){
        const reason=String(data.reason??"pairingFailed");links.set(accountId,{accountId,state:"failed",reason:reasons[reason]??reasons.pairingFailed});
        void setState(accountId,"failed",{detail:reason});return;
      }
      if(event==="paired"){
        if(link)links.set(accountId,{accountId,state:"paired"});
        void setState(accountId,"connecting",{jid:String(data.jid)});return;
      }
      if(event==="connected"){void setState(accountId,"connected");return;}
      if(event==="disconnected"){void setState(accountId,"disconnected",{detail:String(data.reason??"")});return;}
      if(event==="connectFailure"){void setState(accountId,"disconnected",{detail:String(data.reason??"")});return;}
      if(event==="loggedOut"){void setState(accountId,"loggedOut",{detail:String(data.reason??"")}).then(()=>worker("kvClear",{accountId})).catch(()=>{});return;}
      if(event==="replaced"){void setState(accountId,"failed",{detail:"replaced"});return;}
      if(event==="banned"){void setState(accountId,"failed",{detail:"banned"});return;}
      if(event==="outdated"){void setState(accountId,"failed",{detail:"outdated"});return;}
      if(event==="mediaRetried"){void media.retried(accountId,String(data.chat),String(data.id),data.ok===true).catch(()=>{});return;}
    },
    onReady(){void connectAll();},
    env:{DAR_CHAT_DOWNLOADS:path.join(root,"chat-downloads")},
  });
  const media=await createChatMedia({db,root,blobs,bridge,worker,call});

  const retries=new Map<string,ReturnType<typeof setTimeout>>();
  async function connect(accountId:string,attempt=0){
    clearTimeout(retries.get(accountId));retries.delete(accountId);
    if(closing||!active)return;
    try{await setState(accountId,"connecting");await bridge.call("connect",{account:accountId,platform:platforms.get(accountId)??"whatsapp"},120_000);}
    catch(error){
      if(error instanceof AppError&&error.code==="notLinked"){await setState(accountId,"loggedOut",{detail:"notLinked"});return;}
      if(closing||!active||!bridge.status.available)return;
      await setState(accountId,"disconnected",{detail:"unreachable"});
      retries.set(accountId,setTimeout(()=>{void connect(accountId,attempt+1);},Math.min(10*60_000,15_000*2**Math.min(attempt,5))));
    }
  }
  async function connectAll(){
    if(!active)return;
    const {accounts}=await worker<{accounts:{id:string;state:string;platform:ChatPlatform}[]}>("accounts",{}).catch(()=>({accounts:[]}));
    for(const item of accounts){platforms.set(item.id,item.platform);if(item.state!=="failed")void connect(item.id);}
  }
  purgeTimer=setInterval(()=>{void worker("purge",{}).catch(()=>{});},60*60_000);

  async function deliver(sessionId:string,threadId:string,target:SendTarget){
    let ok=false,id="";
    try{
      const sent=await bridge.call<{id?:string}>("send",{account:target.accountId,chat:target.chat,text:target.message.text,id:target.message.messageId,
        ...target.quoted?{quotedId:target.quoted.id,quotedSender:target.quoted.sender,quotedText:target.quoted.text}:{}},45_000);
      ok=true;if(target.platform==="discord"&&typeof sent?.id==="string")id=sent.id;
    }catch{}
    return await call<ChatMessage>(sessionId,"sendResult",{threadId,messageId:target.message.messageId,ok,...id?{id}:{}});
  }
  /** Which platform an account is on; remembered once read. */
  async function platformOf(sessionId:string,accountId:string){
    const known=platforms.get(accountId);if(known)return known;
    const row=await call<{platform:ChatPlatform}>(sessionId,"account",{accountId});platforms.set(accountId,row.platform);return row.platform;
  }
  const accountOf=(threadId:string)=>threadId.slice(0,36);

  return {
    call,
    async overview(sessionId:string):Promise<ChatOverview>{
      const result=await call<Omit<ChatOverview,"bridge">>(sessionId,"overview",{});
      return {...result,bridge:{available:bridge.status.available,detail:active?bridge.status.detail:""}};
    },
    async link(sessionId:string,{name,phone,accountId,platform}:{name?:string;phone?:string;accountId?:string;platform?:ChatPlatform}){
      if(!bridge.status.available)throw new AppError("bridgeUnavailable",503,bridge.status.detail||"The chat bridge is not running.");
      const chosen=accountId?await platformOf(sessionId,accountId):platform??"whatsapp";
      const begun=await call<{accountId:string}>(sessionId,"beginLink",{...name!==undefined?{name}:{},...accountId?{accountId}:{},platform:chosen});
      platforms.set(begun.accountId,chosen);
      links.set(begun.accountId,{accountId:begun.accountId,state:"starting"});
      try{await bridge.call("link",{account:begun.accountId,phone:chosen==="whatsapp"?phone??"":"",platform:chosen},30_000);}
      catch(error){links.set(begun.accountId,{accountId:begun.accountId,state:"failed",reason:error instanceof AppError?error.message:reasons.pairingFailed});throw error;}
      return begun;
    },
    async linkState(sessionId:string,accountId:string):Promise<ChatLinkState>{
      const row=await call<{id:string;state:string;jid:string}>(sessionId,"account",{accountId});
      const link=links.get(accountId);
      if(link)return link;
      return row.state==="linking"?{accountId,state:"failed",reason:reasons.timeout}:{accountId,state:row.jid?"paired":"failed"};
    },
    async cancelLink(sessionId:string,accountId:string){
      const row=await call<{id:string;state:string;jid:string}>(sessionId,"account",{accountId});
      links.delete(accountId);
      if(row.state!=="linking"&&row.state!=="failed")return {ok:true};
      await bridge.call("disconnect",{account:accountId}).catch(()=>{});
      if(!row.jid)await call(sessionId,"deleteAccount",{accountId});
      return {ok:true};
    },
    /** Unlinks Dar on WhatsApp; the history stays, read-only, until the account is deleted. */
    async unlink(sessionId:string,accountId:string){
      const row=await call<{id:string;state:string;jid:string}>(sessionId,"account",{accountId});
      clearTimeout(retries.get(accountId));
      let confirmed=true;
      if(row.state!=="loggedOut"){try{await bridge.call("logout",{account:accountId},30_000);}catch{confirmed=false;await bridge.call("disconnect",{account:accountId}).catch(()=>{});}}
      await setState(accountId,"loggedOut",{detail:confirmed?"unlinked":"unlinkedHere"});
      await worker("kvClear",{accountId});
      return {ok:true,confirmed};
    },
    async remove(sessionId:string,accountId:string){
      const row=await call<{id:string;state:string;jid:string}>(sessionId,"account",{accountId});
      clearTimeout(retries.get(accountId));links.delete(accountId);
      if(row.jid&&row.state!=="loggedOut")await bridge.call("logout",{account:accountId},30_000).catch(()=>bridge.call("disconnect",{account:accountId}).catch(()=>{}));
      else await bridge.call("disconnect",{account:accountId}).catch(()=>{});
      return await call(sessionId,"deleteAccount",{accountId});
    },
    async send(sessionId:string,threadId:string,text:string,quotedId?:string){
      // Chosen before sending so a retry or a crash cannot duplicate the message: WhatsApp's own ID
      // format, or for Discord a nonce it recognises when the same message is sent again.
      const platform=await platformOf(sessionId,accountOf(threadId));
      if(platform==="discord"&&[...text].length>4000)throw new AppError("invalidInput",400,"Discord messages can be up to 4,000 characters.");
      const messageId=platform==="discord"?discordNonce():"3EB0"+randomBytes(9).toString("hex").toUpperCase();
      const target=await call<SendTarget>(sessionId,"prepareSend",{threadId,messageId,text,...quotedId?{quotedId}:{}});
      return await deliver(sessionId,threadId,target);
    },
    async retry(sessionId:string,threadId:string,messageId:string){
      return await deliver(sessionId,threadId,await call<SendTarget>(sessionId,"retry",{threadId,messageId}));
    },
    async markRead(sessionId:string,threadId:string){
      const result=await call<{accountId:string;platform:ChatPlatform;chat:string;items:{id:string;sender:string}[];last:string}>(sessionId,"markRead",{threadId});
      if(result.platform==="discord"){if(result.last)void bridge.call("markRead",{account:result.accountId,chat:result.chat,last:result.last}).catch(()=>{});}
      else if(result.items.length)void bridge.call("markRead",{account:result.accountId,chat:result.chat,items:result.items}).catch(()=>{});
      return {ok:true};
    },
    /** Follows Administration → Apps: a disabled Chat disconnects every account and keeps the data. */
    async setEnabled(value:boolean){
      if(active===value)return;active=value;
      if(value){void connectAll();return;}
      for(const timer of retries.values())clearTimeout(timer);retries.clear();
      const {accounts}=await worker<{accounts:{id:string}[]}>("accounts",{}).catch(()=>({accounts:[]}));
      await Promise.all(accounts.map(item=>bridge.call("disconnect",{account:item.id}).catch(()=>{})));
    },
    /** Reacts to, edits or deletes a message on WhatsApp, then records the change here. */
    async act(sessionId:string,threadId:string,messageId:string,action:"react"|"edit"|"revoke",value:{emoji?:string;text?:string;remove?:boolean}){
      const target=await call<{accountId:string;platform:ChatPlatform;state:string;chat:string;id:string;sender:string;fromMe:boolean;deleted:boolean}>(sessionId,"target",{threadId,messageId});
      if(target.deleted)throw new AppError("conflict",409,"This message was deleted.");
      if(action!=="react"&&!target.fromMe)throw new AppError("forbidden",403,"Only your own messages can be changed.");
      if(target.platform==="discord"&&action==="react"&&!value.emoji)throw new AppError("invalidInput",400,"Choose a reaction.");
      if(target.platform==="discord"&&action==="edit"&&[...value.text??""].length>4000)throw new AppError("invalidInput",400,"Discord messages can be up to 4,000 characters.");
      await bridge.call(action,{account:target.accountId,chat:target.chat,id:target.id,sender:target.sender,...value},45_000);
      if(action==="react"&&target.platform==="discord")return await call(sessionId,"applyDiscordReaction",{threadId,messageId,emoji:value.emoji??"",remove:value.remove===true});
      if(action==="react")return await call(sessionId,"applyReaction",{threadId,messageId,emoji:value.emoji??""});
      if(action==="edit")return await call(sessionId,"applyEdit",{threadId,messageId,text:value.text??""});
      return await call(sessionId,"applyRevoke",{threadId,messageId});
    },
    /** Mutes, pins or archives a chat on WhatsApp too, so the phone follows. */
    async chatSetting(sessionId:string,threadId:string,change:{pinned?:boolean;archived?:boolean;mutedUntil?:number}){
      const target=await call<{accountId:string;platform:ChatPlatform;serverId:string;chat:string;lastAt:number;lastId:string;lastSender:string;lastFromMe:boolean}>(sessionId,"chatTarget",{threadId});
      if(target.platform==="discord"){
        if(change.mutedUntil===undefined)throw new AppError("invalidInput",400,"Discord chats can be muted, not pinned or archived.");
        await bridge.call("chatSetting",{account:target.accountId,chat:target.chat,serverId:target.serverId,mutedUntil:change.mutedUntil},45_000);
        return await call(sessionId,"applyChatSetting",{threadId,mutedUntil:change.mutedUntil});
      }
      await bridge.call("chatSetting",{account:target.accountId,chat:target.chat,...change,lastAt:target.lastAt,lastId:target.lastId,lastSender:target.lastSender,lastFromMe:target.lastFromMe},45_000);
      return await call(sessionId,"applyChatSetting",{threadId,...change});
    },
    /** Chat info; groups add their members from WhatsApp when it is reachable. */
    async info(sessionId:string,threadId:string){
      const info=await call<Record<string,unknown>&{thread:{kind:string;jid:string};account:{id:string;platform:ChatPlatform}}>(sessionId,"chatInfo",{threadId});
      let group=null,groupError="";
      if(info.thread.kind==="group"&&info.account.platform==="whatsapp"){
        try{
          const value=await bridge.call<{name:string;topic:string;created:number;members:{jid:string;name:string;admin?:boolean}[]}>("groupInfo",{account:info.account.id,chat:info.thread.jid},20_000);
          // Members who are Dar contacts go by their contact name.
          const names=await call<Record<string,{name:string}>>(sessionId,"names",{accountId:info.account.id,jids:value.members.map(m=>m.jid)});
          group={...value,members:value.members.map(m=>names[m.jid]?{...m,name:names[m.jid].name}:m)};
        }
        catch(error){groupError=error instanceof AppError&&error.code!=="bridgeError"?error.message:"Members are shown when WhatsApp is connected.";}
      }
      return {...info,group,groupError};
    },
    /** Asks the phone for messages before the oldest one here; they arrive like history. */
    async requestHistory(sessionId:string,threadId:string){
      const oldest=await call<{accountId:string;platform:ChatPlatform;chat:string;message:{id:string;fromMe:boolean;at:number}|null}>(sessionId,"oldest",{threadId});
      if(oldest.platform==="discord"){
        // Discord answers right away; the messages are stored before this returns.
        const result=await bridge.call<{count:number;more?:boolean}>("requestHistory",{account:oldest.accountId,chat:oldest.chat,...oldest.message?{before:oldest.message.id}:{}},60_000);
        return {ok:true,count:result.count,more:result.more===true};
      }
      if(!oldest.message)throw new AppError("conflict",409,"There is no message to start from yet.");
      await bridge.call("requestHistory",{account:oldest.accountId,chat:oldest.chat,...oldest.message},30_000);
      return {ok:true};
    },
    /** Discord: fetches what was said in a chat since the newest message Dar has (or the latest page). */
    async catchUp(sessionId:string,threadId:string){
      const newest=await call<{accountId:string;platform:ChatPlatform;state:string;chat:string;message:{id:string}|null}>(sessionId,"newest",{threadId});
      if(newest.platform!=="discord"||newest.state!=="connected")return {count:0};
      return await bridge.call<{count:number}>("requestHistory",{account:newest.accountId,chat:newest.chat,...newest.message?{after:newest.message.id}:{}},90_000);
    },
    /** Discord: servers with their channels. */
    servers:(sessionId:string,accountId:string)=>call(sessionId,"servers",{accountId}),
    /** Discord: friends to start a direct message with. */
    async friends(sessionId:string,accountId:string){
      if(await platformOf(sessionId,accountId)!=="discord")throw new AppError("invalidInput");
      await call(sessionId,"account",{accountId});
      return await bridge.call<{friends:{id:string;name:string;username:string}[]}>("friends",{account:accountId},20_000);
    },
    /** Opens a chat: by phone number on WhatsApp, with a friend on Discord. */
    async openChat(sessionId:string,accountId:string,{phone,user}:{phone?:string;user?:string}){
      if(await platformOf(sessionId,accountId)!=="discord")return await call(sessionId,"openDirect",{accountId,phone:phone??""});
      await call(sessionId,"account",{accountId});
      if(phone!==undefined||!/^\d{1,25}$/.test(user??""))throw new AppError("invalidInput",400,"Choose a friend to message.");
      const opened=await bridge.call<{chat:string}>("openDirect",{account:accountId,user:user??""},30_000);
      return await call(sessionId,"find",{accountId,chat:opened.chat});
    },
    platformName:(platform:ChatPlatform)=>chatPlatformNames[platform],
    serveMedia:(req:IncomingMessage,res:ServerResponse,sessionId:string,threadId:string,messageId:string,attachment:boolean)=>media.serve(req,res,sessionId,threadId,messageId,attachment),
    saveMedia:(sessionId:string,threadId:string,messageId:string,folderId:string)=>media.save(sessionId,threadId,messageId,folderId),
    async close(){closing=true;media.close();clearInterval(purgeTimer);for(const timer of retries.values())clearTimeout(timer);await bridge.close();},
  };
}
export type ChatRuntime=Awaited<ReturnType<typeof createChatRuntime>>;
