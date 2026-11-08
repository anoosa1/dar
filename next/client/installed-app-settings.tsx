import {useEffect,useState} from "react";
import {request} from "./api.ts";
export function InstalledAppsSettings(){
  const [items,setItems]=useState<{id:string;name:string;version:string;status:string;permissions:string[]}[]>(),[error,setError]=useState("");
  useEffect(()=>{const controller=new AbortController();void request<{items:NonNullable<typeof items>}>("/api/v1/admin/installedApps",{signal:controller.signal}).then(r=>setItems(r.items)).catch(e=>{if(!controller.signal.aborted)setError(e.message);});return()=>controller.abort();},[]);
  return <><h1>Installed apps</h1><p className="muted">Trusted code configured by the host administrator. Installation, updates and disabling take effect after a service restart.</p>{error&&<p className="error" role="alert">{error}</p>}{!items?<p role="status">Loading installed apps…</p>:!items.length?<p className="identity-empty">No external apps are installed.</p>:items.map(item=><section className="identity-card" key={item.id}><h2>{item.name}</h2><p>{item.id} · {item.version}</p><p>{item.status}</p><p className="muted small">{item.permissions.join(" · ")||"No SDK data permissions"}</p></section>)}</>;
}
