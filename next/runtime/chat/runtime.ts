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
import type {ChatLinkState,ChatMessage,ChatOverview} from "../../shared/chat.ts";

interface Entry {key:string;value:string;lazy?:boolean;expiresAt?:number}
interface SendTarget {accountId:string;chat:string;message:ChatMessage;quoted:{id:string;sender:string;text:string}|null}
const accountPattern=/^[0-9a-f-]{36}$/;
const reasons:Record<string,string>={
  timeout:"The code expired before it was used. Start again to get a new one.",
  invalidPhone:"Enter the full number with its country code, without a leading 0.",
  pairingFailed:"WhatsApp could not link this device. Start again.",
  passkey:"This WhatsApp account asks for a passkey to link a device, which Dar does not support yet.",
  outdated:"WhatsApp no longer accepts this version of the bridge. Update Dar.",
  multidevice:"Turn on linked devices in WhatsApp, then scan again.",
};

/**
 * Owns Chat's effects: the bridge process, its encrypted key store, account states,
 * linking and sending. The database checks every account and thread belongs to the caller.
 */
export async function createChatRuntime(db:Database,root:string,{executable,enabled,blobs}:{executable?:string;enabled:boolean;blobs:ReturnType<typeof blobStore>}){
  const box=await keyringBox(path.join(root,"secrets"),"chat-keys");
  const links=new Map<string,ChatLinkState>();
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
    try{await setState(accountId,"connecting");await bridge.call("connect",{account:accountId},120_000);}
    catch(error){
      if(error instanceof AppError&&error.code==="notLinked"){await setState(accountId,"loggedOut",{detail:"notLinked"});return;}
      if(closing||!active||!bridge.status.available)return;
      await setState(accountId,"disconnected",{detail:"unreachable"});
      retries.set(accountId,setTimeout(()=>{void connect(accountId,attempt+1);},Math.min(10*60_000,15_000*2**Math.min(attempt,5))));
    }
  }
  async function connectAll(){
    if(!active)return;
    const {accounts}=await worker<{accounts:{id:string;state:string}[]}>("accounts",{}).catch(()=>({accounts:[]}));
    for(const item of accounts)if(item.state!=="failed")void connect(item.id);
  }
  purgeTimer=setInterval(()=>{void worker("purge",{}).catch(()=>{});},60*60_000);

  async function deliver(sessionId:string,threadId:string,target:SendTarget){
    let ok=false;
    try{
      await bridge.call("send",{account:target.accountId,chat:target.chat,text:target.message.text,id:target.message.messageId,
        ...target.quoted?{quotedId:target.quoted.id,quotedSender:target.quoted.sender,quotedText:target.quoted.text}:{}},45_000);
      ok=true;
    }catch{}
    return await call<ChatMessage>(sessionId,"sendResult",{threadId,messageId:target.message.messageId,ok});
  }

  return {
    call,
    async overview(sessionId:string):Promise<ChatOverview>{
      const result=await call<Omit<ChatOverview,"bridge">>(sessionId,"overview",{});
      return {...result,bridge:{available:bridge.status.available,detail:active?bridge.status.detail:""}};
    },
    async link(sessionId:string,{name,phone,accountId}:{name?:string;phone?:string;accountId?:string}){
      if(!bridge.status.available)throw new AppError("bridgeUnavailable",503,bridge.status.detail||"The chat bridge is not running.");
      const begun=await call<{accountId:string}>(sessionId,"beginLink",{...name!==undefined?{name}:{},...accountId?{accountId}:{}});
      links.set(begun.accountId,{accountId:begun.accountId,state:"starting"});
      try{await bridge.call("link",{account:begun.accountId,phone:phone??""},30_000);}
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
      // WhatsApp's own ID format, chosen before sending so a retry or a crash cannot duplicate the message.
      const messageId="3EB0"+randomBytes(9).toString("hex").toUpperCase();
      const target=await call<SendTarget>(sessionId,"prepareSend",{threadId,messageId,text,...quotedId?{quotedId}:{}});
      return await deliver(sessionId,threadId,target);
    },
    async retry(sessionId:string,threadId:string,messageId:string){
      return await deliver(sessionId,threadId,await call<SendTarget>(sessionId,"retry",{threadId,messageId}));
    },
    async markRead(sessionId:string,threadId:string){
      const result=await call<{accountId:string;chat:string;items:{id:string;sender:string}[]}>(sessionId,"markRead",{threadId});
      if(result.items.length)void bridge.call("markRead",{account:result.accountId,chat:result.chat,items:result.items}).catch(()=>{});
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
    serveMedia:(req:IncomingMessage,res:ServerResponse,sessionId:string,threadId:string,messageId:string,attachment:boolean)=>media.serve(req,res,sessionId,threadId,messageId,attachment),
    saveMedia:(sessionId:string,threadId:string,messageId:string,folderId:string)=>media.save(sessionId,threadId,messageId,folderId),
    async close(){closing=true;media.close();clearInterval(purgeTimer);for(const timer of retries.values())clearTimeout(timer);await bridge.close();},
  };
}
export type ChatRuntime=Awaited<ReturnType<typeof createChatRuntime>>;
