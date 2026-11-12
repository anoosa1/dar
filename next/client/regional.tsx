import type {BrowserSession} from "./api.ts";
import {usePreferenceSaver,SaveProblems} from "./app-settings.tsx";
import {Select} from "./controls.tsx";

export function RegionalSettings({session}:{session:BrowserSession}){
  const saver=usePreferenceSaver(session),preferences=saver.preferences,zone=preferences?.timezone??"UTC",state=saver.states.timezone;
  const zones=[...new Set(["UTC",zone,...Intl.supportedValuesOf("timeZone")])].sort();
  return <><h1>Regional settings</h1><SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>{!preferences?<p role="status">Loading regional settings…</p>:<div className="ui-profile-form"><label>Timezone<Select aria-label="Timezone" value={zone} onChange={event=>saver.change("timezone",event.target.value,"Regional settings saved",false)}>{zones.map(value=><option key={value} value={value}>{value.replaceAll("_"," ")}</option>)}</Select></label><p className="field-hint">Used for calendar display, task dates and your reminders. Saves as you choose.</p><p role="status" className="settings-inline-status">{state==="saving"?"Saving…":state==="saved"?"Regional settings saved":""}</p></div>}</>;
}
