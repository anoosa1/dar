import {useEffect,useId,useRef,useState} from "react";
import {createPortal} from "react-dom";
import {Icon,type IconName} from "./icons.tsx";
import {useInstalledApps} from "./installed-apps.tsx";
import {useAppSettings} from "./app-availability.tsx";
export type AppId="files"|"photos"|"books"|"contacts"|"calendar"|"notes"|"tasks"|"news"|"chat"|"agent"|"git"|"settings"|`app:${string}`;
export interface AppEntry {id:AppId;label:string;icon:IconName;href:string}
const apps:AppEntry[]=[{id:"files",label:"Files",icon:"folder",href:"/"},{id:"photos",label:"Photos",icon:"image",href:"/photos"},{id:"books",label:"Books",icon:"book",href:"/books"},{id:"contacts",label:"Contacts",icon:"contacts",href:"/contacts"},{id:"calendar",label:"Calendar",icon:"calendar",href:"/calendar"},{id:"notes",label:"Notes",icon:"file",href:"/notes"},{id:"tasks",label:"Tasks",icon:"check",href:"/tasks"},{id:"news",label:"News",icon:"rss",href:"/news"},{id:"chat",label:"Chat",icon:"chat",href:"/chat"},{id:"agent",label:"Agent",icon:"agent",href:"/agent"},{id:"git",label:"Git",icon:"branch",href:"/git"}];
const tints:Record<string,string>={files:"app-tint-files",photos:"app-tint-photos",books:"app-tint-books",contacts:"app-tint-contacts",calendar:"app-tint-calendar",notes:"app-tint-notes",tasks:"app-tint-tasks",news:"app-tint-news",chat:"app-tint-chat",agent:"app-tint-agent",git:"app-tint-git"};
/** Built-in apps the administrator has not disabled, followed by enabled installed apps. */
export function useAppList(){
  const installed=useInstalledApps(),{settings}=useAppSettings();
  return [...apps.filter(app=>!settings?.disabled.includes(app.id)),...installed.map(app=>({id:`app:${app.id}` as AppId,label:app.name,icon:"clock" as IconName,href:`/apps/${app.id}`}))];
}
export function appLabel(id:AppId,list:AppEntry[]){return id==="settings"?"Settings":list.find(app=>app.id===id)?.label??apps.find(app=>app.id===id)?.label??"App";}

/** Compact 2-column horizontal app rows (Direction A), shared across the desktop All apps popover and mobile Apps & account hub. */
export function AppGroupsList({active,onPick}:{active:AppId;onPick?:()=>void}){
  const list=useAppList();
  return <div className="apps-menu-groups">
    <section className="apps-menu-group">
      <div className="apps-menu-group-title">Apps</div>
      <div className="apps-menu-grid">
        {list.map(app=>{
          const current=app.id===active;
          const tint=tints[app.id]??"app-tint-ext";
          return <a key={app.id} href={app.href} className={`apps-menu-row${current?" is-active":""}`} aria-current={current?"page":undefined} onClick={onPick}>
            <span className={`apps-menu-badge ${tint}`}><Icon name={app.icon}/></span>
            <span className="apps-menu-label">{app.label}</span>
            {current&&<span className="apps-menu-dot" aria-hidden="true"/>}
          </a>;
        })}
      </div>
    </section>
  </div>;
}

/** Desktop 9-dot All apps trigger and frosted Direction A popover. */
export function AllAppsSwitcher({active,administrator=false}:{active:AppId;administrator?:boolean}){
  const [open,setOpen]=useState(false);
  const trigger=useRef<HTMLButtonElement>(null);
  const panel=useRef<HTMLDivElement>(null);
  const id=useId();
  useEffect(()=>{
    if(!open)return;
    const place=()=>{
      const t=trigger.current?.getBoundingClientRect(),p=panel.current;
      if(!t||!p)return;
      p.style.position="fixed";
      p.style.top=`${Math.round(t.bottom+6)}px`;
      p.style.left=`${Math.max(10,Math.round(t.left))}px`;
    };
    place();
    const down=(e:PointerEvent)=>{
      const target=e.target as Node|null;
      if(!trigger.current?.contains(target)&&!panel.current?.contains(target))setOpen(false);
    };
    const key=(e:KeyboardEvent)=>{
      if(e.key==="Escape"){e.preventDefault();setOpen(false);trigger.current?.focus();}
    };
    document.addEventListener("pointerdown",down,true);
    document.addEventListener("keydown",key);
    window.addEventListener("resize",place);
    return()=>{document.removeEventListener("pointerdown",down,true);document.removeEventListener("keydown",key);window.removeEventListener("resize",place);};
  },[open]);
  return <div className="all-apps-wrap">
    <button ref={trigger} type="button" className="icon-button all-apps-trigger" aria-label="All apps" title="All apps" aria-haspopup="dialog" aria-expanded={open} aria-controls={open?id:undefined} onClick={()=>setOpen(v=>!v)}>
      <Icon name="apps"/>
    </button>
    {open&&createPortal(
      <div id={id} ref={panel} role="dialog" aria-label="All apps" className="dar-popover-surface apps-menu-popover">
        <AppGroupsList active={active} onPick={()=>setOpen(false)}/>
        <div className="apps-menu-footer">
          <span className="apps-menu-hint">Jump with Ctrl K</span>
          <div className="apps-menu-footer-links">
            <a href="/settings?section=appearance" onClick={()=>setOpen(false)}>Customize</a>
            {administrator&&<a href="/settings?section=installedApps" onClick={()=>setOpen(false)}>Manage apps</a>}
          </div>
        </div>
      </div>,
      document.body
    )}
  </div>;
}
