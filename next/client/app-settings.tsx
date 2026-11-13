import {CollectionTransfers} from "./collection-transfers.tsx";
import {useEffect,useRef,useState} from "react";
import {Select} from "./controls.tsx";
import {Icon,type IconName} from "./icons.tsx";
import {request,RequestError,type BrowserSession} from "./api.ts";
import {SettingsSheet,SettingRow,SettingSwitch,Segmented,type SaveState,type SettingsNotice} from "./settings-sheet.tsx";
import type {Preferences} from "../shared/preferences.ts";
import type {AppId} from "./apps.tsx";
export type {Preferences};
const endpoint="/api/v1/pim/preferences";
export function usePreferences(){const [value,setValue]=useState<Preferences>({revision:0}),[loaded,setLoaded]=useState(false),[error,setError]=useState("");useEffect(()=>{let active=true,controller=new AbortController();const load=()=>{controller.abort();controller=new AbortController();void request<Preferences>(endpoint,{signal:controller.signal}).then(p=>{if(active){setValue(p);setLoaded(true);setError("");}}).catch(e=>{if(active&&e.name!=="AbortError")setError(e.message);});};load();window.addEventListener("dar:preferences",load);window.addEventListener("focus",load);return()=>{active=false;controller.abort();window.removeEventListener("dar:preferences",load);window.removeEventListener("focus",load);};},[]);return {preferences:value,error,loaded};}
export async function savePreferences(value:Preferences,session:BrowserSession){const result=await request<Preferences>(endpoint,{method:"PUT",headers:{"if-match":`"preferences:${value.revision}"`},body:JSON.stringify(value)},session.csrfToken);window.dispatchEvent(new Event("dar:preferences"));return result;}
export function AppSettingsButton({app,onClick}:{app:AppId;onClick:()=>void}){return <button type="button" className="nav-item ui-app-settings-entry" onClick={onClick}><Icon name="settings"/>{app[0].toUpperCase()+app.slice(1)} settings</button>;}

type Key=Exclude<keyof Preferences,"revision">;
const same=(a:unknown,b:unknown)=>JSON.stringify(a)===JSON.stringify(b);
/**
 * Saves one preference at a time, in order, with the account revision. The server
 * merges partial updates, so a newer revision only conflicts when the same value
 * changed elsewhere; otherwise the save is retried on the newer revision.
 */
export function usePreferenceSaver(session:BrowserSession){
  const [preferences,setPreferences]=useState<Preferences>(),[states,setStates]=useState<Partial<Record<Key,SaveState>>>({}),[error,setError]=useState(""),[conflict,setConflict]=useState(false),[notice,setNotice]=useState<SettingsNotice|null>(null);
  const saved=useRef<Preferences|undefined>(undefined),queue=useRef(Promise.resolve()),alive=useRef(true);
  async function load(signal?:AbortSignal){const value=await request<Preferences>(endpoint,{signal});saved.current=value;if(alive.current){setPreferences(value);setConflict(false);setError("");setStates({});}}
  useEffect(()=>{alive.current=true;const controller=new AbortController();void load(controller.signal).catch(e=>{if(!controller.signal.aborted)setError(e.message);});return()=>{alive.current=false;controller.abort();};},[]);
  function change<K extends Key>(key:K,value:Preferences[K],message:string,undoable=true){
    const before=saved.current?.[key];if(!saved.current||same(before,value))return;
    setPreferences(current=>current&&{...current,[key]:value});setStates(current=>({...current,[key]:"saving"}));setError("");setNotice(null);
    queue.current=queue.current.then(async()=>{
      const put=(revision:number)=>savePreferences({revision,[key]:value} as Preferences,session);
      try{
        let next:Preferences;
        try{next=await put(saved.current!.revision);}
        catch(cause){
          if(!(cause instanceof RequestError)||cause.status!==412)throw cause;
          const latest=await request<Preferences>(endpoint);
          if(!same(latest[key],before)){saved.current=latest;throw cause;}
          next=await put(latest.revision);
        }
        saved.current=next;if(!alive.current)return;
        setPreferences(current=>current&&{...current,revision:next.revision});setStates(current=>({...current,[key]:"saved"}));
        setNotice({message,undo:undoable?()=>change(key,before as Preferences[K],"Change undone",false):undefined});
      }catch(cause){
        if(!alive.current)return;
        setPreferences(current=>current&&{...current,[key]:saved.current?.[key]});setStates(current=>({...current,[key]:"error"}));
        if(cause instanceof RequestError&&cause.status===412)setConflict(true);else setError((cause as Error).message);
      }
    });
  }
  return {preferences,states,error,conflict,notice,setNotice,change,reload:()=>load().catch(e=>setError(e.message))};
}

/** Conflict and error banners shared by every settings sheet. */
export function SaveProblems({error,conflict,onReload}:{error:string;conflict:boolean;onReload:()=>void}){
  return <>{error&&<p role="alert" className="error settings-problem">{error}</p>}{conflict&&<div role="alert" className="settings-problem settings-conflict"><p><strong>Changed in another tab</strong>This setting was saved elsewhere. Reload to see it before changing it again.</p><button type="button" className="text-button" onClick={onReload}>Reload settings</button></div>}</>;
}

const appIcons:Record<string,IconName>={files:"folder",contacts:"contacts",calendar:"calendar",notes:"file",tasks:"check"};
const sorting={modified:"Recently edited",title:"Title",due:"Due date"} as const;
type SettingsApp=Exclude<AppId,"settings"|"agent"|"news"|"photos"|`app:${string}`>;
export function AppSettings({app,session,onClose,onChanged}:{app:SettingsApp;session:BrowserSession;onClose:()=>void;onChanged?:()=>void}){
  const saver=usePreferenceSaver(session),[section,setSection]=useState("General"),p=saver.preferences,state=saver.states;
  const name=app[0].toUpperCase()+app.slice(1),sections=["General",...(app!=="files"?["Import and export"]:[]),"Keyboard shortcuts"];
  function set<K extends Key>(key:K,value:Preferences[K],message:string){saver.change(key,value,message);onChanged?.();}
  return <SettingsSheet title={`${name} settings`} icon={appIcons[app]} sections={sections} section={section} onSection={setSection} onClose={onClose} notice={saver.notice} onNotice={saver.setNotice}>
    <SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>
    {section==="Import and export"?<CollectionTransfers session={session} app={app==="files"?"calendar":app}/>:section==="Keyboard shortcuts"?<dl className="ui-shortcuts"><dt>Ctrl / Cmd K</dt><dd>Search all apps outside editors and dialogs</dd><dt>Tab / Shift Tab</dt><dd>Move between controls</dd><dt>Arrow keys</dt><dd>Move through menus and choices</dd><dt>Enter / Space</dt><dd>Activate the focused control</dd><dt>Escape</dt><dd>Close menus, then settings</dd>{(app==="notes"||app==="files")&&<><dt>Ctrl / Cmd S</dt><dd>Save the active editor</dd></>}{app==="files"&&<><dt>Ctrl / Cmd C, X, V</dt><dd>Copy, cut and paste the selected file</dd><dt>F2</dt><dd>Rename the selected file</dd></>}</dl>:!p?<p role="status">Loading settings…</p>:<>
      {app==="files"&&<><section className="settings-group" aria-label="Browsing"><h3>Browsing</h3>
        <SettingRow label="Default view" hint="How folders open" state={state.filesView}><Segmented label="Default view" value={p.filesView??"list"} options={[["list","List"],["grid","Grid"]] as const} onChange={value=>set("filesView",value,`Default view set to ${value==="grid"?"Grid":"List"}`)}/></SettingRow>
        <SettingSwitch label="Show hidden files" hint="Names starting with a dot" state={state.filesShowHidden} checked={p.filesShowHidden??false} onChange={value=>set("filesShowHidden",value,value?"Hidden files shown":"Hidden files hidden")}/></section>
        <section className="settings-group" aria-label="Safety"><h3>Safety</h3><SettingSwitch label="Confirm before moving files to trash" hint="Deleted files can always be restored" state={state.filesConfirmTrash} checked={p.filesConfirmTrash??false} onChange={value=>set("filesConfirmTrash",value,value?"Trash confirmation on":"Trash confirmation off")}/></section>
        <section className="settings-group" aria-label="Details"><h3>Details</h3><SettingRow label="Default details tab" hint="Opens first when you choose Details" state={state.filesDetailsTab} stacked><Segmented label="Default details tab" value={p.filesDetailsTab??"details"} options={[["details","Details"],["versions","Versions"],["sharing","Sharing"]] as const} onChange={value=>set("filesDetailsTab",value,`Details open on ${value[0].toUpperCase()+value.slice(1)}`)}/></SettingRow></section></>}
      {app==="contacts"&&<section className="settings-group" aria-label="Display"><h3>Display</h3><SettingRow label="Sort contacts by" hint="Order of the contact list" state={state.contactSort}><Segmented label="Sort contacts by" value={p.contactSort??"firstName"} options={[["firstName","First name"],["lastName","Last name"]] as const} onChange={value=>set("contactSort",value,`Contacts sorted by ${value==="lastName"?"last":"first"} name`)}/></SettingRow></section>}
      {app==="calendar"&&<><section className="settings-group" aria-label="Display"><h3>Display</h3>
        <SettingRow label="Week begins" hint="Month and Week views and the date picker" state={state.weekStart}><Segmented label="Week begins" value={p.weekStart??1} options={[[1,"Monday"],[0,"Sunday"]] as const} onChange={value=>set("weekStart",value,`Week begins on ${value?"Monday":"Sunday"}`)}/></SettingRow>
        <SettingSwitch label="Show cancelled events" hint="Shown struck through instead of hidden" state={state.calendarShowCancelled} checked={p.calendarShowCancelled??false} onChange={value=>set("calendarShowCancelled",value,value?"Cancelled events shown":"Cancelled events hidden")}/></section>
        <section className="settings-group" aria-label="Time"><h3>Time</h3><SettingRow label="Timezone" hint={`${p.timezone??"UTC"} · events with their own timezone keep it`}><a className="secondary-button" href="/settings?section=regional">Regional settings</a></SettingRow></section></>}
      {(app==="notes"||app==="tasks")&&<section className="settings-group" aria-label="Lists"><h3>Lists</h3>
        <SettingRow label="Default sorting" hint={`Order of the ${app} list`} state={state[app==="notes"?"notesSort":"tasksSort"]} stacked={app==="tasks"}><Segmented label="Default sorting" value={(app==="notes"?p.notesSort:p.tasksSort)??(app==="notes"?"modified":"due")} options={(app==="notes"?["modified","title"]:["modified","title","due"]).map(value=>[value,sorting[value as keyof typeof sorting]] as const)} onChange={value=>set(app==="notes"?"notesSort":"tasksSort",value,`Sorted by ${sorting[value as keyof typeof sorting].toLowerCase()}`)}/></SettingRow>
        {app==="notes"&&<SettingRow label="Open notes in" hint="Editing or reading view" state={state.notesEditorMode}><Segmented label="Open notes in" value={p.notesEditorMode??"write"} options={[["write","Write"],["preview","Preview"]] as const} onChange={value=>set("notesEditorMode",value,`Notes open in ${value==="write"?"Write":"Preview"}`)}/></SettingRow>}
        {app==="tasks"&&<SettingRow label="Default view" hint="The list Tasks opens on" state={state.tasksView}><Select aria-label="Default view" value={p.tasksView??"today"} onChange={event=>{const value=event.target.value as NonNullable<Preferences["tasksView"]>;set("tasksView",value,`Tasks opens on ${value[0].toUpperCase()+value.slice(1)}`);}}>{["all","inbox","today","upcoming","important","completed"].map(value=><option key={value} value={value}>{value[0].toUpperCase()+value.slice(1)}</option>)}</Select></SettingRow>}</section>}
    </>}
  </SettingsSheet>;
}
