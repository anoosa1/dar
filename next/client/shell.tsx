import {useState} from "react";
import {Brand} from "./auth.tsx";
import {AppSwitcher,type AppId} from "./apps.tsx";
import {ActionMenu} from "./controls.tsx";
import {Icon} from "./icons.tsx";
import {request,type BrowserSession} from "./api.ts";

export function Avatar({name}:{name:string}){return <span className="avatar" aria-hidden="true">{name.split(/\s+/).filter(Boolean).map(word=>word[0]).join("").slice(0,2).toUpperCase()}</span>;}

export function WorkspaceHeader({app,session,onSignedOut,query,onQuery,onNavigation,searchLabel,searchHint}:{app:AppId;session:BrowserSession;onSignedOut:()=>void;query:string;onQuery:(value:string)=>void;onNavigation?:()=>void;searchLabel?:string;searchHint?:string}){
  const [error,setError]=useState("");
  return <><header className="topbar"><div className="topbar-brand">{onNavigation&&<button type="button" className="icon-button mobile-menu" aria-label="Open navigation" onClick={onNavigation}><Icon name="menu"/></button>}<a href="/" className="shell-brand" aria-label="Dar home"><Brand/></a><AppSwitcher active={app}/></div><label className="global-search"><Icon name="search"/><input aria-label={searchLabel??`Search ${app}`} placeholder={`Search ${app}…`} value={query} onChange={e=>onQuery(e.target.value)}/>{searchHint&&<span className="search-scope">{searchHint}</span>}{query&&<button className="icon-button" aria-label="Clear search" onClick={()=>onQuery("")}><Icon name="close"/></button>}</label><div className="account-area"><ActionMenu label="Open account menu" className="ui-account" heading={<><strong>{session.user.displayName}</strong><small>@{session.user.username}</small></>} actions={[{label:"Settings",icon:"settings",href:"/settings"},{label:"Account security",icon:"lock",href:"/settings?section=security"},...(session.user.administrator?[{label:"Administration",icon:"share" as const,href:"/settings?section=directory"}]:[]),{label:"Sign out",icon:"back",separator:true,run:()=>{void request("/api/v1/auth/logout",{method:"POST"},session.csrfToken).then(onSignedOut).catch(e=>setError(e.message));}}]}><Avatar name={session.user.displayName}/><span className="sr-only">{session.user.displayName}</span></ActionMenu></div></header>{error&&<p role="alert" className="error-banner">{error}</p>}</>;
}
