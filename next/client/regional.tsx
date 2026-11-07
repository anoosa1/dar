import {useEffect,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import {savePreferences} from "./app-settings.tsx";
import {Select} from "./controls.tsx";
import type {Preferences} from "../shared/preferences.ts";

export function RegionalSettings({session}:{session:BrowserSession}){
  const [preferences,setPreferences]=useState<Preferences>(),[zone,setZone]=useState("UTC"),[error,setError]=useState(""),[busy,setBusy]=useState(false),[saved,setSaved]=useState(false);
  useEffect(()=>{const abort=new AbortController();void request<Preferences>("/api/v1/pim/preferences",{signal:abort.signal}).then(p=>{setPreferences(p);setZone(p.timezone??"UTC");}).catch(e=>{if(!abort.signal.aborted)setError(e.message);});return()=>abort.abort();},[]);
  const zones=[...new Set(["UTC",zone,...Intl.supportedValuesOf("timeZone")])].sort();
  return <><h1>Regional settings</h1>{error&&<p className="error" role="alert">{error}</p>}{!preferences?<p role="status">Loading regional settings…</p>:<form className="ui-profile-form" onSubmit={event=>{event.preventDefault();setBusy(true);setError("");void savePreferences({...preferences,timezone:zone},session).then(p=>{setPreferences(p);setSaved(true);}).catch(e=>setError(e.message)).finally(()=>setBusy(false));}}><label>Timezone<Select aria-label="Timezone" value={zone} onChange={event=>{setZone(event.target.value);setSaved(false);}}>{zones.map(value=><option key={value} value={value}>{value.replaceAll("_"," ")}</option>)}</Select></label><p className="field-hint">Used for calendar display, task dates and your reminders.</p><button className="primary-button" disabled={busy}>{busy?"Saving…":"Save regional settings"}</button>{saved&&<p role="status">Regional settings saved</p>}</form>}</>;
}
