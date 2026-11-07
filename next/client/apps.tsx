import {ActionMenu} from "./controls.tsx";
import {Icon,type IconName} from "./icons.tsx";
export type AppId="files"|"contacts"|"calendar"|"notes"|"tasks"|"settings";
const apps:{id:AppId;label:string;icon:IconName;href:string}[]=[{id:"files",label:"Files",icon:"folder",href:"/"},{id:"contacts",label:"Contacts",icon:"share",href:"/contacts"},{id:"calendar",label:"Calendar",icon:"clock",href:"/calendar"},{id:"notes",label:"Notes",icon:"file",href:"/notes"},{id:"tasks",label:"Tasks",icon:"check",href:"/tasks"}];
export function AppSwitcher({active}:{active:AppId}){return <ActionMenu label="Open applications" className="ui-app-launcher" actions={apps.map(app=>({label:app.label,href:app.href,icon:app.icon,current:app.id===active}))}><Icon name="grid"/><span>{apps.find(a=>a.id===active)?.label??"Settings"}</span><Icon name="chevron"/></ActionMenu>;}
