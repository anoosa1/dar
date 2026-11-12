import {ActionMenu} from "./controls.tsx";
import {Icon,type IconName} from "./icons.tsx";
import {useInstalledApps} from "./installed-apps.tsx";
import {useAppSettings} from "./app-availability.tsx";
import type {MenuAction} from "./controls.tsx";
import type {ReactNode} from "react";
export type AppId="files"|"contacts"|"calendar"|"notes"|"tasks"|"agent"|"settings"|`app:${string}`;
export interface AppEntry {id:AppId;label:string;icon:IconName;href:string}
const apps:AppEntry[]=[{id:"files",label:"Files",icon:"folder",href:"/"},{id:"contacts",label:"Contacts",icon:"contacts",href:"/contacts"},{id:"calendar",label:"Calendar",icon:"calendar",href:"/calendar"},{id:"notes",label:"Notes",icon:"file",href:"/notes"},{id:"tasks",label:"Tasks",icon:"check",href:"/tasks"},{id:"agent",label:"Agent",icon:"agent",href:"/agent"}];
/** Built-in apps the administrator has not disabled, followed by enabled installed apps. */
export function useAppList(){
  const installed=useInstalledApps(),{settings}=useAppSettings();
  return [...apps.filter(app=>!settings?.disabled.includes(app.id)),...installed.map(app=>({id:`app:${app.id}` as AppId,label:app.name,icon:"clock" as IconName,href:`/apps/${app.id}`}))];
}
export function appLabel(id:AppId,list:AppEntry[]){return id==="settings"?"Settings":list.find(app=>app.id===id)?.label??apps.find(app=>app.id===id)?.label??"App";}
/** Phones show this many apps in the bottom bar; the rest are under More. */
const phoneSlots=4;
/**
 * The persistent app rail (selected unified look B). Desktop shows every app with
 * settings and the account at the bottom; phones turn it into a bottom navigation
 * bar with four apps and a More menu.
 */
export function AppRail({active,administrator=false,account}:{active:AppId;administrator?:boolean;account:ReactNode}){
  // On phones the current app always keeps a bottom-bar slot, replacing the last default one.
  const list=useAppList(),activeIndex=list.findIndex(app=>app.id===active),onPhone=list.map((_,index)=>index<phoneSlots-(activeIndex>=phoneSlots?1:0)||index===activeIndex);
  const overflow=list.filter((_,index)=>!onPhone[index]),moreActive=active==="settings";
  const more:MenuAction[]=[...overflow.map(app=>({label:app.label,href:app.href,icon:app.icon,current:app.id===active})),{label:"Settings",icon:"settings",href:"/settings",current:active==="settings",separator:overflow.length>0},...(administrator?[{label:"Manage apps",icon:"grid" as IconName,href:"/settings?section=installedApps"}]:[])];
  return <nav className="app-rail" aria-label="Applications">
    <a href="/" className="app-rail-brand" aria-label="Dar home"><span aria-hidden="true">D</span></a>
    <div className="app-rail-apps">{list.map((app,index)=><a key={app.id} href={app.href} className={`app-rail-item${onPhone[index]?"":" app-rail-overflow"}`} aria-current={app.id===active?"page":undefined}><span className="app-rail-icon"><Icon name={app.icon}/></span><span className="app-rail-label">{app.label}</span></a>)}
      <ActionMenu label="More apps" className={`app-rail-more${moreActive?" is-current":""}`} actions={more}><span className="app-rail-icon"><Icon name="grid"/></span><span className="app-rail-label">More</span></ActionMenu>
    </div>
    <div className="app-rail-end"><a href="/settings" className="app-rail-item app-rail-settings" aria-label="Settings" title="Settings" aria-current={active==="settings"?"page":undefined}><span className="app-rail-icon"><Icon name="settings"/></span></a>{account}</div>
  </nav>;
}
