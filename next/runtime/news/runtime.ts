import type {ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {safeFetch,type FetchOptions} from "./net.ts";
import {parseFeed,decodeBody,type ParsedFeed} from "./feed.ts";
import {discoverFeeds,extractArticle,textOf} from "./html.ts";
import {parseOpml,exportOpml} from "./opml.ts";
import type {NewsDiscovered,NewsEntry,NewsFeed} from "../../shared/news.ts";

const feedAccept="application/rss+xml, application/atom+xml, application/feed+json, application/json;q=0.9, application/xml;q=0.9, text/xml;q=0.9, text/html;q=0.5, */*;q=0.1";
interface Due {id:number;url:string;etag:string;lastModifiedHeader:string;fullText:boolean;known:string[]}

/** Splits entries so each database call stays well under the request limits. */
function chunks(entries:NewsEntry[],max=900*1024){
  const out:NewsEntry[][]=[[]];let size=0;
  for(const entry of entries){const bytes=JSON.stringify(entry).length;if(size+bytes>max&&out.at(-1)!.length){out.push([]);size=0;}out.at(-1)!.push(entry);size+=bytes;}
  return out;
}
async function pool<T>(items:T[],limit:number,run:(item:T)=>Promise<void>){
  let next=0;await Promise.all(Array.from({length:Math.min(limit,items.length)},async()=>{while(next<items.length)await run(items[next++]);}));
}

/**
 * Owns all News network effects: the background refresh loop, discovery, subscribe,
 * full-text extraction and the image proxy. Closing stops the loop and aborts fetches.
 */
export function createNewsRuntime(db:Database,{allowPrivate=false}:{allowPrivate?:boolean}={}){
  const controller=new AbortController();let closing=false,running:Promise<void>|undefined;
  const base:FetchOptions={allowPrivate,signal:controller.signal};
  const call=<T>(sessionId:string,operation:string,value:object)=>db.call<T>("service_news",[sessionId,operation,JSON.stringify(value)]);
  const worker=<T>(operation:string,value:object)=>db.call<T>("service_news_worker",[operation,JSON.stringify(value)]);

  async function load(url:string,extra:FetchOptions={}){
    const response=await safeFetch(url,{...base,accept:feedAccept,...extra});
    if(response.status===304)return {response,feed:undefined};
    if(response.status===410)throw new AppError("feedGone",502,"The feed was removed by its site (410 Gone).");
    if(response.status<200||response.status>=300)throw new AppError("feedUnavailable",502,`The site answered ${response.status}.`);
    return {response,feed:parseFeed(response.body,response.contentType,response.url)};
  }
  /** Replaces summaries with the article page's main text for entries not stored yet. */
  async function fullText(entries:NewsEntry[],known:Set<string>){
    const fresh=entries.filter(e=>e.url&&!known.has(e.guidHash)).slice(0,15);
    await pool(fresh,3,async e=>{
      try{
        const page=await safeFetch(e.url,{...base,accept:"text/html,application/xhtml+xml",maxBytes:3*1024*1024,timeout:15000});
        if(page.status!==200||!/html/.test(page.contentType))return;
        const body=extractArticle(decodeBody(page.body,page.contentType),page.url);
        if(body){e.body=body;e.excerpt=textOf(body,2000).slice(0,300);}
      }catch{}
    });
  }
  async function refresh(feed:Due){
    try{
      const {response,feed:parsed}=await load(feed.url,{etag:feed.etag,lastModified:feed.lastModifiedHeader});
      const meta={feedId:feed.id,etag:response.etag,lastModifiedHeader:response.lastModified};
      if(!parsed){await worker("store",{...meta,notModified:true});return;}
      if(feed.fullText)await fullText(parsed.entries,new Set(feed.known));
      const present=parsed.entries.map(e=>e.guidHash),parts=chunks(parsed.entries);
      for(const part of parts)await worker("store",{...meta,title:parsed.title,link:parsed.link,items:part,...parts.length>1?{present}:{}});
    }catch(error){
      if(closing)return;
      await worker("fail",{feedId:feed.id,error:error instanceof AppError?error.message:"The feed could not be updated."}).catch(()=>{});
    }
  }
  function tick(){
    if(running||closing)return running??Promise.resolve();
    running=(async()=>{
      for(let round=0;round<20&&!closing;round++){
        const {feeds}=await worker<{feeds:Due[]}>("due",{limit:20});if(!feeds.length)break;
        await pool(feeds,4,refresh);
      }
    })().catch(()=>{}).finally(()=>{running=undefined;});
    return running;
  }
  const timer=setInterval(()=>{void tick();},30000);const first=setTimeout(()=>{void tick();},3000);

  return {
    tick,
    /** A feed address is used directly; a web page is searched for advertised feeds. */
    async discover(input:string):Promise<NewsDiscovered[]>{
      const url=/^https?:\/\//i.test(input.trim())?input.trim():`https://${input.trim()}`;
      const summary=(parsed:ParsedFeed,feedUrl:string):NewsDiscovered=>({url:feedUrl,title:parsed.title,kind:parsed.kind,items:parsed.entries.slice().sort((a,b)=>b.pubDate-a.pubDate).slice(0,3).map(e=>({title:e.title,pubDate:e.pubDate}))});
      const page=await safeFetch(url,{...base,accept:feedAccept});
      if(page.status<200||page.status>=300)throw new AppError("feedUnavailable",502,`The site answered ${page.status}.`);
      try{return [summary(parseFeed(page.body,page.contentType,page.url),page.url)];}catch(error){if(!(error instanceof AppError)||error.code!=="notAFeed")throw error;}
      let candidates=discoverFeeds(decodeBody(page.body,page.contentType),page.url).map(c=>c.url);
      if(!candidates.length)candidates=["/feed","/rss","/feed.xml","/atom.xml","/rss.xml","/index.xml"].map(path=>new URL(path,page.url).toString());
      const found:NewsDiscovered[]=[];
      await pool(candidates.slice(0,8),3,async candidate=>{try{const {feed}=await load(candidate);if(feed&&!found.some(f=>f.url===candidate))found.push(summary(feed,candidate));}catch{}});
      if(!found.length)throw new AppError("notAFeed",422,"No RSS, Atom or JSON feed was found at that address.");
      return found.sort((a,b)=>candidates.indexOf(a.url)-candidates.indexOf(b.url));
    },
    async subscribe(sessionId:string,input:{url:string;folderId:number;title?:string;fullText?:boolean}):Promise<NewsFeed>{
      const {response,feed}=await load(input.url);if(!feed)throw new AppError("feedUnavailable",502,"The feed could not be read.");
      if(input.fullText)await fullText(feed.entries,new Set());
      const parts=chunks(feed.entries),present=feed.entries.map(e=>e.guidHash);
      const created=await call<NewsFeed>(sessionId,"subscribe",{url:input.url,folderId:input.folderId,title:input.title?.trim()||feed.title,link:feed.link,items:parts[0],etag:response.etag,lastModifiedHeader:response.lastModified,fullText:!!input.fullText});
      for(const part of parts.slice(1))await worker("store",{feedId:created.id,etag:response.etag,lastModifiedHeader:response.lastModified,items:part,present});
      return parts.length>1?(await call<{feeds:NewsFeed[]}>(sessionId,"feeds",{})).feeds.find(f=>f.id===created.id)??created:created;
    },
    async importOpml(sessionId:string,text:string){
      const result=await call<{added:number;skipped:number;folders:number}>(sessionId,"importOpml",{feeds:parseOpml(text)});void tick();return result;
    },
    async exportOpml(sessionId:string,owner:string){return exportOpml(await call(sessionId,"exportData",{}),owner);},
    /** Images load through Dar so sites never see the reader; served inert and never as SVG. */
    async image(url:string,res:ServerResponse){
      const response=await safeFetch(url,{...base,accept:"image/avif,image/webp,image/png,image/jpeg,image/gif;q=0.9",maxBytes:8*1024*1024,timeout:15000});
      const type=response.contentType.split(";")[0].trim();
      if(response.status!==200||!/^image\/(png|jpeg|gif|webp|avif)$/.test(type))throw new AppError("notFound",404);
      res.writeHead(200,{"content-type":type,"content-length":String(response.body.length),"cache-control":"private, max-age=86400","x-content-type-options":"nosniff","content-security-policy":"default-src 'none'; sandbox","content-disposition":"inline"});
      res.end(response.body);
    },
    async close(){closing=true;clearInterval(timer);clearTimeout(first);controller.abort();await running;},
  };
}
export type NewsRuntime=ReturnType<typeof createNewsRuntime>;
