import {localDateTime} from "../dates.ts";
import {useCallback,useEffect,useState,type FormEvent} from "react";
import {request,type BrowserSession} from "../api.ts";
import {Icon} from "../icons.tsx";
import {Select} from "../controls.tsx";
import {SettingRow,SettingSwitch} from "../settings-sheet.tsx";
import type {PhotoImmichProgress} from "../../shared/photos.ts";

const base="/api/v1/photos/immich";
interface Device {id:string;deviceType:string;deviceOS:string;createdAt:number;usedAt:number}
type Connection={connected:false}|{connected:true;url:string;intervalMinutes:number;enabled:boolean;lastRunAt:number;lastError:string;summary:{remote?:number;imported?:number;albums?:number;skipped?:number};revision:number};
interface ImmichState {serverUrl:string;devices:Device[];connection:Connection;progress:PhotoImmichProgress|null}
const intervals:[number,string][]=[[15,"Every 15 minutes"],[60,"Every hour"],[360,"Every 6 hours"],[1440,"Daily"]];
const message=(cause:unknown)=>(cause as Error).message||"The request could not be completed.";
const when=(ms:number)=>localDateTime(ms,{dateStyle:"medium",timeStyle:"short"});

/** Photos settings › Immich (mock A7): the Immich mobile app, and Dar's connection to an Immich server. */
export function ImmichSettings({session}:{session:BrowserSession}){
  const [state,setState]=useState<ImmichState>(),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const [url,setUrl]=useState(""),[apiKey,setApiKey]=useState(""),[interval,setInterval]=useState(60),[enabled,setEnabled]=useState(true),[busy,setBusy]=useState(false);
  const load=useCallback(async()=>{
    try{const value=await request<ImmichState>(base);setState(value);setError("");
      if(value.connection.connected){setUrl(value.connection.url);setInterval(value.connection.intervalMinutes);setEnabled(value.connection.enabled);}}
    catch(cause){setError(message(cause));}
  },[]);
  useEffect(()=>{void load();},[load]);
  // While an import runs, its progress refreshes every two seconds.
  // Just after Connect or Sync now the import has not reported yet: keep looking for ten seconds.
  const [watchUntil,setWatchUntil]=useState(0);
  useEffect(()=>{if(!state?.progress&&Date.now()>watchUntil)return;const timer=setTimeout(()=>{void refresh();},state?.progress?2000:1000);return()=>clearTimeout(timer);},[state,watchUntil]);
  const refresh=async()=>{try{const value=await request<ImmichState>(base);setState(current=>current&&{...current,progress:value.progress,connection:value.connection});}catch{/* the next load reports it */}};
  async function save(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");setNotice("");
    try{
      await request(`${base}/connection`,{method:"PUT",body:JSON.stringify({url,...apiKey?{apiKey}:{},intervalMinutes:interval,enabled,revision:state?.connection.connected?state.connection.revision:0})},session.csrfToken);
      setApiKey("");setNotice(state?.connection.connected?"Connection saved":"Connected to Immich. Importing has started.");setWatchUntil(Date.now()+10_000);await load();
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
      <p className="photos-immich-intro">Import photos from an Immich server into Dar. Albums come along; files Dar already has are matched, not copied. Nothing is sent to Immich.</p>
      {c.connected&&<div className="photos-immich-status" role="status">
        <p><strong>{c.lastRunAt?`Last sync ${when(c.lastRunAt)}`:"Not synced yet"}</strong>{c.lastRunAt?` · ${c.summary.imported??0} imported · ${c.summary.albums??0} albums · ${c.summary.remote??0} on Immich`:""}{(c.summary.skipped??0)>0?` · ${c.summary.skipped} still to import`:""}</p>
        {state.progress&&<ImmichProgressBar progress={state.progress}/>}
        {c.lastError&&<p className="error">{c.lastError}</p>}
        <div className="photos-immich-actions"><button type="button" className="secondary-button" disabled={busy||!!state.progress&&state.progress.phase!=="waiting"} onClick={()=>{setWatchUntil(Date.now()+10_000);void act("/connection/sync","POST","Sync started");}}><Icon name="refresh"/>Sync now</button>
          <button type="button" className="text-button danger" disabled={busy} onClick={()=>{void act("/connection","DELETE","Disconnected. Imported photos stay in Dar.");}}>Disconnect</button></div>
      </div>}
      <form className="photos-immich-form" onSubmit={event=>{void save(event);}}>
        <label>Server address<input value={url} onChange={event=>setUrl(event.target.value)} placeholder="https://photos.example.com" required inputMode="url" autoComplete="off"/></label>
        <label>API key<input type="password" value={apiKey} onChange={event=>setApiKey(event.target.value)} autoComplete="off" placeholder={c.connected?"Saved — enter a new key to replace it":"From Immich › Account settings › API keys"} required={!c.connected||url!==c.url}/></label>
        <label>Sync<Select value={String(interval)} onChange={event=>setInterval(Number(event.target.value))}>{intervals.map(([m,label])=><option key={m} value={String(m)}>{label}</option>)}</Select></label>
        <SettingSwitch label="Sync automatically" checked={enabled} onChange={setEnabled}/>
        {error&&<p role="alert" className="error">{error}</p>}
        <div className="photos-immich-submit"><button className="primary-button" disabled={busy}>{busy?"Checking…":c.connected?"Save":"Connect"}</button></div>
      </form>
    </section>
  </>;
}

const phaseLabels:Record<PhotoImmichProgress["phase"],string>={connecting:"Connecting to Immich…",comparing:"Comparing libraries…",importing:"Importing from Immich",albums:"Matching albums",waiting:"More to import — continuing shortly"};
/** The live status of an Immich import: phase, a bar (indeterminate until the total is known) and counts. */
export function ImmichProgressBar({progress,compact=false}:{progress:PhotoImmichProgress;compact?:boolean}){
  const known=progress.total>0,percent=known?Math.min(100,Math.round(progress.done/progress.total*100)):0;
  const detail=progress.phase==="importing"&&known?`${progress.done.toLocaleString()} of ${progress.total.toLocaleString()} checked · ${progress.imported.toLocaleString()} imported`
    :progress.phase==="albums"&&known?`${progress.done} of ${progress.total} albums`:progress.imported?`${progress.imported.toLocaleString()} imported`:"";
  return <div className={`photos-sync-progress${compact?" is-compact":""}`} role="status" aria-live="polite">
    <span className="photos-sync-label"><Icon name="refresh"/>{phaseLabels[progress.phase]}{known&&!compact?` · ${percent}%`:""}</span>
    {progress.phase!=="waiting"&&<progress max={known?progress.total:undefined} value={known?progress.done:undefined} aria-label={phaseLabels[progress.phase]} aria-valuetext={known?`${percent}%`:undefined}/>}
    {detail&&<small>{detail}</small>}
    {!compact&&progress.file&&<small className="photos-sync-file">{progress.file}</small>}
  </div>;
}
