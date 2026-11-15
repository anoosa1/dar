import {useEffect} from "react";
import type {BrowserSession} from "./api.ts";
import {usePreferenceSaver,SaveProblems,sharedPreferences} from "./app-settings.tsx";
export type Appearance="system"|"latte"|"mocha";
export function useAppearance(session:BrowserSession|null){
  useEffect(()=>{const media=matchMedia("(prefers-color-scheme: dark)");let value:Appearance="system";
    const apply=()=>{document.documentElement.dataset.theme=value==="system"?(media.matches?"mocha":"latte"):value;};
    if(session)try{value=(localStorage.getItem(`dar.appearance.${session.user.id}`) as Appearance)||"system";}catch{}apply();
    // The shared preferences request also serves the open app, so the page asks once.
    const update=()=>{const p=sharedPreferences.read().value;if(!session||!p)return;value=p.appearance??"system";try{localStorage.setItem(`dar.appearance.${session.user.id}`,value);}catch{}apply();};
    const stop=session?sharedPreferences.subscribe(update):undefined;update();media.addEventListener("change",apply);
    return()=>{stop?.();media.removeEventListener("change",apply);};
  },[session?.user.id]);
}
export function AppearanceSettings({session}:{session:BrowserSession}){
  const saver=usePreferenceSaver(session),preferences=saver.preferences,value=preferences?.appearance??"system",state=saver.states.appearance;
  return <><h1>Appearance</h1><SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>{!preferences?<p role="status">Loading appearance…</p>:<><fieldset className="appearance-options"><legend>Color theme</legend>{([["system","Follow system"],["latte","Light — Twilight Afterglow"],["mocha","Dark — Twilight Nightfall"]] as const).map(([id,label])=><label key={id} className={`appearance-option${value===id?" is-checked":""}`}><input type="radio" name="appearance" value={id} checked={value===id} onChange={()=>saver.change("appearance",id,"Appearance saved",false)}/><span>{label}</span><span className={`theme-swatch swatch-${id}`} aria-hidden="true"/></label>)}</fieldset><p className="field-hint">Applies to every app and saves for your account as you choose.</p><p role="status" className="settings-inline-status">{state==="saving"?"Saving…":state==="saved"?"Appearance saved":""}</p></>}</>;
}
