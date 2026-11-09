import type { IncomingMessage,ServerResponse } from "node:http";
import { randomUUID,randomBytes } from "node:crypto";
import type { Database } from "../database.ts";
import type { Authenticated,authService } from "../auth/service.ts";
import { createOpaqueToken,digestToken } from "../auth/session.ts";
import { AppError } from "../errors.ts";
import { checkMatch,directoryService } from "./directory.ts";
import { resource,version,type Document } from "./scim-model.ts";
import type { IdentityProvider } from "./provider.ts";

function json(res:ServerResponse,value:unknown,status=200){res.writeHead(status,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value));}
const text=(value:unknown,max=1024)=>{if(typeof value!=="string"||Buffer.byteLength(value)>max)throw new AppError("invalidInput",400);return value;};
function redirects(value:unknown,development:boolean,optional=false){
  if(!Array.isArray(value)||value.length>(optional?10:20)||(!optional&&!value.length))throw new AppError("invalidRedirectUris",400);
  return [...new Set(value.map(v=>{const raw=text(v,2048);let u:URL;try{u=new URL(raw);}catch{throw new AppError("invalidRedirectUris",400);}if(u.username||u.password||u.hash||u.href!==raw||raw.includes("*")||(u.protocol!=="https:"&&!(development&&u.protocol==="http:"&&["127.0.0.1","localhost","[::1]"].includes(u.hostname))))throw new AppError("invalidRedirectUris",400,"Use exact HTTPS redirect URLs; loopback HTTP is allowed only in development.");return raw;}))];
}
export function clientMetadata(p:Document,id:string,development:boolean,secret?:string):Document {
  const name=text(p.name,100).trim(),type=p.type;
  if(!name||!["public","confidential"].includes(String(type)))throw new AppError("invalidClient",400);
  if(p.requirePkce!==undefined&&typeof p.requirePkce!=="boolean"||type==="public"&&p.requirePkce===false)throw new AppError("invalidClient",400,"PKCE can only be disabled for confidential clients.");
  const scopes=typeof p.scopes==="string"?p.scopes.split(/\s+/):p.scopes;
  if(!Array.isArray(scopes)||!scopes.includes("openid")||scopes.some(s=>!["openid","profile","email","groups","offline_access"].includes(s)))throw new AppError("invalidScopes",400);
  // Persist only the opt-out so unchanged clients keep their revisions and grants.
  return {client_id:id,client_name:name,application_type:"web",redirect_uris:redirects(p.redirectUris,development),post_logout_redirect_uris:redirects(p.postLogoutRedirectUris??[],development,true),response_types:["code"],grant_types:["authorization_code",...(scopes.includes("offline_access")?["refresh_token"]:[])],scope:[...new Set(scopes)].join(" "),token_endpoint_auth_method:type==="public"?"none":"client_secret_basic",id_token_signed_response_alg:"RS256",...(p.requirePkce===false?{require_pkce:false}:{}),...(type==="confidential"?{client_secret:secret,client_secret_expires_at:0}:{})};
}
interface ClientRow {id:string;metadata:string;encrypted:string;enabled:boolean;revision:number;modifiedAt:number}
function clientDocument(row:Omit<ClientRow,"encrypted">){const p=JSON.parse(row.metadata) as {client_name:string;token_endpoint_auth_method:string;redirect_uris:string[];post_logout_redirect_uris:string[];scope:string;require_pkce?:boolean};return {id:row.id,revision:row.revision,enabled:row.enabled,modifiedAt:new Date(row.modifiedAt).toISOString(),name:p.client_name,type:p.token_endpoint_auth_method==="none"?"public":"confidential",redirectUris:p.redirect_uris,postLogoutRedirectUris:p.post_logout_redirect_uris,scopes:p.scope,requirePkce:p.token_endpoint_auth_method==="none"||p.require_pkce!==false};}
export async function handleIdentityHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,auth:Awaited<ReturnType<typeof authService>>,authenticated:Authenticated,provider:IdentityProvider,origin:string,development:boolean,body:()=>Promise<Document>){
  const method=req.method??"GET",path=url.pathname.slice("/api/v1/identity".length);
  const uid=/^\/interactions\/([^/]+)$/.exec(path);
  if(uid){if(method==="GET")return json(res,await provider.details(req,res,uid[1],authenticated));if(method==="POST")return json(res,await provider.decide(req,res,uid[1],authenticated,await body()));throw new AppError("notFound",404);}
  if(path==="/connections"&&method==="GET")return json(res,{items:await db.call("service_connected_applications",[authenticated.session.id,""]),nextCursor:""});
  if(path.startsWith("/connections/")&&method==="DELETE")return json(res,{items:await db.call("service_connected_applications",[authenticated.session.id,decodeURIComponent(path.slice(13))]),nextCursor:""});
  if(!authenticated.user.administrator)throw new AppError("forbidden",403);
  const proof=typeof req.headers["x-identity-proof"]==="string"?digestToken(req.headers["x-identity-proof"]):"";
  const directory=directoryService(db,auth,{sessionId:authenticated.session.id,proof});
  if(path==="/confirmation"&&method==="POST"){
    const p=await body(),verifier=await auth.confirm(authenticated,text(p.password)),token=createOpaqueToken();
    await db.call("service_identity_proof",[authenticated.session.id,token.digest,verifier]);return json(res,{proof:token.secret,expiresAt:new Date(Date.now()+300000).toISOString()});
  }
  if(path==="/status"&&method==="GET")return json(res,{issuer:`${origin}/oidc`,scimUrl:`${origin}/scim/v2`,keys:provider.keys.status()});
  if(path==="/keys/rotate"&&method==="POST"){await directory.call("adminWrite");return json(res,await provider.rotate());}
  const entity=/^\/(users|groups)(?:\/([^/]+))?$/.exec(path);
  if(entity){
    const kind=entity[1]==="users"?"Users":"Groups",id=entity[2],base=`${origin}/scim/v2`;
    if(method==="GET"){if(id){const row=await directory.get(kind,id);res.setHeader("etag",version(row));return json(res,resource(kind,row,base));}return json(res,{items:(await directory.list(kind)).map(v=>resource(kind,v,base)),nextCursor:""});}
    if(method==="POST"&&!id||["PUT","PATCH"].includes(method)&&id){const row=await directory.save(kind,id,await body(),req.headers["if-match"],method==="PATCH",true);res.setHeader("etag",version(row));return json(res,resource(kind,row,base),id?200:201);}
    if(method==="DELETE"&&id){await directory.remove(kind,id,req.headers["if-match"],true);return json(res,{ok:true});}
  }
  if(path==="/clients"&&method==="GET"){const items=await directory.call<Omit<ClientRow,"encrypted">[]>("clients");return json(res,{items:items.map(clientDocument),nextCursor:""});}
  const client=/^\/clients\/([^/]+)(\/secret)?$/.exec(path);
  if(client&&!client[2]&&method==="GET"){const row=await provider.store.call<ClientRow|null>("client",{id:client[1]});if(!row)throw new AppError("notFound",404);res.setHeader("etag",`W/"${row.revision}"`);return json(res,clientDocument(row));}
  if(path==="/clients"&&method==="POST"||client&&method==="PUT"&&!client[2]){
    const p=await body(),id=client?.[1]??randomUUID(),old=await provider.store.call<ClientRow|null>("client",{id});checkMatch(old??undefined,req.headers["if-match"],!!client);
    if(client&&!old)throw new AppError("notFound",404);if(p.enabled!==undefined&&typeof p.enabled!=="boolean")throw new AppError("invalidInput",400);
    const previous=old?provider.store.clientBox.decrypt<Document>(old.encrypted):undefined;
    const secret=p.type==="confidential"?previous?.client_secret as string??randomBytes(32).toString("base64url"):undefined;
    const metadata=clientMetadata(p,id,development,secret),{client_secret:_s,...publicMetadata}=metadata;
    await directory.call("saveClient",{id,revision:old?.revision??0,metadata:JSON.stringify(publicMetadata),encrypted:provider.store.clientBox.encrypt(metadata),enabled:p.enabled!==false});
    res.setHeader("etag",`W/"${(old?.revision??0)+1}"`);
    return json(res,{id,...(!old&&secret||old&&previous?.token_endpoint_auth_method==="none"&&secret?{secret}:{} )},old?200:201);
  }
  if(client?.[2]&&method==="POST"){
    const old=await provider.store.call<ClientRow|null>("client",{id:client[1]});if(!old)throw new AppError("notFound",404);checkMatch(old,req.headers["if-match"],true);
    const metadata=provider.store.clientBox.decrypt<Document>(old.encrypted);if(metadata.token_endpoint_auth_method==="none")throw new AppError("invalidClient",400);
    const secret=randomBytes(32).toString("base64url");metadata.client_secret=secret;
    await directory.call("saveClient",{...old,revision:old.revision,encrypted:provider.store.clientBox.encrypt(metadata)});res.setHeader("etag",`W/"${old.revision+1}"`);return json(res,{id:old.id,secret});
  }
  if(client&&!client[2]&&method==="DELETE"){const old=await provider.store.call<ClientRow|null>("client",{id:client[1]});if(!old)throw new AppError("notFound",404);checkMatch(old,req.headers["if-match"],true);await directory.call("deleteClient",{id:old.id,revision:old.revision});return json(res,{ok:true});}
  if(path==="/credentials"&&method==="GET"){const items=await directory.call<{expiresAt:number}[]>("tokens");return json(res,{items:items.map(c=>({...c,expiresAt:new Date(c.expiresAt).toISOString()})),nextCursor:""});}
  if(path==="/credentials"&&method==="POST"){
    const p=await body(),label=text(p.label,100).trim(),days=Number(p.days),scope=text(p.scope,10);if(!label||!Number.isInteger(days)||days<1||days>365||!["read","write"].includes(scope))throw new AppError("invalidInput",400);
    const token=createOpaqueToken(),id=randomUUID(),expiresAt=Date.now()+days*86400000;await directory.call("createToken",{id,digest:token.digest,label,scope,expiresAt});return json(res,{id,secret:token.secret,expiresAt:new Date(expiresAt).toISOString()},201);
  }
  const credential=/^\/credentials\/([^/]+)$/.exec(path);if(credential&&method==="DELETE"){await directory.call("revokeToken",{id:credential[1]});return json(res,{ok:true});}
  throw new AppError("notFound",404);
}
