import {useEffect,useRef,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import {WorkspaceHeader} from "./shell.tsx";
import type {AppHost,AppModule} from "../sdk/client.tsx";
import type {InstalledApp} from "../shared/installed-apps.ts";
export function useInstalledApps(){
  const [items,setItems]=useState<InstalledApp[]>([]);
  useEffect(()=>{let controller:AbortController;const refresh=()=>{controller?.abort();controller=new AbortController();void request<{items:InstalledApp[]}>("/api/v1/apps",{signal:controller.signal}).then(r=>setItems(r.items)).catch(()=>{});};refresh();window.addEventListener("dar:apps",refresh);window.addEventListener("focus",refresh);return()=>{controller.abort();window.removeEventListener("dar:apps",refresh);window.removeEventListener("focus",refresh);};},[]);return items;
}
export function InstalledAppPage({id,session,onSignedOut}:{id:string;session:BrowserSession;onSignedOut:()=>void}){
  const element=useRef<HTMLDivElement>(null),[error,setError]=useState(""),[loading,setLoading]=useState(true);
  useEffect(()=>{
    const controller=new AbortController(),styles:HTMLLinkElement[]=[];let dispose:(()=>void)|undefined;
    const fail=()=>{if(!controller.signal.aborted){setError("This app could not be opened. Check its installation or try again.");setLoading(false);}};
    void (async()=>{
      const {items}=await request<{items:InstalledApp[]}>("/api/v1/apps",{signal:controller.signal}),app=items.find(a=>a.id===id);if(!app)throw Error("Unavailable app");
      await Promise.all(app.styles.map(href=>new Promise<void>((resolve,reject)=>{const link=document.createElement("link");link.rel="stylesheet";link.href=href;link.onload=()=>resolve();link.onerror=()=>reject(Error("Styles unavailable"));styles.push(link);document.head.append(link);})));
      const module=await import(/* @vite-ignore */ app.clientUrl) as AppModule;if(controller.signal.aborted)return;
      const base=`/api/v1/apps/${encodeURIComponent(id)}/userData`,options={signal:controller.signal};
      const host:AppHost={id,session,signal:controller.signal,onError:fail,data:{
        get:key=>request(`${base}/${encodeURIComponent(key)}`,options),
        put:(key,value,revision,schemaVersion=1)=>request(`${base}/${encodeURIComponent(key)}`,{...options,method:"PUT",headers:{"if-match":`"${revision}"`},body:JSON.stringify({schemaVersion,value})},session.csrfToken),
        delete:async(key,revision)=>{await request(`${base}/${encodeURIComponent(key)}`,{...options,method:"DELETE",headers:{"if-match":`"${revision}"`}},session.csrfToken);},
        list:(cursor="")=>request(`${base}?cursor=${encodeURIComponent(cursor)}`,options),
      },regional:()=>{if(!app.permissions.includes("regional:read"))return Promise.reject(Error("Permission not declared"));return request("/api/v1/pim/preferences",options);}};
      if(typeof module.mount!=="function")throw Error("Missing mount");dispose=module.mount(element.current!,host);setLoading(false);
    })().catch(fail);
    return()=>{controller.abort();try{dispose?.();}finally{styles.forEach(s=>s.remove());}};
  },[id]);
  return <>{(loading||error)&&<div className="app-shell"><WorkspaceHeader app={`app:${id}`} session={session} onSignedOut={onSignedOut} query="" onQuery={()=>{}}/><main className="startup-state">{error?<><p role="alert">{error}</p><button className="primary-button" onClick={()=>location.reload()}>Try again</button><a href="/">Return to Files</a></>:<p role="status">Opening app…</p>}</main></div>}<div ref={element} hidden={!!error}/></>;
}
