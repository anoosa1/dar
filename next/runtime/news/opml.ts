import {SaxesParser} from "saxes";
import {AppError} from "../errors.ts";
import {absolute,escapeHtml} from "./html.ts";
import {newsLimits} from "../../shared/news.ts";

/** Outlines with xmlUrl are feeds; an enclosing outline names their folder (nesting is flattened). */
export function parseOpml(text:string){
  const feeds:{url:string;title:string;folder:string}[]=[],stack:{folder:string}[]=[];let outlines=0,opml=false;
  const parser=new SaxesParser();
  parser.on("doctype",()=>{throw new AppError("invalidOpml",422,"This OPML file uses a document type, which is not supported.");});
  parser.on("error",()=>{throw new AppError("invalidOpml",422,"This file is not valid OPML.");});
  parser.on("opentag",tag=>{
    if(tag.name==="opml")opml=true;
    if(tag.name!=="outline")return;if(++outlines>5000)throw new AppError("invalidOpml",422,"This OPML file has too many entries.");
    const attr=(name:string)=>{const value=tag.attributes[name];return typeof value==="string"?value:"";};
    const xmlUrl=attr("xmlUrl")||attr("xmlurl"),label=(attr("title")||attr("text")).trim().slice(0,200);
    if(xmlUrl){const url=absolute(xmlUrl,"");if(url&&!feeds.some(f=>f.url===url))feeds.push({url,title:label.slice(0,newsLimits.title),folder:stack.find(s=>s.folder)?.folder??""});}
    stack.push({folder:xmlUrl?"":label});
  });
  parser.on("closetag",tag=>{if(tag.name==="outline")stack.pop();});
  parser.write(text).close();
  if(!opml)throw new AppError("invalidOpml",422,"This file is not valid OPML.");
  if(feeds.length>newsLimits.feeds)throw new AppError("invalidOpml",422,`OPML files can hold at most ${newsLimits.feeds} feeds.`);
  return feeds;
}
export function exportOpml(data:{folders:{id:number;name:string}[];feeds:{url:string;title:string;link:string;folderId:number}[]},owner:string){
  const outline=(f:{url:string;title:string;link:string})=>`<outline type="rss" text="${escapeHtml(f.title)}" title="${escapeHtml(f.title)}" xmlUrl="${escapeHtml(f.url)}"${f.link?` htmlUrl="${escapeHtml(f.link)}"`:""}/>`;
  const groups=data.folders.map(folder=>`<outline text="${escapeHtml(folder.name)}" title="${escapeHtml(folder.name)}">${data.feeds.filter(f=>f.folderId===folder.id).map(outline).join("")}</outline>`);
  return `<?xml version="1.0" encoding="UTF-8"?>\n<opml version="2.0"><head><title>${escapeHtml(owner)} subscriptions in Dar</title><dateCreated>${new Date().toUTCString()}</dateCreated></head><body>${groups.join("")}${data.feeds.filter(f=>!f.folderId).map(outline).join("")}</body></opml>\n`;
}
