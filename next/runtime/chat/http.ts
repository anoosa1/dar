import type {IncomingMessage,ServerResponse} from "node:http";
import {AppError} from "../errors.ts";
import type {ChatRuntime} from "./runtime.ts";
import {writeJson} from "../json-response.ts";
import {chatLimits} from "../../shared/chat.ts";

const messages:Record<string,string>={
  quotaExceeded:`You can link up to ${chatLimits.accounts} accounts.`,
  conflict:"This account is not linked. Link it again to send messages.",
  notFound:"This chat or account no longer exists.",
};
const accountPath=/^\/accounts\/([0-9a-f-]{36})(\/link|\/unlink)?$/;

/** Native-session Chat API; the database re-checks the account and that Chat is enabled. */
export async function handleChatHttp(req:IncomingMessage,res:ServerResponse,url:URL,sessionId:string,runtime:ChatRuntime,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",path=url.pathname.slice("/api/v1/chat".length);
  const send=(value:unknown,status=200)=>writeJson(res,status,JSON.stringify(value));
  const input=async(allowed:string[])=>{const value=await body();if(Object.keys(value).some(key=>!allowed.includes(key)))throw new AppError("invalidInput");return value;};
  const str=(value:unknown,max:number,optional=false)=>{
    if(optional&&(value===undefined||value===""))return undefined;
    if(typeof value!=="string"||value.length>max)throw new AppError("invalidInput");return value;
  };
  try{
    if(path==="/overview"&&method==="GET")return send(await runtime.overview(sessionId));
    if(path==="/threads"&&method==="GET"){
      const p=url.searchParams;
      return send(await runtime.call(sessionId,"threads",{accountId:p.get("accountId")??"",view:p.get("view")??"chats",...p.get("cursor")?{cursor:p.get("cursor")}:{}}));
    }
    if(path==="/messages"&&method==="GET"){
      const p=url.searchParams;
      return send(await runtime.call(sessionId,"messages",{threadId:p.get("threadId")??"",...p.get("before")?{before:p.get("before")}:{}}));
    }
    if(path==="/chats"&&method==="POST"){const v=await input(["accountId","phone"]);return send(await runtime.call(sessionId,"openDirect",{accountId:str(v.accountId,64),phone:str(v.phone,40)}),201);}
    if(path==="/read"&&method==="POST"){const v=await input(["threadId"]);return send(await runtime.markRead(sessionId,str(v.threadId,300)!));}
    if(path==="/send"&&method==="POST"){
      const v=await input(["threadId","text","quotedId"]);
      const text=str(v.text,chatLimits.text)!;if(!text.trim())throw new AppError("invalidInput",400,"Write a message first.");
      return send(await runtime.send(sessionId,str(v.threadId,300)!,text,str(v.quotedId,128,true)),201);
    }
    if(path==="/retry"&&method==="POST"){const v=await input(["threadId","messageId"]);return send(await runtime.retry(sessionId,str(v.threadId,300)!,str(v.messageId,128)!));}
    if(path==="/discard"&&method==="POST"){const v=await input(["threadId","messageId"]);return send(await runtime.call(sessionId,"deleteFailed",{threadId:str(v.threadId,300),messageId:str(v.messageId,128)}));}
    if(path==="/accounts/link"&&method==="POST"){
      const v=await input(["name","phone","accountId"]);
      const phone=str(v.phone,40,true);if(phone!==undefined&&!/^\+?[0-9 ()-]{7,30}$/.test(phone))throw new AppError("invalidInput",400,"Enter the full number with its country code.");
      return send(await runtime.link(sessionId,{name:str(v.name,chatLimits.name,true),phone,accountId:str(v.accountId,64,true)}),201);
    }
    const match=accountPath.exec(path);
    if(match){
      const id=match[1];
      if(match[2]==="/link"&&method==="GET")return send(await runtime.linkState(sessionId,id));
      if(match[2]==="/link"&&method==="DELETE")return send(await runtime.cancelLink(sessionId,id));
      if(match[2]==="/unlink"&&method==="POST")return send(await runtime.unlink(sessionId,id));
      if(!match[2]&&method==="PATCH"){const v=await input(["name"]);return send(await runtime.call(sessionId,"rename",{accountId:id,name:str(v.name,chatLimits.name)}));}
      if(!match[2]&&method==="DELETE")return send(await runtime.remove(sessionId,id));
    }
  }catch(error){
    if(error instanceof AppError&&messages[error.code]&&error.message===error.code)throw new AppError(error.code,error.status,messages[error.code]);
    throw error;
  }
  throw new AppError("notFound",404);
}
