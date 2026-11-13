import {SaxesParser} from "saxes";
import {createHash} from "node:crypto";
import {AppError} from "../errors.ts";
import {absolute,escapeHtml,sanitize,textOf} from "./html.ts";
import {newsLimits,type NewsEntry} from "../../shared/news.ts";

interface Node {name:string;uri:string;attrs:Record<string,string>;content:(Node|string)[]}
export interface ParsedFeed {kind:"RSS"|"Atom"|"JSON Feed";title:string;link:string;entries:NewsEntry[]}
const md5=(value:string)=>createHash("md5").update(value).digest("hex");
const ATOM="http://www.w3.org/2005/Atom",MEDIA="http://search.yahoo.com/mrss/";

/** Decodes using the XML declaration or Content-Type charset; unknown labels fall back to UTF-8. */
export function decodeBody(body:Buffer,contentType:string){
  if(body[0]===0xef&&body[1]===0xbb&&body[2]===0xbf)return body.subarray(3).toString("utf8");
  const head=body.subarray(0,300).toString("latin1"),label=/encoding=["']([A-Za-z0-9._-]{1,40})["']/.exec(head)?.[1]??/charset=([A-Za-z0-9._-]{1,40})/i.exec(contentType)?.[1]??"utf-8";
  try{return new TextDecoder(label.toLowerCase()).decode(body);}catch{return body.toString("utf8");}
}
function parseXml(text:string):Node{
  const stack:Node[]=[];let root:Node|undefined,nodes=0;
  const parser=new SaxesParser({xmlns:true});
  // Real feeds contain undefined HTML entities and other recoverable mistakes; keep going.
  parser.on("error",()=>{});
  parser.on("opentag",tag=>{
    if(++nodes>200000||stack.length>64)throw new AppError("feedTooLarge",413,"The feed is too large.");
    const node:Node={name:tag.local,uri:tag.uri,attrs:Object.fromEntries(Object.values(tag.attributes).map(a=>[a.local,a.value])),content:[]};
    if(stack.length)stack.at(-1)!.content.push(node);else if(!root)root=node;
    stack.push(node);
  });
  const add=(value:string)=>{if(stack.length)stack.at(-1)!.content.push(value);};
  parser.on("text",add);parser.on("cdata",add);
  parser.on("closetag",()=>{stack.pop();});
  try{parser.write(text).close();}catch(error){if(error instanceof AppError)throw error;}
  if(!root)throw new AppError("notAFeed",422,"That address is not an RSS, Atom or JSON feed.");
  return root;
}
const kids=(node:Node|undefined,name?:string,uri?:string)=>(node?.content??[]).filter((c):c is Node=>typeof c!=="string"&&(!name||c.name===name)&&(uri===undefined||c.uri===uri));
const kid=(node:Node|undefined,name:string,uri?:string)=>kids(node,name,uri)[0];
const textIn=(node:Node|undefined):string=>(node?.content??[]).map(c=>typeof c==="string"?c:textIn(c)).join("");
const value=(node:Node|undefined,name:string,uri?:string)=>textIn(kid(node,name,uri)).trim();
function serialize(node:Node):string{return node.content.map(c=>typeof c==="string"?escapeHtml(c):`<${c.name}${Object.entries(c.attrs).map(([k,v])=>` ${k}="${escapeHtml(v)}"`).join("")}>${serialize(c)}</${c.name}>`).join("");}
const date=(...values:string[])=>{for(const v of values){const t=Date.parse(v.trim().replace(/\s+(UT|Z)$/," GMT"));if(v&&Number.isFinite(t)&&t>0)return t;}return 0;};
const rtlLanguage=(lang:string)=>/^(ar|fa|he|iw|ur|ps|sd|yi|dv|ckb|ug)\b/i.test(lang.trim());

function entry(input:{guid:string;url:string;title:string;author:string;pubDate:number;updatedDate:number;html:string;enclosureMime:string;enclosureLink:string;thumbnail:string;mediaDescription:string;rtl:boolean},base:string,fetchedAt:number):NewsEntry{
  const url=absolute(input.url,base),body=sanitize(input.html,url||base,newsLimits.body-1024),title=textOf(input.title).slice(0,newsLimits.title);
  const guid=(input.guid.trim()||url||md5(title+body)).slice(0,newsLimits.url),enclosureLink=absolute(input.enclosureLink,base),pubDate=input.pubDate||fetchedAt;
  const author=textOf(input.author).slice(0,500),mediaThumbnail=absolute(input.thumbnail,base),mediaDescription=textOf(input.mediaDescription).slice(0,4000);
  return {guid,guidHash:md5(guid),url,title,author,pubDate,updatedDate:input.updatedDate||pubDate,body,enclosureMime:input.enclosureMime.slice(0,200),enclosureLink,mediaThumbnail,mediaDescription,rtl:input.rtl,
    fingerprint:md5(`${title}\n${url}\n${body}\n${enclosureLink}`),contentHash:md5(`${title}\n${author}\n${url}\n${body}\n${enclosureLink}\n${mediaThumbnail}`),excerpt:textOf(body,2000).slice(0,300)};
}
function media(node:Node){
  const group=kid(node,"group")??node,content=kids(group,"content").find(c=>/^(audio|video)\//.test(c.attrs.type??"")||c.attrs.medium==="audio"||c.attrs.medium==="video");
  const thumb=kid(group,"thumbnail")??kid(node,"thumbnail")??kids(group,"content").find(c=>c.attrs.medium==="image"||/^image\//.test(c.attrs.type??""));
  const image=kid(node,"image");
  return {thumbnail:thumb?.attrs.url??thumb?.attrs.href??image?.attrs.href??"",description:value(group,"description",MEDIA),link:content?.attrs.url??"",type:content?.attrs.type??(content?.attrs.medium?content.attrs.medium+"/*":"")};
}

export function parseFeed(body:Buffer,contentType:string,feedUrl:string,fetchedAt=Date.now()):ParsedFeed{
  const text=decodeBody(body,contentType),start=text.trimStart();
  if(/json/.test(contentType)||start.startsWith("{"))return parseJsonFeed(start,feedUrl,fetchedAt);
  if(!/^<(\?xml|rss|feed|rdf:RDF|!--|!DOCTYPE)/i.test(start)||!/<(rss|feed|rdf:RDF)[\s>]/i.test(start.slice(0,4000)))throw new AppError("notAFeed",422,"That address is not an RSS, Atom or JSON feed.");
  const root=parseXml(start),limit=newsLimits.itemsPerFeed;
  if(root.name==="feed"){
    const link=(node:Node,rel="alternate")=>kids(node,"link").find(l=>(l.attrs.rel??"alternate")===rel)?.attrs.href??"";
    const site=absolute(link(root),feedUrl)||feedUrl,rtl=rtlLanguage(root.attrs.lang??"");
    const content=(node:Node|undefined)=>{if(!node)return "";const type=(node.attrs.type??"text").toLowerCase();if(type==="xhtml")return serialize(kid(node,"div")??node);return type==="html"||type==="text/html"?textIn(node):`<p>${escapeHtml(textIn(node)).replace(/\n{2,}/g,"</p><p>")}</p>`;};
    const entries=kids(root,"entry",ATOM).slice(0,limit).map(e=>{
      const enclosure=kids(e,"link").find(l=>l.attrs.rel==="enclosure"),m=media(e);
      return entry({guid:value(e,"id"),url:link(e)||link(e,"related"),title:textIn(kid(e,"title")),author:value(kid(e,"author")??kid(root,"author"),"name"),pubDate:date(value(e,"published"),value(e,"updated"),value(e,"issued")),updatedDate:date(value(e,"updated")),
        html:content(kid(e,"content"))||content(kid(e,"summary")),enclosureMime:enclosure?.attrs.type??m.type,enclosureLink:enclosure?.attrs.href??m.link,thumbnail:m.thumbnail,mediaDescription:m.description,rtl:rtl||rtlLanguage(e.attrs.lang??"")},site,fetchedAt);
    });
    return {kind:"Atom",title:textOf(textIn(kid(root,"title"))).slice(0,newsLimits.title)||new URL(feedUrl).hostname,link:site,entries};
  }
  const channel=root.name==="rss"?kid(root,"channel"):root.name==="RDF"?kid(root,"channel"):undefined;
  if(!channel)throw new AppError("notAFeed",422,"That address is not an RSS, Atom or JSON feed.");
  const site=absolute(kids(channel,"link").find(l=>l.uri!==ATOM)?textIn(kids(channel,"link").find(l=>l.uri!==ATOM)).trim():"",feedUrl)||feedUrl,rtl=rtlLanguage(value(channel,"language"));
  const items=(root.name==="RDF"?kids(root,"item"):kids(channel,"item")).slice(0,limit).map(i=>{
    const enclosure=kid(i,"enclosure"),m=media(i),guid=kid(i,"guid"),link=kids(i,"link").find(l=>l.uri!==ATOM);
    const url=textIn(link).trim()||(guid&&guid.attrs.isPermaLink!=="false"&&/^https?:/.test(textIn(guid).trim())?textIn(guid).trim():"")||(i.attrs.about??"");
    return entry({guid:textIn(guid).trim()||i.attrs.about||"",url,title:textIn(kid(i,"title")),author:value(i,"creator")||value(i,"author"),pubDate:date(value(i,"pubDate"),value(i,"date"),value(i,"published")),updatedDate:date(value(i,"updated"),value(i,"modified")),
      html:value(i,"encoded")||textIn(kid(i,"description")),enclosureMime:enclosure?.attrs.type??m.type,enclosureLink:enclosure?.attrs.url??m.link,thumbnail:m.thumbnail||kid(i,"image",undefined)?.attrs.href||"",mediaDescription:m.description,rtl},site,fetchedAt);
  });
  return {kind:"RSS",title:textOf(value(channel,"title")).slice(0,newsLimits.title)||new URL(feedUrl).hostname,link:site,entries:items};
}
function parseJsonFeed(text:string,feedUrl:string,fetchedAt:number):ParsedFeed{
  let data:Record<string,unknown>;try{data=JSON.parse(text);}catch{throw new AppError("notAFeed",422,"That address is not an RSS, Atom or JSON feed.");}
  if(!data||typeof data!=="object"||typeof data.version!=="string"||!data.version.startsWith("https://jsonfeed.org/version/")||!Array.isArray(data.items))throw new AppError("notAFeed",422,"That address is not an RSS, Atom or JSON feed.");
  const s=(v:unknown)=>typeof v==="string"?v:"",site=absolute(s(data.home_page_url),feedUrl)||feedUrl,rtl=rtlLanguage(s(data.language));
  const entries=data.items.slice(0,newsLimits.itemsPerFeed).filter(v=>v&&typeof v==="object").map(v=>{
    const i=v as Record<string,unknown>,authors=Array.isArray(i.authors)?i.authors:i.author?[i.author]:[],attachment=Array.isArray(i.attachments)?(i.attachments[0]??{}) as Record<string,unknown>:{};
    const html=s(i.content_html)||(s(i.content_text)?`<p>${escapeHtml(s(i.content_text)).replace(/\n{2,}/g,"</p><p>")}</p>`:escapeHtml(s(i.summary)));
    return entry({guid:String(i.id??""),url:s(i.url)||s(i.external_url),title:s(i.title),author:s((authors[0] as Record<string,unknown>|undefined)?.name),pubDate:date(s(i.date_published),s(i.date_modified)),updatedDate:date(s(i.date_modified)),html,
      enclosureMime:s(attachment.mime_type),enclosureLink:s(attachment.url),thumbnail:s(i.image)||s(i.banner_image),mediaDescription:s(i.summary),rtl:rtl||rtlLanguage(s(i.language))},site,fetchedAt);
  });
  return {kind:"JSON Feed",title:s(data.title).slice(0,newsLimits.title)||new URL(feedUrl).hostname,link:site,entries};
}
