import {useEffect,useState} from "react";
import {request,type BrowserSession} from "./api.ts";
import {Brand} from "./auth.tsx";

export interface InstanceAppearance {revision:number;logoUrl:string|null}
export function InstanceAppearanceSettings({session}:{session:BrowserSession}){
  const [appearance,setAppearance]=useState<InstanceAppearance>(),[preview,setPreview]=useState<string|null>(null),[file,setFile]=useState<File|null>(null),[error,setError]=useState(""),[busy,setBusy]=useState(false),[saved,setSaved]=useState(false);
  useEffect(()=>{const controller=new AbortController();void request<InstanceAppearance>("/api/v1/appearance",{signal:controller.signal}).then(value=>{setAppearance(value);setPreview(value.logoUrl);}).catch(e=>{if(!controller.signal.aborted)setError(e.message);});return()=>controller.abort();},[]);
  useEffect(()=>{if(!file)return;const url=URL.createObjectURL(file);setPreview(url);return()=>URL.revokeObjectURL(url);},[file]);
  return <><h1>Appearance</h1><p className="muted">Customize the sign-in logo for everyone on this instance.</p>{error&&<p role="alert" className="error">{error}</p>}{!appearance?<p role="status">Loading appearance…</p>:<form onSubmit={event=>{
    event.preventDefault();setBusy(true);setError("");setSaved(false);
    void (async()=>{
      let logo="",mediaType="";
      if(file){const bytes=new Uint8Array(await file.arrayBuffer());logo=btoa(Array.from(bytes,b=>String.fromCharCode(b)).join(""));mediaType=file.type;}
      else if(preview){setBusy(false);setSaved(true);return;}
      const value=await request<InstanceAppearance>("/api/v1/appearance",{method:"PUT",headers:{"if-match":`"appearance:${appearance.revision}"`},body:JSON.stringify({revision:appearance.revision,logo,mediaType})},session.csrfToken);
      setAppearance(value);setFile(null);setPreview(value.logoUrl);setSaved(true);
    })().catch(e=>setError(e.message)).finally(()=>setBusy(false));
  }}><h2>Sign-in logo</h2><div className="signin-logo-preview">{preview?<img src={preview} alt="Sign-in logo preview"/>:<Brand/>}</div><label>Upload logo<input type="file" accept="image/png,image/webp" disabled={busy} onChange={event=>{const selected=event.target.files?.[0];setSaved(false);if(!selected)return;if(!["image/png","image/webp"].includes(selected.type)||selected.size>256*1024){setError("Choose a PNG or WebP image up to 256 KiB.");event.target.value="";return;}setError("");setFile(selected);}}/></label><p className="field-hint">PNG or WebP, up to 256 KiB. Transparent images work best.</p><button className="secondary-button" type="button" disabled={busy} onClick={()=>{setFile(null);setPreview(null);setSaved(false);}}>Use Dar logo</button><div className="dialog-actions"><button className="primary-button" disabled={busy}>{busy?"Saving…":"Save appearance"}</button></div>{saved&&<p role="status">Appearance saved</p>}</form>}</>;
}
