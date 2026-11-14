import {Fragment,useCallback,useEffect,useRef,useState,type FormEvent} from "react";
import {request,RequestError} from "../api.ts";
import {useAppearance} from "../appearance.tsx";
import {Icon} from "../icons.tsx";
import type {PhotoAsset,PhotoPage,PhotoPublicDetail,PhotoPublicShare} from "../../shared/photos.ts";
import "./photos.css";
import "../locked-folder.css";

const message=(cause:unknown)=>(cause as Error).message||"The request could not be completed.";
const duration=(seconds:number)=>{const s=Math.round(seconds),m=Math.floor(s/60),r=String(s%60).padStart(2,"0");return `${m}:${r}`;};
interface Transfer {id:string;name:string;progress:number;status:"uploading"|"complete"|"error";message?:string}

/**
 * A Photos shared link (/s/<token>) for visitors without an account: the Photos grid and
 * viewer in a minimal frame, an optional password form, downloads and uploads when allowed.
 */
export function PublicPhotos(){
  useAppearance(null);
  const token=location.pathname.split("/")[2]??"",api=`/public/photos/${token}`;
  const [share,setShare]=useState<PhotoPublicShare>(),[error,setError]=useState(""),[items,setItems]=useState<PhotoAsset[]>([]),[cursor,setCursor]=useState(""),[open,setOpen]=useState<string>();
  const [transfers,setTransfers]=useState<Transfer[]>([]),fileInput=useRef<HTMLInputElement>(null),returnFocus=useRef<string|undefined>(undefined);
  const load=useCallback(async()=>{
    try{
      const value=await request<PhotoPublicShare>(api);setShare(value);setError("");
      if(!value.needsPassword){const page=await request<PhotoPage>(`${api}/assets`);setItems(page.items);setCursor(page.cursor);}
    }catch(cause){setError(cause instanceof RequestError&&cause.status===404?"This link is no longer available. It may have expired or been removed.":message(cause));}
  },[api]);
  useEffect(()=>{void load();},[load]);
  useEffect(()=>{if(share&&!share.needsPassword)document.title=`${share.title} · Dar`;},[share]);
  const more=async()=>{if(!cursor)return;const page=await request<PhotoPage>(`${api}/assets?cursor=${encodeURIComponent(cursor)}`).catch(()=>undefined);if(page){setItems(current=>[...current,...page.items]);setCursor(page.cursor);}};
  const index=open?items.findIndex(i=>i.id===open):-1,current=index>=0?items[index]:undefined;
  useEffect(()=>{
    if(!open)return;
    const keys=(event:KeyboardEvent)=>{if(event.defaultPrevented||(event.target as HTMLElement).closest("input,textarea,select"))return;
      if(event.key==="ArrowRight"&&items[index+1]){event.preventDefault();setOpen(items[index+1].id);}else if(event.key==="ArrowLeft"&&index>0){event.preventDefault();setOpen(items[index-1].id);}else if(event.key==="Escape"){event.preventDefault();close();}};
    document.addEventListener("keydown",keys);return()=>document.removeEventListener("keydown",keys);
  },[open,items]);
  function close(){returnFocus.current=open;setOpen(undefined);}
  useEffect(()=>{if(!open&&returnFocus.current){const id=returnFocus.current;returnFocus.current=undefined;requestAnimationFrame(()=>document.querySelector<HTMLElement>(`[data-asset="${CSS.escape(id)}"] .photos-open`)?.focus());}},[open]);

  async function upload(files:FileList|null){
    for(const file of [...files??[]]){
      const id=crypto.randomUUID(),change=(next:Partial<Transfer>)=>setTransfers(c=>c.map(t=>t.id===id?{...t,...next}:t));
      setTransfers(c=>[...c,{id,name:file.name,progress:0,status:"uploading"}]);let uploadId="";
      try{
        const when=new Date(file.lastModified||Date.now());
        uploadId=(await request<{id:string}>(`${api}/uploads`,{method:"POST",body:JSON.stringify({name:file.name,size:file.size,mediaType:file.type||"application/octet-stream",year:when.getFullYear(),month:when.getMonth()+1})})).id;
        await new Promise<void>((resolve,reject)=>{
          const xhr=new XMLHttpRequest();xhr.open("PUT",`${api}/uploads/${uploadId}`);
          xhr.upload.onprogress=event=>change({progress:event.lengthComputable?event.loaded/event.total:0});
          xhr.onload=()=>{if(xhr.status>=200&&xhr.status<300)resolve();else{try{reject(new Error(JSON.parse(xhr.responseText).error?.message??"The upload could not be completed."));}catch{reject(new Error("The upload could not be completed."));}}};
          xhr.onerror=()=>reject(new Error("The connection was interrupted. Try the upload again."));xhr.send(file);
        });
        change({progress:1,status:"complete"});
      }catch(cause){change({status:"error",message:message(cause)});if(uploadId)await request(`${api}/uploads/${uploadId}`,{method:"DELETE"}).catch(()=>undefined);}
    }
    await load();
  }

  const frame=(content:React.ReactNode)=><div className="public-photos"><header className="public-photos-bar"><span className="brand"><span className="brand-mark" aria-hidden="true"/>Dar</span></header>{content}</div>;
  if(error)return frame(<main className="photos-empty" role="alert"><Icon name="link"/><h1>Link unavailable</h1><p>{error}</p></main>);
  if(!share)return frame(<main className="photos-empty" role="status" aria-busy="true">Opening shared photos…</main>);
  if(share.needsPassword)return frame(<PasswordForm api={api} onUnlocked={()=>{void load();}}/>);
  let lastMonth="";
  return frame(<main className="public-photos-main" aria-label={share.title}>
    {current?<PublicViewer api={api} asset={current} index={index} total={items.length} share={share} onClose={close} onStep={delta=>{const next=items[index+delta];if(next)setOpen(next.id);if(index+delta>=items.length-4)void more();}}/>:<>
      <div className="public-photos-heading">
        <div><h1>{share.title}</h1><p>{share.owner?`Shared by ${share.owner}`:"Shared photos"} · {share.count.toLocaleString()} item{share.count===1?"":"s"}{share.expiresAt?` · until ${new Date(share.expiresAt).toLocaleDateString([],{dateStyle:"medium"})}`:""}</p>{share.description&&<p className="public-photos-description">{share.description}</p>}</div>
        {share.allowUpload&&<><button type="button" className="primary-button" onClick={()=>fileInput.current?.click()}><Icon name="upload"/>Add photos</button><input ref={fileInput} type="file" multiple hidden accept="image/*,video/*,.heic,.heif,.dng,.mov" aria-label="Add photos to this album" onChange={event=>{void upload(event.target.files);event.target.value="";}}/></>}
      </div>
      {!items.length&&<div className="photos-empty"><Icon name="image"/><h2>Nothing here yet</h2>{share.allowUpload&&<p>Add the first photos to this album.</p>}</div>}
      <div className="photos-grid" role="list" aria-label={`${share.title} photos`} data-columns={5}>
        {items.map(item=>{const month=new Date(item.takenAt).toLocaleDateString([],{month:"long",year:"numeric"}),heading=share.showMetadata&&month!==lastMonth?(lastMonth=month,<h2 className="photos-month" key={"m-"+month}>{month}</h2>):null;
          return <Fragment key={item.id}>{heading}<div role="listitem" data-asset={item.id} className="photos-tile"><button type="button" className="photos-open" aria-label={`${item.kind==="video"?"Video":"Photo"} ${item.name}`} onClick={()=>setOpen(item.id)}>
            <PublicThumb api={api} asset={item}/>{item.kind==="video"&&<span className="photos-badge"><Icon name="video"/>{item.duration?duration(item.duration):""}</span>}{item.live&&<span className="photos-badge"><Icon name="live"/>Live</span>}
          </button></div></Fragment>;})}
      </div>
      {cursor&&<div className="photos-more"><button className="text-button" onClick={()=>{void more();}}>Load more</button></div>}
    </>}
    {transfers.length>0&&<section className="transfer-panel photos-transfers" aria-label="Uploads"><div className="transfer-heading"><strong>Uploads</strong><button className="icon-button" aria-label="Dismiss finished uploads" disabled={transfers.some(t=>t.status==="uploading")} onClick={()=>setTransfers([])}><Icon name="close"/></button></div>
      <ul>{transfers.map(t=><li key={t.id} className={`photos-transfer is-${t.status}`}><span>{t.name}</span><progress max={1} value={t.progress} aria-label={`${t.name} upload progress`}/>{t.message?<small role="alert">{t.message}</small>:<small>{t.status==="complete"?"Done":`${Math.round(t.progress*100)}%`}</small>}</li>)}</ul></section>}
  </main>);
}

function PasswordForm({api,onUnlocked}:{api:string;onUnlocked:()=>void}){
  const [password,setPassword]=useState(""),[error,setError]=useState(""),[busy,setBusy]=useState(false);
  async function submit(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{await request(`${api}/password`,{method:"POST",body:JSON.stringify({password})});onUnlocked();}catch(cause){setError(message(cause));setBusy(false);}
  }
  return <main className="public-photos-main"><form className="locked-unlock" onSubmit={event=>{void submit(event);}}>
    <span className="locked-unlock-icon" aria-hidden="true"><Icon name="lock"/></span><h1>Password required</h1><p>Enter the password you were given to see these photos.</p>
    <label>Password<input type="password" autoFocus autoComplete="off" value={password} onChange={event=>{setPassword(event.target.value);setError("");}} aria-invalid={!!error}/></label>
    {error&&<p role="alert" className="error">{error}</p>}
    <button className="primary-button" disabled={busy||!password}>{busy?"Checking…":"Open"}</button>
  </form></main>;
}

function PublicThumb({api,asset,size="s"}:{api:string;asset:PhotoAsset;size?:"s"|"m"}){
  const [failed,setFailed]=useState(false);
  if(failed)return <span className="photos-placeholder"><Icon name={asset.kind==="video"?"video":"image"}/><small>{asset.name}</small></span>;
  return <img src={`${api}/assets/${asset.id}/thumbnail?${size==="m"?"size=m&":""}v=${asset.revision}`} alt={size==="m"?asset.name:""} loading="lazy" decoding="async" draggable={false} onError={()=>setFailed(true)}/>;
}

function PublicViewer({api,asset,index,total,share,onClose,onStep}:{api:string;asset:PhotoAsset;index:number;total:number;share:Extract<PhotoPublicShare,{needsPassword:false}>;onClose:()=>void;onStep:(delta:number)=>void}){
  const [detail,setDetail]=useState<PhotoPublicDetail>(),heading=useRef<HTMLHeadingElement>(null);
  useEffect(()=>{heading.current?.focus();},[]);
  useEffect(()=>{setDetail(undefined);if(!share.showMetadata)return;void request<PhotoPublicDetail>(`${api}/assets/${asset.id}`).then(setDetail,()=>undefined);},[asset.id]);
  const camera=detail?.exif?[detail.exif.make&&detail.exif.model&&!detail.exif.model.startsWith(detail.exif.make)?detail.exif.make:"",detail.exif.model].filter(Boolean).join(" "):"";
  return <section className="photos-viewer public-photos-viewer" aria-label={`Viewer for ${asset.name}`}>
    <div className="photos-viewer-bar">
      <button type="button" className="icon-button" aria-label="Back to photos" title="Back (Escape)" onClick={onClose}><Icon name="back"/></button>
      <h1 ref={heading} tabIndex={-1}>{asset.name}</h1><span className="photos-heading-count">{index+1} of {total}</span><span className="photos-spacer"/>
      <button type="button" className="icon-button" aria-label="Previous" disabled={index<=0} onClick={()=>onStep(-1)}><Icon name="back"/></button>
      <button type="button" className="icon-button" aria-label="Next" disabled={index>=total-1} onClick={()=>onStep(1)}><Icon name="chevron"/></button>
      {share.allowDownload&&<a className="icon-button" href={`${api}/assets/${asset.id}/original`} download aria-label="Download original" title="Download original"><Icon name="download"/></a>}
    </div>
    <div className="photos-stage">{asset.kind==="video"?<PublicVideo key={asset.id} api={api} asset={asset}/>:<PublicThumb key={asset.id} api={api} asset={asset} size="m"/>}</div>
    {detail&&(detail.takenAt||camera||detail.description)&&<p className="public-photos-meta">{[detail.takenAt?new Date(detail.takenAt).toLocaleString([],{dateStyle:"long",timeStyle:"short"}):"",camera,detail.description].filter(Boolean).join(" · ")}</p>}
  </section>;
}

function PublicVideo({api,asset}:{api:string;asset:PhotoAsset}){
  const [state,setState]=useState<{state:string;progress:number;url:string}>();
  useEffect(()=>{
    let alive=true,timer:ReturnType<typeof setTimeout>|undefined;
    const poll=async()=>{try{const value=await request<{state:string;progress:number;url:string}>(`${api}/assets/${asset.id}/playback`,{method:"POST"});if(!alive)return;setState(value);if(value.state==="preparing")timer=setTimeout(()=>{void poll();},1500);}catch{if(alive)setState({state:"failed",progress:0,url:""});}};
    void poll();return()=>{alive=false;clearTimeout(timer);};
  },[asset.id]);
  if(!state||state.state==="preparing")return <div className="photos-video-wait" role="status"><span className="photos-spinner" aria-hidden="true"/><span>Preparing this video{state?.progress?` · ${Math.round(state.progress*100)}%`:"…"}</span></div>;
  if(state.state==="failed")return <div className="photos-empty" role="alert"><Icon name="video"/><h2>This video can’t be played here</h2></div>;
  return <video src={state.url} controls playsInline preload="metadata" aria-label={asset.name}/>;
}
