import { AppError } from "../errors.ts";

export const USER="urn:ietf:params:scim:schemas:core:2.0:User";
export const GROUP="urn:ietf:params:scim:schemas:core:2.0:Group";
export const ENTERPRISE="urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";
export const LIST="urn:ietf:params:scim:api:messages:2.0:ListResponse";
export const PATCH="urn:ietf:params:scim:api:messages:2.0:PatchOp";
export type Document=Record<string,unknown>;
export interface DirectoryRecord extends Document {id:string;revision:number;createdAt:number;modifiedAt:number}
export class ScimError extends AppError {
  readonly scimType?:string;
  constructor(status:number,detail:string,scimType?:string){super("scimError",status,detail);this.scimType=scimType;}
}
function invalid(detail:string):never{throw new ScimError(400,detail,"invalidValue");}
function object(value:unknown):Document {if(!value||typeof value!=="object"||Array.isArray(value))invalid("Expected an object.");return value as Document;}
const text=(value:unknown,max=1024)=>{if(typeof value!=="string"||Buffer.byteLength(value)>max||/[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(value))invalid("Invalid text attribute.");return value as string;};
function canonical(source:Document,names:string[]):Document {
  const result:Document={};
  for(const [key,value] of Object.entries(source)){
    const name=names.find(n=>n.toLowerCase()===key.toLowerCase());
    if(!name)invalid(`Unsupported attribute: ${key.slice(0,100)}.`);
    if(Object.hasOwn(result,name))invalid("Duplicate attribute.");result[name]=value;
  }
  return result;
}
function strings(source:unknown,names:string[]):Document {const value=canonical(object(source),names);return Object.fromEntries(Object.entries(value).map(([k,v])=>[k,text(v)]));}
const userAttributes=["userName","externalId","displayName","name","active","emails","password","title","userType","locale","timezone","preferredLanguage",ENTERPRISE];
const readOnly=["id","meta","groups","schemas"];
/** One validator is used by SCIM and native administration; passwords never enter stored documents. */
export function normalize(kind:"Users"|"Groups",input:unknown):{document:Document;password?:string} {
  const p=canonical(object(input),[...(kind==="Users"?userAttributes:["displayName","externalId","members"]),...readOnly]);
  if(p.schemas!==undefined&&(!Array.isArray(p.schemas)||!p.schemas.includes(kind==="Users"?USER:GROUP)||p.schemas.some(s=>s!==(kind==="Users"?USER:GROUP)&&(kind!=="Users"||s!==ENTERPRISE))))invalid("Unsupported schemas.");
  for(const key of readOnly)delete p[key];
  const password=p.password===undefined?undefined:text(p.password,1024);delete p.password;
  for(const key of ["externalId","displayName","title","userType","locale","timezone","preferredLanguage"])if(p[key]!==undefined)p[key]=text(p[key],key==="displayName"?100:1024);
  if(kind==="Users"){
    if(typeof p.userName!=="string"||!/^[a-z0-9][a-z0-9_.-]{2,63}$/i.test(p.userName))invalid("userName must contain 3–64 letters, digits, dots, underscores or hyphens.");
    p.userName=p.userName.toLowerCase();p.displayName=p.displayName??p.userName;
    if(!String(p.displayName).trim())invalid("displayName cannot be empty.");
    if(p.active!==undefined&&typeof p.active!=="boolean")invalid("active must be a boolean.");p.active=p.active??true;
    if(p.name!==undefined)p.name=strings(p.name,["formatted","familyName","givenName","middleName","honorificPrefix","honorificSuffix"]);
    if(p.emails!==undefined){
      if(!Array.isArray(p.emails)||p.emails.length>30)invalid("At most 30 emails are supported.");
      let primary=0;
      p.emails=p.emails.map(v=>{const e=canonical(object(v),["value","type","primary","display"]);e.value=text(e.value,320);if(!/^[^\s@]+@[^\s@]+$/.test(String(e.value)))invalid("Invalid email address.");if(e.type!==undefined)e.type=text(e.type,100);if(e.display!==undefined)e.display=text(e.display);if(e.primary!==undefined&&typeof e.primary!=="boolean")invalid("primary must be a boolean.");if(e.primary&&++primary>1)invalid("Only one email may be primary.");return e;});
    }
    if(p.timezone!==undefined){try{new Intl.DateTimeFormat("en",{timeZone:String(p.timezone)});}catch{invalid("Invalid IANA timezone.");}}
    if(p[ENTERPRISE]!==undefined){const e=canonical(object(p[ENTERPRISE]),["employeeNumber","costCenter","organization","division","department","manager"]);for(const k of Object.keys(e))if(k!=="manager")e[k]=text(e[k]);if(e.manager!==undefined){const manager=canonical(object(e.manager),["value","$ref","displayName"]);if(manager.value!==undefined)manager.value=text(manager.value,100);if(manager.$ref!==undefined)manager.$ref=text(manager.$ref,2048);delete manager.displayName;e.manager=manager;}p[ENTERPRISE]=e;}
  }else{
    if(typeof p.displayName!=="string"||!p.displayName.trim())invalid("displayName is required.");
    if(p.members===undefined)p.members=[];
    if(!Array.isArray(p.members)||p.members.length>2000)invalid("At most 2000 members are supported.");
    p.members=[...new Map(p.members.map(v=>{const m=canonical(object(v),["value","$ref","display","type"]),id=text(m.value,100);if(m.type!==undefined&&String(m.type).toLowerCase()!=="user")invalid("Nested groups are unsupported.");if(m.$ref!==undefined&&!String(m.$ref).endsWith(`/Users/${encodeURIComponent(id)}`))invalid("Membership must reference a User.");return [id,{value:id}];})).values()];
  }
  if(Buffer.byteLength(JSON.stringify(p))>(kind==="Users"?16384:256*1024))invalid("The directory resource is too large.");
  return {document:p,password};
}
export const version=(r:DirectoryRecord)=>`W/"${r.revision}"`;
export function resource(kind:"Users"|"Groups",row:DirectoryRecord,base:string):Document {
  const {revision:_r,createdAt,modifiedAt,administrator:_a,...data}=row;
  return {...data,schemas:[kind==="Users"?USER:GROUP,...(data[ENTERPRISE]?[ENTERPRISE]:[])],meta:{resourceType:kind==="Users"?"User":"Group",created:new Date(createdAt).toISOString(),lastModified:new Date(modifiedAt).toISOString(),location:`${base}/${kind}/${encodeURIComponent(row.id)}`,version:version(row)},...(kind==="Users"?{groups:((data.groups??[]) as Document[]).map(g=>({...g,$ref:`${base}/Groups/${encodeURIComponent(String(g.value))}`,type:"direct"}))}:{members:((data.members??[]) as Document[]).map(m=>({...m,$ref:`${base}/Users/${encodeURIComponent(String(m.value))}`,type:"User"}))})};
}

function property(value:unknown,key:string):unknown {if(!value||typeof value!=="object")return undefined;const entry=Object.entries(value).find(([k])=>k.toLowerCase()===key.toLowerCase());return entry?.[1];}
function pathParts(path:string){if(path.toLowerCase()===ENTERPRISE.toLowerCase())return [ENTERPRISE];const enterprise=path.toLowerCase().startsWith(`${ENTERPRISE.toLowerCase()}:`);return enterprise?[ENTERPRISE,...path.slice(ENTERPRISE.length+1).split(".")]:path.split(".");}
export function values(value:unknown,path:string):unknown[]{let list=[value];for(const part of pathParts(path)){list=list.flatMap(v=>Array.isArray(v)?v:[v]).map(v=>property(v,part));}return list.flatMap(v=>Array.isArray(v)?v:[v]).filter(v=>v!==undefined);}

/** Bounded recursive descent SCIM filters, including value paths; no dynamic evaluation. */
export function filterPredicate(source:string):(value:unknown)=>boolean {
  if(source.length>2048)throw new ScimError(400,"Filter is too long.","invalidFilter");
  const tokens=source.match(/"(?:[^"\\]|\\.)*"|\[|\]|\(|\)|[^\s()[\]]+/g)??[];let i=0,depth=0;
  const bad=():never=>{throw new ScimError(400,"Unsupported or malformed filter.","invalidFilter");};
  function expression():((v:unknown)=>boolean){if(++depth>12)bad();let left=conjunction();while(tokens[i]?.toLowerCase()==="or"){i++;const a=left,b=conjunction();left=v=>a(v)||b(v);}depth--;return left;}
  function conjunction():((v:unknown)=>boolean){let left=atom();while(tokens[i]?.toLowerCase()==="and"){i++;const a=left,b=atom();left=v=>a(v)&&b(v);}return left;}
  function atom():((v:unknown)=>boolean){
    if(tokens[i]?.toLowerCase()==="not"){i++;if(tokens[i++]!=="(")bad();const f=expression();if(tokens[i++]!==")")bad();return v=>!f(v);}
    if(tokens[i]==="("){i++;const f=expression();if(tokens[i++]!==")")bad();return f;}
    const attr=tokens[i++];if(!attr||!/^[\w$:.\-]+$/.test(attr)||["__proto__","constructor","prototype"].some(s=>attr.includes(s)))bad();
    if(tokens[i]==="["){i++;const f=expression();if(tokens[i++]!=="]")bad();return v=>values(v,attr).some(f);}
    const op=tokens[i++]?.toLowerCase();if(op==="pr")return v=>values(v,attr).some(x=>x!==null&&x!=="");
    if(!["eq","ne","co","sw","ew","gt","ge","lt","le"].includes(op??""))bad();
    let target:unknown;try{target=JSON.parse(tokens[i++]??"");}catch{bad();}if(target!==null&&!["string","boolean","number"].includes(typeof target))bad();
    const fold=(x:unknown)=>typeof x==="string"?(["id","members.value","groups.value"].includes(attr.toLowerCase())?x:x.toLowerCase()):x;
    return value=>{const list=values(value,attr);const match=(raw:unknown)=>{const a=fold(raw),b=fold(target);if(op==="eq"||op==="ne")return a===b;if(typeof a!=="string"||typeof b!=="string")return typeof a==="number"&&typeof b==="number"&&(op==="gt"?a>b:op==="ge"?a>=b:op==="lt"?a<b:a<=b);return op==="co"?a.includes(b):op==="sw"?a.startsWith(b):op==="ew"?a.endsWith(b):op==="gt"?a>b:op==="ge"?a>=b:op==="lt"?a<b:a<=b;};return op==="ne"?!list.some(match):list.some(match);};
  }
  const result=expression();if(i!==tokens.length||!tokens.length)bad();return result;
}
function actualKey(value:Document,key:string){return Object.keys(value).find(k=>k.toLowerCase()===key.toLowerCase())??key;}
export function applyPatch(kind:"Users"|"Groups",current:Document,input:unknown):Document {
  const p=canonical(object(input),["schemas","Operations"]);if(!Array.isArray(p.schemas)||p.schemas.length!==1||p.schemas[0]!==PATCH||!Array.isArray(p.Operations)||!p.Operations.length||p.Operations.length>100)invalid("Invalid PatchOp document.");
  const result=structuredClone(current);delete result.schemas;delete result.meta;delete result.id;delete result.groups;
  for(const entry of p.Operations){
    const operation=canonical(object(entry),["op","path","value"]),op=String(operation.op).toLowerCase();if(!["add","replace","remove"].includes(op))invalid("Unsupported PATCH operation.");
    if(operation.path===undefined){
      if(op==="remove")throw new ScimError(400,"remove needs a path.","noTarget");
      for(const [k,v] of Object.entries(object(operation.value))){
        if(readOnly.some(n=>n.toLowerCase()===k.toLowerCase()))throw new ScimError(400,"Attribute is read-only.","mutability");
        if(["__proto__","constructor","prototype"].includes(k.toLowerCase()))invalid("Invalid PATCH attribute.");
        const key=actualKey(result,k);
        if(op==="add"&&Array.isArray(result[key])&&Array.isArray(v)){if(key.toLowerCase()==="emails"&&v.some(e=>property(e,"primary")===true))for(const e of result[key])delete object(e).primary;result[key]=[...result[key],...v];}
        else if(op==="add"&&result[key]&&typeof result[key]==="object"&&!Array.isArray(result[key])&&v&&typeof v==="object"&&!Array.isArray(v))result[key]={...result[key] as Document,...v};
        else result[key]=v;
      }continue;
    }
    const path=text(operation.path,512),match=/^([^\[\]]+)(?:\[(.+)\])?(?:\.([\w$]+))?$/.exec(path);if(!match||/(?:^|[.:])(constructor|prototype|__proto__)(?:$|[.\[])/i.test(path))throw new ScimError(400,"Unsupported PATCH path.","invalidPath");
    let parts=pathParts(match[1]);const root=parts[0].toLowerCase();if(["id","meta","groups","schemas"].includes(root))throw new ScimError(400,"Attribute is read-only.","mutability");
    const allowed=kind==="Users"?userAttributes:["displayName","externalId","members"];if(!allowed.some(k=>k.toLowerCase()===root))throw new ScimError(400,"Unsupported PATCH path.","invalidPath");
    let parent=result;for(const part of parts.slice(0,-1)){const key=actualKey(parent,part);if(parent[key]===undefined&&op!=="remove")parent[key]={};parent=object(parent[key]);}
    const key=actualKey(parent,parts.at(-1)!);
    if(match[2]){
      const f=filterPredicate(match[2]),array=parent[key];if(!Array.isArray(array))throw new ScimError(400,"The target is not multivalued.","invalidPath");
      if(!array.some(f))throw new ScimError(400,"No matching attribute.","noTarget");
      if(key.toLowerCase()==="emails"&&op!=="remove"&&(match[3]?.toLowerCase()==="primary"&&operation.value===true||property(operation.value,"primary")===true))for(const email of array)if(!f(email))delete object(email).primary;
      parent[key]=array.flatMap(v=>{if(!f(v))return [v];if(match[3]){const copy={...object(v)},sub=actualKey(copy,match[3]);if(op==="remove")delete copy[sub];else copy[sub]=operation.value;return [copy];}if(op==="remove")return [];if(op==="add")return [{...object(v),...object(operation.value)}];return Array.isArray(operation.value)?operation.value:[operation.value];});
    }else if(op==="remove"){if(!Object.hasOwn(parent,key))throw new ScimError(400,"No matching attribute.","noTarget");delete parent[key];}
    else if(op==="add"&&Array.isArray(parent[key])){const entries=Array.isArray(operation.value)?operation.value:[operation.value];if(key.toLowerCase()==="emails"&&entries.some(e=>property(e,"primary")===true))for(const email of parent[key])delete object(email).primary;parent[key]=[...parent[key],...entries];}
    else if(op==="add"&&parent[key]&&typeof parent[key]==="object"&&operation.value&&typeof operation.value==="object"&&!Array.isArray(operation.value))parent[key]={...parent[key] as Document,...operation.value};
    else parent[key]=operation.value;
  }
  return result;
}

export function project(document:Document,url:URL):Document {
  const include=url.searchParams.get("attributes"),exclude=url.searchParams.get("excludedAttributes");if(include&&exclude)invalid("attributes and excludedAttributes cannot be combined.");
  const output=include?{schemas:document.schemas,id:document.id} as Document:structuredClone(document);
  const paths=(include??exclude??"").split(",").filter(Boolean);if(paths.length>100)invalid("Too many attribute paths.");
  function pick(from:unknown,to:unknown,parts:string[]){
    if(Array.isArray(from)){const result=Array.isArray(to)?to:[];from.forEach((v,i)=>{result[i]=pick(v,result[i],parts);});return result;}
    if(!from||typeof from!=="object")return to;
    const source=from as Document,key=actualKey(source,parts[0]);if(source[key]===undefined)return to;
    const result=(to&&typeof to==="object"?to:{}) as Document;
    if(parts.length===1)result[key]=structuredClone(source[key]);else result[key]=pick(source[key],result[key],parts.slice(1));return result;
  }
  function omit(value:unknown,parts:string[]){if(Array.isArray(value)){for(const v of value)omit(v,parts);return;}if(!value||typeof value!=="object")return;const object=value as Document,key=actualKey(object,parts[0]);if(parts.length===1)delete object[key];else omit(object[key],parts.slice(1));}
  for(const path of paths){const parts=pathParts(path);if(parts.some(p=>["__proto__","constructor","prototype"].includes(p.toLowerCase())))invalid("Invalid attribute path.");if(include)pick(document,output,parts);else if(!["id","schemas"].includes(parts[0].toLowerCase()))omit(output,parts);}
  return output;
}
