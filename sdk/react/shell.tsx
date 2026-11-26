import {useEffect,useId,useRef,useState} from "react";
import {createPortal} from "react-dom";
import {AllAppsSwitcher,AppGroupsList,appLabel,useAppList,type AppId} from "./apps.tsx";
import {Icon} from "./icons.tsx";
import {request,type BrowserSession} from "./api.ts";
import {GlobalSearch} from "./global-search.tsx";
import "./global-search.css";

export function Avatar({name}:{name:string}){return <span className="avatar" aria-hidden="true">{name.split(/\s+/).filter(Boolean).map(word=>word[0]).join("").slice(0,2).toUpperCase()}</span>;}

/** Account trigger: frosted User Menu popover on desktop, full-screen Apps & account hub on phones. */
export function WorkspaceAccount({session,active="files",onSignedOut,className=""}:{session:BrowserSession;active?:AppId;onSignedOut:()=>void;className?:string}){
  const [openDesktop,setOpenDesktop]=useState(false);
  const [openMobile,setOpenMobile]=useState(false);
  const [error,setError]=useState("");
  const trigger=useRef<HTMLButtonElement>(null);
  const panel=useRef<HTMLDivElement>(null);
  const id=useId();

  useEffect(()=>{
    if(!openDesktop)return;
    const place=()=>{
      const t=trigger.current?.getBoundingClientRect(),p=panel.current;
      if(!t||!p)return;
      p.style.position="fixed";
      p.style.top=`${Math.round(t.bottom+6)}px`;
      p.style.right=`${Math.max(10,Math.round(window.innerWidth-t.right))}px`;
    };
    place();
    const down=(e:PointerEvent)=>{
      const target=e.target as Node|null;
      if(!trigger.current?.contains(target)&&!panel.current?.contains(target))setOpenDesktop(false);
    };
    document.addEventListener("pointerdown",down,true);
    window.addEventListener("resize",place);
    return()=>{document.removeEventListener("pointerdown",down,true);window.removeEventListener("resize",place);};
  },[openDesktop]);

  useEffect(()=>{
    if(!openDesktop&&!openMobile)return;
    const key=(e:KeyboardEvent)=>{
      if(e.key==="Escape"){e.preventDefault();setOpenDesktop(false);setOpenMobile(false);trigger.current?.focus();}
    };
    document.addEventListener("keydown",key);
    return()=>document.removeEventListener("keydown",key);
  },[openDesktop,openMobile]);

  const signOut=()=>{
    setOpenDesktop(false);setOpenMobile(false);
    void request("/api/v1/auth/logout",{method:"POST"},session.csrfToken).then(onSignedOut).catch(e=>setError(e.message));
  };

  return <div className={`account-area ${className}`}>
    <button ref={trigger} type="button" className="user-avatar-trigger" aria-label="Open account menu" aria-haspopup="menu" aria-expanded={openDesktop||openMobile} aria-controls={openDesktop?id:undefined} onClick={()=>{
      if(window.matchMedia("(max-width: 800px)").matches){
        document.body.classList.remove("mobile-nav-open");
        setOpenMobile(v=>!v);
      }else{
        setOpenDesktop(v=>!v);
      }
    }}>
      <Avatar name={session.user.displayName}/>
      <span className="sr-only">{session.user.displayName}</span>
    </button>

    {openDesktop&&createPortal(
      <div id={id} ref={panel} role="menu" aria-label="Account" className="dar-popover-surface user-menu-popover">
        <div className="user-menu-head">
          <Avatar name={session.user.displayName}/>
          <div className="user-menu-meta">
            <strong>{session.user.displayName}</strong>
            <small>@{session.user.username}</small>
          </div>
          {session.user.administrator&&<span className="user-role-pill">Admin</span>}
        </div>
        <div className="user-menu-divider"/>
        <div className="user-menu-list">
          <a href="/settings" role="menuitem" className="user-menu-item" onClick={()=>setOpenDesktop(false)}><Icon name="settings"/><span>Settings</span></a>
          <a href="/settings?section=appearance" role="menuitem" className="user-menu-item" onClick={()=>setOpenDesktop(false)}><Icon name="palette"/><span>Personalization</span></a>
          <a href="/settings?section=security" role="menuitem" className="user-menu-item" onClick={()=>setOpenDesktop(false)}><Icon name="lock"/><span>Account security</span></a>
          {session.user.administrator&&<>
            <div className="user-menu-section-label">Administration</div>
            <a href="/settings?section=directory" role="menuitem" className="user-menu-item" onClick={()=>setOpenDesktop(false)}><Icon name="shield"/><span>Directory</span></a>
            <a href="/settings?section=installedApps" role="menuitem" className="user-menu-item" onClick={()=>setOpenDesktop(false)}><Icon name="grid"/><span>Manage apps</span></a>
          </>}
          <div className="user-menu-divider"/>
          <button type="button" role="menuitem" className="user-menu-item danger" onClick={signOut}><Icon name="logout"/><span>Sign out</span></button>
        </div>
      </div>,
      document.body
    )}

    {openMobile&&createPortal(
      <div role="dialog" aria-modal="true" aria-label="Apps and account" className="mobile-user-hub">
        <header className="mobile-user-hub-top">
          <a href="/" className="mobile-user-hub-brand" onClick={()=>setOpenMobile(false)}>
            <span className="topbar-brand-mark" aria-hidden="true">D</span>
            <span>Dar</span>
          </a>
          <button type="button" className="icon-button mobile-user-hub-close" aria-label="Close account menu" onClick={()=>setOpenMobile(false)}>
            <Icon name="close"/>
          </button>
        </header>
        <div className="mobile-user-hub-apps">
          <AppGroupsList active={active} onPick={()=>setOpenMobile(false)}/>
        </div>
        <div className="mobile-user-hub-account">
          <div className="user-menu-head">
            <Avatar name={session.user.displayName}/>
            <div className="user-menu-meta">
              <strong>{session.user.displayName}</strong>
              <small>@{session.user.username}</small>
            </div>
            {session.user.administrator&&<span className="user-role-pill">Admin</span>}
          </div>
          <div className="mobile-user-hub-actions">
            <a href="/settings" className="mobile-hub-row" onClick={()=>setOpenMobile(false)}><Icon name="settings"/><span>Settings</span><Icon name="chevron"/></a>
            <a href="/settings?section=appearance" className="mobile-hub-row" onClick={()=>setOpenMobile(false)}><Icon name="palette"/><span>Personalization</span><Icon name="chevron"/></a>
            {session.user.administrator&&<a href="/settings?section=installedApps" className="mobile-hub-row" onClick={()=>setOpenMobile(false)}><Icon name="grid"/><span>Manage apps</span><Icon name="chevron"/></a>}
            <button type="button" className="mobile-hub-row danger" onClick={signOut}><Icon name="logout"/><span>Sign out</span></button>
          </div>
        </div>
      </div>,
      document.body
    )}

    {error&&<p role="alert" className="error">{error}</p>}
  </div>;
}

/** Phone navigation focuses its controls and returns to its opener on dismissal. */
export function useMobileNavigation(){
  const [open,setOpen]=useState(false),id=useId(),ref=useRef<HTMLElement>(null),trigger=useRef<HTMLElement|null>(null),wasOpen=useRef(false);
  useEffect(()=>{
    document.body.classList.toggle("mobile-nav-open",open);
    if(open){trigger.current=document.activeElement as HTMLElement;ref.current?.querySelector<HTMLElement>("button,a")?.focus();}
    else if(wasOpen.current&&(document.activeElement===document.body||ref.current?.contains(document.activeElement)))trigger.current?.focus();
    wasOpen.current=open;
    if(!open)return;
    const escape=(event:KeyboardEvent)=>{if(event.key==="Escape"&&!event.defaultPrevented){event.preventDefault();setOpen(false);}};
    document.addEventListener("keydown",escape);return()=>{document.removeEventListener("keydown",escape);};
  },[open]);
  useEffect(()=>()=>{document.body.classList.remove("mobile-nav-open");},[]);
  return {open,setOpen,id,ref};
}

/**
 * Shared full-canvas top bar for installed apps: Dar logo, All apps switcher, app title,
 * centered cross-app search, app settings, and user avatar (desktop popover / mobile hub).
 */
export function WorkspaceHeader({app,session,onSignedOut,onSettings,onNavigation,navigationOpen}:{app:AppId;session:BrowserSession;onSignedOut:()=>void;query?:string;onQuery?:(value:string)=>void;onNavigation?:()=>void;navigationOpen?:boolean;navigationId?:string;searchLabel?:string;searchHint?:string;onSettings?:()=>void}){
  const list=useAppList(),label=appLabel(app,list);
  return <header className={`topbar${onSettings?" has-settings":""}`}>
    {onNavigation&&<button type="button" className="icon-button mobile-nav-trigger" aria-label="Open navigation" aria-expanded={!!navigationOpen} onClick={onNavigation}><Icon name="menu"/></button>}
    <div className="topbar-start">
      <a href="/" className="topbar-brand" aria-label="Dar home"><span className="topbar-brand-mark" aria-hidden="true">D</span></a>
      <AllAppsSwitcher active={app} administrator={session.user.administrator}/>
      <span className="topbar-title">{label}</span>
    </div>
    <GlobalSearch account={session.user.id}/>
    <div className="topbar-end">
      <div className="topbar-tools">{onSettings&&<button type="button" className="icon-button" aria-label={`${label} settings`} title={`${label} settings`} onClick={onSettings}><Icon name="settings"/></button>}</div>
      <WorkspaceAccount session={session} active={app} onSignedOut={onSignedOut} className="topbar-account"/>
    </div>
  </header>;
}
