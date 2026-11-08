import {useEffect,useId,useRef,useState} from "react";
import {Brand} from "./auth.tsx";
import {AppSwitcher,type AppId} from "./apps.tsx";
import {ActionMenu} from "./controls.tsx";
import {Icon} from "./icons.tsx";
import {request,type BrowserSession} from "./api.ts";

export function Avatar({name}:{name:string}){return <span className="avatar" aria-hidden="true">{name.split(/\s+/).filter(Boolean).map(word=>word[0]).join("").slice(0,2).toUpperCase()}</span>;}

/** Phone navigation focuses its controls and returns to its opener on dismissal. */
export function useMobileNavigation(){
  const [open,setOpen]=useState(false),id=useId(),ref=useRef<HTMLElement>(null),trigger=useRef<HTMLElement|null>(null),wasOpen=useRef(false);
  useEffect(()=>{
    if(open){trigger.current=document.activeElement as HTMLElement;ref.current?.querySelector<HTMLElement>("button,a")?.focus();}
    else if(wasOpen.current&&(document.activeElement===document.body||ref.current?.contains(document.activeElement)))trigger.current?.focus();
    wasOpen.current=open;
    if(!open)return;
    const escape=(event:KeyboardEvent)=>{if(event.key==="Escape"&&!event.defaultPrevented){event.preventDefault();setOpen(false);}};
    document.addEventListener("keydown",escape);return()=>document.removeEventListener("keydown",escape);
  },[open]);
  return {open,setOpen,id,ref};
}

export function WorkspaceHeader({app,session,onSignedOut,query,onQuery,onNavigation,navigationOpen,navigationId,searchLabel,searchHint}:{app:AppId;session:BrowserSession;onSignedOut:()=>void;query:string;onQuery:(value:string)=>void;onNavigation?:()=>void;navigationOpen?:boolean;navigationId?:string;searchLabel?:string;searchHint?:string}){
  const [error,setError]=useState("");
  const search=useRef<HTMLInputElement>(null);
  useEffect(()=>{const focus=(event:KeyboardEvent)=>{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==="k"&&!event.defaultPrevented&&!document.querySelector("dialog[open],[data-editor-pane]")){event.preventDefault();search.current?.focus();search.current?.select();}};document.addEventListener("keydown",focus);return()=>document.removeEventListener("keydown",focus);},[]);
  return <><header className="topbar"><div className="topbar-brand">{onNavigation&&<button type="button" className="icon-button mobile-menu" aria-label="Open navigation" aria-expanded={navigationOpen} aria-controls={navigationId} onClick={onNavigation}><Icon name="menu"/></button>}<a href="/" className="shell-brand" aria-label="Dar home"><Brand/></a><AppSwitcher active={app}/></div><label className="global-search"><Icon name="search"/><input ref={search} onKeyDown={e=>{if(e.key==="Escape"&&query){e.preventDefault();e.stopPropagation();onQuery("");}}} aria-label={searchLabel??`Search ${app}`} placeholder={`${searchLabel??`Search ${app}`}…`} value={query} onChange={e=>onQuery(e.target.value)}/>{!query&&!searchHint&&<kbd className="search-shortcut" aria-hidden="true">{navigator.platform.includes("Mac")?"⌘ K":"Ctrl K"}</kbd>}{searchHint&&<span className="search-scope">{searchHint}</span>}{query&&<button className="icon-button" aria-label="Clear search" onClick={()=>onQuery("")}><Icon name="close"/></button>}</label><div className="account-area"><ActionMenu label="Open account menu" className="ui-account" heading={<><strong>{session.user.displayName}</strong><small>@{session.user.username}</small></>} actions={[{label:"Settings",icon:"settings",href:"/settings"},{label:"Account security",icon:"lock",href:"/settings?section=security"},...(session.user.administrator?[{label:"Administration",icon:"share" as const,href:"/settings?section=directory"}]:[]),{label:"Sign out",icon:"back",separator:true,run:()=>{void request("/api/v1/auth/logout",{method:"POST"},session.csrfToken).then(onSignedOut).catch(e=>setError(e.message));}}]}><Avatar name={session.user.displayName}/><span className="sr-only">{session.user.displayName}</span></ActionMenu></div></header>{error&&<p role="alert" className="error-banner">{error}</p>}</>;
}
