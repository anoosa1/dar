import {ActionMenu} from "./controls.tsx";
import {Icon,type IconName} from "./icons.tsx";
import {useInstalledApps} from "./installed-apps.tsx";
export type AppId="files"|"contacts"|"calendar"|"notes"|"tasks"|"settings"|`app:${string}`;
const apps:{id:AppId;label:string;icon:IconName;href:string}[]=[{id:"files",label:"Files",icon:"folder",href:"/"},{id:"contacts",label:"Contacts",icon:"share",href:"/contacts"},{id:"calendar",label:"Calendar",icon:"clock",href:"/calendar"},{id:"notes",label:"Notes",icon:"file",href:"/notes"},{id:"tasks",label:"Tasks",icon:"check",href:"/tasks"}];
export function AppSwitcher({active}:{active:AppId}){const installed=useInstalledApps(),all=[...apps,...installed.map(app=>({id:`app:${app.id}` as AppId,label:app.name,icon:"clock" as IconName,href:`/apps/${app.id}`}))];return <ActionMenu label="Open applications" className="ui-app-launcher" actions={all.map(app=>({label:app.label,href:app.href,icon:app.icon,current:app.id===active}))}><Icon name="grid"/><span>{all.find(a=>a.id===active)?.label??"Settings"}</span><Icon name="chevron"/></ActionMenu>;}
