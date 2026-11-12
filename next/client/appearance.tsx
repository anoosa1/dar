import {useEffect,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import {usePreferenceSaver,SaveProblems} from "./app-settings.tsx";
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
  const saver=usePreferenceSaver(session),preferences=saver.preferences,value=preferences?.appearance??"system",state=saver.states.appearance;
  return <><h1>Appearance</h1><SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>{!preferences?<p role="status">Loading appearance…</p>:<><fieldset className="appearance-options"><legend>Color theme</legend>{([["system","Follow system"],["latte","Light — Twilight Afterglow"],["mocha","Dark — Twilight Nightfall"]] as const).map(([id,label])=><label key={id} className="appearance-option"><input type="radio" name="appearance" value={id} checked={value===id} onChange={()=>saver.change("appearance",id,"Appearance saved",false)}/><span>{label}</span><span className={`theme-swatch swatch-${id}`} aria-hidden="true"/></label>)}</fieldset><p className="field-hint">Applies to every app and saves for your account as you choose.</p><p role="status" className="settings-inline-status">{state==="saving"?"Saving…":state==="saved"?"Appearance saved":""}</p></>}</>;
}
