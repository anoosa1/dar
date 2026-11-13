import http from "node:http";
import https from "node:https";
import {lookup} from "node:dns";
import {isIP,BlockList} from "node:net";
import zlib from "node:zlib";
import {AppError} from "../errors.ts";

/** Non-public ranges a feed fetch must never reach (SSRF), unless the operator allows them. */
const blocked=new BlockList();
for(const [net,bits] of [["0.0.0.0",8],["10.0.0.0",8],["100.64.0.0",10],["127.0.0.0",8],["169.254.0.0",16],["172.16.0.0",12],["192.0.0.0",24],["192.0.2.0",24],["192.168.0.0",16],["198.18.0.0",15],["198.51.100.0",24],["203.0.113.0",24],["224.0.0.0",4],["240.0.0.0",4]] as const)blocked.addSubnet(net,bits,"ipv4");
for(const [net,bits] of [["::",128],["::1",128],["100::",64],["2001::",32],["2001:db8::",32],["fc00::",7],["fe80::",10],["ff00::",8]] as const)blocked.addSubnet(net,bits,"ipv6");
/** IPv4-mapped IPv6 addresses are checked as the IPv4 address they carry. */
export function publicAddress(address:string){
  const mapped=/^::ffff:(\d+\.\d+\.\d+\.\d+)$/i.exec(address)?.[1]??(/^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/i.exec(address)?.slice(1).map(h=>parseInt(h,16)).flatMap(n=>[n>>8,n&255]).join("."));
  if(mapped)return isIP(mapped)===4&&!blocked.check(mapped,"ipv4");
  const family=isIP(address);return family===4?!blocked.check(address,"ipv4"):family===6?!blocked.check(address,"ipv6"):false;
}

export interface FetchOptions {accept?:string;etag?:string;lastModified?:string;maxBytes?:number;timeout?:number;allowPrivate?:boolean;signal?:AbortSignal}
export interface FetchResult {status:number;url:string;contentType:string;etag:string;lastModified:string;body:Buffer}
const agent="Dar-News/1.0 (feed reader)";

/**
 * GET with redirects re-checked at every hop. The address is validated in the socket
 * lookup itself, so a DNS answer cannot change between the check and the connection.
 * No cookies or credentials are sent; bodies are decompressed within the size limit.
 */
export async function safeFetch(input:string,options:FetchOptions={}):Promise<FetchResult>{
  let current=input;
  for(let hop=0;hop<6;hop++){
    const result=await once(current,options);
    if([301,302,303,307,308].includes(result.status)){
      const location=result.location;if(!location)throw new AppError("feedUnavailable",502,"The site redirected without a destination.");
      current=new URL(location,current).toString();continue;
    }
    return {...result,url:current};
  }
  throw new AppError("feedUnavailable",502,"The site redirected too many times.");
}
function once(target:string,{accept="*/*",etag,lastModified,maxBytes=5*1024*1024,timeout=20000,allowPrivate=false,signal}:FetchOptions){
  let url:URL;try{url=new URL(target);}catch{throw new AppError("invalidInput",400,"Enter a web address starting with http:// or https://.");}
  if(!["http:","https:"].includes(url.protocol)||url.username||url.password)throw new AppError("invalidInput",400,"Only http and https addresses without credentials are supported.");
  if(isIP(url.hostname.replace(/^\[|\]$/g,""))&&!allowPrivate&&!publicAddress(url.hostname.replace(/^\[|\]$/g,"")))throw new AppError("addressNotAllowed",400,"That address points to a private or local network.");
  return new Promise<{status:number;location:string;contentType:string;etag:string;lastModified:string;body:Buffer}>((resolve,reject)=>{
    const guard=(host:string,opts:object,done:(error:Error|null,address:string|import("node:dns").LookupAddress[],family?:number)=>void)=>lookup(host,{...opts,all:true},(error,addresses)=>{
      if(error)return done(error,"",4);
      const list=(addresses as import("node:dns").LookupAddress[]).filter(a=>allowPrivate||publicAddress(a.address));
      if(!list.length)return done(new AppError("addressNotAllowed",400,"That address points to a private or local network."),"",4);
      if((opts as {all?:boolean}).all)done(null,list);else done(null,list[0].address,list[0].family);
    });
    const headers:Record<string,string>={"user-agent":agent,accept,"accept-encoding":"gzip, deflate, br"};
    if(etag)headers["if-none-match"]=etag;if(lastModified)headers["if-modified-since"]=lastModified;
    const request=(url.protocol==="https:"?https:http).request(url,{method:"GET",headers,lookup:guard as never,timeout,signal},response=>{
      const status=response.statusCode??0,header=(name:string)=>{const value=response.headers[name];return typeof value==="string"?value.slice(0,500):"";};
      if(status>=300&&status<400||status===304){response.resume();resolve({status,location:header("location"),contentType:"",etag:header("etag"),lastModified:header("last-modified"),body:Buffer.alloc(0)});return;}
      const encoding=header("content-encoding").toLowerCase(),stream=encoding==="gzip"||encoding==="x-gzip"?response.pipe(zlib.createGunzip()):encoding==="deflate"?response.pipe(zlib.createInflate()):encoding==="br"?response.pipe(zlib.createBrotliDecompress()):response;
      const chunks:Buffer[]=[];let size=0;
      stream.on("data",(chunk:Buffer)=>{size+=chunk.length;if(size>maxBytes){request.destroy();reject(new AppError("feedTooLarge",413,"The response is too large."));}else chunks.push(chunk);});
      stream.on("error",()=>reject(new AppError("feedUnavailable",502,"The site's response could not be read.")));
      stream.on("end",()=>resolve({status,location:"",contentType:header("content-type").toLowerCase(),etag:header("etag"),lastModified:header("last-modified"),body:Buffer.concat(chunks)}));
    });
    request.on("timeout",()=>{request.destroy();reject(new AppError("feedTimedOut",504,"The site did not respond in time."));});
    request.on("error",error=>reject(error instanceof AppError?error:signal?.aborted?new AppError("cancelled",499):new AppError("feedUnavailable",502,`The site could not be reached (${(error as NodeJS.ErrnoException).code??"network error"}).`)));
    request.end();
  });
}
