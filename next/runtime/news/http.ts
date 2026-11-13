import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import type {NewsRuntime} from "./runtime.ts";

const messages:Record<string,Record<string,string>>={
  subscribe:{alreadyExists:"You are already subscribed to that feed.",quotaExceeded:"You have reached the limit of 500 feeds."},
  importOpml:{quotaExceeded:"Importing would exceed the limit of 500 feeds or 100 folders."},
  createFolder:{alreadyExists:"A folder with that name already exists.",quotaExceeded:"You have reached the limit of 100 folders."},
  renameFolder:{alreadyExists:"A folder with that name already exists."},
};
const integer=(value:string|null,fallback?:number)=>{if(value===null||value==="")return fallback;if(!/^-?\d{1,10}$/.test(value))throw new AppError("invalidInput");return Number(value);};
const scope=(url:URL)=>{const type=url.searchParams.get("type")??"unread";if(!["all","unread","starred","feed","folder"].includes(type))throw new AppError("invalidInput");return {type,...type==="feed"||type==="folder"?{id:integer(url.searchParams.get("id"),0)}:{}};};

/** Native-session News API. The database re-checks the account and app availability. */
export async function handleNewsHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,username:string,runtime:NewsRuntime,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",path=url.pathname.slice("/api/v1/news".length);
  const send=(value:unknown,status=200)=>{res.writeHead(status,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value));};
  const call=async<T=unknown>(operation:string,value:object)=>{
    try{return await db.call<T>("service_news",[sessionId,operation,JSON.stringify(value)]);}
    catch(error){if(error instanceof AppError&&messages[operation]?.[error.code])throw new AppError(error.code,error.status,messages[operation][error.code]);throw error;}
  };
  const input=async(allowed:string[])=>{const value=await body();if(Object.keys(value).some(key=>!allowed.includes(key)))throw new AppError("invalidInput");return value;};
  if(path==="/overview"&&method==="GET")return send(await call("overview",{}));
  if(path==="/items"&&method==="GET")return send(await call("items",{...scope(url),offset:integer(url.searchParams.get("offset"),0),batch:integer(url.searchParams.get("batch"),50),getRead:url.searchParams.get("getRead")!=="0",oldestFirst:url.searchParams.get("oldestFirst")==="1"}));
  if(path==="/items/state"&&method==="POST")return send(await call("setItems",await input(["ids","unread","starred"])));
  const item=/^\/items\/(\d{1,10})(\/position)?$/.exec(path);
  if(item&&!item[2]&&method==="GET")return send(await call("item",{id:Number(item[1])}));
  if(item&&item[2]&&method==="POST"){const value=await input(["position"]);return send(await call("setPosition",{id:Number(item[1]),position:value.position}));}
  if(path==="/read"&&method==="POST")return send(await call("markRead",await input(["type","id","newestItemId"])));
  if(path==="/discover"&&method==="POST"){const value=await input(["url"]);if(typeof value.url!=="string"||!value.url.trim()||value.url.length>2048)throw new AppError("invalidInput",400,"Enter a site or feed address.");await call("access",{});return send({items:await runtime.discover(value.url)});}
  if(path==="/feeds"&&method==="POST"){
    const value=await input(["url","folderId","title","fullText"]);if(typeof value.url!=="string"||!/^https?:\/\//i.test(value.url))throw new AppError("invalidInput",400,"Choose a feed address.");
    if(value.title!==undefined&&typeof value.title!=="string"||value.fullText!==undefined&&typeof value.fullText!=="boolean"||!Number.isInteger(value.folderId??0))throw new AppError("invalidInput");
    await call("access",{});
    try{return send(await runtime.subscribe(sessionId,{url:value.url,folderId:Number(value.folderId??0),title:value.title as string|undefined,fullText:value.fullText as boolean|undefined}),201);}
    catch(error){if(error instanceof AppError&&messages.subscribe[error.code])throw new AppError(error.code,error.status,messages.subscribe[error.code]);throw error;}
  }
  const feed=/^\/feeds\/(\d{1,10})$/.exec(path);
  if(feed&&method==="GET")return send(await call("feedInfo",{id:Number(feed[1])}));
  if(feed&&method==="PATCH"){const value=await input(["title","folderId","fullText","pinned","paused","ordering","updateNow"]);const result=await call("updateFeed",{...value,id:Number(feed[1])});if(value.updateNow===true||value.paused===false)await Promise.race([runtime.tick(),new Promise(r=>setTimeout(r,15000))]);return send(value.updateNow===true?await call("feedInfo",{id:Number(feed[1])}):result);}
  if(feed&&method==="DELETE")return send(await call("deleteFeed",{id:Number(feed[1])}));
  if(path==="/folders"&&method==="POST")return send(await call("createFolder",await input(["name"])),201);
  const folder=/^\/folders\/(\d{1,10})$/.exec(path);
  if(folder&&method==="PATCH"){const value=await input(["name","opened"]);const id=Number(folder[1]);let result:unknown;if(value.name!==undefined)result=await call("renameFolder",{id,name:value.name});if(value.opened!==undefined)result=await call("openFolder",{id,opened:value.opened});return send(result??{});}
  if(folder&&method==="DELETE")return send(await call("deleteFolder",{id:Number(folder[1])}));
  if(path==="/refresh"&&method==="POST"){await call("access",{});await Promise.race([runtime.tick(),new Promise(r=>setTimeout(r,15000))]);return send(await call("overview",{}));}
  if(path==="/opml"&&method==="POST"){const value=await input(["text"]);if(typeof value.text!=="string"||value.text.length>2*1024*1024)throw new AppError("invalidInput",400,"Choose an OPML file up to 2 MiB.");try{return send(await runtime.importOpml(sessionId,value.text));}catch(error){if(error instanceof AppError&&messages.importOpml[error.code])throw new AppError(error.code,error.status,messages.importOpml[error.code]);throw error;}}
  if(path==="/opml"&&method==="GET"){const text=await runtime.exportOpml(sessionId,username);res.writeHead(200,{"content-type":"text/x-opml; charset=utf-8","content-disposition":'attachment; filename="subscriptions.opml"',"cache-control":"no-store"});res.end(text);return;}
  if(path==="/starred"&&method==="GET"){const data=await call("exportData",{starred:true});res.writeHead(200,{"content-type":"application/json","content-disposition":'attachment; filename="starred-articles.json"',"cache-control":"no-store"});res.end(JSON.stringify(data,null,1));return;}
  if(path==="/image"&&method==="GET"){const target=url.searchParams.get("url")??"";if(!/^https?:\/\//.test(target)||target.length>2048)throw new AppError("invalidInput");await call("access",{});return await runtime.image(target,res);}
  throw new AppError("notFound",404);
}
