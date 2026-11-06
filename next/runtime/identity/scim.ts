import type { IncomingMessage, ServerResponse } from "node:http";
import type { Database } from "../database.ts";
import type { authService } from "../auth/service.ts";
import { digestToken } from "../auth/session.ts";
import { AppError } from "../errors.ts";
import { directoryService } from "./directory.ts";
import { ENTERPRISE, GROUP, LIST, USER, ScimError, filterPredicate, project, resource, version, type Document } from "./scim-model.ts";

const ERROR="urn:ietf:params:scim:api:messages:2.0:Error";
export function scimJson(res:ServerResponse,value:unknown,status=200){res.writeHead(status,{"content-type":"application/scim+json; charset=utf-8","cache-control":"no-store"});res.end(status===204?undefined:JSON.stringify(value));}
export async function scimBody(req:IncomingMessage){
  if(!/^application\/(scim\+json|json)(?:\s*;|$)/i.test(req.headers["content-type"]??""))throw new ScimError(415,"Use application/scim+json.");
  let size=0;const chunks:Buffer[]=[];
  for await(const chunk of req.iterator({destroyOnReturn:false})){size+=chunk.length;if(size>256*1024){req.resume();throw new ScimError(413,"The request body is too large.");}chunks.push(chunk);}
  try{const value:unknown=JSON.parse(Buffer.concat(chunks).toString("utf8"));if(!value||typeof value!=="object"||Array.isArray(value))throw Error();return value as Document;}catch{throw new ScimError(400,"Invalid JSON.","invalidSyntax");}
}
const attribute=(name:string,type="string",options:Document={})=>({name,type,multiValued:false,description:name,required:false,caseExact:false,mutability:"readWrite",returned:"default",uniqueness:"none",...options});
export function scimSchemas(base:string){
  const common=[attribute("id","string",{mutability:"readOnly",returned:"always",caseExact:true,uniqueness:"global"}),attribute("externalId")];
  return [
    {id:USER,name:"User",description:"Dar native account",attributes:[...common,attribute("userName","string",{required:true,uniqueness:"server"}),attribute("displayName"),attribute("active","boolean"),attribute("name","complex",{subAttributes:["formatted","familyName","givenName","middleName","honorificPrefix","honorificSuffix"].map(n=>attribute(n))}),attribute("emails","complex",{multiValued:true,subAttributes:[attribute("value"),attribute("type"),attribute("primary","boolean"),attribute("display")]}),attribute("password","string",{mutability:"writeOnly",returned:"never"}),...(["title","userType","locale","timezone","preferredLanguage"].map(n=>attribute(n))),attribute("groups","complex",{multiValued:true,mutability:"readOnly",subAttributes:[attribute("value","string",{mutability:"readOnly",caseExact:true}),attribute("$ref","reference",{referenceTypes:["Group"],mutability:"readOnly"}),attribute("display","string",{mutability:"readOnly"}),attribute("type","string",{mutability:"readOnly"})]})]},
    {id:GROUP,name:"Group",description:"Directory groups do not grant administrator privileges",attributes:[...common,attribute("displayName","string",{required:true}),attribute("members","complex",{multiValued:true,subAttributes:[attribute("value","string",{required:true,caseExact:true}),attribute("$ref","reference",{referenceTypes:["User"]}),attribute("display","string",{mutability:"readOnly"}),attribute("type","string",{canonicalValues:["User"]})]})]},
    {id:ENTERPRISE,name:"EnterpriseUser",description:"Enterprise profile",attributes:[...(["employeeNumber","costCenter","organization","division","department"].map(n=>attribute(n))),attribute("manager","complex",{subAttributes:[attribute("value","string",{caseExact:true}),attribute("$ref","reference",{referenceTypes:["User"]}),attribute("displayName","string",{mutability:"readOnly"})]})]}
  ].map(s=>({...s,schemas:["urn:ietf:params:scim:schemas:core:2.0:Schema"],meta:{resourceType:"Schema",location:`${base}/Schemas/${encodeURIComponent(s.id)}`}}));
}
export async function handleScim(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,auth:Awaited<ReturnType<typeof authService>>,origin:string){
  try{
    const credential=/^Bearer ([A-Za-z0-9_-]{43})$/i.exec(req.headers.authorization??"")?.[1];if(!credential)throw new AppError("unauthenticated",401);
    const directory=directoryService(db,auth,{bearer:digestToken(credential)});
    // Authenticate even discovery and empty lists; cookies and OIDC tokens are never provisioning authority.
    await directory.call("authorize");
    const method=req.method??"GET",base=`${origin}/scim/v2`,path=url.pathname.slice("/scim/v2".length).replace(/\/$/,"");
    if(method==="GET"&&path==="/ServiceProviderConfig")return scimJson(res,{schemas:["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],patch:{supported:true},bulk:{supported:false,maxOperations:0,maxPayloadSize:0},filter:{supported:true,maxResults:200},changePassword:{supported:true},sort:{supported:false},etag:{supported:true},authenticationSchemes:[{type:"oauthbearertoken",name:"Provisioning bearer credential",description:"Dedicated expiring administrator-issued SCIM credential",primary:true}],meta:{resourceType:"ServiceProviderConfig",location:`${base}/ServiceProviderConfig`}});
    const discovery=/^\/(Schemas|ResourceTypes)(?:\/(.+))?$/.exec(path);
    if(method==="GET"&&discovery){
      const items:Document[]=discovery[1]==="Schemas"?scimSchemas(base):["User","Group"].map(k=>({schemas:["urn:ietf:params:scim:schemas:core:2.0:ResourceType"],id:k,name:k,endpoint:`/${k}s`,schema:k==="User"?USER:GROUP,...(k==="User"?{schemaExtensions:[{schema:ENTERPRISE,required:false}]}:{}),meta:{resourceType:"ResourceType",location:`${base}/ResourceTypes/${k}`}}));
      if(discovery[2]){const item=items.find(v=>v.id===decodeURIComponent(discovery[2]));if(!item)throw new AppError("notFound",404);return scimJson(res,item);}
      return scimJson(res,{schemas:[LIST],totalResults:items.length,startIndex:1,itemsPerPage:items.length,Resources:items});
    }
    const route=/^\/(Users|Groups)(?:\/([^/]+))?$/.exec(path);if(!route)throw new AppError("notFound",404);
    const kind=route[1] as "Users"|"Groups",id=route[2]?decodeURIComponent(route[2]):undefined;
    if(id&&id.length>100)throw new AppError("notFound",404);
    if(method==="GET"&&!id){
      if(url.searchParams.has("sortBy")||url.searchParams.has("sortOrder"))throw new ScimError(400,"Sorting is unsupported.","invalidValue");
      const parse=(name:string,fallback:number)=>{const value=url.searchParams.get(name);if(value===null)return fallback;if(!/^\d{1,9}$/.test(value))throw new ScimError(400,`Invalid ${name}.`,"invalidValue");return Number(value);};
      const startIndex=Math.max(1,parse("startIndex",1)),count=Math.min(200,parse("count",100));
      const filter=url.searchParams.get("filter"),predicate=filter===null?()=>true:filterPredicate(filter);
      const all=(await directory.list(kind)).map(v=>resource(kind,v,base)).filter(predicate).sort((a,b)=>String(a.id).localeCompare(String(b.id)));
      const rows=all.slice(startIndex-1,startIndex-1+count).map(v=>project(v,url));
      return scimJson(res,{schemas:[LIST],totalResults:all.length,startIndex,itemsPerPage:rows.length,Resources:rows});
    }
    if(method==="GET"&&id){const row=await directory.get(kind,id);res.setHeader("etag",version(row));if(req.headers["if-none-match"]===version(row)){res.writeHead(304);res.end();return;}return scimJson(res,project(resource(kind,row,base),url));}
    if(method==="POST"&&!id||["PUT","PATCH"].includes(method)&&id){
      const row=await directory.save(kind,id,await scimBody(req),req.headers["if-match"],method==="PATCH");
      res.setHeader("etag",version(row));res.setHeader("location",`${base}/${kind}/${encodeURIComponent(row.id)}`);
      return scimJson(res,project(resource(kind,row,base),url),method==="POST"?201:200);
    }
    if(method==="DELETE"&&id){await directory.remove(kind,id,req.headers["if-match"]);return scimJson(res,undefined,204);}
    res.setHeader("allow",id?"GET, PUT, PATCH, DELETE":"GET, POST");throw new ScimError(405,"Method not allowed.");
  }catch(error){
    const status=error instanceof AppError?error.status:500;
    if(status===401)res.setHeader("www-authenticate",'Bearer realm="Dar SCIM"');if(status===413)res.setHeader("connection","close");
    scimJson(res,{schemas:[ERROR],status:String(status),detail:error instanceof AppError?error.message:"The provisioning request could not be completed.",...(error instanceof ScimError&&error.scimType?{scimType:error.scimType}:{})},status);
  }
}
