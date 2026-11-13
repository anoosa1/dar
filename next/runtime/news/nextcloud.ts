import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {digestToken} from "../auth/session.ts";
import {AppError} from "../errors.ts";
import type {NewsRuntime} from "./runtime.ts";
import type {NewsFeed,NewsFolder,NewsItemDetail,NewsOverview} from "../../shared/news.ts";

export const nextcloudBase="/dav/news";
const typeNames=["feed","folder","starred","all"] as const;

/**
 * Nextcloud News API v1.2 and v1.3 for existing mobile and desktop clients. Clients use
 * `<origin>/dav/news` as the server address, the Dar username and an app password with
 * News access. Every call goes through the same checked database operations as the web app.
 */
export async function handleNextcloudNews(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,runtime:NewsRuntime,origin:string){
  const method=req.method??"GET",path=url.pathname.slice(nextcloudBase.length).replace(/\/+$/,"")||"/";
  const send=(value:unknown,status=200)=>{res.writeHead(status,{"content-type":"application/json; charset=utf-8","cache-control":"no-store"});res.end(JSON.stringify(value));};
  if(req.headers.origin&&req.headers.origin!==origin)throw new AppError("invalidOrigin",403);
  if(method==="OPTIONS"){res.writeHead(204,{allow:"GET, POST, PUT, DELETE, OPTIONS"});res.end();return;}
  if(path==="/status.php")return send({installed:true,maintenance:false,needsDbUpgrade:false,version:"28.0.0.0",versionstring:"28.0.0",edition:"",productname:"Nextcloud",extendedSupport:false});
  // HTTP Basic with an app password; the scope decides read or write access.
  const authorization=req.headers.authorization;
  if(!authorization?.startsWith("Basic ")||authorization.length>2048)throw new AppError("unauthenticated",401);
  const decoded=Buffer.from(authorization.slice(6),"base64").toString("utf8"),split=decoded.indexOf(":");
  const username=decoded.slice(0,split),secret=decoded.slice(split+1);
  if(split<1||!/^[A-Za-z0-9_-]{43}$/.test(secret))throw new AppError("unauthenticated",401);
  const write=!["GET","HEAD"].includes(method);
  const session=await db.call<{sessionId:string;username:string}>("service_pim_authenticate",[username,digestToken(secret),"news",write]);
  const call=<T=unknown>(operation:string,value:object)=>db.call<T>("service_news",[session.sessionId,operation,JSON.stringify(value)]);
  if(/^\/ocs\/v[12]\.php\/cloud\/user$/.test(path)){
    const v2=path.includes("/v2.");return send({ocs:{meta:{status:"ok",statuscode:v2?200:100,message:"OK"},data:{id:session.username,"display-name":session.username,displayname:session.username,email:null,enabled:true}}});
  }
  if(/^\/ocs\/v[12]\.php\/cloud\/capabilities$/.test(path))return send({ocs:{meta:{status:"ok",statuscode:path.includes("/v2.")?200:100,message:"OK"},data:{version:{major:28,minor:0,micro:0,string:"28.0.0",edition:"",extendedSupport:false},capabilities:{news:{version:"24.0.0"}}}}});
  const api=/^(?:\/index\.php)?\/apps\/news\/api\/v1-([23])(\/.*)?$/.exec(path);
  if(!api)throw new AppError("notFound",404);
  const v13=api[1]==="3",route=api[2]??"/";
  let input:Record<string,unknown>={};
  if(write&&Number(req.headers["content-length"]??0)>0){
    const chunks:Buffer[]=[];let size=0;for await(const chunk of req){size+=chunk.length;if(size>256*1024)throw new AppError("requestTooLarge",413);chunks.push(chunk as Buffer);}
    try{const value=JSON.parse(Buffer.concat(chunks).toString("utf8"));if(value&&typeof value==="object"&&!Array.isArray(value))input=value;}catch{throw new AppError("invalidJson",400);}
  }
  const param=(name:string)=>input[name]??url.searchParams.get(name);
  const num=(value:unknown,fallback=0)=>{if(value===null||value===undefined||value==="")return fallback;const n=Number(value);if(!Number.isFinite(n))throw new AppError("invalidInput",422);return Math.trunc(n);};
  const bool=(value:unknown,fallback:boolean)=>value===null||value===undefined||value===""?fallback:value===true||value==="true"||value==="1"||value===1;
  const ids=(value:unknown)=>Array.isArray(value)?value.map(v=>num(v)).filter(v=>v>0).slice(0,1000):[];
  const folder=(row:NewsFolder)=>({id:row.id,name:row.name,opened:row.opened,feeds:[]});
  const feed=(row:NewsFeed)=>({id:row.id,url:row.url,title:row.title,faviconLink:null,added:Math.floor(row.added/1000),folderId:row.folderId||(v13?null:0),unreadCount:row.unreadCount,ordering:row.ordering,link:row.link||null,pinned:row.pinned,updateErrorCount:row.updateErrorCount,lastUpdateError:row.lastUpdateError||null});
  const item=(row:NewsItemDetail)=>({id:row.id,guid:row.guid,guidHash:row.guidHash,url:row.url||null,title:row.title,author:row.author,pubDate:Math.floor(row.pubDate/1000),updatedDate:Math.floor(row.updatedDate/1000),body:row.body,
    enclosureMime:row.enclosureMime||null,enclosureLink:row.enclosureLink||null,mediaThumbnail:row.mediaThumbnail||null,mediaDescription:row.mediaDescription||null,feedId:row.feedId,unread:row.unread,starred:row.starred,rtl:row.rtl,
    lastModified:Math.floor(row.lastModified/1e6),fingerprint:row.fingerprint,contentHash:row.contentHash});
  const scope=()=>{const type=typeNames[num(param("type"),3)]??"all",id=num(param("id"));return type==="feed"?{type,id}:type==="folder"?{type,id}:{type};};
  const ok=()=>send({});
  if(route==="/version"&&method==="GET")return send({version:"24.0.0"});
  if(route==="/status"&&method==="GET")return send({version:"24.0.0",warnings:{improperlyConfiguredCron:false,incorrectDbCharset:false}});
  if(route==="/user"&&method==="GET")return send({userId:session.username,displayName:session.username,lastLoginTimestamp:0,avatar:null});
  if(route==="/folders"){
    if(method==="GET")return send({folders:(await call<NewsOverview>("overview",{})).folders.map(folder)});
    if(method==="POST"){try{return send({folders:[folder(await call<NewsFolder>("createFolder",{name:String(input.name??"")}))]});}catch(error){throw error instanceof AppError&&error.code==="alreadyExists"?new AppError("alreadyExists",409,"Folder exists"):error instanceof AppError&&error.code==="invalidInput"?new AppError("invalidInput",422,"Invalid folder name"):error;}}
  }
  let m=/^\/folders\/(\d+)(\/read)?$/.exec(route);
  if(m){
    const id=Number(m[1]);
    if(m[2]&&(method==="PUT"||method==="POST")){await call("markRead",{type:"folder",id,newestItemId:num(param("newestItemId"))});return ok();}
    if(method==="PUT"){try{await call("renameFolder",{id,name:String(input.name??"")});return ok();}catch(error){throw error instanceof AppError&&error.code==="alreadyExists"?new AppError("alreadyExists",409,"Folder exists"):error;}}
    if(method==="DELETE"){await call("deleteFolder",{id});return ok();}
  }
  if(route==="/feeds"){
    if(method==="GET"){const value=await call<NewsOverview>("overview",{});return send({feeds:value.feeds.map(feed),starredCount:value.starredCount,newestItemId:value.newestItemId||undefined});}
    if(method==="POST"){
      const target=String(input.url??"");if(!/^https?:\/\//i.test(target))throw new AppError("invalidInput",422,"Invalid feed address");
      try{const created=await runtime.subscribe(session.sessionId,{url:target,folderId:num(input.folderId)});return send({feeds:[feed(created)],newestItemId:(await call<NewsOverview>("overview",{})).newestItemId});}
      catch(error){if(error instanceof AppError&&error.code==="alreadyExists")throw new AppError("alreadyExists",409,"Feed exists");if(error instanceof AppError&&!["forbidden","unauthenticated","notFound"].includes(error.code))throw new AppError(error.code,422,error.message);throw error;}
    }
  }
  m=/^\/feeds\/(\d+)(?:\/(move|rename|read))?$/.exec(route);
  if(m){
    const id=Number(m[1]);
    if(!m[2]&&method==="DELETE"){await call("deleteFeed",{id});return ok();}
    if(m[2]==="move"&&(method==="PUT"||method==="POST")){await call("updateFeed",{id,folderId:num(input.folderId)});return ok();}
    if(m[2]==="rename"&&(method==="PUT"||method==="POST")){await call("updateFeed",{id,title:String(input.feedTitle??"")});return ok();}
    if(m[2]==="read"&&(method==="PUT"||method==="POST")){await call("markRead",{type:"feed",id,newestItemId:num(param("newestItemId"))});return ok();}
  }
  if(route==="/items"&&method==="GET"){
    const batch=num(param("batchSize"),-1),result=await call<{items:NewsItemDetail[]}>("items",{...scope(),offset:num(param("offset")),batch:batch<=0?-1:Math.min(batch,2000),getRead:bool(param("getRead"),true),oldestFirst:bool(param("oldestFirst"),false),bodies:true});
    return send({items:result.items.map(item)});
  }
  if(route==="/items/updated"&&method==="GET"){
    const since=num(param("lastModified"));
    return send({items:(await call<{items:NewsItemDetail[]}>("itemsUpdated",{...scope(),lastModified:since<1e12?since*1e6:since})).items.map(item)});
  }
  if(route==="/items/read"&&(method==="PUT"||method==="POST")){await call("markRead",{type:"all",newestItemId:num(param("newestItemId"))});return ok();}
  m=/^\/items\/(read|unread|star|unstar)\/multiple$/.exec(route);
  if(m&&(method==="PUT"||method==="POST")){
    const action=m[1],list=input.itemIds??input.items;
    if(action==="star"||action==="unstar"){
      if(Array.isArray(list)&&list.some(v=>v&&typeof v==="object"))await call("starByGuid",{items:list.slice(0,1000).map(v=>({feedId:num((v as Record<string,unknown>).feedId),guidHash:String((v as Record<string,unknown>).guidHash??"")})),starred:action==="star"});
      else await call("setItems",{ids:ids(list),starred:action==="star"});
    }else await call("setItems",{ids:ids(list),unread:action==="unread"});
    return ok();
  }
  m=/^\/items\/(\d+)\/(read|unread|star|unstar)$/.exec(route);
  if(m&&(method==="PUT"||method==="POST")){const change=m[2]==="read"||m[2]==="unread"?{unread:m[2]==="unread"}:{starred:m[2]==="star"};const result=await call<{items:unknown[]}>("setItems",{ids:[Number(m[1])],...change});if(!result.items.length)throw new AppError("notFound",404,"Item not found");return ok();}
  m=/^\/items\/(\d+)\/([a-f0-9]{32})\/(star|unstar)$/.exec(route);
  if(m&&(method==="PUT"||method==="POST")){const result=await call<{changed:number}>("starByGuid",{items:[{feedId:Number(m[1]),guidHash:m[2]}],starred:m[3]==="star"});if(!result.changed)throw new AppError("notFound",404,"Item not found");return ok();}
  throw new AppError("notFound",404,"Not found");
}
