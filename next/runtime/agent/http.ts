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
    if(Object.keys(input).some(key=>!["id","messageId","content","model"].includes(key)))throw new AppError("invalidInput");
    return send(await runtime.start(sessionId,userId,{...input,threadId:start[1],revision:Number(etag.slice(1,-1))}),201);
  }
  let operation:string,p:Record<string,unknown>,status=200;
  if(path==="/folders"&&method==="GET"){operation="folders";p={parentId:url.searchParams.get("parentId")??"",cursor:url.searchParams.get("cursor")??""};}
  else if(path==="/projects"&&method==="GET"){operation="projects";p={};}
  else if(path==="/projects"&&method==="POST"){operation="createProject";p=await body();status=201;}
  else if(path==="/threads"&&method==="GET"){operation="threads";p={projectId:url.searchParams.get("projectId")};}
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
