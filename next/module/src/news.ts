import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {appEnabled} from "./app-settings.ts";
import {newsDefaults,newsKey,newsLimits,newsRefreshChoices,newsKeepChoices,type NewsEntry} from "../../shared/news.ts";
type Item=NonNullable<ReturnType<Ctx["db"]["newsItems"]["id"]["find"]>>;
type Feed=NonNullable<ReturnType<Ctx["db"]["newsFeeds"]["id"]["find"]>>;
type Folder=NonNullable<ReturnType<Ctx["db"]["newsFolders"]["id"]["find"]>>;

function fail(code:string):never{throw new SenderError(code);}
function text(value:unknown,max:number,allowEmpty=false){
  if(typeof value!=="string"||!allowEmpty&&!value.trim()||value.includes("\0")||value.length>max)fail("invalidInput");
  if([...value].some(c=>c.codePointAt(0)!>=0xd800&&c.codePointAt(0)!<=0xdfff))fail("invalidInput");
  return value;
}
function int(value:unknown,min=0,max=0xffffffff){if(typeof value!=="number"||!Number.isInteger(value)||value<min||value>max)fail("invalidInput");return value;}
function keys(p:Record<string,unknown>,allowed:string[]){if(Object.keys(p).some(k=>!allowed.includes(k)))fail("invalidInput");}
const url=(value:unknown)=>{const v=text(value,newsLimits.url);if(!/^https?:\/\/[^\s]+$/i.test(v))fail("invalidInput");return v;};
const optionalUrl=(value:unknown)=>value===""?"":url(value);
const prefix=(value:string)=>new Range<string>({tag:"included",value:value+"/"},{tag:"excluded",value:value+"0"});
const feedPrefix=(id:number)=>String(id).padStart(10,"0");
const modifiedKey=(ownerId:string,micros:number)=>`${ownerId}/${String(micros).padStart(17,"0")}`;
const micros=(ctx:Ctx)=>Number(ctx.timestamp.microsSinceUnixEpoch);
const dueKey=(at:number)=>String(Math.max(0,Math.min(Math.floor(at),9999999999999999))).padStart(16,"0");
const paused=1e15;

/** Monotonic per-name integer IDs; restores raise them past restored rows. */
function nextId(ctx:Ctx,name:string){
  const row=ctx.db.newsSequences.name.find(name),value=(row?.value??0)+1;if(value>=0xfffffffe)fail("quotaExceeded");
  if(row)ctx.db.newsSequences.name.update({name,value});else ctx.db.newsSequences.insert({name,value});return value;
}
export function newsPreferences(ctx:Ctx,userId:string){
  const value=JSON.parse(ctx.db.pimPreferences.userId.find(userId)?.value??"{}") as Record<string,unknown>;
  const refresh=newsRefreshChoices.includes(value.newsRefresh as 30)?Number(value.newsRefresh):newsDefaults.newsRefresh;
  const keep=newsKeepChoices.includes(value.newsKeepDays as 30)?Number(value.newsKeepDays):newsDefaults.newsKeepDays;
  return {refreshMs:refresh*60000,keepMs:keep*86400000};
}
function adjustCount(ctx:Ctx,ownerId:string,delta:number){
  const row=ctx.db.newsCounts.ownerId.find(ownerId),items=Math.max(0,(row?.items??0)+delta);
  if(row)ctx.db.newsCounts.ownerId.update({ownerId,items});else ctx.db.newsCounts.insert({ownerId,items});
}
function exposeItem(row:Item){
  return {id:row.id,feedId:row.feedId,guidHash:row.guidHash,url:row.url,title:row.title,author:row.author,pubDate:row.pubDate,excerpt:row.excerpt,enclosureMime:row.enclosureMime,
    enclosureLink:row.enclosureLink,mediaThumbnail:row.mediaThumbnail,unread:row.unread,starred:row.starred,lastModified:row.lastModified,position:row.position};
}
function detail(ctx:Ctx,row:Item){
  return {...exposeItem(row),guid:row.guid,body:ctx.db.newsBodies.itemId.find(row.id)?.body??"",updatedDate:row.updatedDate,mediaDescription:row.mediaDescription,rtl:row.rtl,fingerprint:row.fingerprint,contentHash:row.contentHash};
}
function setState(ctx:Ctx,row:Item,change:{unread?:boolean;starred?:boolean;position?:number}){
  const unread=change.unread??row.unread,starred=change.starred??row.starred,position=change.position??row.position;
  if(unread===row.unread&&starred===row.starred&&position===row.position)return row;
  const lastModified=micros(ctx),next={...row,unread,starred,position,unreadKey:unread?row.ownerId:"",starredKey:starred?row.ownerId:"",lastModified,modifiedKey:modifiedKey(row.ownerId,lastModified)};
  ctx.db.newsItems.id.update(next);return next;
}
function deleteItem(ctx:Ctx,row:Item){ctx.db.newsBodies.itemId.delete(row.id);ctx.db.newsItems.id.delete(row.id);adjustCount(ctx,row.ownerId,-1);}
function deleteFeedRows(ctx:Ctx,feed:Feed){for(const row of [...ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(feed.id)))])deleteItem(ctx,row);ctx.db.newsFeeds.id.delete(feed.id);}

function validEntry(value:unknown):NewsEntry{
  if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");const e=value as Record<string,unknown>;
  keys(e,["guid","guidHash","url","title","author","pubDate","updatedDate","body","enclosureMime","enclosureLink","mediaThumbnail","mediaDescription","rtl","fingerprint","contentHash","excerpt"]);
  if(typeof e.rtl!=="boolean"||!/^[a-f0-9]{32}$/.test(String(e.guidHash))||!/^[a-f0-9]{32}$/.test(String(e.fingerprint))||!/^[a-f0-9]{32}$/.test(String(e.contentHash)))fail("invalidInput");
  for(const key of ["pubDate","updatedDate"])if(typeof e[key]!=="number"||!Number.isFinite(e[key])||Number(e[key])<0)fail("invalidInput");
  return {guid:text(e.guid,newsLimits.url),guidHash:String(e.guidHash),url:e.url===""?"":url(e.url),title:text(e.title,newsLimits.title,true),author:text(e.author,500,true),pubDate:Number(e.pubDate),updatedDate:Number(e.updatedDate),
    body:text(e.body,newsLimits.body,true),enclosureMime:text(e.enclosureMime,200,true),enclosureLink:optionalUrl(e.enclosureLink),mediaThumbnail:optionalUrl(e.mediaThumbnail),mediaDescription:text(e.mediaDescription,4000,true),
    rtl:e.rtl,fingerprint:String(e.fingerprint),contentHash:String(e.contentHash),excerpt:text(e.excerpt??"",600,true)};
}

/**
 * Inserts new entries (oldest first, so IDs follow time) and refreshes changed ones without
 * touching read or starred state. Retention never deletes starred items or entries still
 * present in the feed, which would otherwise return as unread on the next update.
 */
function storeEntries(ctx:Ctx,feed:Feed,input:unknown,all?:unknown){
  if(!Array.isArray(input)||input.length>newsLimits.itemsPerFeed)fail("invalidInput");
  if(all!==undefined&&(!Array.isArray(all)||all.length>newsLimits.itemsPerFeed||all.some(hash=>typeof hash!=="string"||!/^[a-f0-9]{32}$/.test(hash))))fail("invalidInput");
  // The fetcher may store a large feed in chunks; "all" lists every entry still in the feed.
  const entries=input.map(validEntry).sort((a,b)=>a.pubDate-b.pubDate),present=new Set([...entries.map(e=>e.guidHash),...(all as string[]|undefined)??[]]),now=Number(ctx.timestamp.toMillis()),stamp=micros(ctx);
  let added=0;const count=ctx.db.newsCounts.ownerId.find(feed.ownerId)?.items??0;
  for(const entry of entries){
    const dedupeKey=`${feed.id}:${entry.guidHash}`,old=ctx.db.newsItems.dedupeKey.find(dedupeKey),{body,...meta}=entry;
    if(old){
      if(old.contentHash===entry.contentHash)continue;
      ctx.db.newsItems.id.update({...old,...meta,lastModified:stamp,modifiedKey:modifiedKey(feed.ownerId,stamp)});
      const stored=ctx.db.newsBodies.itemId.find(old.id);if(stored)ctx.db.newsBodies.itemId.update({itemId:old.id,body});else ctx.db.newsBodies.insert({itemId:old.id,body});continue;
    }
    if(count+added>=newsLimits.itemsPerAccount)break;
    const id=nextId(ctx,"item");added++;
    ctx.db.newsItems.insert({...meta,id,ownerId:feed.ownerId,feedId:feed.id,dedupeKey,ownerKey:`${feed.ownerId}/${newsKey(id)}`,feedKey:`${feedPrefix(feed.id)}/${newsKey(id)}`,
      unreadKey:feed.ownerId,starredKey:"",modifiedKey:modifiedKey(feed.ownerId,stamp),unread:true,starred:false,lastModified:stamp,position:0,fetchedAt:now});
    ctx.db.newsBodies.insert({itemId:id,body});
  }
  if(added)adjustCount(ctx,feed.ownerId,added);
  const {keepMs}=newsPreferences(ctx,feed.ownerId);let kept=0;
  for(const row of [...ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(feed.id)))]){
    if(row.starred||present.has(row.guidHash)){kept++;continue;}
    if(++kept>newsLimits.itemsPerFeed||!row.unread&&row.fetchedAt<now-keepMs)deleteItem(ctx,row);
  }
  return added;
}
function exposeFeed(row:Feed,unread:Map<number,number>){
  return {id:row.id,url:row.url,title:row.title,link:row.link,folderId:row.folderId,added:row.added,ordering:row.ordering,pinned:row.pinned,fullText:row.fullText,paused:row.paused,
    unreadCount:unread.get(row.id)??0,lastUpdated:row.lastUpdated,nextUpdate:row.nextUpdate,updateErrorCount:row.errorCount,lastUpdateError:row.lastError};
}
function unreadCounts(ctx:Ctx,userId:string){const counts=new Map<number,number>();for(const row of ctx.db.newsItems.unreadKey.filter(userId))counts.set(row.feedId,(counts.get(row.feedId)??0)+1);return counts;}

export const newsReadOperations=new Set(["access","overview","folders","feeds","items","itemsUpdated","item","exportData","feedInfo"]);
/** Account-scoped News operations shared by the browser API and the Nextcloud News API. */
export function newsOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>){
  if(!appEnabled(ctx,"news"))fail("notFound");
  const now=Number(ctx.timestamp.toMillis());
  const folder=(value:unknown):Folder=>{const row=ctx.db.newsFolders.id.find(int(value,1));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const feed=(value:unknown):Feed=>{const row=ctx.db.newsFeeds.id.find(int(value,1));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const folderId=(value:unknown)=>value===0||value===null||value===undefined?0:folder(value).id;
  const name=(value:unknown)=>text(value,200).trim();
  const feeds=()=>[...ctx.db.newsFeeds.ownerId.filter(userId)];
  const inScope=(scope:Record<string,unknown>)=>{
    const type=scope.type;
    if(type==="feed"){const id=feed(scope.id).id;return (row:Item)=>row.feedId===id;}
    if(type==="folder"){const ids=new Set(feeds().filter(row=>row.folderId===(scope.id===0?0:folder(scope.id).id)).map(row=>row.id));return (row:Item)=>ids.has(row.feedId);}
    if(type==="starred")return (row:Item)=>row.starred;
    if(type==="all"||type==="unread")return (row:Item)=>type==="all"||row.unread;
    fail("invalidInput");
  };
  if(operation==="access"){keys(p,[]);return {ok:true};}
  if(operation==="overview"||operation==="folders"||operation==="feeds"){
    keys(p,[]);const counts=unreadCounts(ctx,userId),list=feeds().sort((a,b)=>Number(b.pinned)-Number(a.pinned)||a.title.localeCompare(b.title)||a.id-b.id);
    let newest=0;for(const row of ctx.db.newsItems.ownerKey.filter(prefix(userId))){newest=row.id;break;}
    return {folders:[...ctx.db.newsFolders.ownerId.filter(userId)].sort((a,b)=>a.name.localeCompare(b.name)||a.id-b.id).map(({id,name,opened})=>({id,name,opened})),
      feeds:list.map(row=>exposeFeed(row,counts)),starredCount:[...ctx.db.newsItems.starredKey.filter(userId)].length,unreadCount:[...counts.values()].reduce((a,b)=>a+b,0),newestItemId:newest};
  }
  if(operation==="items"){
    keys(p,["type","id","offset","batch","getRead","oldestFirst","bodies"]);
    const match=inScope(p),getRead=p.getRead!==false,oldest=p.oldestFirst===true,offset=p.offset===undefined?0:int(p.offset),batch=p.batch===-1?2000:int(p.batch??newsLimits.page,1,2000);
    const source:Iterable<Item>=p.type==="unread"||!getRead&&p.type!=="starred"?[...ctx.db.newsItems.unreadKey.filter(userId)].sort((a,b)=>b.id-a.id):p.type==="starred"?[...ctx.db.newsItems.starredKey.filter(userId)].sort((a,b)=>b.id-a.id)
      :p.type==="feed"?ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(feed(p.id).id))):ctx.db.newsItems.ownerKey.filter(prefix(userId));
    let rows:Item[]=[];
    for(const row of source){if(!match(row)||!getRead&&!row.unread)continue;if(!oldest&&offset&&row.id>=offset)continue;rows.push(row);if(!oldest&&rows.length>batch)break;}
    if(oldest)rows=rows.reverse().filter(row=>!offset||row.id>offset);
    const page=rows.slice(0,batch);
    return {items:page.map(row=>p.bodies===true?detail(ctx,row):exposeItem(row)),nextOffset:rows.length>batch?page.at(-1)!.id:null};
  }
  if(operation==="itemsUpdated"){
    keys(p,["type","id","lastModified"]);const since=typeof p.lastModified==="number"&&Number.isFinite(p.lastModified)&&p.lastModified>=0?p.lastModified:fail("invalidInput"),match=inScope(p);
    const range=new Range<string>({tag:"included",value:modifiedKey(userId,since)},{tag:"excluded",value:userId+"0"}),items=[];
    for(const row of ctx.db.newsItems.modifiedKey.filter(range)){if(match(row))items.push(detail(ctx,row));if(items.length>=5000)break;}
    return {items};
  }
  if(operation==="item"){keys(p,["id"]);const row=ctx.db.newsItems.id.find(int(p.id,1));if(!row||row.ownerId!==userId)fail("notFound");return detail(ctx,row);}
  if(operation==="feedInfo"){
    keys(p,["id"]);const row=feed(p.id),recent=[...ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(row.id)))].filter(item=>item.fetchedAt>=now-14*86400000);
    const perDay=Array.from({length:14},(_,day)=>recent.filter(item=>Math.floor((now-item.fetchedAt)/86400000)===13-day).length);
    return {...exposeFeed(row,unreadCounts(ctx,userId)),perDay,starred:[...ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(row.id)))].filter(item=>item.starred).length};
  }
  if(operation==="exportData"){
    keys(p,["starred"]);
    if(p.starred===true)return {items:[...ctx.db.newsItems.starredKey.filter(userId)].sort((a,b)=>b.id-a.id).slice(0,5000).map(row=>({...detail(ctx,row),feed:ctx.db.newsFeeds.id.find(row.feedId)?.title??""}))};
    return {folders:[...ctx.db.newsFolders.ownerId.filter(userId)].map(({id,name})=>({id,name})),feeds:feeds().map(({id,url,title,link,folderId})=>({id,url,title,link,folderId}))};
  }
  if(operation==="createFolder"){
    keys(p,["name"]);const value=name(p.name),existing=[...ctx.db.newsFolders.ownerId.filter(userId)];
    if(existing.some(row=>row.name.toLocaleLowerCase()===value.toLocaleLowerCase()))fail("alreadyExists");if(existing.length>=newsLimits.folders)fail("quotaExceeded");
    const row={id:nextId(ctx,"folder"),ownerId:userId,name:value,opened:true,createdAt:now};ctx.db.newsFolders.insert(row);return {id:row.id,name:row.name,opened:true};
  }
  if(operation==="renameFolder"){
    keys(p,["id","name"]);const row=folder(p.id),value=name(p.name);
    if([...ctx.db.newsFolders.ownerId.filter(userId)].some(other=>other.id!==row.id&&other.name.toLocaleLowerCase()===value.toLocaleLowerCase()))fail("alreadyExists");
    ctx.db.newsFolders.id.update({...row,name:value});return {id:row.id,name:value,opened:row.opened};
  }
  if(operation==="openFolder"){keys(p,["id","opened"]);const row=folder(p.id);if(typeof p.opened!=="boolean")fail("invalidInput");ctx.db.newsFolders.id.update({...row,opened:p.opened});return {id:row.id,name:row.name,opened:p.opened};}
  if(operation==="deleteFolder"){
    keys(p,["id"]);const row=folder(p.id);for(const child of feeds().filter(item=>item.folderId===row.id))deleteFeedRows(ctx,child);
    ctx.db.newsFolders.id.delete(row.id);return {id:row.id,deleted:true};
  }
  if(operation==="subscribe"){
    keys(p,["url","folderId","title","link","items","etag","lastModifiedHeader","fullText"]);const feedUrl=url(p.url),parent=folderId(p.folderId),urlKey=`${userId}\n${feedUrl}`;
    if(ctx.db.newsFeeds.urlKey.find(urlKey))fail("alreadyExists");if(feeds().length>=newsLimits.feeds)fail("quotaExceeded");
    const {refreshMs}=newsPreferences(ctx,userId);
    const row={id:nextId(ctx,"feed"),ownerId:userId,folderId:parent,url:feedUrl,urlKey,title:text(p.title,newsLimits.title).trim(),link:p.link?optionalUrl(p.link):"",added:now,ordering:0,pinned:false,fullText:p.fullText===true,paused:false,
      etag:text(p.etag??"",500,true),lastModifiedHeader:text(p.lastModifiedHeader??"",200,true),lastUpdated:now,nextUpdate:now+refreshMs,dueKey:dueKey(now+refreshMs),errorCount:0,lastError:""};
    ctx.db.newsFeeds.insert(row);storeEntries(ctx,row,p.items??[]);return exposeFeed(row,unreadCounts(ctx,userId));
  }
  if(operation==="importOpml"){
    // Feeds are created pending; the fetcher fills titles and items on its next pass.
    keys(p,["feeds"]);if(!Array.isArray(p.feeds)||p.feeds.length>newsLimits.feeds)fail("invalidInput");let added=0,skipped=0,folders=0;
    for(const value of p.feeds){
      const input=(value??{}) as Record<string,unknown>;keys(input,["url","title","folder"]);const feedUrl=url(input.url),urlKey=`${userId}\n${feedUrl}`;
      if(ctx.db.newsFeeds.urlKey.find(urlKey)){skipped++;continue;}if(feeds().length>=newsLimits.feeds)fail("quotaExceeded");
      let parent=0;if(input.folder){const label=name(input.folder),found=[...ctx.db.newsFolders.ownerId.filter(userId)].find(row=>row.name.toLocaleLowerCase()===label.toLocaleLowerCase());
        if(found)parent=found.id;else{if([...ctx.db.newsFolders.ownerId.filter(userId)].length>=newsLimits.folders)fail("quotaExceeded");parent=nextId(ctx,"folder");folders++;ctx.db.newsFolders.insert({id:parent,ownerId:userId,name:label,opened:true,createdAt:now});}}
      ctx.db.newsFeeds.insert({id:nextId(ctx,"feed"),ownerId:userId,folderId:parent,url:feedUrl,urlKey,title:text(input.title||feedUrl,newsLimits.title).trim(),link:"",added:now,ordering:0,pinned:false,fullText:false,paused:false,etag:"",lastModifiedHeader:"",lastUpdated:0,nextUpdate:0,dueKey:dueKey(0),errorCount:0,lastError:""});added++;
    }
    return {added,skipped,folders};
  }
  if(operation==="updateFeed"){
    keys(p,["id","title","folderId","fullText","pinned","paused","ordering","updateNow"]);const row=feed(p.id),next={...row};
    if(p.title!==undefined)next.title=text(p.title,newsLimits.title).trim();
    if(p.folderId!==undefined)next.folderId=folderId(p.folderId);
    for(const key of ["fullText","pinned","paused"] as const)if(p[key]!==undefined){if(typeof p[key]!=="boolean")fail("invalidInput");next[key]=p[key] as boolean;}
    if(p.ordering!==undefined)next.ordering=int(p.ordering,0,2);
    if(p.paused===false&&row.paused||p.updateNow===true)next.nextUpdate=0;
    if(p.paused===true)next.nextUpdate=paused;
    next.dueKey=dueKey(next.nextUpdate);ctx.db.newsFeeds.id.update(next);return exposeFeed(next,unreadCounts(ctx,userId));
  }
  if(operation==="deleteFeed"){keys(p,["id"]);const row=feed(p.id);deleteFeedRows(ctx,row);return {id:row.id,deleted:true};}
  if(operation==="markRead"){
    // Marks only items no newer than the newest one the reader had loaded.
    keys(p,["type","id","newestItemId"]);const match=inScope(p),newest=int(p.newestItemId),ids:number[]=[];
    for(const row of [...ctx.db.newsItems.unreadKey.filter(userId)])if(row.id<=newest&&match(row)){setState(ctx,row,{unread:false});ids.push(row.id);}
    return {changed:ids.length,ids:ids.slice(0,5000)};
  }
  if(operation==="setItems"){
    keys(p,["ids","unread","starred"]);if(!Array.isArray(p.ids)||p.ids.length>1000)fail("invalidInput");
    if(p.unread!==undefined&&typeof p.unread!=="boolean"||p.starred!==undefined&&typeof p.starred!=="boolean")fail("invalidInput");const items=[];
    for(const value of p.ids){const row=ctx.db.newsItems.id.find(int(value,1));if(!row||row.ownerId!==userId)continue;items.push(exposeItem(setState(ctx,row,{unread:p.unread as boolean|undefined,starred:p.starred as boolean|undefined})));}
    return {items};
  }
  if(operation==="starByGuid"){
    keys(p,["items","starred"]);if(!Array.isArray(p.items)||p.items.length>1000||typeof p.starred!=="boolean")fail("invalidInput");let changed=0;
    for(const value of p.items){const input=(value??{}) as Record<string,unknown>;const parent=ctx.db.newsFeeds.id.find(int(input.feedId,1));if(!parent||parent.ownerId!==userId)continue;
      const row=ctx.db.newsItems.dedupeKey.find(`${parent.id}:${String(input.guidHash)}`);if(row){setState(ctx,row,{starred:p.starred});changed++;}}
    return {changed};
  }
  if(operation==="setPosition"){
    keys(p,["id","position"]);const row=ctx.db.newsItems.id.find(int(p.id,1));if(!row||row.ownerId!==userId)fail("notFound");
    const position=typeof p.position==="number"&&Number.isFinite(p.position)&&p.position>=0&&p.position<1e7?Math.round(p.position):fail("invalidInput");
    return exposeItem(setState(ctx,row,{position}));
  }
  fail("invalidInput");
}

/** Service-only fetcher steps: claim due feeds, then store results or failures. */
export function newsWorker(ctx:Ctx,operation:string,p:Record<string,unknown>){
  const now=Number(ctx.timestamp.toMillis());
  if(operation==="due"){
    keys(p,["limit"]);if(!appEnabled(ctx,"news"))return {feeds:[]};const limit=int(p.limit,1,50),due=[];
    for(const row of [...ctx.db.newsFeeds.dueKey.filter(new Range<string>({tag:"included",value:dueKey(0)},{tag:"included",value:dueKey(now)}))]){
      if(row.paused)continue;const owner=ctx.db.accounts.id.find(row.ownerId);if(!owner?.enabled)continue;
      // A lease: if the runtime stops mid-fetch the feed becomes due again in ten minutes.
      ctx.db.newsFeeds.id.update({...row,nextUpdate:now+600000,dueKey:dueKey(now+600000)});
      due.push({id:row.id,url:row.url,etag:row.etag,lastModifiedHeader:row.lastModifiedHeader,fullText:row.fullText,known:[...ctx.db.newsItems.feedKey.filter(prefix(feedPrefix(row.id)))].map(item=>item.guidHash)});
      if(due.length>=limit)break;
    }
    return {feeds:due};
  }
  const row=ctx.db.newsFeeds.id.find(int(p.feedId,1));if(!row)return {stored:false};
  const {refreshMs}=newsPreferences(ctx,row.ownerId);
  if(operation==="store"){
    keys(p,["feedId","etag","lastModifiedHeader","title","link","items","notModified","present"]);
    const at=row.paused?paused:now+refreshMs,next:Feed={...row,etag:text(p.etag??"",500,true),lastModifiedHeader:text(p.lastModifiedHeader??"",200,true),errorCount:0,lastError:"",lastUpdated:now,nextUpdate:at,dueKey:dueKey(at)};
    // Imported feeds take their title from the first successful fetch.
    if(typeof p.title==="string"&&p.title.trim()&&(row.lastUpdated===0||row.title===row.url))next.title=text(p.title,newsLimits.title).trim();
    if(typeof p.link==="string"&&p.link)next.link=optionalUrl(p.link);
    ctx.db.newsFeeds.id.update(next);
    return {stored:true,added:p.notModified===true?0:storeEntries(ctx,next,p.items??[],p.present)};
  }
  if(operation==="fail"){
    keys(p,["feedId","error"]);const errors=row.errorCount+1,delay=Math.min(refreshMs*2**Math.min(errors-1,6),86400000);
    const at=row.paused?paused:now+delay;ctx.db.newsFeeds.id.update({...row,errorCount:errors,lastError:text(p.error,500),nextUpdate:at,dueKey:dueKey(at)});return {stored:true};
  }
  fail("invalidInput");
}

export function deleteNewsAccount(ctx:Ctx,ownerId:string){
  for(const row of [...ctx.db.newsFeeds.ownerId.filter(ownerId)])deleteFeedRows(ctx,row);
  for(const row of [...ctx.db.newsFolders.ownerId.filter(ownerId)])ctx.db.newsFolders.id.delete(row.id);
  ctx.db.newsCounts.ownerId.delete(ownerId);
}
export function newsSearch(ctx:Ctx,userId:string,matches:(value:string)=>boolean,visit:()=>boolean,add:(result:{id:string;title:string;detail:string;href:string})=>void){
  const titles=new Map([...ctx.db.newsFeeds.ownerId.filter(userId)].map(row=>[row.id,row.title]));
  for(const [id,title] of titles){if(!visit())return;if(matches(title))add({id:`feed-${id}`,title,detail:"News feed",href:`/news?feed=${id}`});}
  for(const row of ctx.db.newsItems.ownerKey.filter(prefix(userId))){if(!visit())return;if(matches(row.title))add({id:`item-${row.id}`,title:row.title||"Untitled",detail:titles.get(row.feedId)??"News",href:`/news?item=${row.id}`});}
}

export function newsSnapshot(ctx:Ctx){
  return {folders:[...ctx.db.newsFolders.iter()],feeds:[...ctx.db.newsFeeds.iter()].map(({urlKey,dueKey,...row})=>row),
    items:[...ctx.db.newsItems.iter()].map(({ownerKey,feedKey,unreadKey,starredKey,modifiedKey,dedupeKey,...row})=>({...row,body:ctx.db.newsBodies.itemId.find(row.id)?.body??""})),sequences:[...ctx.db.newsSequences.iter()]};
}
/** V7 backups carry News; earlier formats have none. Keys and counters are rebuilt. */
export function restoreNews(ctx:Ctx,value?:ReturnType<typeof newsSnapshot>){
  if(!value)return;
  if(!Array.isArray(value.folders)||!Array.isArray(value.feeds)||!Array.isArray(value.items)||!Array.isArray(value.sequences))fail("invalidInput");
  for(const row of value.folders){if(!ctx.db.accounts.id.find(row.ownerId)||int(row.id,1)!==row.id||text(row.name,200).trim()!==row.name)fail("invalidInput");ctx.db.newsFolders.insert(row);}
  for(const row of value.feeds){
    if(!ctx.db.accounts.id.find(row.ownerId)||int(row.id,1)!==row.id||row.folderId&&ctx.db.newsFolders.id.find(row.folderId)?.ownerId!==row.ownerId)fail("invalidInput");
    url(row.url);ctx.db.newsFeeds.insert({...row,urlKey:`${row.ownerId}\n${row.url}`,dueKey:dueKey(row.nextUpdate)});
  }
  for(const input of value.items){
    const {body,...row}=input,parent=ctx.db.newsFeeds.id.find(row.feedId);if(!parent||parent.ownerId!==row.ownerId||int(row.id,1)!==row.id)fail("invalidInput");text(body,newsLimits.body,true);
    ctx.db.newsItems.insert({...row,dedupeKey:`${row.feedId}:${row.guidHash}`,ownerKey:`${row.ownerId}/${newsKey(row.id)}`,feedKey:`${feedPrefix(row.feedId)}/${newsKey(row.id)}`,unreadKey:row.unread?row.ownerId:"",starredKey:row.starred?row.ownerId:"",modifiedKey:modifiedKey(row.ownerId,row.lastModified)});
    ctx.db.newsBodies.insert({itemId:row.id,body});adjustCount(ctx,row.ownerId,1);
  }
  const top=(rows:{id:number}[])=>rows.reduce((max,row)=>Math.max(max,row.id),0),highest={folder:top(value.folders),feed:top(value.feeds),item:top(value.items as {id:number}[])};
  for(const name of ["folder","feed","item"] as const){const saved=value.sequences.find(r=>r.name===name)?.value??0;ctx.db.newsSequences.insert({name,value:Math.max(saved,highest[name])});}
}
