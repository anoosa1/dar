import {useCallback,useEffect,useState,type FormEvent} from "react";
import {request,type BrowserSession} from "../api.ts";
import {Icon} from "../icons.tsx";
import {Select} from "../controls.tsx";
import {SettingRow,SettingSwitch,Segmented} from "../settings-sheet.tsx";

const base="/api/v1/photos/immich";
interface Device {id:string;deviceType:string;deviceOS:string;createdAt:number;usedAt:number}
type Connection={connected:false}|{connected:true;url:string;direction:"import"|"twoWay";intervalMinutes:number;enabled:boolean;lastRunAt:number;lastError:string;summary:{remote?:number;imported?:number;albums?:number;exported?:number;skipped?:number};revision:number};
interface ImmichState {serverUrl:string;devices:Device[];connection:Connection}
const intervals:[number,string][]=[[15,"Every 15 minutes"],[60,"Every hour"],[360,"Every 6 hours"],[1440,"Daily"]];
const message=(cause:unknown)=>(cause as Error).message||"The request could not be completed.";
const when=(ms:number)=>new Date(ms).toLocaleString([],{dateStyle:"medium",timeStyle:"short"});

/** Photos settings › Immich (mock A7): the Immich mobile app, and Dar's connection to an Immich server. */
export function ImmichSettings({session}:{session:BrowserSession}){
  const [state,setState]=useState<ImmichState>(),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const [url,setUrl]=useState(""),[apiKey,setApiKey]=useState(""),[direction,setDirection]=useState<"import"|"twoWay">("import"),[interval,setInterval]=useState(60),[enabled,setEnabled]=useState(true),[busy,setBusy]=useState(false);
  const load=useCallback(async()=>{
    try{const value=await request<ImmichState>(base);setState(value);setError("");
      if(value.connection.connected){setUrl(value.connection.url);setDirection(value.connection.direction);setInterval(value.connection.intervalMinutes);setEnabled(value.connection.enabled);}}
    catch(cause){setError(message(cause));}
  },[]);
  useEffect(()=>{void load();},[load]);
  async function save(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");setNotice("");
    try{
      await request(`${base}/connection`,{method:"PUT",body:JSON.stringify({url,...apiKey?{apiKey}:{},direction,intervalMinutes:interval,enabled,revision:state?.connection.connected?state.connection.revision:0})},session.csrfToken);
      setApiKey("");setNotice(state?.connection.connected?"Connection saved":"Connected to Immich. The first sync starts within a few minutes.");await load();
    }catch(cause){setError(message(cause));}finally{setBusy(false);}
  }
  async function act(path:string,method:string,done:string){
    setBusy(true);setError("");setNotice("");
    try{await request(`${base}${path}`,{method},session.csrfToken);setNotice(done);await load();}catch(cause){setError(message(cause));}finally{setBusy(false);}
  }
  if(!state)return error?<p role="alert" className="error">{error}</p>:<p role="status">Loading…</p>;
  const c=state.connection;
  return <>

    {notice&&<p role="status" className="photos-saved">{notice}</p>}
    <section className="settings-group" aria-label="Immich mobile app"><h3>Immich mobile app</h3>
      <SettingRow label="Server URL" hint="In the Immich app, enter this address. It finds Dar’s Immich-compatible API by itself." stacked><input readOnly className="photos-immich-url" value={state.serverUrl} aria-label="Server URL for the Immich app" onFocus={event=>event.target.select()}/></SettingRow>
      <SettingRow label="Email and password" hint={<>Email: <strong>{session.user.username}@dar</strong> (your username, @ and anything). Password: an app password with Photos read and write.</>} stacked><a className="secondary-button" href="/settings">Create an app password</a></SettingRow>
      <SettingRow label="Backup and sync" hint="Backups go to your Photos folder; photos already here (same file) are skipped. The app shows your library, albums shared with you, favorites, archive and trash. Locked photos stay on Dar." stacked><span className="photos-hint">Immich 3.4 protocol</span></SettingRow>
      <div className="photos-devices" role="group" aria-label="Signed-in devices"><strong>Signed-in devices</strong>
        {state.devices.length?<ul>{state.devices.map(d=><li key={d.id}><Icon name="image"/><span><strong>{d.deviceOS||d.deviceType||"Immich app"}</strong><small>Last used {when(d.usedAt)}</small></span><button type="button" className="text-button danger" disabled={busy} onClick={()=>{void act(`/devices/${d.id}`,"DELETE","Device signed out");}}>Sign out</button></li>)}</ul>
          :<p className="photos-hint">No devices yet.</p>}
      </div>
    </section>
    <section className="settings-group" aria-label="Immich server"><h3>Immich server</h3>
      <p className="photos-immich-intro">Bring photos from an Immich server into Dar (Import), and optionally send Dar photos there too (Two-way). Albums come along; files Dar already has are matched, not copied.</p>
      <form className="photos-immich-form" onSubmit={event=>{void save(event);}}>
        <label>Server address<input value={url} onChange={event=>setUrl(event.target.value)} placeholder="https://photos.example.com" required inputMode="url" autoComplete="off"/></label>
        <label>API key<input type="password" value={apiKey} onChange={event=>setApiKey(event.target.value)} autoComplete="off" placeholder={c.connected?"Saved — enter a new key to replace it":"From Immich › Account settings › API keys"} required={!c.connected||url!==c.url}/></label>
        <SettingRow label="Direction" hint={direction==="import"?"Immich to Dar":"Both ways; Dar photos Immich lacks are uploaded"}><Segmented label="Direction" value={direction} options={[["import","Import"],["twoWay","Two-way"]] as const} onChange={setDirection}/></SettingRow>
        <label>Sync<Select value={String(interval)} onChange={event=>setInterval(Number(event.target.value))}>{intervals.map(([m,label])=><option key={m} value={String(m)}>{label}</option>)}</Select></label>
        <SettingSwitch label="Sync automatically" checked={enabled} onChange={setEnabled}/>
        {error&&<p role="alert" className="error">{error}</p>}
        <div className="photos-immich-submit"><button className="primary-button" disabled={busy}>{busy?"Checking…":c.connected?"Save":"Connect"}</button></div>
      </form>
      {c.connected&&<div className="photos-immich-status" role="status">
        <p><strong>{c.lastRunAt?`Last sync ${when(c.lastRunAt)}`:"Not synced yet"}</strong>{c.lastRunAt?` · ${c.summary.imported??0} imported · ${c.summary.exported??0} sent · ${c.summary.albums??0} albums · ${c.summary.remote??0} on Immich`:""}{(c.summary.skipped??0)>0?` · ${c.summary.skipped} left for the next run`:""}</p>
        {c.lastError&&<p className="error">{c.lastError}</p>}
        <div className="photos-immich-actions"><button type="button" className="secondary-button" disabled={busy} onClick={()=>{void act("/connection/sync","POST","Sync started");}}><Icon name="refresh"/>Sync now</button>
          <button type="button" className="text-button danger" disabled={busy} onClick={()=>{void act("/connection","DELETE","Disconnected. Imported photos stay in Dar.");}}>Disconnect</button></div>
      </div>}
    </section>
  </>;
}
