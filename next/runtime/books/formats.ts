import {SaxesParser} from "saxes";
import path from "node:path/posix";
import type {BookMeta,BookTocEntry} from "../../shared/books.ts";

interface Node {name:string;prefix:string;attrs:Record<string,string>;content:(Node|string)[]}
/** A small XML tree. Book files often carry HTML entities or other recoverable mistakes; parsing continues past them. */
export function parseXml(text:string,maxNodes=200000):Node|undefined{
  const stack:Node[]=[];let root:Node|undefined,nodes=0,stop=false;
  const parser=new SaxesParser({xmlns:false});
  parser.on("error",()=>{});
  parser.on("opentag",tag=>{
    if(stop)return;if(++nodes>maxNodes||stack.length>100){stop=true;return;}
    const colon=tag.name.indexOf(":"),node:Node={name:(colon>=0?tag.name.slice(colon+1):tag.name).toLowerCase(),prefix:colon>=0?tag.name.slice(0,colon):"",attrs:{},content:[]};
    for(const [key,value] of Object.entries(tag.attributes as Record<string,string>))node.attrs[key.toLowerCase()]=value;
    if(stack.length)stack.at(-1)!.content.push(node);else if(!root)root=node;
    if(!tag.isSelfClosing)stack.push(node);
  });
  const add=(value:string)=>{if(!stop&&stack.length)stack.at(-1)!.content.push(value);};
  parser.on("text",add);parser.on("cdata",add);
  parser.on("closetag",tag=>{if(!stop&&!tag.isSelfClosing)stack.pop();});
  try{parser.write(text.replace(/^﻿/,"")).close();}catch{/* keep what was read */}
  return root;
}
const kids=(node:Node|undefined,name?:string)=>(node?.content??[]).filter((c):c is Node=>typeof c!=="string"&&(!name||c.name===name));
const kid=(node:Node|undefined,name:string)=>kids(node,name)[0];
function* walk(node:Node|undefined):Generator<Node>{if(!node)return;yield node;for(const child of kids(node))yield* walk(child);}
export const textOf=(node:Node|undefined):string=>(node?.content??[]).map(c=>typeof c==="string"?c:textOf(c)).join("");
const clean=(value:string,max:number)=>value.replace(/\s+/g," ").trim().slice(0,max);
/** Book descriptions are often HTML, sometimes escaped twice; keep plain paragraphs. */
export function plainDescription(value:string){
  const unescaped=/&lt;\/?[a-z]/i.test(value)?value.replace(/&lt;/g,"<").replace(/&gt;/g,">").replace(/&quot;/g,"\"").replace(/&#39;/g,"'").replace(/&amp;/g,"&"):value;
  return unescaped.replace(/<\s*(br|\/p|\/div|\/li)\b[^>]*>/gi,"\n").replace(/<[^>]*>/g,"").replace(/&nbsp;/g," ").replace(/&amp;/g,"&").replace(/&lt;/g,"<").replace(/&gt;/g,">").replace(/&quot;/g,"\"").replace(/&#(\d+);/g,(_m,n)=>{const code=Number(n);return code>0&&code<0x110000?String.fromCodePoint(code):"";})
    .split(/\n+/).map(line=>line.replace(/[ \t]+/g," ").trim()).filter(Boolean).join("\n").slice(0,4000);
}
const rtl=(lang:string)=>/^(ar|fa|he|iw|ur|ps|sd|yi|dv|ckb|ug)\b/i.test(lang.trim());
const decode=(href:string)=>{try{return decodeURIComponent(href);}catch{return href;}};
/** Resolves a manifest href against its document; undefined when it leaves the archive. */
export function resolveHref(base:string,href:string){
  const target=decode(href.split("#")[0]);if(!target||/^[a-z][a-z0-9+.-]*:/i.test(target))return undefined;
  const joined=path.normalize(path.join(path.dirname(base),target));
  return joined.startsWith("../")||joined==="."||joined.startsWith("/")?undefined:joined;
}
export const emptyMeta=(title:string):BookMeta=>({title,authors:[],series:"",seriesIndex:null,language:"",publisher:"",published:"",description:"",subjects:[],pages:0,cover:false,rightToLeft:false,error:""});

export interface EpubPackage {
  meta:BookMeta;opf:string;spine:{href:string;size:number}[];toc:BookTocEntry[];cover:string|undefined;encrypted:boolean;
}
/** container.xml → package document: metadata, reading order, cover and table of contents. */
export async function readEpub(get:(name:string)=>{size:number}|undefined,read:(name:string,max:number)=>Promise<Buffer>,fallbackTitle:string):Promise<EpubPackage>{
  const container=parseXml((await read("META-INF/container.xml",256*1024)).toString("utf8"));
  const opf=[...walk(container)].find(n=>n.name==="rootfile"&&/oebps-package|^$/.test(n.attrs["media-type"]??"")&&n.attrs["full-path"])?.attrs["full-path"];
  if(!opf||!get(opf))throw new Error("No package document.");
  // Font obfuscation is not DRM; any other encrypted resource is.
  let encrypted=false;
  if(get("META-INF/encryption.xml")){
    const doc=parseXml((await read("META-INF/encryption.xml",1024*1024)).toString("utf8"));
    encrypted=[...walk(doc)].some(n=>n.name==="encryptionmethod"&&!/(idpf\.org\/2008\/embedding|ns\.adobe\.com\/pdf\/enc#RC)/.test(n.attrs.algorithm??""));
  }
  const pkg=parseXml((await read(opf,4*1024*1024)).toString("utf8"));if(!pkg)throw new Error("Damaged package document.");
  const metadata=kid(pkg,"metadata"),manifest=kid(pkg,"manifest"),spineNode=kid(pkg,"spine");
  const items=new Map<string,{href:string;type:string;properties:string}>();
  for(const item of kids(manifest,"item")){const href=resolveHref(opf,item.attrs.href??"");if(item.attrs.id&&href)items.set(item.attrs.id,{href,type:item.attrs["media-type"]??"",properties:item.attrs.properties??""});}
  const metaNodes=kids(metadata),values=(name:string)=>metaNodes.filter(n=>n.name===name).map(n=>clean(textOf(n),500)).filter(Boolean);
  const refines=(id:string|undefined,property:string)=>id?metaNodes.find(n=>n.name==="meta"&&n.attrs.refines===`#${id}`&&n.attrs.property===property):undefined;
  const named=(name:string)=>metaNodes.find(n=>n.name==="meta"&&n.attrs.name===name)?.attrs.content;
  const creators=metaNodes.filter(n=>n.name==="creator").filter(n=>{const role=n.attrs["opf:role"]??n.attrs.role??(refines(n.attrs.id,"role")?textOf(refines(n.attrs.id,"role")).trim():"");return !role||role==="aut";});
  const meta=emptyMeta(values("title")[0]||fallbackTitle);
  meta.authors=[...new Set(creators.map(n=>clean(textOf(n),200)).filter(Boolean))].slice(0,10);
  meta.language=(values("language")[0]??"").slice(0,35);meta.publisher=(values("publisher")[0]??"").slice(0,200);
  meta.published=(values("date")[0]??"").slice(0,10);meta.subjects=values("subject").slice(0,12).map(s=>s.slice(0,60));
  meta.description=plainDescription(metaNodes.filter(n=>n.name==="description").map(textOf).join("\n"));
  // Calibre series, or EPUB 3 belongs-to-collection of type series.
  const collection=metaNodes.find(n=>n.name==="meta"&&n.attrs.property==="belongs-to-collection"&&(!refines(n.attrs.id,"collection-type")||textOf(refines(n.attrs.id,"collection-type")).trim()==="series"));
  meta.series=clean(named("calibre:series")??(collection?textOf(collection):""),200);
  const index=Number(named("calibre:series_index")??(collection?textOf(refines(collection.attrs.id,"group-position")):""));
  meta.seriesIndex=meta.series&&Number.isFinite(index)&&index>=0&&index<100000?index:null;
  meta.rightToLeft=spineNode?.attrs["page-progression-direction"]==="rtl"||(!spineNode?.attrs["page-progression-direction"]&&rtl(meta.language));
  const spine:{href:string;size:number}[]=[];
  for(const ref of kids(spineNode,"itemref")){const item=items.get(ref.attrs.idref??"");if(item&&/x?html|xml/.test(item.type)&&get(item.href)&&spine.length<5000)spine.push({href:item.href,size:get(item.href)!.size});}
  if(!spine.length)throw new Error("Empty reading order.");
  // Cover: EPUB 3 cover-image, EPUB 2 <meta name="cover">, else an image named like a cover.
  const images=[...items.values()].filter(i=>/^image\/(jpeg|png|gif|webp|svg\+xml)$/.test(i.type));
  const cover=[...items.values()].find(i=>/\bcover-image\b/.test(i.properties))?.href??items.get(named("cover")??"")?.href??images.find(i=>/cover/i.test(i.href))?.href;
  meta.cover=!!cover&&!!get(cover);
  const position=(href:string|undefined)=>{if(!href)return -1;return spine.findIndex(s=>s.href===href);};
  let toc:BookTocEntry[]=[];
  const nav=[...items.values()].find(i=>/\bnav\b/.test(i.properties));
  if(nav&&get(nav.href)){
    const doc=parseXml((await read(nav.href,4*1024*1024)).toString("utf8"));
    const navs=[...walk(doc)].filter(n=>n.name==="nav"),tocNav=navs.find(n=>/\btoc\b/.test(n.attrs["epub:type"]??n.attrs.type??""))??navs[0];
    const list=(ol:Node|undefined,depth:number)=>{for(const li of kids(ol,"li")){if(toc.length>=2000)return;const a=kid(li,"a")??kid(li,"span"),href=a?.attrs.href?resolveHref(nav.href,a.attrs.href):undefined,label=clean(textOf(a),200);
      if(label)toc.push({label,href:href?href+(a!.attrs.href.includes("#")?"#"+a!.attrs.href.split("#")[1]:""):"",depth:Math.min(depth,6),spine:position(href)});list(kid(li,"ol"),depth+1);}};
    list(kid(tocNav,"ol"),0);
  }
  const ncxId=spineNode?.attrs.toc,ncx=ncxId?items.get(ncxId):[...items.values()].find(i=>i.type==="application/x-dtbncx+xml");
  if(!toc.length&&ncx&&get(ncx.href)){
    const doc=parseXml((await read(ncx.href,4*1024*1024)).toString("utf8"));
    const points=(node:Node|undefined,depth:number)=>{for(const point of kids(node,"navpoint")){if(toc.length>=2000)return;const src=kid(point,"content")?.attrs.src??"",href=resolveHref(ncx.href,src),label=clean(textOf(kid(kid(point,"navlabel"),"text")),200);
      if(label)toc.push({label,href:href?href+(src.includes("#")?"#"+src.split("#")[1]:""):"",depth:Math.min(depth,6),spine:position(href)});points(point,depth+1);}};
    points(kid(doc,"navmap"),0);
  }
  if(!toc.length)toc=spine.map((item,index)=>({label:`Section ${index+1}`,href:item.href,depth:0,spine:index}));
  meta.pages=Math.max(1,Math.round(spine.reduce((n,s)=>n+s.size,0)/2200));
  return {meta,opf,spine,toc,cover:meta.cover?cover:undefined,encrypted};
}

/** Comic pages: images in natural name order, skipping macOS metadata and hidden files. */
export function comicPages(names:string[]){
  const collator=new Intl.Collator("en",{numeric:true,sensitivity:"base"});
  return names.filter(name=>/\.(jpe?g|png|gif|webp|avif|bmp)$/i.test(name)&&!/(^|\/)(__MACOSX|\.)/.test(name)).sort(collator.compare).slice(0,5000);
}
/** ComicInfo.xml (ComicRack) fields that matter for a library. */
export function readComicInfo(xml:string,meta:BookMeta){
  const doc=parseXml(xml,20000),field=(name:string)=>clean(textOf(kid(doc,name.toLowerCase())),500);
  if(field("Title"))meta.title=field("Title").slice(0,300);
  if(field("Series")){meta.series=field("Series").slice(0,200);const n=Number(field("Number"));meta.seriesIndex=Number.isFinite(n)&&field("Number")?n:null;if(!field("Title"))meta.title=`${meta.series}${field("Number")?` #${field("Number")}`:""}`;}
  const writers=field("Writer").split(/,\s*/).filter(Boolean).slice(0,10);if(writers.length)meta.authors=writers;
  meta.description=plainDescription(field("Summary"));meta.publisher=field("Publisher").slice(0,200);
  if(field("Year"))meta.published=field("Year").slice(0,4);meta.language=field("LanguageISO").slice(0,35);
  meta.rightToLeft=/^YesAndRightToLeft$/i.test(field("Manga"));
  return meta;
}
