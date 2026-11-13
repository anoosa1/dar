import {useEffect,useId,useRef,useState} from "react";
import {AppRail,appLabel,useAppList,type AppId} from "./apps.tsx";
import {ActionMenu} from "./controls.tsx";
import {Icon} from "./icons.tsx";
import {request,type BrowserSession} from "./api.ts";
import {GlobalSearch} from "./global-search.tsx";
import "./global-search.css";

export function Avatar({name}:{name:string}){return <span className="avatar" aria-hidden="true">{name.split(/\s+/).filter(Boolean).map(word=>word[0]).join("").slice(0,2).toUpperCase()}</span>;}

/** The same account actions are available from the rail and the phone search bar. */
export function WorkspaceAccount({session,onSignedOut,className=""}:{session:BrowserSession;onSignedOut:()=>void;className?:string}){
  const [error,setError]=useState("");
  return <div className={`account-area ${className}`}><ActionMenu label="Open account menu" className="ui-account" heading={<><Avatar name={session.user.displayName}/><span><strong>{session.user.displayName}</strong><small>@{session.user.username}</small></span></>} actions={[{label:"Settings",icon:"settings",href:"/settings"},{label:"Account security",icon:"lock",href:"/settings?section=security"},...(session.user.administrator?[{label:"Directory",icon:"shield" as const,href:"/settings?section=directory",section:"Administration"},{label:"Manage apps",icon:"grid" as const,href:"/settings?section=installedApps"}]:[]),{label:"Sign out",icon:"logout",separator:true,run:()=>{void request("/api/v1/auth/logout",{method:"POST"},session.csrfToken).then(onSignedOut).catch(e=>setError(e.message));}}]}><Avatar name={session.user.displayName}/><span className="sr-only">{session.user.displayName}</span></ActionMenu>{error&&<p role="alert" className="error">{error}</p>}</div>;
}

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

/**
 * Shared app chrome: the app rail (a bottom bar on phones) and a top bar with the
 * app title, centered cross-app search and app-level tools. Phone collections and
 * views are chips in each app's sidebar, so the former navigation props are unused.
 */
export function WorkspaceHeader({app,session,onSignedOut,onSettings}:{app:AppId;session:BrowserSession;onSignedOut:()=>void;query?:string;onQuery?:(value:string)=>void;onNavigation?:()=>void;navigationOpen?:boolean;navigationId?:string;searchLabel?:string;searchHint?:string;onSettings?:()=>void}){
  const list=useAppList(),label=appLabel(app,list);
  return <>
    <AppRail active={app} administrator={session.user.administrator} account={<WorkspaceAccount session={session} onSignedOut={onSignedOut} className="rail-account"/>}/>
    <header className="topbar"><span className="topbar-title">{label}</span><GlobalSearch/><div className="topbar-tools">{onSettings&&<button type="button" className="icon-button" aria-label={`${label} settings`} title={`${label} settings`} onClick={onSettings}><Icon name="settings"/></button>}</div><WorkspaceAccount session={session} onSignedOut={onSignedOut} className="topbar-account"/></header>
  </>;
}
