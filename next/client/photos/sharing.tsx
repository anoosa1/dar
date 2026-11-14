import {useCallback,useEffect,useState,type FormEvent} from "react";
import {request,RequestError,type BrowserSession} from "../api.ts";
import {Select} from "../controls.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {Icon} from "../icons.tsx";
import {SettingSwitch} from "../settings-sheet.tsx";
import {validLinkPassword,type PhotoAlbum,type PhotoAlbumMember,type PhotoAlbumSharing,type PhotoLink} from "../../shared/photos.ts";

const base="/api/v1/photos";
const message=(cause:unknown)=>(cause as Error).message||"The request could not be completed.";
/** Expiry choices for shared links; 0 never expires. */
const expiries:[string,number][]=[["Never",0],["1 day",1],["7 days",7],["30 days",30],["90 days",90],["1 year",365]];
export const expiryLabel=(at:number)=>!at?"Never expires":at<Date.now()?"Expired":`Expires ${new Date(at).toLocaleDateString([],{dateStyle:"medium"})}`;

/** Album sharing in the right editing pane (mock A's Share album): people on this Dar, then shared links. */
export function SharePane({album,session,onClose,onChanged}:{album:PhotoAlbum;session:BrowserSession;onClose:()=>void;onChanged:()=>void}){
  const [sharing,setSharing]=useState<PhotoAlbumSharing>(),[error,setError]=useState(""),[username,setUsername]=useState(""),[role,setRole]=useState<"viewer"|"editor">("viewer"),[busy,setBusy]=useState(false);
  const load=useCallback(async()=>{try{setSharing(await request<PhotoAlbumSharing>(`${base}/albums/${album.id}/sharing`));setError("");}catch(cause){setError(message(cause));}},[album.id]);
  useEffect(()=>{void load();},[load]);
  async function share(event:FormEvent){
    event.preventDefault();if(!username.trim())return setError("Enter a username.");setBusy(true);setError("");
    try{await request(`${base}/albums/${album.id}/members`,{method:"POST",body:JSON.stringify({username:username.trim(),role})},session.csrfToken);setUsername("");await load();onChanged();}
    catch(cause){setError(message(cause));}finally{setBusy(false);}
  }
  async function change(member:PhotoAlbumMember,next:"viewer"|"editor"){
    try{await request(`${base}/albums/${album.id}/members`,{method:"POST",body:JSON.stringify({username:member.username,role:next})},session.csrfToken);await load();}catch(cause){setError(message(cause));}
  }
  async function remove(member:PhotoAlbumMember){
    try{await request(`${base}/albums/${album.id}/members/${member.userId}`,{method:"DELETE"},session.csrfToken);await load();onChanged();}catch(cause){setError(message(cause));}
  }
  async function createLink(){
    setBusy(true);setError("");
    try{await request(`${base}/links`,{method:"POST",body:JSON.stringify({albumId:album.id,expiresAt:0,allowDownload:true,allowUpload:false,showMetadata:true})},session.csrfToken);await load();onChanged();}
    catch(cause){setError(message(cause));}finally{setBusy(false);}
  }
  return <EditorPane title={`Share ${album.name}`} onClose={onClose}>
    <div className="photos-share">
      {error&&<p role="alert" className="error">{error}</p>}
      <section aria-label="People on this Dar"><h3>People on this Dar</h3>
        <form className="photos-share-add" onSubmit={event=>{void share(event);}}>
          <label>Username<input value={username} maxLength={64} autoComplete="off" placeholder="Their Dar username" onChange={event=>setUsername(event.target.value)}/></label>
          <label>Access<Select value={role} onChange={event=>setRole(event.target.value as "viewer"|"editor")}><option value="viewer">Can view</option><option value="editor">Can add</option></Select></label>
          <button className="primary-button" disabled={busy}>Share</button>
        </form>
        {!sharing?<p role="status" className="photos-hint">Loading…</p>:sharing.members.length?<ul className="photos-share-people">{sharing.members.map(member=><li key={member.userId}>
          <span className="photos-share-avatar" aria-hidden="true">{member.displayName.slice(0,2).toUpperCase()}</span>
          <span className="photos-share-name"><strong>{member.displayName}</strong><small>@{member.username}</small></span>
          <Select aria-label={`Access for ${member.displayName}`} value={member.role} onChange={event=>{void change(member,event.target.value as "viewer"|"editor");}}><option value="viewer">Can view</option><option value="editor">Can add</option></Select>
          <button type="button" className="icon-button" aria-label={`Stop sharing with ${member.displayName}`} title="Stop sharing" onClick={()=>{void remove(member);}}><Icon name="close"/></button>
        </li>)}</ul>:<p className="photos-hint">Not shared with anyone yet. People who can add put their own photos in this album.</p>}
      </section>
      <section aria-label="Shared links"><h3>Shared links</h3>
        <p className="photos-hint">Anyone with a link can open the album without an account.</p>
        {sharing?.links.map(link=><LinkEditor key={link.id} link={link} session={session} onChanged={value=>{setSharing(current=>current&&{...current,links:value?current.links.map(l=>l.id===value.id?value:l):current.links.filter(l=>l.id!==link.id)});onChanged();}}/>)}
        <button type="button" className="secondary-button" disabled={busy||!sharing} onClick={()=>{void createLink();}}><Icon name="link"/>Create link</button>
      </section>
      <div className="dialog-actions"><button type="button" className="primary-button" onClick={onClose}>Done</button></div>
    </div>
  </EditorPane>;
}

/** One shared link: copy, expiry, password and the download, upload and metadata switches. Changes save at once. */
export function LinkEditor({link,session,onChanged}:{link:PhotoLink;session:BrowserSession;onChanged:(link:PhotoLink|null)=>void}){
  const [current,setCurrent]=useState(link),[error,setError]=useState(""),[password,setPassword]=useState(""),[editingPassword,setEditingPassword]=useState(false),[copied,setCopied]=useState(false);
  useEffect(()=>setCurrent(link),[link]);
  async function save(change:Record<string,unknown>){
    setError("");
    try{
      const put=(revision:number)=>request<PhotoLink>(`${base}/links/${current.id}`,{method:"PATCH",body:JSON.stringify({revision,...change})},session.csrfToken);
      let next:PhotoLink;
      try{next=await put(current.revision);}
      catch(cause){if(!(cause instanceof RequestError&&cause.status===409))throw cause;const fresh=(await request<{links:PhotoLink[]}>(`${base}/links`)).links.find(l=>l.id===current.id);if(!fresh)throw cause;next=await put(fresh.revision);}
      setCurrent(next);onChanged(next);return true;
    }catch(cause){setError(message(cause));return false;}
  }
  async function remove(){try{await request(`${base}/links/${current.id}`,{method:"DELETE"},session.csrfToken);onChanged(null);}catch(cause){setError(message(cause));}}
  async function copy(){try{await navigator.clipboard.writeText(current.url);setCopied(true);setTimeout(()=>setCopied(false),2000);}catch{setError("Copy the link from the field.");}}
  const days=current.expiresAt?Math.max(1,Math.round((current.expiresAt-current.createdAt)/86400_000)):0,choice=expiries.find(([,d])=>d===days)?.[1]??-1;
  return <article className="photos-link" aria-label={`Shared link${current.albumName?` to ${current.albumName}`:""}`}>
    {current.url?<div className="photos-link-url"><input readOnly value={current.url} aria-label="Link address" onFocus={event=>event.target.select()}/><button type="button" className="secondary-button" onClick={()=>{void copy();}}><Icon name={copied?"check":"copy"}/>{copied?"Copied":"Copy"}</button><a className="icon-button" href={current.url} target="_blank" rel="noreferrer" aria-label="Open link" title="Open link"><Icon name="external"/></a></div>
      :<p className="photos-hint">This link was made on another installation and can’t be shown. Remove it and create a new one.</p>}
    <p className="photos-link-meta">{expiryLabel(current.expiresAt)}{current.hasPassword?" · Password":""}{!current.albumId?` · ${current.count} photo${current.count===1?"":"s"}`:""}</p>
    <label className="photos-link-row">Expires<Select value={String(choice)} onChange={event=>{const d=Number(event.target.value);if(d>=0)void save({expiresAt:d?Date.now()+d*86400_000:0});}}>{choice<0&&<option value="-1">{expiryLabel(current.expiresAt)}</option>}{expiries.map(([label,d])=><option key={d} value={String(d)}>{label}</option>)}</Select></label>
    {editingPassword?<form className="photos-link-password" onSubmit={event=>{event.preventDefault();if(!validLinkPassword(password))return setError("Use a link password of 4 to 200 characters.");void save({password}).then(ok=>{if(ok){setPassword("");setEditingPassword(false);}});}}>
        <label>Link password<input type="password" autoComplete="new-password" value={password} maxLength={200} onChange={event=>setPassword(event.target.value)}/></label>
        <button className="secondary-button">Set password</button><button type="button" className="text-button" onClick={()=>{setEditingPassword(false);setPassword("");}}>Cancel</button></form>
      :<div className="photos-link-row"><span>Password</span><span className="photos-hint">{current.hasPassword?"On":"Off"}</span><button type="button" className="text-button" onClick={()=>setEditingPassword(true)}>{current.hasPassword?"Change":"Add password"}</button>{current.hasPassword&&<button type="button" className="text-button" onClick={()=>{void save({password:null});}}>Remove</button>}</div>}
    <SettingSwitch label="Allow download" hint="Visitors can download originals" checked={current.allowDownload} onChange={value=>{void save({allowDownload:value});}}/>
    {current.albumId&&<SettingSwitch label="Allow upload" hint="Visitors can add photos; they go to your Photos folder and this album" checked={current.allowUpload} onChange={value=>{void save({allowUpload:value});}}/>}
    <SettingSwitch label="Show metadata" hint="Dates, camera details and location" checked={current.showMetadata} onChange={value=>{void save({showMetadata:value});}}/>
    {error&&<p role="alert" className="error">{error}</p>}
    <button type="button" className="text-button danger" onClick={()=>{void remove();}}><Icon name="trash"/>Remove link</button>
  </article>;
}

/** A new link to chosen photos, then its settings. */
export function LinkDialog({ids,session,onClose,onCreated}:{ids:string[];session:BrowserSession;onClose:()=>void;onCreated:()=>void}){
  const [link,setLink]=useState<PhotoLink>(),[error,setError]=useState("");
  useEffect(()=>{let alive=true;void request<PhotoLink>(`${base}/links`,{method:"POST",body:JSON.stringify({ids,expiresAt:0,allowDownload:true,showMetadata:true})},session.csrfToken).then(value=>{if(alive){setLink(value);onCreated();}},cause=>{if(alive)setError(message(cause));});return()=>{alive=false;};},[]);
  return <Dialog title="Shared link" onClose={onClose}>
    <p>{ids.length} photo{ids.length===1?"":"s"} · anyone with the link can view {ids.length===1?"it":"them"} without an account.</p>
    {error&&<p role="alert" className="error">{error}</p>}
    {!link&&!error&&<p role="status">Creating link…</p>}
    {link&&<LinkEditor link={link} session={session} onChanged={value=>{if(value)setLink(value);else onClose();}}/>}
    <div className="dialog-actions"><button type="button" className="primary-button" onClick={onClose}>Done</button></div>
  </Dialog>;
}

/** Every link you made, for Photos settings. */
export function LinksList({session}:{session:BrowserSession}){
  const [links,setLinks]=useState<PhotoLink[]>(),[error,setError]=useState("");
  useEffect(()=>{void request<{links:PhotoLink[]}>(`${base}/links`).then(value=>setLinks(value.links),cause=>setError(message(cause)));},[]);
  if(error)return <p role="alert" className="error">{error}</p>;
  if(!links)return <p role="status" className="photos-hint">Loading…</p>;
  if(!links.length)return <p className="photos-hint">No shared links. Create one from an album’s Share pane or from selected photos.</p>;
  return <div className="photos-links">{links.map(link=><details key={link.id} className="photos-link-summary"><summary><Icon name="link"/><span>{link.albumName||`${link.count} photo${link.count===1?"":"s"}`}</span><small>{expiryLabel(link.expiresAt)}</small></summary>
    <LinkEditor link={link} session={session} onChanged={value=>setLinks(current=>current&&(value?current.map(l=>l.id===value.id?value:l):current.filter(l=>l.id!==link.id)))}/></details>)}</div>;
}
