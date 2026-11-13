import {Fragment,useCallback,useEffect,useMemo,useRef,useState,type FormEvent,type ReactNode} from "react";
import {request,type BrowserSession} from "../api.ts";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {ActionMenu,NewItemButton,Select,type MenuAction} from "../controls.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {Icon} from "../icons.tsx";
import {usePreferences,usePreferenceSaver,SaveProblems} from "../app-settings.tsx";
import {SettingsSheet,SettingRow,SettingSwitch,Segmented} from "../settings-sheet.tsx";
import {renderArticle} from "./render.ts";
import {newsDefaults,newsKeepChoices,newsRefreshChoices,type NewsDiscovered,type NewsFeed,type NewsFolder,type NewsItem,type NewsItemDetail,type NewsOverview} from "../../shared/news.ts";
import "./news.css";

const base="/api/v1/news";
type View={type:"unread"|"starred"|"all"}|{type:"feed";id:number}|{type:"folder";id:number};
type Pane={kind:"subscribe"}|{kind:"feed";feed:NewsFeed}|{kind:"folder";folder?:NewsFolder}|{kind:"settings"};
interface Notice {message:string;undo?:()=>void}
const markClass=(id:number)=>`news-mark tone-${id%6}`;
const initial=(title:string)=>[...title.replace(/^[^\p{L}\p{N}]+/u,"")][0]?.toUpperCase()??"•";
const minutes=(value:number)=>value>=60?`${value/60} hour${value===60?"":"s"}`:`${value} minutes`;
function dayLabel(time:number){
  const day=new Date(time),today=new Date();today.setHours(0,0,0,0);const start=new Date(day);start.setHours(0,0,0,0);const diff=Math.round((today.getTime()-start.getTime())/86400000);
  return diff<=0?"Today":diff===1?"Yesterday":diff<7?day.toLocaleDateString([],{weekday:"long"}):day.toLocaleDateString([],{day:"numeric",month:"long",...day.getFullYear()!==today.getFullYear()?{year:"numeric"}:{}});
}
const timeLabel=(time:number)=>dayLabel(time)==="Today"?new Date(time).toLocaleTimeString([],{hour:"numeric",minute:"2-digit"}):new Date(time).toLocaleDateString([],{month:"short",day:"numeric"});
function readView():{view:View;item?:number}{
  const p=new URLSearchParams(location.search),num=(key:string)=>Number(p.get(key))||0;
  const view:View=num("feed")?{type:"feed",id:num("feed")}:num("folder")?{type:"folder",id:num("folder")}:p.get("view")==="starred"?{type:"starred"}:p.get("view")==="all"?{type:"all"}:{type:"unread"};
  return {view,item:num("item")||undefined};
}
function writeView(view:View,item?:number){
  const p=new URLSearchParams();if(view.type==="feed"||view.type==="folder")p.set(view.type,String(view.id));else if(view.type!=="unread")p.set("view",view.type);if(item)p.set("item",String(item));
  const next=`/news${p.size?"?"+p:""}`;if(next!==location.pathname+location.search)history.replaceState(null,"",next);
}

/** News (selected mock A): feeds and folders, a day-grouped list and the article in the right sheet. */
export function NewsWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readView,[]),stored=usePreferences(),prefs={...newsDefaults,...Object.fromEntries(Object.entries(stored.preferences).filter(([key])=>key.startsWith("news")))} as typeof newsDefaults;
  const [overview,setOverview]=useState<NewsOverview>(),[loadError,setLoadError]=useState(""),[view,setView]=useState<View>(start.view),[items,setItems]=useState<NewsItem[]>([]),[nextOffset,setNextOffset]=useState<number|null>(null);
  const [listState,setListState]=useState<"loading"|"ready"|"error">("loading"),[listError,setListError]=useState(""),[selected,setSelected]=useState<number|undefined>(start.item),[article,setArticle]=useState<NewsItemDetail>(),[articleError,setArticleError]=useState("");
  const [pane,setPane]=useState<Pane>(),[confirm,setConfirm]=useState<{title:string;text:string;action:string;run:()=>Promise<void>}>(),[notice,setNotice]=useState<Notice|null>(null),[busy,setBusy]=useState(false),[refreshing,setRefreshing]=useState(false);
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const list=useRef<HTMLDivElement>(null),generation=useRef(0),pendingRead=useRef(new Set<number>());
  const feeds=overview?.feeds??[],folders=overview?.folders??[],feedById=useMemo(()=>new Map(feeds.map(f=>[f.id,f])),[feeds]);
  const fail=(cause:unknown)=>setNotice({message:(cause as Error).message||"The request could not be completed."});

  const loadOverview=useCallback(async()=>{try{setOverview(await request<NewsOverview>(`${base}/overview`));setLoadError("");}catch(cause){setLoadError((cause as Error).message);}},[]);
  const query=(offset=0)=>{
    const p=new URLSearchParams({type:view.type,batch:"50",offset:String(offset),oldestFirst:prefs.newsOrder==="oldest"?"1":"0",getRead:view.type==="unread"||(view.type==="feed"||view.type==="folder")&&!prefs.newsShowRead?"0":"1"});
    if("id" in view)p.set("id",String(view.id));return `${base}/items?${p}`;
  };
  const loadItems=useCallback(async()=>{
    const ticket=++generation.current;setListState("loading");
    try{const page=await request<{items:NewsItem[];nextOffset:number|null}>(query());if(ticket!==generation.current)return;setItems(page.items);setNextOffset(page.nextOffset);setListState("ready");list.current?.scrollTo({top:0});}
    catch(cause){if(ticket===generation.current){setListState("error");setListError((cause as Error).message);}}
  },[view.type,"id" in view?view.id:0,prefs.newsOrder,prefs.newsShowRead]);
  async function loadMore(){if(nextOffset===null||listState!=="ready")return;const ticket=generation.current;const page=await request<{items:NewsItem[];nextOffset:number|null}>(query(nextOffset)).catch(()=>undefined);if(!page||ticket!==generation.current)return;setItems(current=>[...current,...page.items.filter(i=>!current.some(c=>c.id===i.id))]);setNextOffset(page.nextOffset);}
  useEffect(()=>{void loadOverview();const timer=setInterval(()=>{void loadOverview();},60000);return()=>clearInterval(timer);},[]);
  useEffect(()=>{void loadItems();},[loadItems]);
  useEffect(()=>{writeView(view,selected);},[view,selected]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(null),8000);return()=>clearTimeout(timer);},[notice]);

  // Local count changes keep the sidebar in step without refetching after every read.
  function adjust(changes:{id:number;feedId:number;unread?:boolean;starred?:boolean}[]){
    setItems(current=>current.map(item=>{const change=changes.find(c=>c.id===item.id);return change?{...item,...change.unread!==undefined?{unread:change.unread}:{},...change.starred!==undefined?{starred:change.starred}:{}}:item;}));
    setOverview(current=>{if(!current)return current;let unread=0,starred=0;const perFeed=new Map<number,number>();
      for(const c of changes){const item=items.find(i=>i.id===c.id)??(article?.id===c.id?article:undefined);if(!item)continue;
        if(c.unread!==undefined&&c.unread!==item.unread){const d=c.unread?1:-1;unread+=d;perFeed.set(c.feedId,(perFeed.get(c.feedId)??0)+d);}
        if(c.starred!==undefined&&c.starred!==item.starred)starred+=c.starred?1:-1;}
      return {...current,unreadCount:Math.max(0,current.unreadCount+unread),starredCount:Math.max(0,current.starredCount+starred),feeds:current.feeds.map(f=>perFeed.has(f.id)?{...f,unreadCount:Math.max(0,f.unreadCount+perFeed.get(f.id)!)}:f)};});
    setArticle(current=>{const change=current&&changes.find(c=>c.id===current.id);return change&&current?{...current,...change.unread!==undefined?{unread:change.unread}:{},...change.starred!==undefined?{starred:change.starred}:{}}:current;});
  }
  async function setState(targets:{id:number;feedId:number}[],change:{unread?:boolean;starred?:boolean}){
    if(!targets.length)return;adjust(targets.map(t=>({...t,...change})));
    try{await request(`${base}/items/state`,{method:"POST",body:JSON.stringify({ids:targets.map(t=>t.id),...change})},session.csrfToken);}
    catch(cause){adjust(targets.map(t=>({...t,...change.unread!==undefined?{unread:!change.unread}:{},...change.starred!==undefined?{starred:!change.starred}:{}})));fail(cause);}
  }
  async function open(id:number){
    setSelected(id);setArticleError("");if(article?.id!==id)setArticle(undefined);
    try{
      const detail=await request<NewsItemDetail>(`${base}/items/${id}`);setArticle(detail);
      if(detail.unread&&prefs.newsMarkRead==="open"){adjust([{id,feedId:detail.feedId,unread:false}]);await request(`${base}/items/state`,{method:"POST",body:JSON.stringify({ids:[id],unread:false})},session.csrfToken).catch(fail);setArticle(a=>a&&{...a,unread:false});}
    }catch(cause){setArticleError((cause as Error).message);}
  }
  useEffect(()=>{if(start.item)void open(start.item);},[]);
  function step(delta:number,unreadOnly=false){
    const index=items.findIndex(i=>i.id===selected),candidates=delta>0?items.slice(index+1):items.slice(0,Math.max(0,index)).reverse(),next=candidates.find(i=>!unreadOnly||i.unread);
    if(next){void open(next.id);requestAnimationFrame(()=>list.current?.querySelector(`[data-item="${next.id}"]`)?.scrollIntoView({block:"nearest"}));}
    else if(delta>0&&nextOffset!==null)void loadMore();
  }
  async function markAll(){
    const newest=Math.max(0,...items.map(i=>i.id),overview?.newestItemId??0);setBusy(true);
    try{const result=await request<{changed:number;ids:number[]}>(`${base}/read`,{method:"POST",body:JSON.stringify({type:view.type,..."id" in view?{id:view.id}:{},newestItemId:newest})},session.csrfToken);
      setItems(current=>current.map(i=>({...i,unread:false})));await loadOverview();
      setNotice({message:`${result.changed} article${result.changed===1?"":"s"} marked read`,undo:result.ids.length?()=>{void request(`${base}/items/state`,{method:"POST",body:JSON.stringify({ids:result.ids.slice(0,1000),unread:true})},session.csrfToken).then(()=>Promise.all([loadOverview(),loadItems()])).catch(fail);}:undefined});
    }catch(cause){fail(cause);}finally{setBusy(false);}
  }
  async function refresh(){setRefreshing(true);try{setOverview(await request<NewsOverview>(`${base}/refresh`,{method:"POST"},session.csrfToken));await loadItems();}catch(cause){fail(cause);}finally{setRefreshing(false);}}
  async function updateFeed(feed:NewsFeed,change:Partial<NewsFeed>&{updateNow?:boolean},message?:string){
    try{await request(`${base}/feeds/${feed.id}`,{method:"PATCH",body:JSON.stringify(change)},session.csrfToken);await loadOverview();if(change.updateNow||change.paused===false)await loadItems();if(message)setNotice({message});}catch(cause){fail(cause);}
  }
  function choose(next:View){setView(next);setSelected(undefined);setArticle(undefined);setNavigation(false);}

  // Mark on scroll: rows whose bottom passes the list's top edge count as read.
  useEffect(()=>{
    const root=list.current;if(prefs.newsMarkRead!=="scroll"||!root)return;let timer:ReturnType<typeof setTimeout>|undefined;
    const flush=()=>{const ids=[...pendingRead.current];pendingRead.current.clear();const targets=ids.map(id=>items.find(i=>i.id===id)).filter((i):i is NewsItem=>!!i&&i.unread);void setState(targets.map(t=>({id:t.id,feedId:t.feedId})),{unread:false});};
    const observer=new IntersectionObserver(entries=>{for(const entry of entries){const bounds=entry.rootBounds;if(!entry.isIntersecting&&bounds&&entry.boundingClientRect.bottom<=bounds.top+1){const id=Number((entry.target as HTMLElement).dataset.item);if(items.find(i=>i.id===id)?.unread)pendingRead.current.add(id);}}if(pendingRead.current.size){clearTimeout(timer);timer=setTimeout(flush,600);}},{root,threshold:0});
    for(const row of root.querySelectorAll("[data-item]"))observer.observe(row);
    return()=>{observer.disconnect();clearTimeout(timer);};
  },[items,prefs.newsMarkRead]);
  useEffect(()=>{const sentinel=list.current?.querySelector(".news-more");if(!sentinel)return;const observer=new IntersectionObserver(e=>{if(e.some(x=>x.isIntersecting))void loadMore();},{root:list.current});observer.observe(sentinel);return()=>observer.disconnect();},[items,nextOffset,listState]);

  // Shortcuts work outside fields, dialogs, menus and editors.
  useEffect(()=>{
    const keys=(event:KeyboardEvent)=>{
      const target=event.target as HTMLElement;if(event.defaultPrevented||event.ctrlKey||event.metaKey||event.altKey||target.closest("input,textarea,select,[contenteditable],[role=dialog],[role=menu],.editing-pane"))return;
      const current=article&&article.id===selected?article:items.find(i=>i.id===selected);
      if(event.key==="j"||event.key==="n"){event.preventDefault();step(1,event.key==="n");}
      else if(event.key==="k"||event.key==="p"){event.preventDefault();step(-1);}
      else if(event.key==="s"&&current){event.preventDefault();void setState([current],{starred:!current.starred});}
      else if(event.key==="m"&&current){event.preventDefault();void setState([current],{unread:!current.unread});}
      else if(event.key==="o"&&current?.url){event.preventDefault();window.open(current.url,"_blank","noopener,noreferrer");}
      else if(event.key==="r"){event.preventDefault();void refresh();}
      else if(event.key==="A"&&event.shiftKey){event.preventDefault();void markAll();}
      else if(event.key==="Escape"&&selected){event.preventDefault();setSelected(undefined);setArticle(undefined);}
    };
    document.addEventListener("keydown",keys);return()=>document.removeEventListener("keydown",keys);
  });

  const title=view.type==="feed"?feedById.get(view.id)?.title??"Feed":view.type==="folder"?folders.find(f=>f.id===view.id)?.name??"Folder":view.type==="starred"?"Starred":view.type==="all"?"All articles":"Unread";
  const count=view.type==="unread"?overview?.unreadCount:view.type==="starred"?overview?.starredCount:view.type==="feed"?feedById.get(view.id)?.unreadCount:view.type==="folder"?feeds.filter(f=>f.folderId===view.id).reduce((n,f)=>n+f.unreadCount,0):undefined;
  const failing=feeds.filter(f=>f.updateErrorCount>0&&(view.type==="feed"?f.id===view.id:view.type==="folder"?f.folderId===view.id:false));
  const updated=Math.max(0,...feeds.map(f=>f.lastUpdated));
  const feedActions=(feed:NewsFeed):MenuAction[]=>[
    {label:"Mark all read",icon:"checks",run:()=>{choose({type:"feed",id:feed.id});void request<{changed:number}>(`${base}/read`,{method:"POST",body:JSON.stringify({type:"feed",id:feed.id,newestItemId:overview?.newestItemId??0})},session.csrfToken).then(()=>Promise.all([loadOverview(),loadItems()])).catch(fail);}},
    {label:"Edit feed",icon:"pencil",run:()=>setPane({kind:"feed",feed})},
    {label:feed.fullText?"Full text: on":"Full text: off",icon:"file",run:()=>{void updateFeed(feed,{fullText:!feed.fullText},feed.fullText?"Full text off for new articles":"Full text on for new articles");}},
    {label:feed.pinned?"Unpin":"Pin to top",icon:"pin",run:()=>{void updateFeed(feed,{pinned:!feed.pinned});}},
    {label:feed.paused?"Resume updates":"Pause updates",icon:"pause",run:()=>{void updateFeed(feed,{paused:!feed.paused},feed.paused?"Updates resumed":"Updates paused");}},
    {label:"Update now",icon:"refresh",disabled:feed.paused,run:()=>{void updateFeed(feed,{updateNow:true},"Feed updated");}},
    {label:"Unsubscribe…",icon:"trash",danger:true,separator:true,run:()=>setConfirm({title:`Unsubscribe from ${feed.title}?`,text:"This removes the feed and all of its saved articles, including starred ones.",action:"Unsubscribe",run:async()=>{await request(`${base}/feeds/${feed.id}`,{method:"DELETE"},session.csrfToken);if(view.type==="feed"&&view.id===feed.id)choose({type:"unread"});await loadOverview();await loadItems();}})},
  ];
  const folderActions=(folder:NewsFolder):MenuAction[]=>[
    {label:"Mark all read",icon:"checks",run:()=>{void request(`${base}/read`,{method:"POST",body:JSON.stringify({type:"folder",id:folder.id,newestItemId:overview?.newestItemId??0})},session.csrfToken).then(()=>Promise.all([loadOverview(),loadItems()])).catch(fail);}},
    {label:"Rename folder",icon:"pencil",run:()=>setPane({kind:"folder",folder})},
    {label:"Delete folder…",icon:"trash",danger:true,separator:true,run:()=>{const n=feeds.filter(f=>f.folderId===folder.id).length;setConfirm({title:`Delete ${folder.name}?`,text:n?`This unsubscribes from its ${n} feed${n===1?"":"s"} and removes their saved articles.`:"The folder is empty.",action:"Delete folder",run:async()=>{await request(`${base}/folders/${folder.id}`,{method:"DELETE"},session.csrfToken);if(view.type==="folder"&&view.id===folder.id)choose({type:"unread"});await loadOverview();await loadItems();}});}},
  ];
  const feedRow=(feed:NewsFeed,nested:boolean)=><div className={`news-row news-feed-row${nested?" is-nested":""}`} key={`feed-${feed.id}`}><button className={`nav-item${view.type==="feed"&&view.id===feed.id?" active":""}${feed.paused?" is-paused":""}`} aria-current={view.type==="feed"&&view.id===feed.id?"page":undefined} onClick={()=>choose({type:"feed",id:feed.id})}><span className={markClass(feed.id)} aria-hidden="true">{initial(feed.title)}</span><span className="news-row-title">{feed.title}</span>{feed.updateErrorCount>0&&<span className="news-warning" title={feed.lastUpdateError}><Icon name="alert"/><span className="sr-only">Updates failing</span></span>}{feed.paused&&<span className="news-paused" title="Updates paused"><Icon name="pause"/><span className="sr-only">Paused</span></span>}{feed.unreadCount>0&&<span className="news-count">{feed.unreadCount}<span className="sr-only"> unread</span></span>}</button><ActionMenu label={`Actions for ${feed.title}`} className="news-row-menu" actions={feedActions(feed)}><Icon name="more"/></ActionMenu></div>;
  const sidebar=<>
    <NewItemButton onClick={()=>setPane({kind:"subscribe"})}>Subscribe</NewItemButton>
    <nav aria-label="News views">{([["unread","Unread",overview?.unreadCount,"list"],["starred","Starred",overview?.starredCount,"star"],["all","All articles",undefined,"file"]] as const).map(([type,label,n,icon])=><button key={type} className={`nav-item news-view${view.type===type?" active":""}`} aria-current={view.type===type?"page":undefined} onClick={()=>choose({type})}><Icon name={icon}/><span className="news-row-title">{label}</span>{!!n&&<span className="news-count">{n}</span>}</button>)}</nav>
    <div className="sidebar-heading news-heading"><h2>Folders</h2><button type="button" className="icon-button" aria-label="New folder" title="New folder" onClick={()=>setPane({kind:"folder"})}><Icon name="plus"/></button></div>
    <nav aria-label="Feeds" className="news-feeds">
      {folders.map(folder=>{const children=feeds.filter(f=>f.folderId===folder.id),unread=children.reduce((n,f)=>n+f.unreadCount,0);return <Fragment key={`folder-${folder.id}`}><div className="news-row news-folder-row"><button type="button" className="icon-button news-toggle" aria-label={folder.opened?`Collapse ${folder.name}`:`Expand ${folder.name}`} aria-expanded={folder.opened} onClick={()=>{setOverview(o=>o&&{...o,folders:o.folders.map(f=>f.id===folder.id?{...f,opened:!f.opened}:f)});void request(`${base}/folders/${folder.id}`,{method:"PATCH",body:JSON.stringify({opened:!folder.opened})},session.csrfToken).catch(fail);}}><Icon name="chevron"/></button><button className={`nav-item${view.type==="folder"&&view.id===folder.id?" active":""}`} aria-current={view.type==="folder"&&view.id===folder.id?"page":undefined} onClick={()=>choose({type:"folder",id:folder.id})}><Icon name="folder"/><span className="news-row-title">{folder.name}</span>{children.some(f=>f.updateErrorCount>0)&&<span className="news-warning"><Icon name="alert"/><span className="sr-only">A feed is failing</span></span>}{unread>0&&<span className="news-count">{unread}</span>}</button><ActionMenu label={`Actions for folder ${folder.name}`} className="news-row-menu" actions={folderActions(folder)}><Icon name="more"/></ActionMenu></div>{folder.opened&&children.map(feed=>feedRow(feed,true))}</Fragment>;})}
      {feeds.filter(f=>!f.folderId).map(feed=>feedRow(feed,false))}
    </nav>
    {overview&&<div className="sidebar-footer news-footer"><button type="button" className="text-button" disabled={refreshing} onClick={()=>{void refresh();}}><Icon name="refresh"/>{refreshing?"Updating…":updated?`Updated ${relative(updated)}`:"Update feeds"}</button></div>}
    <button type="button" className="nav-item ui-app-settings-entry" onClick={()=>setPane({kind:"settings"})}><Icon name="settings"/>News settings</button>
  </>;
  let lastDay="";
  const reader=selected?<Reader item={article?.id===selected?article:undefined} error={articleError} feed={article?feedById.get(article.feedId):undefined} images={prefs.newsImages} session={session}
    onClose={()=>{setSelected(undefined);setArticle(undefined);}} onState={change=>{if(article)void setState([article],change);}} onNext={()=>step(1,true)} onRetry={()=>{void open(selected);}}/>:undefined;
  return <div className={`app-shell pim-shell news-shell${reader?" has-pim-details":""}`}>
    <WorkspaceHeader app="news" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId} onSettings={()=>setPane({kind:"settings"})}/>
    <div className="pim-workspace news-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar news-sidebar${navigation?" is-open":""}`} aria-label="News navigation">{sidebar}</aside>
      <main className="pim-main news-main" tabIndex={-1} aria-label={title}>
        <div className="news-list-heading"><h1>{title}</h1>{count!==undefined&&<span className="news-heading-count">{count}<span className="sr-only">{view.type==="starred"?" starred":" unread"}</span></span>}<div className="news-tools">
          <button type="button" className="icon-button" aria-label="Update feeds" title="Update feeds (r)" disabled={refreshing} onClick={()=>{void refresh();}}><Icon name="refresh"/></button>
          {view.type!=="starred"&&<button type="button" className="secondary-button news-mark-all" disabled={busy||!items.some(i=>i.unread)} onClick={()=>{void markAll();}} title="Mark all read (Shift A)"><Icon name="checks"/>Mark all read</button>}
          {view.type==="feed"&&feedById.get(view.id)&&<ActionMenu label={`Actions for ${title}`} actions={feedActions(feedById.get(view.id)!)}><Icon name="more"/></ActionMenu>}
          {view.type==="folder"&&folders.find(f=>f.id===view.id)&&<ActionMenu label={`Actions for folder ${title}`} actions={folderActions(folders.find(f=>f.id===view.id)!)}><Icon name="more"/></ActionMenu>}
        </div></div>
        {loadError&&<div className="news-banner is-error" role="alert"><Icon name="alert"/><span><b>News could not be loaded</b>{loadError}</span><button className="secondary-button" onClick={()=>{void loadOverview();void loadItems();}}>Retry</button></div>}
        {failing.map(feed=><div className="news-banner" key={feed.id} role="status"><Icon name="alert"/><span><b>{feed.title}: last {feed.updateErrorCount} update{feed.updateErrorCount===1?"":"s"} failed</b>{feed.lastUpdateError} Dar retries on its own; existing articles stay readable.</span><button className="secondary-button" onClick={()=>{void updateFeed(feed,{updateNow:true},"Feed updated");}}>Retry now</button></div>)}
        <div className="news-list" ref={list} role="list" aria-label={`${title} articles`}>
          {listState==="loading"&&!items.length&&<p className="news-empty" role="status">Loading articles…</p>}
          {listState==="error"&&<div className="news-empty" role="alert"><h2>Articles could not be loaded</h2><p>{listError}</p><button className="secondary-button" onClick={()=>{void loadItems();}}>Retry</button></div>}
          {listState==="ready"&&!items.length&&<div className="news-empty">{!feeds.length?<><h2>No feeds yet</h2><p>Subscribe to a site or import an OPML file from another reader.</p><button className="primary-button" onClick={()=>setPane({kind:"subscribe"})}>Subscribe</button></>:view.type==="starred"?<><h2>No starred articles</h2><p>Star an article with <kbd>s</kbd> to keep it here.</p></>:<><h2>All caught up</h2><p>{view.type==="unread"?"There are no unread articles.":"No articles in this view."}</p></>}</div>}
          {items.map(item=>{const day=dayLabel(item.pubDate),feed=feedById.get(item.feedId),heading=day!==lastDay?(lastDay=day,<div className="news-day" role="presentation" key={`day-${day}-${item.id}`}>{day}</div>):null;
            return <Fragment key={item.id}>{heading}<div role="listitem" data-item={item.id} className={`news-article${item.unread?"":" is-read"}${item.id===selected?" is-selected":""}`}><button type="button" aria-current={item.id===selected?"true":undefined} onClick={()=>{void open(item.id);}}>
              <span className="news-dot" aria-hidden="true"/><span className={markClass(item.feedId)} aria-hidden="true">{initial(feed?.title??"?")}</span>
              <span className="news-article-title">{item.title||"Untitled"}{item.unread&&<span className="sr-only">, unread</span>}</span>
              <span className="news-article-time">{item.starred&&<span className="news-star"><Icon name="star"/><span className="sr-only">Starred</span></span>}{item.enclosureLink&&<Icon name="clock"/>}<time dateTime={new Date(item.pubDate).toISOString()}>{timeLabel(item.pubDate)}</time></span>
              <span className="news-article-excerpt">{feed?.title??"Feed"}{item.excerpt?` · ${item.excerpt}`:""}</span></button></div></Fragment>;})}
          {nextOffset!==null&&listState==="ready"&&<div className="news-more"><button className="text-button" onClick={()=>{void loadMore();}}>Load more</button></div>}
        </div>
      </main>
      {reader&&<aside className="pim-details news-details" tabIndex={-1} aria-label="Article">{reader}</aside>}
    </div>
    {pane?.kind==="subscribe"&&<SubscribePane folders={folders} session={session} onClose={()=>setPane(undefined)} onDone={async feed=>{setPane(undefined);await loadOverview();choose({type:"feed",id:feed.id});setNotice({message:`Subscribed to ${feed.title}`});}} onSettings={()=>setPane({kind:"settings"})}/>}
    {pane?.kind==="feed"&&<FeedPane feed={feedById.get(pane.feed.id)??pane.feed} folders={folders} session={session} onClose={()=>setPane(undefined)} onSaved={async()=>{setPane(undefined);await loadOverview();await loadItems();setNotice({message:"Feed saved"});}}/>}
    {pane?.kind==="folder"&&<FolderPane folder={pane.folder} session={session} onClose={()=>setPane(undefined)} onSaved={async folder=>{setPane(undefined);await loadOverview();if(!pane.folder)choose({type:"folder",id:folder.id});}}/>}
    {pane?.kind==="settings"&&<NewsSettings session={session} onClose={()=>setPane(undefined)} onImported={async()=>{await loadOverview();await loadItems();}}/>}
    {confirm&&<ConfirmAction {...confirm} onClose={()=>setConfirm(undefined)}/>}
    <div className="news-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast news-toast"><span>{notice.message}</span>{notice.undo&&<button type="button" className="text-button" onClick={()=>{const undo=notice.undo!;setNotice(null);undo();}}>Undo</button>}</div>}</div>
  </div>;
}
function relative(time:number){const minutes=Math.round((Date.now()-time)/60000);return minutes<1?"just now":minutes<60?`${minutes} min ago`:minutes<1440?`${Math.round(minutes/60)} h ago`:new Date(time).toLocaleDateString();}

function ConfirmAction({title,text,action,run,onClose}:{title:string;text:string;action:string;run:()=>Promise<void>;onClose:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  return <Dialog title={title} onClose={onClose}><p>{text}</p>{error&&<p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button className="text-button" onClick={onClose}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setBusy(true);setError("");void run().then(onClose,cause=>{setError((cause as Error).message);setBusy(false);});}}>{busy?"Working…":action}</button></div></Dialog>;
}

function Reader({item,feed,error,images,session,onClose,onState,onNext,onRetry}:{item?:NewsItemDetail;feed?:NewsFeed;error:string;images:"proxy"|"block";session:BrowserSession;onClose:()=>void;onState:(change:{unread?:boolean;starred?:boolean})=>void;onNext:()=>void;onRetry:()=>void}){
  const body=useRef<HTMLDivElement>(null),media=useRef<HTMLMediaElement|null>(null),saved=useRef(0),heading=useRef<HTMLHeadingElement>(null);
  useEffect(()=>{if(body.current)body.current.replaceChildren(item?renderArticle(item.body,images):document.createDocumentFragment());},[item?.id,item?.body,images]);
  useEffect(()=>{if(item&&matchMedia("(max-width:800px)").matches)heading.current?.focus();},[item?.id]);
  useEffect(()=>{saved.current=item?.position??0;const element=media.current;if(element&&item?.position)element.currentTime=item.position;},[item?.id]);
  function remember(){const element=media.current;if(!element||!item||Math.abs(element.currentTime-saved.current)<15)return;saved.current=element.currentTime;void request(`/api/v1/news/items/${item.id}/position`,{method:"POST",body:JSON.stringify({position:Math.floor(element.currentTime)})},session.csrfToken).catch(()=>{});}
  const share:MenuAction[]=item?.url?[{label:"Copy link",icon:"copy",run:()=>{void navigator.clipboard?.writeText(item.url);}},...("share" in navigator?[{label:"Share…",icon:"share" as const,run:()=>{void navigator.share({title:item.title,url:item.url}).catch(()=>{});}}]:[])]:[];
  const words=item?(body.current?.textContent??item.excerpt).split(/\s+/).length:0,video=item?.enclosureMime.startsWith("video/");
  return <section className="news-reader">
    <div className="news-reader-head"><button type="button" className="icon-button news-back" aria-label="Back to articles" onClick={onClose}><Icon name="back"/></button>{feed&&<span className="news-reader-feed"><span className={markClass(feed.id)} aria-hidden="true">{initial(feed.title)}</span>{feed.title}</span>}
      {item&&<div className="news-reader-tools">
        <button type="button" className={`icon-button${item.starred?" is-starred":""}`} aria-pressed={item.starred} aria-label="Star" title="Star (s)" onClick={()=>onState({starred:!item.starred})}><Icon name="star"/></button>
        <button type="button" className="icon-button" aria-pressed={item.unread} aria-label="Keep unread" title="Keep unread (m)" onClick={()=>onState({unread:!item.unread})}><Icon name="unread"/></button>
        {item.url&&<a className="icon-button" href={item.url} target="_blank" rel="noopener noreferrer" aria-label="Open original" title="Open original (o)"><Icon name="external"/></a>}
        {share.length>0&&<ActionMenu label="Share" actions={share}><Icon name="share"/></ActionMenu>}
        <button type="button" className="icon-button news-close" aria-label="Close article" onClick={onClose}><Icon name="close"/></button></div>}
    </div>
    {error?<div className="news-empty" role="alert"><h2>This article could not be opened</h2><p>{error}</p><button className="secondary-button" onClick={onRetry}>Retry</button></div>
    :!item?<p className="news-empty" role="status">Opening article…</p>
    :<article className="news-article-body" dir={item.rtl?"rtl":undefined}>
      {feed?.fullText&&<span className="news-fulltext"><Icon name="file"/>Full text</span>}
      <h1 ref={heading} tabIndex={-1}>{item.url?<a href={item.url} target="_blank" rel="noopener noreferrer">{item.title||"Untitled"}</a>:item.title||"Untitled"}</h1>
      <p className="news-byline">{[item.author&&`By ${item.author}`,new Date(item.pubDate).toLocaleString([],{dateStyle:"medium",timeStyle:"short"}),words>120?`${Math.max(1,Math.round(words/230))} min read`:""].filter(Boolean).join(" · ")}</p>
      {item.enclosureLink&&/^https:/.test(item.enclosureLink)&&(/^(audio|video)\//.test(item.enclosureMime)||/\.(mp3|m4a|ogg|opus|mp4|webm)(\?|$)/i.test(item.enclosureLink))&&<div className="news-player">{video?<video ref={el=>{media.current=el;}} controls preload="metadata" src={item.enclosureLink} onPause={remember} onTimeUpdate={remember}/>:<audio ref={el=>{media.current=el;}} controls preload="metadata" src={item.enclosureLink} onPause={remember} onTimeUpdate={remember}/>}{item.mediaDescription&&<p>{item.mediaDescription}</p>}</div>}
      {item.enclosureLink&&!/^https:/.test(item.enclosureLink)&&<p><a href={item.enclosureLink} target="_blank" rel="noopener noreferrer">Open attachment</a></p>}
      <div ref={body} className="news-body"/>
    </article>}
    {item&&<div className="news-reader-foot"><span><kbd>j</kbd> <kbd>k</kbd> Next / previous</span><button type="button" className="secondary-button" onClick={onNext}>Next unread<Icon name="chevron"/></button></div>}
  </section>;
}

function SubscribePane({folders,session,onClose,onDone,onSettings}:{folders:NewsFolder[];session:BrowserSession;onClose:()=>void;onDone:(feed:NewsFeed)=>Promise<void>;onSettings:()=>void}){
  const [address,setAddress]=useState(""),[found,setFound]=useState<NewsDiscovered[]>(),[choice,setChoice]=useState(""),[name,setName]=useState(""),[folder,setFolder]=useState("0"),[fullText,setFullText]=useState(false),[busy,setBusy]=useState(""),[error,setError]=useState("");
  async function discover(event?:FormEvent){event?.preventDefault();if(!address.trim())return;setBusy("discover");setError("");setFound(undefined);
    try{const result=await request<{items:NewsDiscovered[]}>(`${base}/discover`,{method:"POST",body:JSON.stringify({url:address.trim()})},session.csrfToken);setFound(result.items);setChoice(result.items[0].url);setName(result.items[0].title);}
    catch(cause){setError((cause as Error).message);}finally{setBusy("");}}
  async function subscribe(event:FormEvent){event.preventDefault();if(!choice)return;setBusy("subscribe");setError("");
    try{await onDone(await request<NewsFeed>(`${base}/feeds`,{method:"POST",body:JSON.stringify({url:choice,folderId:Number(folder),title:name.trim()||undefined,fullText})},session.csrfToken));}
    catch(cause){setError((cause as Error).message);setBusy("");}}
  const selected=found?.find(f=>f.url===choice);
  return <EditorPane title="Subscribe" onClose={onClose}><form className="news-subscribe" onSubmit={found?subscribe:discover}>
    <label>Site or feed address<span className="news-address"><input autoFocus type="text" inputMode="url" required value={address} placeholder="example.com or https://example.com/feed" onChange={e=>{setAddress(e.target.value);setFound(undefined);}}/><button type="button" className="secondary-button" disabled={!!busy||!address.trim()} onClick={()=>{void discover();}}>{busy==="discover"?"Looking…":"Find feeds"}</button></span></label>
    {found&&<fieldset className="news-found"><legend>{found.length===1?"Found 1 feed":`Found ${found.length} feeds`}</legend>{found.map(f=><label key={f.url} className={f.url===choice?"is-checked":""}><input type="radio" name="feed" checked={f.url===choice} onChange={()=>{setChoice(f.url);setName(f.title);}}/><span><b>{f.title}</b><small>{f.kind} · {f.url}</small></span></label>)}</fieldset>}
    {found&&<><div className="news-form-row"><label>Folder<Select value={folder} onChange={e=>setFolder(e.target.value)}><option value="0">No folder</option>{folders.map(f=><option key={f.id} value={f.id}>{f.name}</option>)}</Select></label><label>Name<input value={name} maxLength={1000} onChange={e=>setName(e.target.value)}/></label></div>
      <label className="news-check"><input type="checkbox" checked={fullText} onChange={e=>setFullText(e.target.checked)}/><span><b>Full text</b><small>Fetch each article page for the complete text</small></span></label>
      {selected&&selected.items.length>0&&<div className="news-preview"><h3>Latest items</h3><ul>{selected.items.map((i,n)=><li key={n}><span>{i.title||"Untitled"}</span><small>{dayLabel(i.pubDate)}</small></li>)}</ul></div>}</>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onSettings}>Import OPML…</button><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={!!busy||(found?!choice:!address.trim())}>{found?busy==="subscribe"?"Subscribing…":"Subscribe":busy?"Looking…":"Find feeds"}</button></div>
  </form></EditorPane>;
}

function FeedPane({feed,folders,session,onClose,onSaved}:{feed:NewsFeed;folders:NewsFolder[];session:BrowserSession;onClose:()=>void;onSaved:()=>Promise<void>}){
  const [error,setError]=useState(""),[busy,setBusy]=useState(false);
  async function save(event:FormEvent<HTMLFormElement>){event.preventDefault();const data=new FormData(event.currentTarget);setBusy(true);setError("");
    try{await request(`${base}/feeds/${feed.id}`,{method:"PATCH",body:JSON.stringify({title:String(data.get("title")),folderId:Number(data.get("folderId")),fullText:data.get("fullText")==="on",pinned:data.get("pinned")==="on",paused:data.get("paused")==="on"})},session.csrfToken);await onSaved();}
    catch(cause){setError((cause as Error).message);setBusy(false);}}
  return <EditorPane title="Edit feed" onClose={onClose}><form className="news-subscribe" onSubmit={save}>
    <label>Name<input name="title" required maxLength={1000} defaultValue={feed.title}/></label>
    <label>Folder<Select name="folderId" defaultValue={String(feed.folderId)}><option value="0">No folder</option>{folders.map(f=><option key={f.id} value={f.id}>{f.name}</option>)}</Select></label>
    <label className="news-check"><input type="checkbox" name="fullText" defaultChecked={feed.fullText}/><span><b>Full text</b><small>Fetch each new article's page for the complete text</small></span></label>
    <label className="news-check"><input type="checkbox" name="pinned" defaultChecked={feed.pinned}/><span><b>Pin to top</b><small>Listed first in the sidebar</small></span></label>
    <label className="news-check"><input type="checkbox" name="paused" defaultChecked={feed.paused}/><span><b>Pause updates</b><small>Keep its articles without checking for new ones</small></span></label>
    <dl className="news-facts"><dt>Address</dt><dd><a href={feed.url} target="_blank" rel="noopener noreferrer">{feed.url}</a></dd><dt>Last update</dt><dd>{feed.lastUpdated?new Date(feed.lastUpdated).toLocaleString():"Not yet"}</dd>{feed.updateErrorCount>0&&<><dt>Problem</dt><dd className="error">{feed.lastUpdateError} ({feed.updateErrorCount} failed update{feed.updateErrorCount===1?"":"s"})</dd></>}</dl>
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy}>{busy?"Saving…":"Save"}</button></div>
  </form></EditorPane>;
}

function FolderPane({folder,session,onClose,onSaved}:{folder?:NewsFolder;session:BrowserSession;onClose:()=>void;onSaved:(folder:NewsFolder)=>Promise<void>}){
  const [error,setError]=useState(""),[busy,setBusy]=useState(false);
  async function save(event:FormEvent<HTMLFormElement>){event.preventDefault();const name=String(new FormData(event.currentTarget).get("name")).trim();setBusy(true);setError("");
    try{await onSaved(await request<NewsFolder>(folder?`${base}/folders/${folder.id}`:`${base}/folders`,{method:folder?"PATCH":"POST",body:JSON.stringify({name})},session.csrfToken));}
    catch(cause){setError((cause as Error).message);setBusy(false);}}
  return <EditorPane title={folder?"Rename folder":"New folder"} onClose={onClose}><form className="news-subscribe" onSubmit={save}><label>Name<input name="name" autoFocus required maxLength={200} defaultValue={folder?.name}/></label>{error&&<p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy}>{busy?"Saving…":folder?"Rename":"Create folder"}</button></div></form></EditorPane>;
}

function NewsSettings({session,onClose,onImported}:{session:BrowserSession;onClose:()=>void;onImported:()=>Promise<void>}){
  const saver=usePreferenceSaver(session),[section,setSection]=useState("General"),p={...newsDefaults,...saver.preferences} as typeof newsDefaults&Record<string,unknown>,state=saver.states;
  const [importing,setImporting]=useState(false),[importResult,setImportResult]=useState(""),[importError,setImportError]=useState("");
  async function importFile(file:File|undefined){if(!file)return;setImporting(true);setImportError("");setImportResult("");
    try{if(file.size>2*1024*1024)throw new Error("Choose an OPML file up to 2 MiB.");const result=await request<{added:number;skipped:number;folders:number}>(`${base}/opml`,{method:"POST",body:JSON.stringify({text:await file.text()})},session.csrfToken);
      setImportResult(`Imported ${result.added} feed${result.added===1?"":"s"}${result.folders?` into ${result.folders} new folder${result.folders===1?"":"s"}`:""}${result.skipped?`; ${result.skipped} already subscribed`:""}. New feeds fill in as they update.`);await onImported();}
    catch(cause){setImportError((cause as Error).message);}finally{setImporting(false);}}
  const address=`${location.origin}/dav/news`;
  const row=(content:ReactNode)=>content;
  return <SettingsSheet title="News settings" icon="rss" sections={["General","Import and export","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={onClose} notice={saver.notice} onNotice={saver.setNotice}>
    <SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>
    {section==="General"&&saver.preferences&&row(<>
      <section className="settings-group" aria-label="Reading"><h3>Reading</h3>
        <SettingRow label="Mark as read" hint="When an article opens, or as it scrolls past in the list" state={state.newsMarkRead}><Segmented label="Mark as read" value={p.newsMarkRead} options={[["open","On open"],["scroll","On scroll"]] as const} onChange={value=>saver.change("newsMarkRead",value,value==="open"?"Articles are marked read when opened":"Articles are marked read as they scroll past")}/></SettingRow>
        <SettingRow label="Order" state={state.newsOrder}><Segmented label="Order" value={p.newsOrder} options={[["newest","Newest first"],["oldest","Oldest first"]] as const} onChange={value=>saver.change("newsOrder",value,`Showing ${value} articles first`)}/></SettingRow>
        <SettingSwitch label="Show read articles in feeds and folders" hint="Unread always shows only unread articles" state={state.newsShowRead} checked={p.newsShowRead} onChange={value=>saver.change("newsShowRead",value,value?"Read articles shown":"Read articles hidden")}/>
        <SettingRow label="Remote images" hint="Through Dar keeps sites from seeing when you read" state={state.newsImages}><Segmented label="Remote images" value={p.newsImages} options={[["proxy","Through Dar"],["block","Block"]] as const} onChange={value=>saver.change("newsImages",value,value==="proxy"?"Images load through Dar":"Images blocked")}/></SettingRow></section>
      <section className="settings-group" aria-label="Updates"><h3>Updates</h3>
        <SettingRow label="Check feeds" hint="Failing feeds back off automatically" state={state.newsRefresh}><Select aria-label="Check feeds" value={String(p.newsRefresh)} onChange={e=>saver.change("newsRefresh",Number(e.target.value),`Feeds are checked every ${minutes(Number(e.target.value))}`)}>{newsRefreshChoices.map(v=><option key={v} value={v}>Every {minutes(v)}</option>)}</Select></SettingRow>
        <SettingRow label="Keep read articles" hint="Starred articles and articles still in a feed are always kept" state={state.newsKeepDays}><Select aria-label="Keep read articles" value={String(p.newsKeepDays)} onChange={e=>saver.change("newsKeepDays",Number(e.target.value),`Read articles are kept for ${e.target.value} days`)}>{newsKeepChoices.map(v=><option key={v} value={v}>{v} days</option>)}</Select></SettingRow></section>
      <section className="settings-group" aria-label="Mobile apps"><h3>Mobile apps</h3>
        <SettingRow label="Nextcloud News apps" hint={<>Use server address <code>{address}</code>, your username <code>{session.user.username}</code> and an app password with News access.</>} stacked><span className="news-sync"><button type="button" className="secondary-button" onClick={()=>{void navigator.clipboard?.writeText(address);saver.setNotice({message:"Server address copied"});}}><Icon name="copy"/>Copy address</button><a className="secondary-button" href="/settings?section=security">App passwords</a></span></SettingRow></section>
    </>)}
    {section==="Import and export"&&<section className="settings-group" aria-label="Import and export"><h3>Subscriptions</h3>
      <SettingRow label="Import OPML" hint="Folders are kept; feeds you already follow are skipped" stacked><label className="secondary-button news-file"><Icon name="upload"/>{importing?"Importing…":"Choose OPML file"}<input type="file" accept=".opml,.xml,text/xml,text/x-opml,application/xml" disabled={importing} onChange={e=>{void importFile(e.target.files?.[0]);e.target.value="";}}/></label></SettingRow>
      {importResult&&<p className="news-import-result" role="status">{importResult}</p>}{importError&&<p className="error" role="alert">{importError}</p>}
      <SettingRow label="Export OPML" hint="Every feed and folder, for any other reader"><a className="secondary-button" href={`${base}/opml`} download><Icon name="download"/>Export</a></SettingRow>
      <SettingRow label="Export starred articles" hint="Titles, links and text as JSON"><a className="secondary-button" href={`${base}/starred`} download><Icon name="download"/>Export</a></SettingRow></section>}
    {section==="Keyboard shortcuts"&&<dl className="ui-shortcuts"><dt>j / k</dt><dd>Next or previous article</dd><dt>n</dt><dd>Next unread article</dd><dt>s</dt><dd>Star or unstar</dd><dt>m</dt><dd>Mark read or unread</dd><dt>o</dt><dd>Open the original page</dd><dt>r</dt><dd>Update feeds</dd><dt>Shift A</dt><dd>Mark everything in this view read</dd><dt>Escape</dt><dd>Close the article</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd></dl>}
  </SettingsSheet>;
}
