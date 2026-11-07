import {useEffect,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import {savePreferences} from "./app-settings.tsx";
import type {Preferences} from "../shared/preferences.ts";
export type Appearance="system"|"latte"|"mocha";
export function useAppearance(session:BrowserSession|null){
  useEffect(()=>{const media=matchMedia("(prefers-color-scheme: dark)"),controller=new AbortController();let value:Appearance="system",active=true;
    const apply=()=>{document.documentElement.dataset.theme=value==="system"?(media.matches?"mocha":"latte"):value;};
    if(session)try{value=(localStorage.getItem(`dar.appearance.${session.user.id}`) as Appearance)||"system";}catch{}apply();
    const load=()=>{if(session)void request<Preferences>("/api/v1/pim/preferences",{signal:controller.signal}).then(p=>{if(active){value=p.appearance??"system";try{localStorage.setItem(`dar.appearance.${session.user.id}`,value);}catch{}apply();}}).catch(()=>{});};load();media.addEventListener("change",apply);window.addEventListener("dar:preferences",load);window.addEventListener("focus",load);
    return()=>{active=false;controller.abort();media.removeEventListener("change",apply);window.removeEventListener("dar:preferences",load);window.removeEventListener("focus",load);};
  },[session?.user.id]);
}
export function AppearanceSettings({session}:{session:BrowserSession}){
  const [preferences,setPreferences]=useState<Preferences>(),[value,setValue]=useState<Appearance>("system"),[error,setError]=useState(""),[busy,setBusy]=useState(false),[saved,setSaved]=useState(false);
  useEffect(()=>{const abort=new AbortController();void request<Preferences>("/api/v1/pim/preferences",{signal:abort.signal}).then(p=>{setPreferences(p);setValue(p.appearance??"system");}).catch(e=>{if(!abort.signal.aborted)setError(e.message);});return()=>abort.abort();},[]);
  return <><h1>Appearance</h1>{error&&<p className="error" role="alert">{error}</p>}{!preferences?<p role="status">Loading appearance…</p>:<form onSubmit={event=>{event.preventDefault();setBusy(true);setError("");void savePreferences({...preferences,appearance:value},session).then(p=>{setPreferences(p);setSaved(true);}).catch(e=>setError(e.message)).finally(()=>setBusy(false));}}><fieldset className="appearance-options"><legend>Color theme</legend>{([['system','Follow system'],['latte','Light — Catppuccin Latte'],['mocha','Dark — Catppuccin Mocha']] as const).map(([id,label])=><label key={id} className="appearance-option"><input type="radio" name="appearance" value={id} checked={value===id} onChange={()=>{setValue(id);setSaved(false);}}/><span>{label}</span><span className={`theme-swatch swatch-${id}`} aria-hidden="true"/></label>)}</fieldset><p className="field-hint">Applies to every app and is saved for your account.</p><button className="primary-button" disabled={busy}>{busy?"Saving…":"Save appearance"}</button>{saved&&<p role="status">Appearance saved</p>}</form>}</>;
}
