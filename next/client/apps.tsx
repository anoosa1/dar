import {ActionMenu} from "./controls.tsx";
import {Icon,type IconName} from "./icons.tsx";
import {useInstalledApps} from "./installed-apps.tsx";
import {useAppSettings} from "./app-availability.tsx";
import type {MenuAction} from "./controls.tsx";
export type AppId="files"|"contacts"|"calendar"|"notes"|"tasks"|"settings"|`app:${string}`;
const apps:{id:AppId;label:string;icon:IconName;href:string}[]=[{id:"files",label:"Files",icon:"folder",href:"/"},{id:"contacts",label:"Contacts",icon:"contacts",href:"/contacts"},{id:"calendar",label:"Calendar",icon:"calendar",href:"/calendar"},{id:"notes",label:"Notes",icon:"file",href:"/notes"},{id:"tasks",label:"Tasks",icon:"check",href:"/tasks"}];
export function AppSwitcher({active,administrator=false}:{active:AppId;administrator?:boolean}){
  const installed=useInstalledApps(),{settings}=useAppSettings(),all=[...apps.filter(app=>!settings?.disabled.includes(app.id)),...installed.map(app=>({id:`app:${app.id}` as AppId,label:app.name,icon:"clock" as IconName,href:`/apps/${app.id}`}))];
  const actions:MenuAction[]=all.map(app=>({label:app.label,href:app.href,icon:app.icon,current:app.id===active}));
  if(administrator)actions.push({label:"Manage apps",icon:"settings",href:"/settings?section=installedApps",separator:true});
  return <ActionMenu label="Open applications" className="ui-app-launcher" heading={<strong>Applications</strong>} actions={actions}><Icon name="grid"/><span>{all.find(a=>a.id===active)?.label??"Settings"}</span><Icon name="chevron"/></ActionMenu>;
}
