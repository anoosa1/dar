import {Fragment,useCallback,useEffect,useMemo,useRef,useState,type FormEvent} from "react";
import {request,formatBytes,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {ActionMenu,NewItemButton,type MenuAction} from "../controls.tsx";
import {Dialog} from "../dialog.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {SettingsSheet,SettingRow} from "../settings-sheet.tsx";
import type {PhotoAlbum,PhotoAsset,PhotoDetail,PhotoOverview,PhotoPage,PhotoView} from "../../shared/photos.ts";
import {flushSync} from "react-dom";
import "./photos.css";

const base="/api/v1/photos";
type View={type:Exclude<PhotoView,"album">}|{type:"album";id:string};
interface Notice {message:string;undo?:()=>void}
interface Transfer {id:string;name:string;progress:number;status:"uploading"|"complete"|"error";message?:string}
const views:[Exclude<PhotoView,"album">,string,IconName][]=[["library","Library","image"],["favorites","Favorites","star"],["videos","Videos","video"],["archive","Archive","lock"]];
const zoomSteps=[3,4,5,6,8,10,12];

function readView():{view:View;asset?:string}{
  const p=new URLSearchParams(location.search),album=p.get("album"),v=p.get("view");
  return {view:album?{type:"album",id:album}:views.some(([id])=>id===v)?{type:v as Exclude<PhotoView,"album">}:{type:"library"},asset:p.get("asset")??undefined};
}
function writeView(view:View,asset?:string){
  const p=new URLSearchParams();if(view.type==="album")p.set("album",view.id);else if(view.type!=="library")p.set("view",view.type);if(asset)p.set("asset",asset);
  const next=`/photos${p.size?"?"+p:""}`;if(next!==location.pathname+location.search)history.replaceState(null,"",next);
}
const thumb=(id:string,size:"s"|"m"="s")=>`${base}/assets/${id}/thumbnail${size==="m"?"?size=m":""}`;
const content=(id:string)=>`/api/v1/files/${id}/content?preview=1`;
const monthLabel=(time:number)=>new Date(time).toLocaleDateString([],{month:"long",year:"numeric"});
const duration=(seconds:number)=>{const s=Math.round(seconds),h=Math.floor(s/3600),m=Math.floor(s%3600/60),r=String(s%60).padStart(2,"0");return h?`${h}:${String(m).padStart(2,"0")}:${r}`:`${m}:${r}`;};
const message=(cause:unknown)=>(cause as Error).message||"The request could not be completed.";

/** Photos (selected mock B): views and albums in the sidebar, a month-grouped grid, the photo in the middle sheet and details on the right. */
export function PhotosWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readView,[]);
  const [overview,setOverview]=useState<PhotoOverview>(),[overviewError,setOverviewError]=useState(""),[view,setView]=useState<View>(start.view);
  const [items,setItems]=useState<PhotoAsset[]>([]),[cursor,setCursor]=useState(""),[state,setState]=useState<"loading"|"ready"|"error">("loading"),[listError,setListError]=useState("");
  const [open,setOpen]=useState<string|undefined>(start.asset),[selected,setSelected]=useState<Set<string>>(new Set()),[zoom,setZoom]=useState(()=>Number(localStorage.getItem("dar:photos:zoom"))||5);
  const [notice,setNotice]=useState<Notice|null>(null),[albumDialog,setAlbumDialog]=useState<{album?:PhotoAlbum;ids?:string[]}>(),[confirm,setConfirm]=useState<PhotoAlbum>(),[settings,setSettings]=useState(false);
  const [transfers,setTransfers]=useState<Transfer[]>([]),[dragging,setDragging]=useState(false),[info,setInfo]=useState(false);
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const generation=useRef(0),shownKey=useRef(""),fileInput=useRef<HTMLInputElement>(null),sentinel=useRef<HTMLDivElement>(null),grid=useRef<HTMLDivElement>(null);
  const albums=overview?.albums??[],album=view.type==="album"?albums.find(a=>a.id===view.id):undefined;
  const title=view.type==="album"?album?.name??"Album":views.find(([id])=>id===view.type)![1];
  const fail=(cause:unknown)=>setNotice({message:message(cause)});

  const loadOverview=useCallback(async()=>{try{setOverview(await request<PhotoOverview>(`${base}/overview`));setOverviewError("");}catch(cause){setOverviewError(message(cause));}},[]);
  const query=(after="")=>{const p=new URLSearchParams({view:view.type,limit:"120"});if(view.type==="album")p.set("album",view.id);if(after)p.set("cursor",after);return `${base}/assets?${p}`;};
  const viewKey=view.type==="album"?"album:"+view.id:view.type;
  const loadItems=useCallback(async()=>{
    const ticket=++generation.current;setState("loading");setSelected(new Set());
    // A different view never shows the previous view's photos; refreshes of the same view keep them while loading.
    if(shownKey.current!==viewKey){shownKey.current=viewKey;setItems([]);setCursor("");}
    try{const page=await request<PhotoPage>(query());if(ticket!==generation.current)return;setItems(page.items);setCursor(page.cursor);setState("ready");grid.current?.scrollTo({top:0});}
    catch(cause){if(ticket===generation.current){setState("error");setListError(message(cause));}}
  },[viewKey]);
  const loadMore=useCallback(async()=>{
    if(!cursor||state!=="ready")return;const ticket=generation.current,page=await request<PhotoPage>(query(cursor)).catch(()=>undefined);
    if(!page||ticket!==generation.current)return;setItems(current=>[...current,...page.items.filter(i=>!current.some(c=>c.id===i.id))]);setCursor(page.cursor);
  },[cursor,state,viewKey]);
  useEffect(()=>{void loadOverview();},[loadOverview]);
  useEffect(()=>{void loadItems();},[loadItems]);
  useEffect(()=>{writeView(view,open);if(!open)setInfo(false);},[view,open]);
  useEffect(()=>{localStorage.setItem("dar:photos:zoom",String(zoom));},[zoom]);
  // While the background scan is running, refresh so newly indexed media appears.
  const latest=useRef({loadItems,busy:false});latest.current={loadItems,busy:selected.size>0||!!open};
  useEffect(()=>{if(!overview?.indexing)return;const timer=setTimeout(()=>{void loadOverview();if(!latest.current.busy)void latest.current.loadItems();},8000);return()=>clearTimeout(timer);},[overview]);
  useEffect(()=>{const node=sentinel.current;if(!node||!cursor)return;const observer=new IntersectionObserver(entries=>{if(entries.some(e=>e.isIntersecting))void loadMore();},{root:grid.current,rootMargin:"600px"});observer.observe(node);return()=>observer.disconnect();},[cursor,loadMore]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(null),notice.undo?8000:4000);return()=>clearTimeout(timer);},[notice]);

  function choose(next:View){const key=next.type==="album"?"album:"+next.id:next.type;if(key!==viewKey){setItems([]);setCursor("");setState("loading");}setView(next);setOpen(undefined);setNavigation(false);}
  const patchLocal=(ids:string[],change:Partial<PhotoAsset>)=>setItems(current=>current.map(i=>ids.includes(i.id)?{...i,...change}:i));
  async function update(ids:string[],change:{favorite?:boolean;archived?:boolean},undo?:{favorite?:boolean;archived?:boolean}){
    try{
      await request(`${base}/assets`,{method:"PATCH",body:JSON.stringify({ids,...change})},session.csrfToken);
      const leaves=change.archived!==undefined&&(view.type==="archive"?!change.archived:view.type!=="album"&&view.type!=="favorites"&&change.archived)||change.favorite===false&&view.type==="favorites";
      if(leaves){setItems(current=>current.filter(i=>!ids.includes(i.id)));if(open&&ids.includes(open))setOpen(undefined);}else patchLocal(ids,change);
      setSelected(new Set());void loadOverview();
      if(undo){const n=ids.length,noun=`${n} item${n===1?"":"s"}`;setNotice({message:change.archived!==undefined?(change.archived?`${noun} archived`:`${noun} restored to the library`):change.favorite?`${noun} added to Favorites`:`${noun} removed from Favorites`,undo:()=>{void update(ids,undo).then(()=>loadItems());}});}
    }catch(cause){fail(cause);}
  }
  async function addTo(target:PhotoAlbum,ids:string[]){
    try{const result=await request<PhotoAlbum&{added:number}>(`${base}/albums/${target.id}/add`,{method:"POST",body:JSON.stringify({ids})},session.csrfToken);setSelected(new Set());await loadOverview();setNotice({message:result.added?`Added ${result.added} to ${target.name}`:`Already in ${target.name}`});}
    catch(cause){fail(cause);}
  }
  async function removeFromAlbum(ids:string[]){
    if(view.type!=="album")return;const id=view.id;
    try{await request(`${base}/albums/${id}/remove`,{method:"POST",body:JSON.stringify({ids})},session.csrfToken);setItems(current=>current.filter(i=>!ids.includes(i.id)));setSelected(new Set());if(open&&ids.includes(open))setOpen(undefined);await loadOverview();
      setNotice({message:`Removed ${ids.length} from ${album?.name??"the album"}`,undo:()=>{void request(`${base}/albums/${id}/add`,{method:"POST",body:JSON.stringify({ids})},session.csrfToken).then(()=>Promise.all([loadItems(),loadOverview()])).catch(fail);}});}
    catch(cause){fail(cause);}
  }

  async function upload(files:FileList|File[]|null){
    const list=[...files??[]].filter(f=>/^(image|video)\//.test(f.type)||/\.(heic|heif|dng|cr2|cr3|nef|arw|orf|rw2|raf|mov|mkv)$/i.test(f.name));
    if(!list.length){if(files&&[...files].length)setNotice({message:"Only photos and videos can be uploaded here. Use Files for other documents."});return;}
    for(const file of list){
      const transferId=crypto.randomUUID(),change=(next:Partial<Transfer>)=>setTransfers(current=>current.map(t=>t.id===transferId?{...t,...next}:t));
      setTransfers(current=>[...current,{id:transferId,name:file.name,progress:0,status:"uploading"}]);let uploadId="";
      try{
        const when=new Date(file.lastModified||Date.now()),{parentId}=await request<{parentId:string}>(`${base}/upload-target`,{method:"POST",body:JSON.stringify({year:when.getFullYear(),month:when.getMonth()+1})},session.csrfToken);
        const dot=file.name.lastIndexOf("."),stem=dot>0?file.name.slice(0,dot):file.name,ext=dot>0?file.name.slice(dot):"";
        for(let attempt=0;!uploadId;attempt++){
          // Same-named files from different cameras are common: keep both with a numbered name.
          try{uploadId=(await request<{id:string}>("/api/v1/files/uploads",{method:"POST",body:JSON.stringify({parentId,name:attempt?`${stem} (${attempt+1})${ext}`:file.name,size:file.size,mediaType:file.type||"application/octet-stream"})},session.csrfToken)).id;}
          catch(cause){if(!(cause instanceof RequestError&&cause.status===409)||attempt>=20)throw cause;}
        }
        const fileId=await new Promise<string>((resolve,reject)=>{
          const xhr=new XMLHttpRequest();xhr.open("PUT",`/api/v1/files/uploads/${uploadId}`);xhr.setRequestHeader("x-csrf-token",session.csrfToken);
          xhr.upload.onprogress=event=>change({progress:event.lengthComputable?event.loaded/event.total:0});
          xhr.onload=()=>{try{const value=JSON.parse(xhr.responseText);if(xhr.status>=200&&xhr.status<300)resolve(value.id);else reject(new Error(value.error?.message??"The upload could not be completed."));}catch{reject(new Error("The upload could not be completed."));}};
          xhr.onerror=()=>reject(new Error("The connection was interrupted. Try the upload again."));xhr.send(file);
        });
        await request(`${base}/assets/${fileId}/index`,{method:"POST"},session.csrfToken).catch(()=>undefined);
        change({progress:1,status:"complete"});
      }catch(cause){change({status:"error",message:message(cause)});if(uploadId)await request(`/api/v1/files/uploads/${uploadId}`,{method:"DELETE"},session.csrfToken).catch(()=>undefined);}
    }
    await Promise.all([loadOverview(),loadItems()]);
  }

  const index=open?items.findIndex(i=>i.id===open):-1,current=index>=0?items[index]:undefined;
  const step=(delta:number)=>{if(index<0)return;const next=items[index+delta];if(next)setOpen(next.id);if(index+delta>=items.length-8)void loadMore();};
  useEffect(()=>{
    if(!open)return;
    const keys=(event:KeyboardEvent)=>{const target=event.target as HTMLElement;if(event.defaultPrevented||event.ctrlKey||event.metaKey||event.altKey||target.closest("input,textarea,select,[contenteditable=true],dialog,.ui-menu")||document.querySelector("dialog[open]"))return;
      if(event.key==="ArrowRight"){event.preventDefault();step(1);}else if(event.key==="ArrowLeft"){event.preventDefault();step(-1);}else if(event.key==="Escape"){event.preventDefault();close();}
      else if(event.key==="f"&&current){event.preventDefault();void update([current.id],{favorite:!current.favorite});}};
    document.addEventListener("keydown",keys);return()=>document.removeEventListener("keydown",keys);
  },[open,items]);
  const returnFocus=useRef<string|undefined>(undefined);
  function close(){returnFocus.current=open;setOpen(undefined);}
  useEffect(()=>{if(!open&&returnFocus.current){const id=returnFocus.current;returnFocus.current=undefined;requestAnimationFrame(()=>grid.current?.querySelector<HTMLElement>(`[data-asset="${CSS.escape(id)}"] .photos-open`)?.focus());}},[open]);

  const ids=[...selected];
  const allFavorite=ids.length>0&&ids.every(id=>items.find(i=>i.id===id)?.favorite);
  const addMenu:MenuAction[]=[{label:"New album…",icon:"plus",run:()=>setAlbumDialog({ids})},...albums.filter(a=>view.type!=="album"||a.id!==view.id).map((a,n)=>({label:a.name,icon:"image" as IconName,separator:n===0,run:()=>{void addTo(a,ids);}}))];
  const albumActions=(a:PhotoAlbum):MenuAction[]=>[{label:"Rename album",icon:"pencil",run:()=>setAlbumDialog({album:a})},{label:"Delete album…",icon:"trash",danger:true,separator:true,run:()=>setConfirm(a)}];
  const sidebar=<>
    <NewItemButton onClick={()=>fileInput.current?.click()}>Upload</NewItemButton>
    <input ref={fileInput} type="file" multiple hidden accept="image/*,video/*,.heic,.heif,.dng,.cr2,.cr3,.nef,.arw,.orf,.rw2,.raf" aria-label="Upload photos and videos" onChange={e=>{void upload(e.target.files);e.target.value="";}}/>
    <nav aria-label="Photos views">{views.map(([id,label,icon])=>{const n=overview?.counts[id];return <button key={id} className={`nav-item${view.type===id?" active":""}`} aria-current={view.type===id?"page":undefined} onClick={()=>choose({type:id})}><Icon name={icon}/><span className="photos-row-title">{label}</span>{!!n&&<span className="photos-count">{n.toLocaleString()}</span>}</button>;})}</nav>
    <div className="sidebar-heading photos-heading"><h2>Albums</h2><button type="button" className="icon-button" aria-label="New album" title="New album" onClick={()=>setAlbumDialog({})}><Icon name="plus"/></button></div>
    <nav aria-label="Albums" className="photos-albums">
      {albums.map(a=><div className="photos-row" key={a.id}><button className={`nav-item${view.type==="album"&&view.id===a.id?" active":""}`} aria-current={view.type==="album"&&view.id===a.id?"page":undefined} onClick={()=>choose({type:"album",id:a.id})}>
        {a.coverId?<img className="photos-album-cover" src={thumb(a.coverId)} alt="" loading="lazy"/>:<span className="photos-album-cover is-empty" aria-hidden="true"><Icon name="image"/></span>}<span className="photos-row-title">{a.name}</span><span className="photos-count">{a.count.toLocaleString()}</span></button>
        <ActionMenu label={`Actions for ${a.name}`} className="photos-row-menu" actions={albumActions(a)}><Icon name="more"/></ActionMenu></div>)}
      {overview&&!albums.length&&<p className="photos-hint">Albums you create appear here.</p>}
    </nav>
    {overview?.indexing&&<p className="photos-indexing" role="status"><span className="photos-spinner" aria-hidden="true"/>Finding photos in Files…</p>}
    <button type="button" className="nav-item ui-app-settings-entry" onClick={()=>setSettings(true)}><Icon name="settings"/>Photos settings</button>
  </>;

  let lastMonth="";
  const columns=zoomSteps.includes(zoom)?zoom:5;
  const header=ids.length?<div className="photos-selection" role="toolbar" aria-label="Selection">
      <button type="button" className="icon-button" aria-label="Clear selection" onClick={()=>setSelected(new Set())}><Icon name="close"/></button><strong>{ids.length} selected</strong><span className="photos-spacer"/>
      <button type="button" className="icon-button" aria-label={allFavorite?"Remove from Favorites":"Add to Favorites"} title={allFavorite?"Remove from Favorites":"Add to Favorites"} aria-pressed={allFavorite} onClick={()=>{void update(ids,{favorite:!allFavorite},{favorite:allFavorite});}}><Icon name="star"/></button>
      <ActionMenu label="Add to album" actions={addMenu}><Icon name="plus"/></ActionMenu>
      {view.type==="album"&&<button type="button" className="icon-button" aria-label="Remove from album" title="Remove from album" onClick={()=>{void removeFromAlbum(ids);}}><Icon name="close"/></button>}
      <button type="button" className="icon-button" aria-label={view.type==="archive"?"Restore to library":"Archive"} title={view.type==="archive"?"Restore to library":"Archive (hidden from the library, kept in albums and Files)"} onClick={()=>{void update(ids,{archived:view.type!=="archive"},{archived:view.type==="archive"});}}><Icon name={view.type==="archive"?"undo":"lock"}/></button>
    </div>:<div className="photos-list-heading"><h1>{title}</h1>{state==="ready"&&<span className="photos-heading-count">{(view.type==="album"?album?.count:overview?.counts[view.type])?.toLocaleString()??items.length}</span>}<div className="photos-tools">
      <label className="photos-zoom"><Icon name="grid"/><span className="sr-only">Columns</span><input type="range" min={0} max={zoomSteps.length-1} step={1} value={zoomSteps.indexOf(columns)} aria-valuetext={`${columns} columns`} onChange={e=>setZoom(zoomSteps[Number(e.target.value)])}/></label>
      {album&&<ActionMenu label={`Actions for ${album.name}`} actions={albumActions(album)}><Icon name="more"/></ActionMenu>}
    </div></div>;
  const gridView=<div className={`photos-grid-scroll${dragging?" is-dropping":""}`} ref={grid} onDragOver={e=>{if(e.dataTransfer.types.includes("Files")){e.preventDefault();setDragging(true);}}} onDragLeave={e=>{if(e.currentTarget===e.target)setDragging(false);}} onDrop={e=>{e.preventDefault();setDragging(false);void upload(e.dataTransfer.files);}}>
    {(overviewError||state==="error")&&<div className="photos-empty" role="alert"><h2>Photos could not be loaded</h2><p>{listError||overviewError}</p><button className="secondary-button" onClick={()=>{void loadOverview();void loadItems();}}>Retry</button></div>}
    {state==="loading"&&!items.length&&<p className="photos-empty" role="status">Loading photos…</p>}
    {state==="ready"&&!items.length&&<div className="photos-empty">{view.type==="library"?<><Icon name="image"/><h2>{overview?.indexing?"Looking for photos…":"No photos yet"}</h2><p>Upload photos and videos, or drop them here. Photos also shows the pictures and videos already in your Files.</p><button className="primary-button" onClick={()=>fileInput.current?.click()}><Icon name="upload"/>Upload</button></>:view.type==="favorites"?<><h2>No favorites</h2><p>Star a photo to keep it here.</p></>:view.type==="archive"?<><h2>Archive is empty</h2><p>Archived photos are hidden from the library but stay in albums and Files.</p></>:view.type==="videos"?<><h2>No videos</h2><p>Videos you upload or keep in Files appear here.</p></>:<><h2>This album is empty</h2><p>Select photos in the library and choose Add to album.</p></>}</div>}
    <div className="photos-grid" role="list" aria-label={`${title} photos`} data-columns={columns}>
      {items.map(item=>{const month=monthLabel(item.takenAt),heading=month!==lastMonth?(lastMonth=month,<h2 className="photos-month" key={"m-"+month}>{month}</h2>):null,isSelected=selected.has(item.id);
        return <Fragment key={item.id}>{heading}<div role="listitem" data-asset={item.id} className={`photos-tile${isSelected?" is-selected":""}`}>
          <button type="button" className="photos-open" aria-label={`${item.kind==="video"?"Video":"Photo"} ${item.name}, ${new Date(item.takenAt).toLocaleString([],{dateStyle:"medium",timeStyle:"short"})}${item.favorite?", favorite":""}`} onClick={event=>{if(selected.size||event.shiftKey){setSelected(s=>{const n=new Set(s);if(n.has(item.id))n.delete(item.id);else n.add(item.id);return n;});}else setOpen(item.id);}}>
            <Thumb asset={item}/>{item.kind==="video"&&<span className="photos-badge"><Icon name="video"/>{item.duration?duration(item.duration):""}</span>}{item.favorite&&<span className="photos-fav"><Icon name="star"/></span>}
          </button>
          <input type="checkbox" className="photos-check" aria-label={`Select ${item.name}`} checked={isSelected} onChange={()=>setSelected(s=>{const n=new Set(s);if(n.has(item.id))n.delete(item.id);else n.add(item.id);return n;})}/>
        </div></Fragment>;})}
    </div>
    {cursor&&state==="ready"&&<div className="photos-more" ref={sentinel}><button className="text-button" onClick={()=>{void loadMore();}}>Load more</button></div>}
  </div>;

  return <div className={`app-shell pim-shell photos-shell${current?" has-pim-details":""}${current&&info?" is-info":""}`}>
    <WorkspaceHeader app="photos" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId} onSettings={()=>setSettings(true)}/>
    <div className="pim-workspace photos-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar photos-sidebar${navigation?" is-open":""}`} aria-label="Photos navigation">{sidebar}</aside>
      <main className="pim-main photos-main" tabIndex={-1} aria-label={current?current.name:title}>
        {current?<Viewer asset={current} items={items} index={index} albumView={view.type==="album"} onInfo={()=>{flushSync(()=>setInfo(true));document.querySelector<HTMLElement>(".photos-detail-back")?.focus();}} onClose={close} onStep={step} onOpen={setOpen}
          onFavorite={()=>{void update([current.id],{favorite:!current.favorite});}} onArchive={()=>{void update([current.id],{archived:!current.archived},{archived:current.archived});}} onRemove={()=>{void removeFromAlbum([current.id]);}} addMenu={[{label:"New album…",icon:"plus",run:()=>setAlbumDialog({ids:[current.id]})},...albums.map((a,n)=>({label:a.name,icon:"image" as IconName,separator:n===0,run:()=>{void addTo(a,[current.id]);}}))]}/>
          :open&&state==="ready"?<div className="photos-empty" role="alert"><h2>This photo is not available</h2><p>It may have been moved to the trash, archived or removed from this view.</p><button className="secondary-button" onClick={()=>setOpen(undefined)}>Back to {title}</button></div>
          :<>{header}{gridView}</>}
      </main>
      {current&&<aside className="pim-details photos-details" tabIndex={-1} aria-label="Photo details"><Details id={current.id} session={session} onBack={()=>{flushSync(()=>setInfo(false));document.querySelector<HTMLElement>(".photos-info-toggle")?.focus();}} onChanged={change=>patchLocal([current.id],change)} onAlbum={id=>choose({type:"album",id})} onError={fail}/></aside>}
    </div>
    {transfers.length>0&&<section className="transfer-panel photos-transfers" aria-label="Uploads"><div className="transfer-heading"><strong>{transfers.some(t=>t.status==="uploading")?`Uploading ${transfers.filter(t=>t.status==="uploading").length}`:"Uploads"}</strong><button className="icon-button" aria-label="Dismiss finished uploads" disabled={transfers.some(t=>t.status==="uploading")} onClick={()=>setTransfers([])}><Icon name="close"/></button></div>
      <ul>{transfers.map(t=><li key={t.id} className={`photos-transfer is-${t.status}`}><span>{t.name}</span><progress max={1} value={t.progress} aria-label={`${t.name} upload progress`}/>{t.message?<small role="alert">{t.message}</small>:<small>{t.status==="complete"?"Done":`${Math.round(t.progress*100)}%`}</small>}</li>)}</ul></section>}
    {albumDialog&&<AlbumDialog album={albumDialog.album} ids={albumDialog.ids} session={session} onClose={()=>setAlbumDialog(undefined)} onSaved={async(saved,created)=>{setAlbumDialog(undefined);setSelected(new Set());await loadOverview();if(created)setNotice({message:albumDialog.ids?.length?`Added ${albumDialog.ids.length} to ${saved.name}`:`Created ${saved.name}`});}}/>}
    {confirm&&<Dialog title={`Delete ${confirm.name}?`} onClose={()=>setConfirm(undefined)}><ConfirmBody album={confirm} session={session} onClose={()=>setConfirm(undefined)} onDeleted={async()=>{if(view.type==="album"&&view.id===confirm.id)choose({type:"library"});setConfirm(undefined);await loadOverview();setNotice({message:`Deleted ${confirm.name}`});}}/></Dialog>}
    {settings&&<PhotosSettings overview={overview} onClose={()=>setSettings(false)}/>}
    <div className="photos-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast photos-toast"><span>{notice.message}</span>{notice.undo&&<button type="button" className="text-button" onClick={()=>{const undo=notice.undo!;setNotice(null);undo();}}>Undo</button>}</div>}</div>
  </div>;
}

function Thumb({asset,size="s"}:{asset:PhotoAsset;size?:"s"|"m"}){
  const [failed,setFailed]=useState(false);
  useEffect(()=>setFailed(false),[asset.id,asset.revision]);
  if(failed||asset.thumbnail==="failed"&&asset.kind==="video")return <span className="photos-placeholder"><Icon name={asset.kind==="video"?"video":"image"}/><small>{asset.name}</small></span>;
  return <img src={`${thumb(asset.id,size)}${size==="m"?"&":"?"}v=${asset.revision}`} alt="" loading="lazy" decoding="async" draggable={false} onError={()=>setFailed(true)}/>;
}

function Viewer({asset,items,index,albumView,addMenu,onInfo,onClose,onStep,onOpen,onFavorite,onArchive,onRemove}:{asset:PhotoAsset;items:PhotoAsset[];index:number;albumView:boolean;addMenu:MenuAction[];onInfo:()=>void;onClose:()=>void;onStep:(delta:number)=>void;onOpen:(id:string)=>void;onFavorite:()=>void;onArchive:()=>void;onRemove:()=>void}){
  const heading=useRef<HTMLHeadingElement>(null),strip=useRef<HTMLDivElement>(null);
  useEffect(()=>{heading.current?.focus();},[]);
  useEffect(()=>{strip.current?.querySelector(".is-current")?.scrollIntoView({block:"nearest",inline:"center"});},[asset.id]);
  const more:MenuAction[]=[{label:"Add to album",icon:"plus",items:addMenu},{label:asset.archived?"Restore to library":"Archive",icon:asset.archived?"undo":"lock",run:onArchive},...albumView?[{label:"Remove from album",icon:"close" as IconName,run:onRemove}]:[],{label:"Download original",icon:"download",href:`/api/v1/files/${asset.id}/content`,download:true,separator:true}];
  const near=items.slice(Math.max(0,index-12),index+13);
  return <section className="photos-viewer" aria-label={`Viewer for ${asset.name}`}>
    <div className="photos-viewer-bar">
      <button type="button" className="icon-button" aria-label="Back to photos" title="Back (Escape)" onClick={onClose}><Icon name="back"/></button>
      <h1 ref={heading} tabIndex={-1}>{asset.name}</h1><span className="photos-heading-count">{index+1} of {items.length}</span><span className="photos-spacer"/>
      <button type="button" className="icon-button" aria-label="Previous" title="Previous (←)" disabled={index<=0} onClick={()=>onStep(-1)}><Icon name="back"/></button>
      <button type="button" className="icon-button" aria-label="Next" title="Next (→)" disabled={index>=items.length-1} onClick={()=>onStep(1)}><Icon name="chevron"/></button>
      <button type="button" className={`icon-button${asset.favorite?" is-on":""}`} aria-label={asset.favorite?"Remove from Favorites":"Add to Favorites"} aria-pressed={asset.favorite} title="Favorite (f)" onClick={onFavorite}><Icon name="star"/></button>
      <button type="button" className="icon-button photos-info-toggle" aria-label="Photo info" onClick={onInfo}><Icon name="info"/></button>
      <ActionMenu label="More photo actions" actions={more}><Icon name="more"/></ActionMenu>
    </div>
    <div className="photos-stage">{asset.kind==="video"?<video key={asset.id} src={content(asset.id)} controls playsInline preload="metadata" poster={thumb(asset.id,"m")} aria-label={asset.name}/>:<StageImage asset={asset}/>}</div>
    <div className="photos-strip" ref={strip} aria-label="Nearby photos">{near.map(item=><button type="button" key={item.id} className={item.id===asset.id?"is-current":""} aria-label={item.name} aria-current={item.id===asset.id?"true":undefined} onClick={()=>onOpen(item.id)}><Thumb asset={item}/></button>)}</div>
  </section>;
}
/** Shows the large preview, falling back to the original for browser-decodable files. */
function StageImage({asset}:{asset:PhotoAsset}){
  const [source,setSource]=useState<"m"|"original"|"none">("m");
  useEffect(()=>setSource("m"),[asset.id]);
  if(source==="none")return <div className="photos-empty"><Icon name="image"/><h2>No preview available</h2><p>Dar could not render this file. You can still download the original.</p><a className="secondary-button" href={`/api/v1/files/${asset.id}/content`} download><Icon name="download"/>Download</a></div>;
  return <img key={asset.id+source} src={source==="m"?`${thumb(asset.id,"m")}&v=${asset.revision}`:content(asset.id)} alt={asset.name} onError={()=>setSource(source==="m"&&/^image\/(jpeg|png|gif|webp|avif)$/.test(asset.mediaType)?"original":"none")}/>;
}

function Details({id,session,onBack,onChanged,onAlbum,onError}:{id:string;session:BrowserSession;onBack:()=>void;onChanged:(change:Partial<PhotoAsset>)=>void;onAlbum:(id:string)=>void;onError:(cause:unknown)=>void}){
  const [detail,setDetail]=useState<PhotoDetail>(),[error,setError]=useState(""),[description,setDescription]=useState(""),[saving,setSaving]=useState<""|"saving"|"saved">(""),[conflict,setConflict]=useState(false);
  const load=useCallback(async()=>{setError("");try{const value=await request<PhotoDetail>(`${base}/assets/${id}`);setDetail(value);setDescription(value.description);setConflict(false);}catch(cause){setError(message(cause));}},[id]);
  useEffect(()=>{setDetail(undefined);setSaving("");void load();},[load]);
  async function save(){
    if(!detail||description.trim()===detail.description)return;setSaving("saving");
    const put=(revision:number)=>request<PhotoAsset>(`${base}/assets/${id}/description`,{method:"PUT",body:JSON.stringify({description,revision})},session.csrfToken);
    try{
      let value:PhotoAsset;
      try{value=await put(detail.revision);}
      catch(cause){
        // Favorite or archive changes also advance the revision; only a different description is a real conflict.
        if(!(cause instanceof RequestError&&cause.status===409))throw cause;
        const fresh=await request<PhotoDetail>(`${base}/assets/${id}`);if(fresh.description!==detail.description){setDetail(fresh);throw cause;}
        value=await put(fresh.revision);
      }
      setDetail(current=>current&&{...current,...value,description:description.trim()});onChanged({revision:value.revision});setSaving("saved");
    }
    catch(cause){setSaving("");if(cause instanceof RequestError&&cause.status===409)setConflict(true);else onError(cause);}
  }
  if(error)return <div className="photos-empty" role="alert"><h2>Details could not be loaded</h2><p>{error}</p><button className="secondary-button" onClick={()=>{void load();}}>Retry</button></div>;
  if(!detail)return <p className="photos-empty" role="status">Loading details…</p>;
  const e=detail.exif,camera=[e.make&&e.model&&!e.model.startsWith(e.make)?e.make:"",e.model].filter(Boolean).join(" "),megapixels=detail.width&&detail.height?`${(detail.width*detail.height/1e6).toFixed(1)} MP · `:"";
  const settings=[e.focalLength?`${Math.round(e.focalLength)} mm`:"",e.fNumber?`ƒ/${e.fNumber}`:"",e.exposure?(e.exposure<1?`1/${Math.round(1/e.exposure)} s`:`${e.exposure} s`):"",e.iso?`ISO ${e.iso}`:""].filter(Boolean).join(" · ");
  return <div className="photos-detail">
    <div className="photos-detail-head"><button type="button" className="icon-button photos-detail-back" aria-label="Back to photo" onClick={onBack}><Icon name="back"/></button><h2>Details</h2></div>
    <div className="photos-detail-body">
      <label className="photos-description"><span className="sr-only">Description</span><textarea value={description} maxLength={2000} rows={2} placeholder="Add a description…" onChange={event=>{setDescription(event.target.value);setSaving("");}} onBlur={()=>{void save();}}/></label>
      {saving&&<p className="photos-saved" role="status">{saving==="saving"?"Saving…":"Saved"}</p>}
      {conflict&&<div className="error" role="alert"><p>This description changed elsewhere.</p><button className="text-button" onClick={()=>{void load();}}>Reload</button></div>}
      <ul className="photos-props">
        <li><Icon name="calendar"/><span><strong>{new Date(detail.takenAt).toLocaleDateString([],{weekday:"long",year:"numeric",month:"long",day:"numeric"})}</strong><small>{new Date(detail.takenAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit",timeZoneName:"short"})}</small></span></li>
        <li><Icon name={detail.kind==="video"?"video":"image"}/><span><strong>{detail.name}</strong><small>{megapixels}{detail.width?`${detail.width} × ${detail.height} · `:""}{detail.kind==="video"&&detail.duration?`${duration(detail.duration)} · `:""}{formatBytes(detail.size)}</small></span></li>
        {(camera||e.lens||settings)&&<li><Icon name="image"/><span><strong>{camera||"Camera"}</strong><small>{[e.lens,settings].filter(Boolean).join(" · ")}</small></span></li>}
        {detail.latitude!==null&&detail.longitude!==null&&<li><Icon name="location"/><span><strong>Location</strong><small>{detail.latitude.toFixed(5)}, {detail.longitude.toFixed(5)}</small></span></li>}
      </ul>
      <h3 className="photos-detail-section">Albums</h3>
      {detail.albums.length?<div className="photos-chips">{detail.albums.map(a=><button type="button" key={a.id} className="photos-chip" onClick={()=>onAlbum(a.id)}>{a.name}</button>)}</div>:<p className="photos-hint">Not in any album.</p>}
      <a className="secondary-button photos-in-files" href={`/?folder=${detail.parentId}&file=${detail.id}`}><Icon name="folder"/>Show in Files</a>
    </div>
  </div>;
}

function AlbumDialog({album,ids,session,onClose,onSaved}:{album?:PhotoAlbum;ids?:string[];session:BrowserSession;onClose:()=>void;onSaved:(album:PhotoAlbum,created:boolean)=>Promise<void>}){
  const [name,setName]=useState(album?.name??""),[error,setError]=useState(""),[busy,setBusy]=useState(false);
  async function submit(event:FormEvent){
    event.preventDefault();if(!name.trim())return setError("Enter an album name.");setBusy(true);setError("");
    try{const saved=album?await request<PhotoAlbum>(`${base}/albums/${album.id}`,{method:"PATCH",body:JSON.stringify({name:name.trim(),revision:album.revision})},session.csrfToken)
      :await request<PhotoAlbum>(`${base}/albums`,{method:"POST",body:JSON.stringify({name:name.trim(),...ids?.length?{ids}:{}})},session.csrfToken);await onSaved(saved,!album);}
    catch(cause){setError(message(cause));setBusy(false);}
  }
  return <Dialog title={album?"Rename album":"New album"} onClose={onClose} dirty={name!==(album?.name??"")}><form onSubmit={event=>{void submit(event);}}>
    <label className="field">Name<input autoFocus value={name} maxLength={200} onChange={e=>setName(e.target.value)} aria-invalid={!!error}/></label>
    {ids&&ids.length>0&&<p>{ids.length} selected item{ids.length===1?"":"s"} will be added.</p>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy}>{busy?"Saving…":album?"Rename":"Create album"}</button></div>
  </form></Dialog>;
}
function ConfirmBody({album,session,onClose,onDeleted}:{album:PhotoAlbum;session:BrowserSession;onClose:()=>void;onDeleted:()=>Promise<void>}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  return <><p>The album is removed. Its {album.count.toLocaleString()} photo{album.count===1?"":"s"} stay in your library and in Files.</p>{error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button className="text-button" onClick={onClose}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setBusy(true);void request(`${base}/albums/${album.id}`,{method:"DELETE"},session.csrfToken).then(onDeleted,cause=>{setError(message(cause));setBusy(false);});}}>{busy?"Deleting…":"Delete album"}</button></div></>;
}

function PhotosSettings({overview,onClose}:{overview?:PhotoOverview;onClose:()=>void}){
  const [section,setSection]=useState("General");
  return <SettingsSheet title="Photos settings" icon="image" sections={["General","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={onClose} footer="Photos keeps favorites, archive, descriptions and albums with your account.">
    {section==="General"&&<section className="settings-group" aria-label="Library"><h3>Library</h3>
      <SettingRow label="Where photos come from" hint="Every picture and video you own in Files, including RAW, HEIC and videos. Shared files from others are not included." stacked><a className="secondary-button" href="/">Open Files</a></SettingRow>
      <SettingRow label="Uploads" hint="Uploads go to Files › Photos › year › month. Same-named files are kept with a number."><span className="photos-hint">Photos/YYYY/MM</span></SettingRow>
      <SettingRow label="Indexing" hint="New and changed files are picked up in the background">{overview?.indexing?<span className="photos-indexing"><span className="photos-spinner" aria-hidden="true"/>Working…</span>:<span className="photos-hint">Up to date</span>}</SettingRow></section>}
    {section==="Keyboard shortcuts"&&<dl className="ui-shortcuts"><dt>Shift click</dt><dd>Select a photo</dd><dt>← / →</dt><dd>Previous or next photo</dd><dt>f</dt><dd>Favorite or unfavorite</dd><dt>Escape</dt><dd>Close the photo</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd></dl>}
  </SettingsSheet>;
}
