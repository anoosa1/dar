import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import type {AgentRuntime} from "./runtime.ts";

/** Native-session gateway; account scope is resolved again inside the database. */
export async function handleAgentHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,userId:string,runtime:AgentRuntime,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",path=url.pathname.slice("/api/v1/agent".length);
  const send=(value:unknown,status=200)=>{res.writeHead(status,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value));};
  if(path==="/provider"&&method==="GET"){
    await db.call("service_agent",[sessionId,"projects","{}"]);return send(await runtime.status(userId));
  }
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
    if(!run[2]&&method==="GET")return send(await db.call("service_agent",[sessionId,"run",JSON.stringify({id:run[1]})]));
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
    return send(await db.call("service_agent",[sessionId,"renameThread",JSON.stringify({threadId:single[1],title:input.title})]));
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
  const result=await db.call("service_agent",[sessionId,operation,JSON.stringify(p)]);
  return send(result,status);
}
