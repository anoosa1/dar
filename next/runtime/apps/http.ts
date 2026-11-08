import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import type {loadApps} from "./registry.ts";
import type {ServerApp} from "../../sdk/server.ts";
type Registry=Awaited<ReturnType<typeof loadApps>>;
export async function handleAppHttp(req:IncomingMessage,res:ServerResponse,url:URL,registry:Registry,db:Database,sessionId:string,user:{id:string;username:string;displayName:string;administrator:boolean},handlers:Map<string,ServerApp>,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",send=(value:unknown)=>{res.writeHead(200,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value));};
  if(url.pathname==="/api/v1/apps"&&method==="GET")return send({items:[...registry.apps.values()].map(a=>a.descriptor)});
  if(url.pathname==="/api/v1/admin/installedApps"&&method==="GET"){if(!user.administrator)throw new AppError("forbidden",403);return send({items:registry.inventory,restartRequired:true});}
  const custom=/^\/api\/v1\/apps\/([^/]+)\/api(?:\/(.*))?$/.exec(url.pathname),handler=custom&&handlers.get(custom[1]);
  if(custom&&handler){
    const controller=new AbortController(),abort=()=>controller.abort();res.once("close",abort);
    const data={get:(key:string)=>db.call<import("../../shared/installed-apps.ts").AppRecord|null>("service_app_data",[sessionId,custom[1],"get",key,0,1,""]),put:(key:string,value:unknown,revision:number,schemaVersion=1)=>db.call<import("../../shared/installed-apps.ts").AppRecord>("service_app_data",[sessionId,custom[1],"put",key,revision,schemaVersion,JSON.stringify(value)]),delete:(key:string,revision:number)=>db.call("service_app_data",[sessionId,custom[1],"delete",key,revision,1,""])};
    try{return send(await handler.handle({method,path:custom[2]??"",query:Object.fromEntries(url.searchParams),body:["GET","HEAD"].includes(method)?{}:await body(),user,signal:controller.signal,data}));}finally{res.off("close",abort);}
  }
  const match=/^\/api\/v1\/apps\/([^/]+)\/userData(?:\/([^/]+))?$/.exec(url.pathname);
  const app=match&&registry.apps.get(match[1]);if(!match||!app)throw new AppError("notFound",404);
  const key=match[2]??url.searchParams.get("cursor")??"";
  const operation=method==="GET"?match[2]?"get":"list":method==="PUT"&&match[2]?"put":method==="DELETE"&&match[2]?"delete":"";
  if(!operation)throw new AppError("methodNotAllowed",405);
  let revision=0,schemaVersion=1,value="";
  if(operation==="put"||operation==="delete"){
    const header=req.headers["if-match"];
    if(typeof header!=="string"||!/^"\d+"$/.test(header))throw new AppError("preconditionRequired",428);
    revision=Number(header.slice(1,-1));if(!Number.isSafeInteger(revision)||revision>0xffffffff)throw new AppError("invalidInput");
    if(operation==="put"){const input=await body();if(!Number.isInteger(input.schemaVersion)||Number(input.schemaVersion)<1||input.value===undefined||Object.keys(input).some(k=>!["schemaVersion","value"].includes(k)))throw new AppError("invalidInput");schemaVersion=Number(input.schemaVersion);value=JSON.stringify(input.value);}
  }
  const result=await db.call<{revision?:number}|null>("service_app_data",[sessionId,app.manifest.id,operation,key,revision,schemaVersion,value]);
  if(result?.revision)res.setHeader("etag",`"${result.revision}"`);return send(result);
}
