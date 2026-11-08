import {useEffect,useState} from "react";
import {request} from "./api.ts";
import type {AppSettings} from "../shared/app-settings.ts";
export function useAppSettings(){
  const [settings,setSettings]=useState<AppSettings>(),[error,setError]=useState("");
  useEffect(()=>{
    let controller:AbortController;
    const refresh=()=>{controller?.abort();controller=new AbortController();const signal=controller.signal;void request<AppSettings>("/api/v1/appSettings",{signal}).then(value=>{setSettings(value);setError("");}).catch(e=>{if(!signal.aborted)setError(e.message);});};
    refresh();window.addEventListener("dar:apps",refresh);window.addEventListener("focus",refresh);
    return()=>{controller.abort();window.removeEventListener("dar:apps",refresh);window.removeEventListener("focus",refresh);};
  },[]);
  return {settings,error};
}
