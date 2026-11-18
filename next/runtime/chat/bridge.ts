import {spawn,type ChildProcessWithoutNullStreams} from "node:child_process";
import {createInterface} from "node:readline";
import {AppError} from "../errors.ts";

/** Requests the bridge makes of the runtime (its key store, incoming chats). */
export type BridgeRequest=(method:string,params:Record<string,unknown>)=>Promise<unknown>;
export type BridgeEvent=(account:string,event:string,data:Record<string,unknown>)=>void;
export interface BridgeStatus {available:boolean;detail:string}

const protocolVersion=1;
interface Pending {resolve:(value:unknown)=>void;reject:(error:Error)=>void;timer:ReturnType<typeof setTimeout>}

/**
 * Supervises dar-chat-bridge: newline-delimited JSON over stdio (see bridge/rpc.go).
 * The process restarts with backoff after a crash; onReady runs after every start so
 * accounts reconnect. close() stops it and fails anything still waiting.
 */
export function startBridge(executable:string|undefined,{request,event,onReady,env={}}:{request:BridgeRequest;event:BridgeEvent;onReady:()=>void;env?:Record<string,string>}){
  let child:ChildProcessWithoutNullStreams|undefined,closing=false,ready=false,restarts=0,restartTimer:ReturnType<typeof setTimeout>|undefined,next=0;
  const status:BridgeStatus={available:false,detail:executable?"Starting the chat bridge…":"The chat bridge is not installed on this server."};
  const pending=new Map<string,Pending>();
  const write=(frame:unknown)=>{if(child?.stdin.writable)child.stdin.write(JSON.stringify(frame)+"\n");};
  const failAll=(message:string)=>{for(const [id,item] of pending){clearTimeout(item.timer);item.reject(new AppError("bridgeUnavailable",503,message));pending.delete(id);}};

  function call<T=unknown>(method:string,params:Record<string,unknown>={},timeout=60_000):Promise<T>{
    if(!child||(!ready&&method!=="hello"))return Promise.reject(new AppError("bridgeUnavailable",503,status.detail||"The chat bridge is not running."));
    const id=`n${++next}`;
    return new Promise<T>((resolve,reject)=>{
      const timer=setTimeout(()=>{pending.delete(id);reject(new AppError("bridgeTimeout",504,"WhatsApp did not answer in time."));},timeout);
      pending.set(id,{resolve:resolve as (value:unknown)=>void,reject,timer});
      write({id,method,params});
    });
  }
  async function answer(frame:{id:string;method:string;params?:Record<string,unknown>}){
    try{write({id:frame.id,result:await request(frame.method,frame.params??{})??{}});}
    catch(error){write({id:frame.id,error:{code:error instanceof AppError?error.code:"runtimeError",message:error instanceof Error?error.message:"Request failed."}});}
  }
  function start(){
    if(closing||!executable)return;
    let proc:ChildProcessWithoutNullStreams;
    try{proc=spawn(executable,[],{stdio:["pipe","pipe","pipe"],env:{PATH:process.env.PATH??"",DAR_CHAT_BRIDGE_LOG:process.env.DAR_CHAT_BRIDGE_LOG??"warn",TZ:"UTC",...env}});}
    catch{status.detail="The chat bridge could not be started.";return;}
    child=proc;ready=false;
    proc.on("error",error=>{
      status.available=false;
      status.detail=(error as NodeJS.ErrnoException).code==="ENOENT"?"The chat bridge is not installed on this server.":"The chat bridge could not be started.";
    });
    createInterface({input:proc.stdout,crlfDelay:Infinity}).on("line",line=>{
      let frame:Record<string,unknown>;
      try{frame=JSON.parse(line);}catch{return;}
      if(typeof frame.method==="string"&&typeof frame.id==="string"){void answer(frame as {id:string;method:string;params?:Record<string,unknown>});return;}
      if(typeof frame.event==="string"){try{event(String(frame.account??""),frame.event,(frame.data??{}) as Record<string,unknown>);}catch{}return;}
      const item=typeof frame.id==="string"?pending.get(frame.id):undefined;if(!item)return;
      pending.delete(frame.id as string);clearTimeout(item.timer);
      const error=frame.error as {code?:string;message?:string}|undefined;
      if(error)item.reject(new AppError(error.code??"bridgeError",error.code==="invalidInput"?400:error.code==="notConnected"?409:502,error.message??"The chat bridge reported an error."));
      else item.resolve(frame.result);
    });
    // whatsmeow logs at warn and above; keep them, bounded per line.
    createInterface({input:proc.stderr,crlfDelay:Infinity}).on("line",line=>{if(line.trim())console.warn(`chat-bridge: ${line.slice(0,500)}`);});
    proc.once("exit",()=>{
      if(child===proc)child=undefined;
      ready=false;status.available=false;failAll("The chat bridge stopped.");
      if(closing||status.detail==="The chat bridge is not installed on this server.")return;
      status.detail="The chat bridge stopped; restarting.";
      const delay=Math.min(60_000,1000*2**Math.min(restarts++,6));
      restartTimer=setTimeout(start,delay);
    });
    void call<{protocol:number}>("hello",{},10_000).then(hello=>{
      if(hello.protocol!==protocolVersion){status.detail="The chat bridge does not match this version of Dar.";proc.kill("SIGTERM");return;}
      ready=true;restarts=0;status.available=true;status.detail="";onReady();
    }).catch(()=>{});
  }
  start();
  return {
    call,status,
    async close(){
      closing=true;clearTimeout(restartTimer);failAll("Dar is stopping.");
      const proc=child;if(!proc)return;
      await new Promise<void>(resolve=>{const timer=setTimeout(()=>{proc.kill("SIGKILL");resolve();},5000);proc.once("exit",()=>{clearTimeout(timer);resolve();});proc.stdin.end();proc.kill("SIGTERM");});
    },
  };
}
export type Bridge=ReturnType<typeof startBridge>;
