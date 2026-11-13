import {useEffect,useId,useMemo,useRef,useState} from "react";
import {createPortal} from "react-dom";
import {request} from "./api.ts";
import {Icon,type IconName} from "./icons.tsx";
import {FileTile} from "./file-types.tsx";
import {useAppList} from "./apps.tsx";
import type {SearchResults,SearchResult,SearchGroup} from "../shared/search.ts";

export function navigateSearchResult(href:string){const event=new CustomEvent("dar:navigate",{cancelable:true,detail:{href}});if(window.dispatchEvent(event))location.assign(href);}

const order:SearchGroup[]=["Photos","Files","Contacts","Calendar","Notes","Tasks","News","Applications"];
const groupIcon:Record<string,IconName>={Photos:"image",Files:"file",Contacts:"contacts",Calendar:"calendar",Notes:"file",Tasks:"check",News:"rss",Applications:"grid",Actions:"chevron",Recent:"undo"};
const isMac=typeof navigator!=="undefined"&&/Mac|iP(hone|ad)/.test(navigator.platform);

/** One selectable palette row: a search result, a recently opened result or a command. */
interface Entry {key:string;section:string;title:string;detail:string;href:string;group?:SearchGroup;thumbnail?:string;result?:SearchResult}

/** Recent queries and opened results stay on this device, per account. */
function recentStore(account:string|undefined){
  const key=account?`dar:search:${account}`:"";
  const read=():{queries:string[];opened:SearchResult[]}=>{try{const value=key?JSON.parse(localStorage.getItem(key)??"{}"):{};return {queries:Array.isArray(value.queries)?value.queries.filter((q:unknown)=>typeof q==="string").slice(0,6):[],opened:Array.isArray(value.opened)?value.opened.filter((r:SearchResult)=>r&&typeof r.href==="string"&&r.href.startsWith("/")).slice(0,5):[]};}catch{return {queries:[],opened:[]};}};
  const write=(value:{queries:string[];opened:SearchResult[]})=>{if(key)try{localStorage.setItem(key,JSON.stringify(value));}catch{/* storage full or disabled: recents are optional */}};
  return {read,write,clear:()=>{if(key)try{localStorage.removeItem(key);}catch{/* ignore */}}};
}

/**
 * The top-bar search trigger. Ctrl/Cmd+K or a click opens the spotlight palette
 * (selected search mock A): a centered modal with app filters, grouped results,
 * a preview of the active row, recents and `>` commands.
 */
export function GlobalSearch({account}:{account?:string}){
  const [open,setOpen]=useState(false),trigger=useRef<HTMLButtonElement>(null);
  useEffect(()=>{
    const shortcut=(event:KeyboardEvent)=>{if((event.ctrlKey||event.metaKey)&&!event.altKey&&event.key.toLowerCase()==="k"&&!event.defaultPrevented&&!document.querySelector("dialog[open]:not(.spotlight),[data-editor-pane]")){event.preventDefault();setOpen(true);document.querySelector<HTMLInputElement>(".spotlight input")?.select();}};
    document.addEventListener("keydown",shortcut);return()=>document.removeEventListener("keydown",shortcut);
  },[]);
  return <div className="global-search-wrap">
    <button ref={trigger} type="button" className="global-search spotlight-trigger" aria-label="Open search" aria-haspopup="dialog" aria-keyshortcuts={isMac?"Meta+K":"Control+K"} onClick={()=>setOpen(true)}><Icon name="search"/><span>Search all apps…</span><kbd className="search-shortcut" aria-hidden="true">{isMac?"⌘ K":"Ctrl K"}</kbd></button>
    {open&&<Spotlight account={account} onClose={()=>setOpen(false)}/>}
  </div>;
}

function Spotlight({account,onClose}:{account?:string;onClose:()=>void}){
  const [query,setQuery]=useState(""),[scope,setScope]=useState<SearchGroup|"All">("All"),[result,setResult]=useState<SearchResults>({items:[],truncated:false}),[loading,setLoading]=useState(false),[error,setError]=useState(""),[retry,setRetry]=useState(0),[active,setActive]=useState(0);
  const store=useMemo(()=>recentStore(account),[account]),[recent,setRecent]=useState(store.read);
  const dialog=useRef<HTMLDialogElement>(null),input=useRef<HTMLInputElement>(null),id=useId(),apps=useAppList();
  const commandMode=query.startsWith(">"),term=(commandMode?query.slice(1):query).trim();

  useEffect(()=>{const previous=document.activeElement as HTMLElement|null;dialog.current?.showModal();input.current?.focus();
    return()=>{if(previous?.isConnected&&previous!==document.body)previous.focus();};},[]);
  useEffect(()=>{const refresh=()=>setRetry(n=>n+1);window.addEventListener("dar:apps",refresh);return()=>window.removeEventListener("dar:apps",refresh);},[]);
  useEffect(()=>{
    const controller=new AbortController();setResult({items:[],truncated:false});setError("");
    if(commandMode||!term){setLoading(false);return;}
    setLoading(true);const timer=setTimeout(()=>{void request<SearchResults>(`/api/v1/search?q=${encodeURIComponent(term)}`,{signal:controller.signal}).then(value=>{if(!controller.signal.aborted)setResult(value);}).catch(e=>{if(!controller.signal.aborted)setError(e.message);}).finally(()=>{if(!controller.signal.aborted)setLoading(false);});},160);
    return()=>{clearTimeout(timer);controller.abort();};
  },[term,commandMode,retry]);

  const commands=useMemo<Entry[]>(()=>[...apps.map(app=>({key:"go:"+app.id,section:"Actions",title:`Go to ${app.label}`,detail:"Open app",href:app.href})),
    {key:"go:settings",section:"Actions",title:"Open Dar settings",detail:"Settings",href:"/settings"},
    {key:"go:appearance",section:"Actions",title:"Appearance settings",detail:"Theme and colors",href:"/settings?section=appearance"},
    {key:"go:security",section:"Actions",title:"Account security",detail:"Passwords and app passwords",href:"/settings?section=security"},
    {key:"go:regional",section:"Actions",title:"Regional settings",detail:"Time zone",href:"/settings?section=regional"}],[apps]);
  const matchCommand=(entry:Entry)=>entry.title.toLowerCase().includes(term.toLowerCase());

  const counts=new Map<SearchGroup,number>();for(const item of result.items)counts.set(item.group,(counts.get(item.group)??0)+1);
  const scopes=order.filter(group=>counts.has(group));
  const effectiveScope=scope!=="All"&&counts.has(scope)?scope:"All";
  const entries:Entry[]=commandMode?commands.filter(matchCommand)
    :!term?[...recent.opened.map(item=>({key:"recent:"+item.id,section:"Jump back in",title:item.title,detail:`${item.group} · ${item.detail}`,href:item.href,group:item.group,thumbnail:item.thumbnail,result:item})),...commands.slice(0,4)]
    :[...(effectiveScope==="All"?commands.filter(matchCommand).slice(0,3):[]),...order.filter(group=>effectiveScope==="All"||group===effectiveScope).flatMap(group=>result.items.filter(item=>item.group===group).map(item=>({key:item.id+":"+item.group,section:item.group,title:item.title,detail:item.detail,href:item.href,group:item.group,thumbnail:item.thumbnail,result:item})))];
  const current=entries[Math.min(active,entries.length-1)];
  useEffect(()=>setActive(0),[query,effectiveScope,result]);
  useEffect(()=>{dialog.current?.querySelector(`#${CSS.escape(id+"-"+active)}`)?.scrollIntoView({block:"nearest"});},[active,id]);

  function remember(entry:Entry){
    const queries=term&&!commandMode?[term,...recent.queries.filter(q=>q!==term)].slice(0,6):recent.queries;
    const opened=entry.result?[entry.result,...recent.opened.filter(r=>r.href!==entry.href)].slice(0,5):recent.opened;
    store.write({queries,opened});
  }
  function choose(entry:Entry|undefined,newTab=false){if(!entry)return;remember(entry);if(newTab){window.open(entry.href,"_blank","noopener");return;}onClose();navigateSearchResult(entry.href);}
  function cycleScope(step:number){const list:(SearchGroup|"All")[]=["All",...scopes];if(list.length<2)return false;setScope(list[(list.indexOf(effectiveScope)+step+list.length)%list.length]);return true;}

  const status=commandMode?(!entries.length?"No matching commands.":""):!term?"":loading?"Searching all apps…":error?"Search could not be completed.":!result.items.length?"No matches across your apps.":result.truncated?"Showing the first matches. Refine your search for more.":"";
  const sections=[...new Set(entries.map(entry=>entry.section))];

  return createPortal(<dialog ref={dialog} className="spotlight" aria-label="Search" onCancel={event=>{event.preventDefault();if(query){setQuery("");setScope("All");}else onClose();}} onClick={event=>{if(event.target===dialog.current)onClose();}}>
    <div className="spotlight-panel">
      <div className="spotlight-input"><Icon name="search"/>
        <input ref={input} type="search" role="combobox" aria-label="Search all apps" aria-autocomplete="list" aria-controls={id} aria-expanded={entries.length>0} aria-activedescendant={current?id+"-"+entries.indexOf(current):undefined} placeholder="Search or type > for commands" maxLength={200} value={query} autoComplete="off" spellCheck={false}
          onChange={event=>setQuery(event.target.value)}
          onKeyDown={event=>{
            if(event.key==="ArrowDown"||event.key==="ArrowUp"){event.preventDefault();if(entries.length)setActive(old=>(Math.min(old,entries.length-1)+(event.key==="ArrowDown"?1:-1)+entries.length)%entries.length);}
            else if(event.key==="Enter"&&current){event.preventDefault();choose(current,event.ctrlKey||event.metaKey);}
            else if(event.key==="Tab"&&term&&!commandMode){if(cycleScope(event.shiftKey?-1:1))event.preventDefault();}
            else if(event.key==="Backspace"&&query===">"){event.preventDefault();setQuery("");}
          }}/>
        {query?<button type="button" className="icon-button" aria-label="Clear search" onClick={()=>{setQuery("");setScope("All");input.current?.focus();}}><Icon name="close"/></button>:<button type="button" className="spotlight-esc" aria-label="Close search" onClick={onClose}>Esc</button>}
      </div>
      {term&&!commandMode&&scopes.length>0&&<div className="spotlight-scopes" role="toolbar" aria-label="Filter by app">{(["All",...scopes] as const).map(group=><button key={group} type="button" className="spotlight-scope" aria-pressed={effectiveScope===group} onClick={()=>{setScope(group);input.current?.focus();}}>{group!=="All"&&<Icon name={groupIcon[group]}/>}{group}<b>{group==="All"?result.items.length:counts.get(group)}</b></button>)}</div>}
      <div className="spotlight-body">
        <div className="spotlight-results">
          {!term&&!commandMode&&recent.queries.length>0&&<div className="spotlight-recent"><h2>Recent searches<button type="button" className="text-button" onClick={()=>{store.clear();setRecent({queries:[],opened:[]});input.current?.focus();}}>Clear</button></h2><div>{recent.queries.map(q=><button key={q} type="button" className="spotlight-chip" onClick={()=>{setQuery(q);input.current?.focus();}}><Icon name="undo"/>{q}</button>)}</div></div>}
          <div role="status" aria-live="polite" className="spotlight-status">{status}</div>
          {error&&<div className="error"><p role="alert">{error}</p><button className="text-button" onClick={()=>setRetry(n=>n+1)}>Retry search</button></div>}
          <div role="listbox" id={id} aria-label="Search results">{sections.map(section=>{const rows=entries.filter(entry=>entry.section===section);return <div role="group" aria-label={section} key={section}>
            <h2>{section}{section!=="Actions"&&section!=="Jump back in"&&<span>{counts.get(section as SearchGroup)}</span>}</h2>
            {rows.map(entry=>{const index=entries.indexOf(entry);return <a key={entry.key} href={entry.href} role="option" aria-selected={current===entry} id={id+"-"+index} tabIndex={-1} className="global-search-result" onMouseMove={()=>setActive(index)} onClick={event=>{event.preventDefault();choose(entry,event.ctrlKey||event.metaKey);}}>
              <Glyph entry={entry}/><span><strong><Highlight text={entry.title} query={term}/></strong><small><Highlight text={entry.detail} query={term}/></small></span><Icon name="chevron" className="spotlight-enter"/></a>;})}
          </div>;})}</div>
        </div>
        {current&&<aside className="spotlight-preview" aria-label="Preview">
          <div className={`spotlight-preview-art${current.thumbnail?" has-image":""}`}>{current.thumbnail?<img src={current.thumbnail} alt=""/>:<Glyph entry={current}/>}</div>
          <h3>{current.title}</h3><p>{current.detail}</p><p className="spotlight-preview-app">{current.group??"Command"}</p>
          <div className="spotlight-preview-actions"><button type="button" className="primary-button" onClick={()=>choose(current)}>Open</button>{current.group&&<button type="button" className="secondary-button" onClick={()=>choose(current,true)}>New tab</button>}</div>
        </aside>}
      </div>
      <div className="spotlight-footer" aria-hidden="true"><span><kbd>↑↓</kbd>Move</span><span><kbd>↵</kbd>Open</span><span><kbd>Tab</kbd>Filter app</span><span><kbd>{isMac?"⌘":"Ctrl"} ↵</kbd>New tab</span><span className="spotlight-footer-end">Type <kbd>&gt;</kbd> for commands</span></div>
    </div>
  </dialog>,document.body);
}

function Glyph({entry}:{entry:Entry}){
  const group=entry.group;
  if(entry.thumbnail)return <span className="global-search-glyph glyph-photos"><img src={entry.thumbnail} alt=""/></span>;
  return <span className={`global-search-glyph glyph-${(group??"actions").toLowerCase()}`}>{group==="Contacts"?initials(entry.title):group==="Files"?<FileTile entry={{kind:entry.detail==="Folder"?"folder":"file",name:entry.title,mediaType:""}}/>:<Icon name={groupIcon[group??"Actions"]??"grid"}/>}</span>;
}
function initials(name:string){return name.split(/\s+/).filter(Boolean).map(word=>word[0]).join("").slice(0,2).toUpperCase()||"?";}
/** Marks the first case-insensitive occurrence of the query. */
function Highlight({text,query}:{text:string;query:string}){const needle=query.trim().toLowerCase(),index=needle?text.toLowerCase().indexOf(needle):-1;if(index<0)return <>{text}</>;return <>{text.slice(0,index)}<mark>{text.slice(index,index+needle.length)}</mark>{text.slice(index+needle.length)}</>;}
