import {useEffect,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import type {AppSettings,ManagedApp} from "../shared/app-settings.ts";
import {Icon,type IconName} from "./icons.tsx";
type Inventory=AppSettings&{items:ManagedApp[]};
const icons:Record<string,IconName>={files:"folder",contacts:"contacts",calendar:"calendar",notes:"file",tasks:"check"};
export function InstalledAppsSettings({session}:{session:BrowserSession}){
  const [inventory,setInventory]=useState<Inventory>(),[error,setError]=useState(""),[busy,setBusy]=useState(false),[notice,setNotice]=useState("");
  async function refresh(signal?:AbortSignal){setInventory(await request<Inventory>("/api/v1/admin/installedApps",{signal}));}
  useEffect(()=>{const controller=new AbortController();void refresh(controller.signal).catch(e=>{if(!controller.signal.aborted)setError(e.message);});return()=>controller.abort();},[]);
  async function toggle(item:ManagedApp){
    if(!inventory)return;setBusy(true);setError("");setNotice("");
    try{const result=await request<Inventory>("/api/v1/admin/installedApps",{method:"PUT",headers:{"if-match":`"apps:${inventory.revision}"`},body:JSON.stringify({id:item.id,enabled:!item.enabled,revision:inventory.revision})},session.csrfToken);setInventory(result);window.dispatchEvent(new Event("dar:apps"));const updated=result.items.find(app=>app.id===item.id);setNotice(`${item.name} ${updated?.enabled?"enabled":"disabled"}.`);}
    catch(e){setError((e as Error).message);await refresh().catch(()=>{});}finally{setBusy(false);}
  }
  return <><h1>Apps</h1><p className="muted">Enable or disable apps for everyone. Saved data is retained.</p>{error&&<p className="error" role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}{!inventory?<><p role="status">Loading apps…</p>{error&&<button className="secondary-button" onClick={()=>{setError("");void refresh().catch(e=>setError(e.message));}}>Try again</button>}</>:<div className="managed-apps">{inventory.items.map(item=><section className="managed-app" key={item.id} aria-label={item.name}><span className="managed-app-icon"><Icon name={icons[item.id]??"clock"}/></span><div className="managed-app-name"><h2>{item.name}</h2>{item.version&&<><p className="muted small">{item.id} · {item.version}</p><p className="muted small">{item.permissions.join(" · ")||"No SDK data permissions"}</p></>}{item.status&&item.status!=="Ready"&&<p className="muted small">{item.status}</p>}</div>{item.canToggle&&<><span className="managed-app-status">{item.enabled?"Enabled":"Disabled"}</span><button type="button" className="secondary-button" disabled={busy} aria-label={`${item.enabled?"Disable":"Enable"} ${item.name}`} onClick={()=>{void toggle(item);}}>{item.enabled?"Disable":"Enable"}</button></>}</section>)}</div>}</>;
}
