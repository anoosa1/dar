import {randomUUID} from "node:crypto";
import {lstat} from "node:fs/promises";
import path from "node:path";
import {AppError} from "../errors.ts";
import type {CodexConfig} from "./codex.ts";
import {openAppServer,type AppServer} from "./app-server.ts";

/**
 * Interactive shells for a thread's working copy. Each session is a PTY started through
 * `codex app-server` command/exec, so it runs under the same Dar permission profile as
 * agent commands: workspace writes only, no network, no Codex home. Output is a bounded
 * byte log that clients read by offset (long polling); sessions close when idle.
 */
interface Session {key:string;server:AppServer;processId:string;chunks:{offset:number;data:Buffer}[];start:number;end:number;exited:boolean;exitCode?:number;waiters:Set<()=>void>;idle:ReturnType<typeof setTimeout>}
const cap=512*1024,idleMs=15*60000;

export function createTerminals(root:string){
  const sessions=new Map<string,Session>();
  const wake=(session:Session)=>{for(const resolve of session.waiters)resolve();session.waiters.clear();};
  const touch=(session:Session)=>{clearTimeout(session.idle);session.idle=setTimeout(()=>close(session.key),idleMs);};
  function close(key:string){const session=sessions.get(key);if(!session)return;sessions.delete(key);clearTimeout(session.idle);session.exited=true;wake(session);session.server.close();}
  function append(session:Session,data:Buffer){
    session.chunks.push({offset:session.end,data});session.end+=data.length;
    while(session.end-session.start>cap&&session.chunks.length>1){const dropped=session.chunks.shift()!;session.start=dropped.offset+dropped.data.length;}
    wake(session);
  }
  return {
    async open(config:CodexConfig,userId:string,threadId:string,size:{rows:number;cols:number}){
      const key=`${userId}/${threadId}`,existing=sessions.get(key);if(existing&&!existing.exited){touch(existing);return {start:existing.start,end:existing.end};}
      if(existing)close(key);
      const workspace=path.join(root,"agent-workspaces",userId,threadId,"work");
      try{const stat=await lstat(workspace);if(!stat.isDirectory()||stat.isSymbolicLink())throw Error();}catch{throw new AppError("workspaceUnavailable",409,"Run Codex once in this thread to prepare its working copy, then open a terminal.");}
      const processId=randomUUID();let session:Session|undefined;
      const server=openAppServer(config,workspace,{
        notify(method,params){if(session&&method==="command/exec/outputDelta"&&params.processId===processId&&typeof params.deltaBase64==="string")append(session,Buffer.from(params.deltaBase64,"base64"));},
        async request(){throw new AppError("declined",403);},
      },["-c",'approval_policy="never"']);
      session={key,server,processId,chunks:[],start:0,end:0,exited:false,waiters:new Set(),idle:setTimeout(()=>close(key),idleMs)};sessions.set(key,session);
      const current=session;void server.exited.then(()=>{current.exited=true;wake(current);});
      try{
        await server.request("initialize",{clientInfo:{name:"dar",title:"Dar",version:"1.0.0"},capabilities:{experimentalApi:true}});server.notify("initialized",{});
        void server.request<{exitCode:number}>("command/exec",{command:["/bin/sh","-i"],processId,tty:true,disableTimeout:true,cwd:workspace,env:{TERM:"xterm-256color",PS1:"$ "},size:{rows:clamp(size.rows,4,200),cols:clamp(size.cols,20,400)}})
          .then(result=>{current.exitCode=result.exitCode;},()=>{}).finally(()=>{current.exited=true;wake(current);});
      }catch(error){close(key);throw error instanceof AppError?error:new AppError("codexUnavailable",503,"The terminal could not be started.");}
      return {start:0,end:0};
    },
    /** Returns bytes after `after`, waiting up to 15 seconds for new output. */
    async read(userId:string,threadId:string,after:number,signal:AbortSignal){
      const session=sessions.get(`${userId}/${threadId}`);if(!session)return {open:false,start:0,end:0,data:"",exited:true};
      touch(session);
      if(after>=session.end&&!session.exited)await new Promise<void>(resolve=>{const timer=setTimeout(done,15000);function done(){clearTimeout(timer);session!.waiters.delete(done);resolve();}session.waiters.add(done);signal.addEventListener("abort",done,{once:true});});
      const from=Math.max(after,session.start),data=Buffer.concat(session.chunks.filter(chunk=>chunk.offset+chunk.data.length>from).map(chunk=>chunk.data.subarray(Math.max(0,from-chunk.offset))));
      return {open:true,start:session.start,end:session.end,data:data.toString("base64"),exited:session.exited,...session.exitCode!==undefined?{exitCode:session.exitCode}:{}};
    },
    async write(userId:string,threadId:string,data:string){
      const session=sessions.get(`${userId}/${threadId}`);if(!session||session.exited)throw new AppError("terminalClosed",409,"The terminal is closed.");
      if(data.length>65536)throw new AppError("requestTooLarge",413);touch(session);
      await session.server.request("command/exec/write",{processId:session.processId,deltaBase64:Buffer.from(data,"utf8").toString("base64")});return {ok:true};
    },
    async resize(userId:string,threadId:string,size:{rows:number;cols:number}){
      const session=sessions.get(`${userId}/${threadId}`);if(!session||session.exited)return {ok:false};
      await session.server.request("command/exec/resize",{processId:session.processId,size:{rows:clamp(size.rows,4,200),cols:clamp(size.cols,20,400)}}).catch(()=>{});return {ok:true};
    },
    close(userId:string,threadId:string){close(`${userId}/${threadId}`);return {ok:true};},
    closeAll(){for(const key of [...sessions.keys()])close(key);},
  };
}
const clamp=(value:unknown,min:number,max:number)=>Number.isInteger(value)?Math.min(max,Math.max(min,Number(value))):min;
