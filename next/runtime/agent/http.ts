import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {createHash} from "node:crypto";
import {agentReadOperations} from "../../shared/read-operations.ts";
import type {AgentRuntime} from "./runtime.ts";
import {writeJson} from "../json-response.ts";

/** Native-session gateway; account scope is resolved again inside the database. */
export async function handleAgentHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,userId:string,runtime:AgentRuntime,body:()=>Promise<Record<string,unknown>>,receive:(data:Buffer,id:string)=>Promise<string>){
  const method=req.method??"GET",path=url.pathname.slice("/api/v1/agent".length);
  // Reads skip the durability wait; see shared/read-operations.ts.
  const agent=(args:string[])=>db.operation("service_agent",args[1],agentReadOperations,args);
  const send=(value:unknown,status=200)=>writeJson(res,status,JSON.stringify(value));
  if(path==="/shared"&&method==="GET")return send(await agent([sessionId,"sharedThreads","{}"]));
  const sharedHistory=/^\/shared\/([a-f0-9-]+)\/messages$/.exec(path);
  if(sharedHistory&&method==="GET"){const before=url.searchParams.get("before");if(before!==null&&!/^[1-9]\d{0,9}$/.test(before))throw new AppError("invalidInput");return send(await agent([sessionId,"sharedHistory",JSON.stringify({threadId:sharedHistory[1],...before?{before:Number(before)}:{}})]));}
  const shares=/^\/threads\/([a-f0-9-]+)\/shares(?:\/([a-f0-9-]+))?$/.exec(path);
  if(shares){
    if(method==="GET"&&!shares[2])return send(await agent([sessionId,"threadShares",JSON.stringify({threadId:shares[1]})]));
    if(method==="POST"&&!shares[2]){const input=await body();if(Object.keys(input).some(key=>key!=="id"&&key!=="username"))throw new AppError("invalidInput");return send(await agent([sessionId,"shareThread",JSON.stringify({threadId:shares[1],...input})]),201);}
    if(method==="DELETE"&&shares[2])return send(await agent([sessionId,"unshareThread",JSON.stringify({threadId:shares[1],id:shares[2]})]));
    throw new AppError("methodNotAllowed",405);
  }
  if(path==="/provider/limits"&&method==="GET"){await agent([sessionId,"projects","{}"]);return send(await runtime.limits(userId));}
  const fork=/^\/threads\/([a-f0-9-]+)\/fork$/.exec(path);
  if(fork&&method==="POST"){const input=await body();if(Object.keys(input).some(key=>key!=="id"&&key!=="sequence"))throw new AppError("invalidInput");return send(await agent([sessionId,"forkThread",JSON.stringify({threadId:fork[1],...input})]),201);}
  if(path==="/provider"&&method==="GET"){
    await agent([sessionId,"projects","{}"]);return send(await runtime.status(userId));
  }
  // A Files project whose folder keeps history: the thread's branch (folder-history.md).
  const branchPath=/^\/threads\/([a-f0-9-]+)\/branch$/.exec(path);
  if(branchPath){if(method==="GET")return send(await runtime.branch(sessionId,userId,branchPath[1],false,receive));if(method==="POST")return send(await runtime.branch(sessionId,userId,branchPath[1],true,receive),201);throw new AppError("methodNotAllowed",405);}
  const gitPath=/^\/threads\/([a-f0-9-]+)\/git$/.exec(path);
  if(gitPath){if(method==="GET")return send(await runtime.git(sessionId,userId,gitPath[1],{action:"status"}));if(method==="POST"){const input=await body();if(Object.keys(input).some(key=>!["action","name","message"].includes(key))||input.action==="status")throw new AppError("invalidInput");return send(await runtime.git(sessionId,userId,gitPath[1],input));}throw new AppError("methodNotAllowed",405);}
  const terminal=/^\/threads\/([a-f0-9-]+)\/terminal(?:\/(input|resize))?$/.exec(path);
  if(terminal){
    const abort=new AbortController();res.once("close",()=>abort.abort());
    const operation=method==="GET"&&!terminal[2]?"read":method==="DELETE"&&!terminal[2]?"close":method==="POST"?terminal[2]??"open":undefined;
    if(!operation)throw new AppError("methodNotAllowed",405);
    return send(await runtime.terminal(sessionId,userId,terminal[1],operation,operation==="read"?{after:url.searchParams.get("after")??"0"}:operation==="close"?{}:await body(),abort.signal));
  }
  const action=/^\/runs\/([a-f0-9-]+)\/(steer|answer)$/.exec(path);
  if(action){if(method!=="POST")throw new AppError("methodNotAllowed",405);const input=await body();return send(action[2]==="steer"?await runtime.steer(sessionId,action[1],input):await runtime.answer(sessionId,action[1],input),action[2]==="steer"?201:200);}
  const attachment=/^\/threads\/([a-f0-9-]+)\/attachments\/([a-f0-9-]+)$/.exec(path);
  if(attachment&&method==="PUT"){
    const name=url.searchParams.get("name")??"",type=String(req.headers["content-type"]??"").split(";")[0].trim().toLowerCase(),length=Number(req.headers["content-length"]??0);
    if(length>8*1024*1024)throw new AppError("requestTooLarge",413,"Images must be 8 MiB or smaller.");
    return send(await runtime.upload(sessionId,userId,attachment[1],attachment[2],name,type,req),201);
  }
  if(attachment&&method==="GET"){
    // Served as an inert download-safe image: fixed type from metadata, no sniffing, sandboxed.
    const {info,file}=await runtime.attachment(sessionId,userId,attachment[1],attachment[2]);
    try{res.writeHead(200,{"content-type":info.mediaType,"content-length":String(info.size),"cache-control":"private, max-age=3600","x-content-type-options":"nosniff","content-security-policy":"default-src 'none'; sandbox","content-disposition":`inline; filename="attachment"`});
      await new Promise<void>((resolve,reject)=>{const stream=file.createReadStream();stream.on("error",reject);res.on("close",resolve);stream.pipe(res);});}
    finally{await file.close().catch(()=>{});}
    return;
  }
  const run=/^\/runs\/([a-f0-9-]+)(\/cancel)?$/.exec(path);
  if(run){
    if(!run[2]&&method==="GET"){
      // Running turns are polled every second; an unchanged document answers 304 instead of resending it.
      const text=JSON.stringify(await agent([sessionId,"run",JSON.stringify({id:run[1]})])),tag=`"${createHash("sha256").update(text).digest("base64url")}"`;
      if(req.headers["if-none-match"]===tag){res.writeHead(304,{etag:tag,"cache-control":"no-store"});res.end();return;}
      writeJson(res,200,text,{etag:tag});return;
    }
    if(run[2]&&method==="POST")return send(await runtime.cancel(sessionId,run[1]));
    throw new AppError("methodNotAllowed",405);
  }
  const start=/^\/threads\/([a-f0-9-]+)\/runs$/.exec(path);
  if(start&&method==="POST"){
    const input=await body(),etag=req.headers["if-match"];
    if(typeof etag!=="string"||!/^"\d+"$/.test(etag))throw new AppError("preconditionRequired",428);
    if(Object.keys(input).some(key=>!["id","messageId","content","model","effort","mode","attachments"].includes(key)))throw new AppError("invalidInput");
    return send(await runtime.start(sessionId,userId,{...input,threadId:start[1],revision:Number(etag.slice(1,-1))}),201);
  }
  const files=/^\/threads\/([a-f0-9-]+)\/files$/.exec(path);
  if(files&&method==="GET"){const query=url.searchParams.get("query")??"";if(query.length>200)throw new AppError("invalidInput");return send(await runtime.files(sessionId,userId,files[1],query));}
  const single=/^\/threads\/([a-f0-9-]+)$/.exec(path);
  if(single&&method==="DELETE")return send(await runtime.deleteThread(sessionId,userId,single[1]));
  if(single&&method==="PATCH"){
    const input=await body();if(Object.keys(input).some(key=>key!=="title"))throw new AppError("invalidInput");
    return send(await agent([sessionId,"renameThread",JSON.stringify({threadId:single[1],title:input.title})]));
  }
  let operation:string,p:Record<string,unknown>,status=200;
  if(path==="/folders"&&method==="GET"){operation="folders";p={parentId:url.searchParams.get("parentId")??"",cursor:url.searchParams.get("cursor")??""};}
  else if(path==="/projects"&&method==="GET"){operation="projects";p={};}
  else if(path==="/projects"&&method==="POST"){operation="createProject";p=await body();status=201;}
  else if(path==="/threads"&&method==="GET"){operation="threads";p=url.searchParams.has("projectId")?{projectId:url.searchParams.get("projectId")}:{};}
  else if(path==="/threads"&&method==="POST"){operation="createThread";p=await body();status=201;}
  else{
    const match=/^\/threads\/([a-f0-9-]+)\/messages$/.exec(path);
    if(!match)throw new AppError("notFound",404);
    if(method==="GET"){
      operation="history";p={threadId:match[1]};
      const before=url.searchParams.get("before");if(before!==null){if(!/^[1-9]\d{0,9}$/.test(before))throw new AppError("invalidInput");p.before=Number(before);}
    }else if(method==="POST"){
      const input=await body(),etag=req.headers["if-match"];
      if(typeof etag!=="string"||!/^"\d+"$/.test(etag))throw new AppError("preconditionRequired",428);
      if(Object.keys(input).some(key=>!["id","content"].includes(key)))throw new AppError("invalidInput");
      operation="appendMessage";p={...input,threadId:match[1],revision:Number(etag.slice(1,-1))};status=201;
    }else throw new AppError("methodNotAllowed",405);
  }
  const result=await agent([sessionId,operation,JSON.stringify(p)]);
  return send(result,status);
}
