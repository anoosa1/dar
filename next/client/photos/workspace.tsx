import {Fragment,useCallback,useEffect,useMemo,useRef,useState,type FormEvent} from "react";
import {request,formatBytes,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {ActionMenu,NewItemButton,type MenuAction} from "../controls.tsx";
import {Dialog} from "../dialog.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {SettingsSheet,SettingRow} from "../settings-sheet.tsx";
import {usePreferenceSaver,SaveProblems} from "../app-settings.tsx";
import {useLockedFolder,UnlockForm,UnlockDialog,type LockedFolder} from "../locked-folder.tsx";
import {photoFolderParts,type PhotoAlbum,type PhotoAsset,type PhotoDetail,type PhotoOverview,type PhotoPage,type PhotoView,type PhotoStack,type PhotoMoved,type PhotoDuplicateGroup,type PhotoDuplicatePage,type PhotoCounts} from "../../shared/photos.ts";
import {flushSync} from "react-dom";
import {SharePane,LinkDialog,LinksList} from "./sharing.tsx";
import {ImmichSettings} from "./immich-settings.tsx";
import "./photos.css";

const base="/api/v1/photos";
type ListView=Exclude<PhotoView,"album">|"duplicates"|"shared";
type View={type:ListView}|{type:"album";id:string};
interface Notice {message:string;undo?:()=>void}
interface Transfer {id:string;name:string;progress:number;status:"uploading"|"complete"|"error";message?:string}
const mainViews:[ListView,string,IconName][]=[["library","Library","image"],["favorites","Favorites","star"],["shared","Shared with me","share"],["videos","Videos","video"]];
const moreViews:[ListView,string,IconName][]=[["duplicates","Duplicates","copy"],["archive","Archive","archive"],["locked","Locked folder","lock"],["trash","Trash","trash"]];
const allViews=[...mainViews,...moreViews];
const zoomSteps=[3,4,5,6,8,10,12];

function readView():{view:View;asset?:string}{
  const p=new URLSearchParams(location.search),album=p.get("album"),v=p.get("view");
  return {view:album?{type:"album",id:album}:allViews.some(([id])=>id===v)?{type:v as ListView}:{type:"library"},asset:p.get("asset")??undefined};
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
const noun=(n:number)=>`${n} item${n===1?"":"s"}`;
const lockedError=(cause:unknown)=>cause instanceof RequestError&&cause.status===423;

/** Photos (selected mock B): views and albums in the sidebar, a month-grouped grid, the photo in the middle sheet and details on the right. */
export function PhotosWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readView,[]);
  const [overview,setOverview]=useState<PhotoOverview>(),[overviewError,setOverviewError]=useState(""),[view,setView]=useState<View>(start.view);
  const [items,setItems]=useState<PhotoAsset[]>([]),[cursor,setCursor]=useState(""),[state,setState]=useState<"loading"|"ready"|"error">("loading"),[listError,setListError]=useState("");
  const [open,setOpen]=useState<string|undefined>(start.asset),[selected,setSelected]=useState<Set<string>>(new Set()),[zoom,setZoom]=useState(()=>Number(localStorage.getItem("dar:photos:zoom"))||5);
  const [notice,setNotice]=useState<Notice|null>(null),[albumDialog,setAlbumDialog]=useState<{album?:PhotoAlbum;ids?:string[]}>(),[confirm,setConfirm]=useState<PhotoAlbum>(),[settings,setSettings]=useState(false);
  const [transfers,setTransfers]=useState<Transfer[]>([]),[dragging,setDragging]=useState(false),[info,setInfo]=useState(false),[member,setMember]=useState<PhotoAsset>();
  const [unlockAction,setUnlockAction]=useState<{action:string;run:()=>void}|null>(null),[deleting,setDeleting]=useState<{ids:string[];empty?:boolean}>();
  const [sharing,setSharing]=useState<PhotoAlbum>(),[linkFor,setLinkFor]=useState<string[]>();
  const folder=useLockedFolder(session);
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const generation=useRef(0),shownKey=useRef(""),fileInput=useRef<HTMLInputElement>(null),sentinel=useRef<HTMLDivElement>(null),grid=useRef<HTMLDivElement>(null);
  const albums=overview?.albums??[],sharedAlbums=overview?.sharedAlbums??[],album=view.type==="album"?[...albums,...sharedAlbums].find(a=>a.id===view.id):undefined,albumOwner=album?.role==="owner";
  const title=view.type==="album"?album?.name??"Album":allViews.find(([id])=>id===view.type)![1];
  const fail=(cause:unknown)=>setNotice({message:message(cause)});
  const lockedClosed=view.type==="locked"&&!folder.unlocked;
  /** Runs work that may need the Locked folder; a 423 asks for the password, then retries. */
  async function guard(work:()=>Promise<void>,action:string){
    try{await work();}
    catch(cause){if(lockedError(cause)){void folder.refresh();setUnlockAction({action,run:()=>{void guard(work,action);}});}else fail(cause);}
  }

  const loadOverview=useCallback(async()=>{try{setOverview(await request<PhotoOverview>(`${base}/overview`));setOverviewError("");}catch(cause){setOverviewError(message(cause));}},[]);
  const query=(after="")=>{const p=new URLSearchParams({view:view.type,limit:"120"});if(view.type==="album")p.set("album",view.id);if(after)p.set("cursor",after);return `${base}/assets?${p}`;};
  const viewKey=view.type==="album"?"album:"+view.id:view.type;
  const loadItems=useCallback(async()=>{
    const ticket=++generation.current;setState("loading");setSelected(new Set());
    // A different view never shows the previous view's photos; refreshes of the same view keep them while loading.
    if(shownKey.current!==viewKey){shownKey.current=viewKey;setItems([]);setCursor("");}
    if(view.type==="duplicates"||view.type==="shared"||lockedClosed){setItems([]);setCursor("");setState("ready");return;}
    try{const page=await request<PhotoPage>(query());if(ticket!==generation.current)return;setItems(page.items);setCursor(page.cursor);setState("ready");grid.current?.scrollTo({top:0});}
    catch(cause){if(ticket!==generation.current)return;if(lockedError(cause)){void folder.refresh();setItems([]);setState("ready");return;}setState("error");setListError(message(cause));}
  },[viewKey,lockedClosed]);
  const loadMore=useCallback(async()=>{
    if(!cursor||state!=="ready")return;const ticket=generation.current,page=await request<PhotoPage>(query(cursor)).catch(()=>undefined);
    if(!page||ticket!==generation.current)return;setItems(current=>[...current,...page.items.filter(i=>!current.some(c=>c.id===i.id))]);setCursor(page.cursor);
  },[cursor,state,viewKey]);
  useEffect(()=>{void loadOverview();},[loadOverview]);
  useEffect(()=>{void loadItems();},[loadItems]);
  useEffect(()=>{writeView(view,open);if(!open)setInfo(false);setMember(undefined);},[view,open]);
  useEffect(()=>{localStorage.setItem("dar:photos:zoom",String(zoom));},[zoom]);
  // When the Locked folder relocks, its photos leave the screen.
  useEffect(()=>{if(lockedClosed&&open)setOpen(undefined);},[lockedClosed]);
  // While the background scan is running, refresh so newly indexed media appears.
  const latest=useRef({loadItems,busy:false});latest.current={loadItems,busy:selected.size>0||!!open};
  useEffect(()=>{if(!overview?.indexing)return;const timer=setTimeout(()=>{void loadOverview();if(!latest.current.busy)void latest.current.loadItems();},8000);return()=>clearTimeout(timer);},[overview]);
  useEffect(()=>{const node=sentinel.current;if(!node||!cursor)return;const observer=new IntersectionObserver(entries=>{if(entries.some(e=>e.isIntersecting))void loadMore();},{root:grid.current,rootMargin:"600px"});observer.observe(node);return()=>observer.disconnect();},[cursor,loadMore]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(null),notice.undo?8000:4000);return()=>clearTimeout(timer);},[notice]);

  function choose(next:View){const key=next.type==="album"?"album:"+next.id:next.type;if(key!==viewKey){setItems([]);setCursor("");setState("loading");}setView(next);setOpen(undefined);setNavigation(false);}
  const patchLocal=(ids:string[],change:Partial<PhotoAsset>)=>setItems(current=>current.map(i=>ids.includes(i.id)?{...i,...change}:i));
  const removeLocal=(ids:string[])=>{setItems(current=>current.filter(i=>!ids.includes(i.id)));setSelected(new Set());if(open&&ids.includes(open))setOpen(undefined);};
  const post=<T,>(path:string,value:unknown)=>request<T>(`${base}${path}`,{method:"POST",body:JSON.stringify(value)},session.csrfToken);
  const refresh=()=>Promise.all([loadItems(),loadOverview()]);
  async function update(ids:string[],change:{favorite?:boolean;archived?:boolean},undo?:{favorite?:boolean;archived?:boolean}){
    await guard(async()=>{
      await request(`${base}/assets`,{method:"PATCH",body:JSON.stringify({ids,...change})},session.csrfToken);
      const leaves=change.archived!==undefined&&(view.type==="archive"?!change.archived:view.type!=="album"&&view.type!=="favorites"&&view.type!=="locked"&&change.archived)||change.favorite===false&&view.type==="favorites";
      if(leaves)removeLocal(ids);else{patchLocal(ids,change);setSelected(new Set());}
      void loadOverview();
      if(undo)setNotice({message:change.archived!==undefined?(change.archived?`${noun(ids.length)} archived`:`${noun(ids.length)} restored to the library`):change.favorite?`${noun(ids.length)} added to Favorites`:`${noun(ids.length)} removed from Favorites`,undo:()=>{void update(ids,undo).then(()=>loadItems());}});
    },"change these photos");
  }
  async function trash(ids:string[]){
    await guard(async()=>{
      const {trashed}=await post<{trashed:string[]}>("/assets/trash",{ids});removeLocal(trashed);void loadOverview();
      setNotice({message:`${noun(ids.length)} moved to the trash`,undo:()=>{void post("/assets/restore",{ids:trashed}).then(refresh,fail);}});
    },"move these photos");
  }
  async function restore(ids:string[]){
    await guard(async()=>{
      const {restored}=await post<{restored:string[]}>("/assets/restore",{ids});removeLocal(restored);void loadOverview();
      setNotice({message:`${noun(ids.length)} restored`,undo:()=>{void post("/assets/trash",{ids:restored}).then(refresh,fail);}});
    },"restore these photos");
  }
  async function lockPhotos(ids:string[]){
    await guard(async()=>{
      const {moved}=await post<{moved:PhotoMoved[]}>("/assets/lock",{ids});removeLocal(moved.map(m=>m.id));void loadOverview();
      setNotice({message:`${noun(ids.length)} moved to the Locked folder`,undo:()=>{void guard(async()=>{await post("/assets/unlock",{ids:moved.map(m=>m.id),to:moved});await refresh();},"move them back");}});
    },"move photos into it");
  }
  async function unlockPhotos(ids:string[]){
    await guard(async()=>{
      const {moved}=await post<{moved:string[]}>("/assets/unlock",{ids});removeLocal(moved);void loadOverview();
      setNotice({message:`${noun(ids.length)} moved to ${overview?.uploadFolder??"your Photos folder"}`,undo:()=>{void guard(async()=>{await post("/assets/lock",{ids:moved});await refresh();},"move them back");}});
    },"move photos out of it");
  }
  async function stack(ids:string[]){
    await guard(async()=>{
      const created=await post<{id:string}>("/stacks",{ids});await refresh();
      setNotice({message:`Stacked ${ids.length} photos`,undo:()=>{void request(`${base}/stacks/${created.id}`,{method:"DELETE"},session.csrfToken).then(refresh,fail);}});
    },"stack these photos");
  }
  async function unstack(value:PhotoStack){
    await guard(async()=>{
      await request(`${base}/stacks/${value.id}`,{method:"DELETE"},session.csrfToken);setMember(undefined);await refresh();
      setNotice({message:`Unstacked ${value.items.length} photos`,undo:()=>{void post("/stacks",{ids:[value.primaryId,...value.items.map(i=>i.id).filter(id=>id!==value.primaryId)]}).then(refresh,fail);}});
    },"unstack these photos");
  }
  async function leave(target:PhotoAlbum){
    try{await request(`${base}/albums/${target.id}/members/${session.user.id}`,{method:"DELETE"},session.csrfToken);if(view.type==="album"&&view.id===target.id)choose({type:"shared"});await loadOverview();setNotice({message:`You left ${target.name}`});}
    catch(cause){fail(cause);}
  }
  async function addTo(target:PhotoAlbum,ids:string[]){
    try{const result=await post<PhotoAlbum&{added:number}>(`/albums/${target.id}/add`,{ids});setSelected(new Set());await loadOverview();setNotice({message:result.added?`Added ${result.added} to ${target.name}`:`Already in ${target.name}`});}
    catch(cause){fail(cause);}
  }
  async function removeFromAlbum(ids:string[]){
    if(view.type!=="album")return;const id=view.id;
    try{await post(`/albums/${id}/remove`,{ids});removeLocal(ids);await loadOverview();
      setNotice({message:`Removed ${ids.length} from ${album?.name??"the album"}`,undo:()=>{void post(`/albums/${id}/add`,{ids}).then(refresh).catch(fail);}});}
    catch(cause){fail(cause);}
  }

  async function upload(files:FileList|File[]|null){
    const list=[...files??[]].filter(f=>/^(image|video)\//.test(f.type)||/\.(heic|heif|dng|cr2|cr3|nef|arw|orf|rw2|raf|mov|mkv)$/i.test(f.name));
    if(!list.length){if(files&&[...files].length)setNotice({message:"Only photos and videos can be uploaded here. Use Files for other documents."});return;}
    for(const file of list){
      const transferId=crypto.randomUUID(),change=(next:Partial<Transfer>)=>setTransfers(current=>current.map(t=>t.id===transferId?{...t,...next}:t));
      setTransfers(current=>[...current,{id:transferId,name:file.name,progress:0,status:"uploading"}]);let uploadId="";
      try{
        const when=new Date(file.lastModified||Date.now()),{parentId}=await post<{parentId:string}>("/upload-target",{year:when.getFullYear(),month:when.getMonth()+1});
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

  const index=open?items.findIndex(i=>i.id===open):-1,current=index>=0?items[index]:undefined,shown=member??current;
  const step=(delta:number)=>{if(index<0)return;const next=items[index+delta];if(next)setOpen(next.id);if(index+delta>=items.length-8)void loadMore();};
  const inTrash=view.type==="trash",inLocked=view.type==="locked";
  useEffect(()=>{
    if(!open)return;
    const keys=(event:KeyboardEvent)=>{const target=event.target as HTMLElement;if(event.defaultPrevented||event.ctrlKey||event.metaKey||event.altKey||target.closest("input,textarea,select,[contenteditable=true],dialog,.ui-menu")||document.querySelector("dialog[open]"))return;
      if(event.key==="ArrowRight"){event.preventDefault();step(1);}else if(event.key==="ArrowLeft"){event.preventDefault();step(-1);}else if(event.key==="Escape"){event.preventDefault();close();}
      else if(event.key==="f"&&current&&!inTrash){event.preventDefault();void update([current.id],{favorite:!current.favorite});}
      else if(event.key==="Delete"&&current&&!inTrash&&!inLocked){event.preventDefault();void trash([current.id]);}};
    document.addEventListener("keydown",keys);return()=>document.removeEventListener("keydown",keys);
  },[open,items,view]);
  const returnFocus=useRef<string|undefined>(undefined);
  function close(){returnFocus.current=open;setOpen(undefined);}
  useEffect(()=>{if(!open&&returnFocus.current){const id=returnFocus.current;returnFocus.current=undefined;requestAnimationFrame(()=>grid.current?.querySelector<HTMLElement>(`[data-asset="${CSS.escape(id)}"] .photos-open`)?.focus());}},[open]);

  const ids=[...selected];
  const allFavorite=ids.length>0&&ids.every(id=>items.find(i=>i.id===id)?.favorite);
  const albumMenu=(target:string[]):MenuAction[]=>[{label:"New album…",icon:"plus",run:()=>setAlbumDialog({ids:target})},...[...albums,...sharedAlbums.filter(a=>a.role==="editor")].filter(a=>view.type!=="album"||a.id!==view.id).map((a,n)=>({label:a.owner?`${a.name} (${a.owner.name})`:a.name,icon:"image" as IconName,separator:n===0,run:()=>{void addTo(a,target);}}))];
  const albumActions=(a:PhotoAlbum):MenuAction[]=>a.role==="owner"?[{label:"Share…",icon:"share",run:()=>setSharing(a)},{label:"Rename album",icon:"pencil",run:()=>setAlbumDialog({album:a})},{label:"Delete album…",icon:"trash",danger:true,separator:true,run:()=>setConfirm(a)}]
    :[{label:"Leave album",icon:"logout",danger:true,run:()=>{void leave(a);}}];
  const navButton=([id,label,icon]:[ListView,string,IconName])=>{const n=id==="locked"?undefined:overview?.counts[id as keyof PhotoCounts];return <button key={id} className={`nav-item${view.type===id?" active":""}`} aria-current={view.type===id?"page":undefined} onClick={()=>choose({type:id})}><Icon name={icon}/><span className="photos-row-title">{label}</span>{!!n&&<span className="photos-count">{n.toLocaleString()}</span>}</button>;};
  const sidebar=<>
    <NewItemButton onClick={()=>fileInput.current?.click()}>Upload</NewItemButton>
    <input ref={fileInput} type="file" multiple hidden accept="image/*,video/*,.heic,.heif,.dng,.cr2,.cr3,.nef,.arw,.orf,.rw2,.raf" aria-label="Upload photos and videos" onChange={e=>{void upload(e.target.files);e.target.value="";}}/>
    <nav aria-label="Photos views">{mainViews.map(navButton)}</nav>
    <div className="sidebar-heading photos-heading"><h2>Albums</h2><button type="button" className="icon-button" aria-label="New album" title="New album" onClick={()=>setAlbumDialog({})}><Icon name="plus"/></button></div>
    <nav aria-label="Albums" className="photos-albums">
      {albums.map(a=><div className="photos-row" key={a.id}><button className={`nav-item${view.type==="album"&&view.id===a.id?" active":""}`} aria-current={view.type==="album"&&view.id===a.id?"page":undefined} onClick={()=>choose({type:"album",id:a.id})}>
        {a.coverId?<img className="photos-album-cover" src={thumb(a.coverId)} alt="" loading="lazy"/>:<span className="photos-album-cover is-empty" aria-hidden="true"><Icon name="image"/></span>}<span className="photos-row-title">{a.name}</span>{a.shared&&<span className="photos-shared-mark" title="Shared"><Icon name="share"/><span className="sr-only">, shared</span></span>}<span className="photos-count">{a.count.toLocaleString()}</span></button>
        <ActionMenu label={`Actions for ${a.name}`} className="photos-row-menu" actions={albumActions(a)}><Icon name="more"/></ActionMenu></div>)}
      {overview&&!albums.length&&<p className="photos-hint">Albums you create appear here.</p>}
    </nav>
    <div className="sidebar-heading photos-heading"><h2>More</h2></div>
    <nav aria-label="More photos views">{moreViews.map(navButton)}</nav>
    {overview?.indexing&&<p className="photos-indexing" role="status"><span className="photos-spinner" aria-hidden="true"/>Finding photos in Files…</p>}
    <button type="button" className="nav-item ui-app-settings-entry" onClick={()=>setSettings(true)}><Icon name="settings"/>Photos settings</button>
  </>;

  let lastMonth="";
  const columns=zoomSteps.includes(zoom)?zoom:5;
  // Others' photos (in a shared album) only leave the album; your own keep every action.
  const foreign=ids.some(id=>!!items.find(i=>i.id===id)?.owner);
  const selectionTools=inTrash?<>
      <button type="button" className="text-button" onClick={()=>{void restore(ids);}}><Icon name="undo"/>Restore</button>
      <button type="button" className="text-button danger" onClick={()=>setDeleting({ids})}><Icon name="trash"/>Delete permanently</button>
    </>:inLocked?<>
      <button type="button" className="text-button" onClick={()=>{void unlockPhotos(ids);}}><Icon name="lock"/>Move out</button>
      <button type="button" className="text-button danger" onClick={()=>setDeleting({ids})}><Icon name="trash"/>Delete permanently</button>
    </>:foreign?<>
      {view.type==="album"&&albumOwner&&<button type="button" className="text-button" onClick={()=>{void removeFromAlbum(ids);}}><Icon name="close"/>Remove from album</button>}
      {!albumOwner&&<span className="photos-hint">Only the owner of these photos can change them.</span>}
    </>:<>
      <button type="button" className="icon-button" aria-label={allFavorite?"Remove from Favorites":"Add to Favorites"} title={allFavorite?"Remove from Favorites":"Add to Favorites"} aria-pressed={allFavorite} onClick={()=>{void update(ids,{favorite:!allFavorite},{favorite:allFavorite});}}><Icon name="star"/></button>
      <ActionMenu label="Add to album" actions={albumMenu(ids)}><Icon name="plus"/></ActionMenu>
      {view.type==="album"&&<button type="button" className="icon-button" aria-label="Remove from album" title="Remove from album" onClick={()=>{void removeFromAlbum(ids);}}><Icon name="close"/></button>}
      <button type="button" className="icon-button" aria-label="Create shared link" title="Create shared link" onClick={()=>setLinkFor(ids)}><Icon name="link"/></button>
      {ids.length>1&&<button type="button" className="icon-button" aria-label="Stack" title="Stack (show one cover for these photos)" onClick={()=>{void stack(ids);}}><Icon name="stack"/></button>}
      <button type="button" className="icon-button" aria-label={view.type==="archive"?"Restore to library":"Archive"} title={view.type==="archive"?"Restore to library":"Archive (hidden from the library, kept in albums and Files)"} onClick={()=>{void update(ids,{archived:view.type!=="archive"},{archived:view.type==="archive"});}}><Icon name={view.type==="archive"?"undo":"archive"}/></button>
      <button type="button" className="icon-button" aria-label="Move to Locked folder" title="Move to Locked folder" onClick={()=>{void lockPhotos(ids);}}><Icon name="lock"/></button>
      <button type="button" className="icon-button" aria-label="Move to trash" title="Move to trash" onClick={()=>{void trash(ids);}}><Icon name="trash"/></button>
    </>;
  const count=view.type==="album"?album?.count:view.type==="locked"||view.type==="duplicates"?undefined:overview?.counts[view.type];
  const sharedView=<><div className="photos-list-heading"><h1>Shared with me</h1>{overview&&<span className="photos-heading-count">{sharedAlbums.length}</span>}</div>
    <div className="photos-grid-scroll">{overview&&!sharedAlbums.length?<div className="photos-empty"><Icon name="share"/><h2>Nothing shared with you</h2><p>Albums other people on this Dar share with you appear here.</p></div>
      :<ul className="photos-album-cards" aria-label="Albums shared with you">{sharedAlbums.map(a=><li key={a.id}><button type="button" className="photos-album-card" onClick={()=>choose({type:"album",id:a.id})}>
        <span className="photos-album-card-cover">{a.coverId?<img src={thumb(a.coverId)} alt="" loading="lazy"/>:<Icon name="image"/>}</span><strong>{a.name}</strong><small>{a.owner?.name} · {a.count.toLocaleString()} item{a.count===1?"":"s"} · {a.role==="editor"?"Can add":"Can view"}</small></button></li>)}</ul>}</div></>;
  const header=ids.length?<div className="photos-selection" role="toolbar" aria-label="Selection">
      <button type="button" className="icon-button" aria-label="Clear selection" onClick={()=>setSelected(new Set())}><Icon name="close"/></button><strong>{ids.length} selected</strong><span className="photos-spacer"/>{selectionTools}
    </div>:<div className="photos-list-heading"><h1>{title}</h1>{state==="ready"&&!lockedClosed&&<span className="photos-heading-count">{(count??items.length).toLocaleString()}</span>}{album?.owner&&<span className="photos-shared-by">Shared by {album.owner.name} · {album.role==="editor"?"you can add photos":"view only"}</span>}<div className="photos-tools">
      {inTrash&&items.length>0&&<button type="button" className="text-button danger" onClick={()=>setDeleting({ids:[],empty:true})}>Empty trash</button>}
      {inLocked&&folder.unlocked&&<button type="button" className="text-button" onClick={()=>{void folder.lock().catch(fail);}}><Icon name="lock"/>Lock</button>}
      <label className="photos-zoom"><Icon name="grid"/><span className="sr-only">Columns</span><input type="range" min={0} max={zoomSteps.length-1} step={1} value={zoomSteps.indexOf(columns)} aria-valuetext={`${columns} columns`} onChange={e=>setZoom(zoomSteps[Number(e.target.value)])}/></label>
      {album&&albumOwner&&<button type="button" className="secondary-button photos-share-button" onClick={()=>setSharing(album)}><Icon name="share"/>Share</button>}
      {album&&<ActionMenu label={`Actions for ${album.name}`} actions={albumActions(album)}><Icon name="more"/></ActionMenu>}
    </div></div>;
  const empty=view.type==="library"?<><Icon name="image"/><h2>{overview?.indexing?"Looking for photos…":"No photos yet"}</h2><p>Upload photos and videos, or drop them here. Photos also shows the pictures and videos already in your Files.</p><button className="primary-button" onClick={()=>fileInput.current?.click()}><Icon name="upload"/>Upload</button></>
    :view.type==="favorites"?<><h2>No favorites</h2><p>Star a photo to keep it here.</p></>
    :view.type==="archive"?<><h2>Archive is empty</h2><p>Archived photos are hidden from the library but stay in albums and Files.</p></>
    :view.type==="videos"?<><h2>No videos</h2><p>Videos you upload or keep in Files appear here.</p></>
    :inTrash?<><Icon name="trash"/><h2>Trash is empty</h2><p>Photos you delete stay here, and in Files › Deleted files, until you delete them permanently.</p></>
    :inLocked?<><Icon name="lock"/><h2>Nothing locked yet</h2><p>Select photos and choose Move to Locked folder. They move to Files › Locked folder and disappear from the library, albums and search.</p></>
    :<><h2>This album is empty</h2><p>Select photos in the library and choose Add to album.</p></>;
  const gridView=<div className={`photos-grid-scroll${dragging?" is-dropping":""}`} ref={grid} onDragOver={e=>{if(e.dataTransfer.types.includes("Files")){e.preventDefault();setDragging(true);}}} onDragLeave={e=>{if(e.currentTarget===e.target)setDragging(false);}} onDrop={e=>{e.preventDefault();setDragging(false);void upload(e.dataTransfer.files);}}>
    {(overviewError||state==="error")&&<div className="photos-empty" role="alert"><h2>Photos could not be loaded</h2><p>{listError||overviewError}</p><button className="secondary-button" onClick={()=>{void loadOverview();void loadItems();}}>Retry</button></div>}
    {lockedClosed?(folder.state?<UnlockForm folder={folder} description="Enter your password to see your locked photos. They are also in Files › Locked folder, which opens with the same password."/>:<p className="photos-empty" role="status">{folder.error||"Checking the Locked folder…"}</p>):<>
    {state==="loading"&&!items.length&&<p className="photos-empty" role="status">Loading photos…</p>}
    {state==="ready"&&!items.length&&<div className="photos-empty">{empty}</div>}
    <div className="photos-grid" role="list" aria-label={`${title} photos`} data-columns={columns}>
      {items.map(item=>{const month=monthLabel(inTrash?item.trashedAt??item.takenAt:item.takenAt),heading=month!==lastMonth?(lastMonth=month,<h2 className="photos-month" key={"m-"+month}>{inTrash?`Deleted ${month}`:month}</h2>):null,isSelected=selected.has(item.id);
        return <Fragment key={item.id}>{heading}<div role="listitem" data-asset={item.id} className={`photos-tile${isSelected?" is-selected":""}`}>
          <button type="button" className="photos-open" aria-label={`${item.kind==="video"?"Video":item.live?"Live Photo":"Photo"} ${item.name}, ${new Date(item.takenAt).toLocaleString([],{dateStyle:"medium",timeStyle:"short"})}${item.favorite?", favorite":""}${item.stack?`, stack of ${item.stack.count}`:""}`} onClick={event=>{if(selected.size||event.shiftKey){setSelected(s=>{const n=new Set(s);if(n.has(item.id))n.delete(item.id);else n.add(item.id);return n;});}else setOpen(item.id);}}>
            <Thumb asset={item}/>{item.kind==="video"&&<span className="photos-badge"><Icon name="video"/>{item.duration?duration(item.duration):""}</span>}
            {item.live&&<span className="photos-badge"><Icon name="live"/>Live</span>}{item.stack&&<span className="photos-badge photos-stack-badge"><Icon name="stack"/>{item.stack.count}</span>}
            {item.favorite&&<span className="photos-fav"><Icon name="star"/></span>}
          </button>
          <input type="checkbox" className="photos-check" aria-label={`Select ${item.name}`} checked={isSelected} onChange={()=>setSelected(s=>{const n=new Set(s);if(n.has(item.id))n.delete(item.id);else n.add(item.id);return n;})}/>
        </div></Fragment>;})}
    </div>
    {cursor&&state==="ready"&&<div className="photos-more" ref={sentinel}><button className="text-button" onClick={()=>{void loadMore();}}>Load more</button></div>}</>}
  </div>;

  const viewerMore=(asset:PhotoAsset):MenuAction[]=>asset.owner?[
      {label:"Download original",icon:"download",href:`${base}/assets/${asset.id}/original`,download:true},...view.type==="album"&&albumOwner?[{label:"Remove from album",icon:"close" as IconName,separator:true,run:()=>{void removeFromAlbum([asset.id]);}}]:[]]
    :inTrash?[
      {label:"Restore",icon:"undo",run:()=>{void restore([asset.id]);}},{label:"Delete permanently…",icon:"trash",danger:true,separator:true,run:()=>setDeleting({ids:[asset.id]})}]
    :inLocked?[
      {label:"Move out of Locked folder",icon:"lock",run:()=>{void unlockPhotos([asset.id]);}},{label:"Download original",icon:"download",href:`/api/v1/files/${asset.id}/content`,download:true,separator:true},{label:"Delete permanently…",icon:"trash",danger:true,separator:true,run:()=>setDeleting({ids:[asset.id]})}]
    :[{label:"Add to album",icon:"plus",items:albumMenu([asset.id])},{label:asset.archived?"Restore to library":"Archive",icon:asset.archived?"undo":"archive",run:()=>{void update([asset.id],{archived:!asset.archived},{archived:asset.archived});}},
      ...view.type==="album"?[{label:"Remove from album",icon:"close" as IconName,run:()=>{void removeFromAlbum([asset.id]);}}]:[],
      {label:"Create shared link",icon:"link",run:()=>setLinkFor([asset.id])},{label:"Move to Locked folder",icon:"lock",run:()=>{void lockPhotos([asset.id]);}},{label:"Download original",icon:"download",href:`/api/v1/files/${asset.id}/content`,download:true,separator:true},
      {label:"Move to trash",icon:"trash",danger:true,separator:true,shortcut:"Delete",run:()=>{void trash([asset.id]);}}];

  return <div className={`app-shell pim-shell photos-shell${current?" has-pim-details":""}${current&&info?" is-info":""}`}>
    <WorkspaceHeader app="photos" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId} onSettings={()=>setSettings(true)}/>
    <div className="pim-workspace photos-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar photos-sidebar${navigation?" is-open":""}`} aria-label="Photos navigation">{sidebar}</aside>
      <main className="pim-main photos-main" tabIndex={-1} aria-label={current?current.name:title}>
        {view.type==="duplicates"?<DuplicatesView session={session} onNotice={setNotice} onChanged={()=>{void loadOverview();}}/>
          :view.type==="shared"?sharedView
          :current&&shown?<Viewer asset={current} shown={shown} items={items} index={index} session={session} trash={inTrash||!!shown.owner} more={viewerMore(shown)} onMember={setMember} onUnstack={value=>{void unstack(value);}}
            onCover={value=>{void guard(async()=>{await request(`${base}/stacks/${value.id}`,{method:"PATCH",body:JSON.stringify({primaryId:shown.id})},session.csrfToken);setMember(undefined);await refresh();setOpen(shown.id);setNotice({message:"Stack cover changed"});},"change the cover");}}
            onInfo={()=>{flushSync(()=>setInfo(true));document.querySelector<HTMLElement>(".photos-detail-back")?.focus();}} onClose={close} onStep={step} onOpen={setOpen}
            onFavorite={()=>{void update([shown.id],{favorite:!shown.favorite});if(member)setMember({...member,favorite:!member.favorite});}}/>
          :open&&state==="ready"?<div className="photos-empty" role="alert"><h2>This photo is not available</h2><p>It may have been moved to the trash, archived, locked or removed from this view.</p><button className="secondary-button" onClick={()=>setOpen(undefined)}>Back to {title}</button></div>
          :<>{header}{gridView}</>}
      </main>
      {current&&shown&&view.type!=="duplicates"&&<aside className="pim-details photos-details" tabIndex={-1} aria-label="Photo details"><Details id={shown.id} session={session} readOnly={inTrash||!!shown.owner} onBack={()=>{flushSync(()=>setInfo(false));document.querySelector<HTMLElement>(".photos-info-toggle")?.focus();}} onChanged={change=>patchLocal([shown.id],change)} onAlbum={id=>choose({type:"album",id})} onError={fail}/></aside>}
    </div>
    {transfers.length>0&&<section className="transfer-panel photos-transfers" aria-label="Uploads"><div className="transfer-heading"><strong>{transfers.some(t=>t.status==="uploading")?`Uploading ${transfers.filter(t=>t.status==="uploading").length}`:"Uploads"}</strong><button className="icon-button" aria-label="Dismiss finished uploads" disabled={transfers.some(t=>t.status==="uploading")} onClick={()=>setTransfers([])}><Icon name="close"/></button></div>
      <ul>{transfers.map(t=><li key={t.id} className={`photos-transfer is-${t.status}`}><span>{t.name}</span><progress max={1} value={t.progress} aria-label={`${t.name} upload progress`}/>{t.message?<small role="alert">{t.message}</small>:<small>{t.status==="complete"?"Done":`${Math.round(t.progress*100)}%`}</small>}</li>)}</ul></section>}
    {albumDialog&&<AlbumDialog album={albumDialog.album} ids={albumDialog.ids} session={session} onClose={()=>setAlbumDialog(undefined)} onSaved={async(saved,created)=>{setAlbumDialog(undefined);setSelected(new Set());await loadOverview();if(created)setNotice({message:albumDialog.ids?.length?`Added ${albumDialog.ids.length} to ${saved.name}`:`Created ${saved.name}`});}}/>}
    {confirm&&<Dialog title={`Delete ${confirm.name}?`} onClose={()=>setConfirm(undefined)}><ConfirmBody album={confirm} session={session} onClose={()=>setConfirm(undefined)} onDeleted={async()=>{if(view.type==="album"&&view.id===confirm.id)choose({type:"library"});setConfirm(undefined);await loadOverview();setNotice({message:`Deleted ${confirm.name}`});}}/></Dialog>}
    {deleting&&<DeleteDialog session={session} ids={deleting.ids} empty={!!deleting.empty} locked={inLocked} onClose={()=>setDeleting(undefined)} onDeleted={async(removed,label)=>{setDeleting(undefined);if(removed)removeLocal(removed);else{setItems([]);setOpen(undefined);}await refresh();setNotice({message:label});}} onLocked={()=>{setDeleting(undefined);void folder.refresh();}}/>}
    {unlockAction&&<UnlockDialog folder={folder} action={unlockAction.action} onClose={()=>setUnlockAction(null)} onUnlocked={()=>{const run=unlockAction.run;setUnlockAction(null);run();}}/>}
    {sharing&&<SharePane album={sharing} session={session} onClose={()=>setSharing(undefined)} onChanged={()=>{void loadOverview();}}/>}
    {linkFor&&<LinkDialog ids={linkFor} session={session} onClose={()=>{setLinkFor(undefined);setSelected(new Set());}} onCreated={()=>undefined}/>}
    {settings&&<PhotosSettings session={session} overview={overview} folder={folder} onChanged={()=>{void loadOverview();}} onClose={()=>setSettings(false)}/>}
    <div className="photos-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast photos-toast"><span>{notice.message}</span>{notice.undo&&<button type="button" className="text-button" onClick={()=>{const undo=notice.undo!;setNotice(null);undo();}}>Undo</button>}</div>}</div>
  </div>;
}

function Thumb({asset,size="s"}:{asset:PhotoAsset;size?:"s"|"m"}){
  const [failed,setFailed]=useState(false);
  useEffect(()=>setFailed(false),[asset.id,asset.revision]);
  if(failed||asset.thumbnail==="failed"&&asset.kind==="video")return <span className="photos-placeholder"><Icon name={asset.kind==="video"?"video":"image"}/><small>{asset.name}</small></span>;
  return <img src={`${thumb(asset.id,size)}${size==="m"?"&":"?"}v=${asset.revision}`} alt="" loading="lazy" decoding="async" draggable={false} onError={()=>setFailed(true)}/>;
}

function Viewer({asset,shown,items,index,session,trash,more,onMember,onUnstack,onCover,onInfo,onClose,onStep,onOpen,onFavorite}:{asset:PhotoAsset;shown:PhotoAsset;items:PhotoAsset[];index:number;session:BrowserSession;trash:boolean;more:MenuAction[];onMember:(asset:PhotoAsset|undefined)=>void;onUnstack:(stack:PhotoStack)=>void;onCover:(stack:PhotoStack)=>void;onInfo:()=>void;onClose:()=>void;onStep:(delta:number)=>void;onOpen:(id:string)=>void;onFavorite:()=>void}){
  const heading=useRef<HTMLHeadingElement>(null),strip=useRef<HTMLDivElement>(null),[stack,setStack]=useState<PhotoStack>();
  useEffect(()=>{heading.current?.focus();},[]);
  useEffect(()=>{strip.current?.querySelector(".is-current")?.scrollIntoView({block:"nearest",inline:"center"});},[asset.id]);
  useEffect(()=>{setStack(undefined);if(!asset.stack)return;let alive=true;void request<PhotoStack>(`${base}/stacks/${asset.stack.id}`).then(value=>{if(alive)setStack(value);}).catch(()=>undefined);return()=>{alive=false;};},[asset.id,asset.stack?.id,asset.stack?.count]);
  const near=items.slice(Math.max(0,index-12),index+13);
  const menu:MenuAction[]=stack?[...more,{label:"Stack",icon:"stack",separator:true,items:[...shown.id!==stack.primaryId?[{label:"Make this the cover",icon:"star" as IconName,run:()=>onCover(stack)}]:[],{label:"Unstack",icon:"close",run:()=>onUnstack(stack)}]}]:more;
  return <section className="photos-viewer" aria-label={`Viewer for ${shown.name}`}>
    <div className="photos-viewer-bar">
      <button type="button" className="icon-button" aria-label="Back to photos" title="Back (Escape)" onClick={onClose}><Icon name="back"/></button>
      <h1 ref={heading} tabIndex={-1}>{shown.name}</h1><span className="photos-heading-count">{index+1} of {items.length}</span><span className="photos-spacer"/>
      <button type="button" className="icon-button" aria-label="Previous" title="Previous (←)" disabled={index<=0} onClick={()=>onStep(-1)}><Icon name="back"/></button>
      <button type="button" className="icon-button" aria-label="Next" title="Next (→)" disabled={index>=items.length-1} onClick={()=>onStep(1)}><Icon name="chevron"/></button>
      {!trash&&<button type="button" className={`icon-button${shown.favorite?" is-on":""}`} aria-label={shown.favorite?"Remove from Favorites":"Add to Favorites"} aria-pressed={shown.favorite} title="Favorite (f)" onClick={onFavorite}><Icon name="star"/></button>}
      <button type="button" className="icon-button photos-info-toggle" aria-label="Photo info" onClick={onInfo}><Icon name="info"/></button>
      <ActionMenu label="More photo actions" actions={menu}><Icon name="more"/></ActionMenu>
    </div>
    <div className="photos-stage">{shown.kind==="video"?<VideoStage key={shown.id} asset={shown} session={session} controls poster={thumb(shown.id,"m")}/>:shown.live?<LiveStage key={shown.id} asset={shown} motion={shown.live} session={session}/>:<StageImage asset={shown}/>}</div>
    {stack&&stack.items.length>1&&<div className="photos-stack-strip" aria-label={`Stack of ${stack.items.length}`} role="group"><Icon name="stack"/>{stack.items.map(item=><button type="button" key={item.id} className={item.id===shown.id?"is-current":""} aria-label={`${item.name}${item.id===stack.primaryId?", cover":""}`} aria-pressed={item.id===shown.id} onClick={()=>onMember(item.id===asset.id?undefined:item)}><Thumb asset={item}/></button>)}</div>}
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
const hevcPlayable=()=>typeof document!=="undefined"&&document.createElement("video").canPlayType('video/mp4; codecs="hvc1.1.6.L93.B0"')!=="";
interface Playback {state:"preparing"|"ready"|"failed";progress:number;url:string}
/**
 * Plays a video the way this browser can: the original, the original HEVC where the
 * browser decodes it, or a copy the server remuxes or transcodes once (with progress).
 */
function VideoStage({asset,session,controls=false,poster,autoPlay=false,onEnded}:{asset:Pick<PhotoAsset,"id"|"name"|"playback"|"videoCodec">;session:BrowserSession;controls?:boolean;poster?:string;autoPlay?:boolean;onEnded?:()=>void}){
  const [playback,setPlayback]=useState<Playback>(),[convert,setConvert]=useState(false);
  useEffect(()=>{
    let alive=true,timer:ReturnType<typeof setTimeout>|undefined;
    // Not yet indexed for playback: the Files original, as before.
    if(!asset.playback){setPlayback({state:"ready",progress:1,url:content(asset.id)});return;}
    if(asset.playback==="original"||!convert&&asset.videoCodec==="hevc"&&hevcPlayable()){setPlayback({state:"ready",progress:1,url:`${base}/assets/${asset.id}/video${asset.playback==="original"?"":"?source=original"}`});return;}
    const poll=async()=>{
      try{const value=await request<Playback>(`${base}/assets/${asset.id}/playback`,{method:"POST"},session.csrfToken);if(!alive)return;setPlayback(value);if(value.state==="preparing")timer=setTimeout(()=>{void poll();},1500);}
      catch{if(alive)setPlayback({state:"failed",progress:0,url:""});}
    };
    setPlayback({state:"preparing",progress:0,url:""});void poll();
    return()=>{alive=false;clearTimeout(timer);};
  },[asset.id,asset.playback,convert]);
  if(!playback||playback.state==="preparing")return <div className="photos-video-wait" role="status"><span className="photos-spinner" aria-hidden="true"/><span>Preparing this video for your browser{playback?.progress?` · ${Math.round(playback.progress*100)}%`:"…"}</span></div>;
  if(playback.state==="failed")return <div className="photos-empty" role="alert"><Icon name="video"/><h2>This video can’t be played here</h2><p>Dar could not convert it for your browser. You can still download the original.</p><a className="secondary-button" href={`/api/v1/files/${asset.id}/content`} download><Icon name="download"/>Download</a></div>;
  return <video key={playback.url} src={playback.url} controls={controls} playsInline preload="metadata" poster={poster} autoPlay={autoPlay} muted={autoPlay} aria-label={asset.name} onEnded={onEnded}
    onError={()=>{if(asset.playback!=="original"&&!convert&&asset.playback)setConvert(true);}}/>;
}
/** A Live Photo: the still, with its motion played over it on demand. */
function LiveStage({asset,motion,session}:{asset:PhotoAsset;motion:string;session:BrowserSession}){
  const [playing,setPlaying]=useState(false),[clip,setClip]=useState<PhotoDetail>(),[error,setError]=useState(false);
  useEffect(()=>{if(!playing||clip)return;let alive=true;void request<PhotoDetail>(`${base}/assets/${motion}`).then(value=>{if(alive)setClip(value);},()=>{if(alive){setError(true);setPlaying(false);}});return()=>{alive=false;};},[playing,motion]);
  return <div className="photos-live"><StageImage asset={asset}/>
    {playing&&clip&&<div className="photos-live-motion"><VideoStage asset={clip} session={session} autoPlay onEnded={()=>setPlaying(false)}/></div>}
    <button type="button" className="photos-live-toggle" aria-pressed={playing} disabled={error} title={error?"The motion part is not available":"Play the Live Photo"} onClick={()=>setPlaying(!playing)}><Icon name="live"/>Live</button>
  </div>;
}

function Details({id,session,readOnly,onBack,onChanged,onAlbum,onError}:{id:string;session:BrowserSession;readOnly:boolean;onBack:()=>void;onChanged:(change:Partial<PhotoAsset>)=>void;onAlbum:(id:string)=>void;onError:(cause:unknown)=>void}){
  const [detail,setDetail]=useState<PhotoDetail>(),[error,setError]=useState(""),[description,setDescription]=useState(""),[saving,setSaving]=useState<""|"saving"|"saved">(""),[conflict,setConflict]=useState(false);
  const load=useCallback(async()=>{setError("");try{const value=await request<PhotoDetail>(`${base}/assets/${id}`);setDetail(value);setDescription(value.description);setConflict(false);}catch(cause){setError(message(cause));}},[id]);
  useEffect(()=>{setDetail(undefined);setSaving("");void load();},[load]);
  async function save(){
    if(!detail||readOnly||description.trim()===detail.description)return;setSaving("saving");
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
  const video=detail.kind==="video"?[e.videoCodec?.toUpperCase(),e.audioCodec?.toUpperCase(),detail.playback==="transcode"?"Converted for browsers":detail.playback==="remux"?"Repackaged for browsers":""].filter(Boolean).join(" · "):"";
  return <div className="photos-detail">
    <div className="photos-detail-head"><button type="button" className="icon-button photos-detail-back" aria-label="Back to photo" onClick={onBack}><Icon name="back"/></button><h2>Details</h2></div>
    <div className="photos-detail-body">
      {detail.owner&&<p className="photos-detail-note"><Icon name="share"/>Shared by {detail.owner}. <a href={`${base}/assets/${detail.id}/original`} download>Download original</a></p>}
      {detail.locked&&<p className="photos-detail-note"><Icon name="lock"/>In the Locked folder: hidden from the library, albums and search.</p>}
      {readOnly&&detail.trashedAt&&<p className="photos-detail-note"><Icon name="trash"/>Moved to the trash {new Date(detail.trashedAt).toLocaleDateString([],{dateStyle:"medium"})}.</p>}
      <label className="photos-description"><span className="sr-only">Description</span><textarea value={description} maxLength={2000} rows={2} readOnly={readOnly} placeholder={readOnly?"No description":"Add a description…"} onChange={event=>{setDescription(event.target.value);setSaving("");}} onBlur={()=>{void save();}}/></label>
      {saving&&<p className="photos-saved" role="status">{saving==="saving"?"Saving…":"Saved"}</p>}
      {conflict&&<div className="error" role="alert"><p>This description changed elsewhere.</p><button className="text-button" onClick={()=>{void load();}}>Reload</button></div>}
      <ul className="photos-props">
        <li><Icon name="calendar"/><span><strong>{new Date(detail.takenAt).toLocaleDateString([],{weekday:"long",year:"numeric",month:"long",day:"numeric"})}</strong><small>{new Date(detail.takenAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit",timeZoneName:"short"})}</small></span></li>
        <li><Icon name={detail.kind==="video"?"video":"image"}/><span><strong>{detail.name}</strong><small>{megapixels}{detail.width?`${detail.width} × ${detail.height} · `:""}{detail.kind==="video"&&detail.duration?`${duration(detail.duration)} · `:""}{formatBytes(detail.size)}</small>{video&&<small>{video}</small>}</span></li>
        {detail.live&&<li><Icon name="live"/><span><strong>Live Photo</strong><small>The motion part is kept next to the still in Files.</small></span></li>}
        {(camera||e.lens||settings)&&<li><Icon name="image"/><span><strong>{camera||"Camera"}</strong><small>{[e.lens,settings].filter(Boolean).join(" · ")}</small></span></li>}
        {detail.latitude!==null&&detail.longitude!==null&&<li><Icon name="location"/><span><strong>Location</strong><small>{detail.latitude.toFixed(5)}, {detail.longitude.toFixed(5)}</small></span></li>}
      </ul>
      <h3 className="photos-detail-section">Albums</h3>
      {detail.albums.length?<div className="photos-chips">{detail.albums.map(a=><button type="button" key={a.id} className="photos-chip" onClick={()=>onAlbum(a.id)}>{a.name}</button>)}</div>:<p className="photos-hint">Not in any album.</p>}
      {!readOnly&&!detail.owner&&<a className="secondary-button photos-in-files" href={detail.locked?`/?section=locked${detail.parentId?`&folder=${detail.parentId}`:""}`:`/?folder=${detail.parentId}&file=${detail.id}`}><Icon name="folder"/>Show in Files</a>}
    </div>
  </div>;
}

/** Duplicates: groups of the same picture; keep some, the rest go to the trash with Undo. */
function DuplicatesView({session,onNotice,onChanged}:{session:BrowserSession;onNotice:(notice:Notice)=>void;onChanged:()=>void}){
  const [groups,setGroups]=useState<PhotoDuplicateGroup[]>(),[cursor,setCursor]=useState(""),[error,setError]=useState(""),[keep,setKeep]=useState<Record<string,string[]>>({}),[busy,setBusy]=useState("");
  const load=useCallback(async(after="")=>{
    setError("");
    try{const page=await request<PhotoDuplicatePage>(`${base}/duplicates${after?`?cursor=${encodeURIComponent(after)}`:""}`);setGroups(current=>after?[...current??[],...page.groups]:page.groups);setCursor(page.cursor);
      setKeep(current=>{const next={...current};for(const g of page.groups)if(!next[g.key])next[g.key]=[g.items[0].id];return next;});}
    catch(cause){setError(message(cause));}
  },[]);
  useEffect(()=>{void load();},[load]);
  const post=<T,>(path:string,value:unknown)=>request<T>(`${base}${path}`,{method:"POST",body:JSON.stringify(value)},session.csrfToken);
  async function resolve(group:PhotoDuplicateGroup){
    const chosen=keep[group.key]??[];if(!chosen.length)return;setBusy(group.key);
    try{
      const {trashed}=await post<{trashed:string[]}>("/duplicates/resolve",{key:group.key,keep:chosen});
      setGroups(current=>current?.filter(g=>g.key!==group.key));onChanged();
      onNotice({message:`Kept ${chosen.length}, moved ${trashed.length} to the trash`,undo:()=>{void post("/assets/restore",{ids:trashed}).then(()=>{onChanged();void load();},cause=>onNotice({message:message(cause)}));}});
    }catch(cause){onNotice({message:message(cause)});if(cause instanceof RequestError&&cause.status===409)void load();}
    finally{setBusy("");}
  }
  async function dismiss(group:PhotoDuplicateGroup){
    setBusy(group.key);
    try{await post("/duplicates/dismiss",{key:group.key});setGroups(current=>current?.filter(g=>g.key!==group.key));onChanged();onNotice({message:"Marked as not duplicates"});}
    catch(cause){onNotice({message:message(cause)});}finally{setBusy("");}
  }
  const toggle=(group:PhotoDuplicateGroup,id:string)=>setKeep(current=>{const list=current[group.key]??[];return {...current,[group.key]:list.includes(id)?list.filter(x=>x!==id):[...list,id]};});
  return <>
    <div className="photos-list-heading"><h1>Duplicates</h1>{groups&&<span className="photos-heading-count">{groups.length}{cursor?"+":""} {groups.length===1&&!cursor?"group":"groups"}</span>}</div>
    <div className="photos-grid-scroll photos-duplicates">
      {error&&<div className="photos-empty" role="alert"><h2>Duplicates could not be loaded</h2><p>{error}</p><button className="secondary-button" onClick={()=>{void load();}}>Retry</button></div>}
      {!groups&&!error&&<p className="photos-empty" role="status">Looking for duplicates…</p>}
      {groups&&!groups.length&&<div className="photos-empty"><Icon name="copy"/><h2>No duplicates</h2><p>Copies of the same picture, even resized or re-saved, show up here so you can keep the best one.</p></div>}
      {groups?.map(group=>{const chosen=keep[group.key]??[];return <section key={group.key} className="photos-dup-group" aria-label={`${group.items.length} copies of ${group.items[0].name}`}>
        <div className="photos-dup-head"><strong>{group.identical?"Identical files":"Same picture"}</strong><small>{group.items.length} copies · keep {chosen.length}</small><span className="photos-spacer"/>
          <button type="button" className="text-button" disabled={!!busy} onClick={()=>{void dismiss(group);}}>Not duplicates</button>
          <button type="button" className="primary-button" disabled={!!busy||!chosen.length||chosen.length===group.items.length} onClick={()=>{void resolve(group);}}>{busy===group.key?"Saving…":`Keep ${chosen.length}, trash ${group.items.length-chosen.length}`}</button></div>
        <ul className="photos-dup-items">{group.items.map(item=><li key={item.id} className={chosen.includes(item.id)?"is-kept":""}>
          <label><span className="photos-dup-thumb"><Thumb asset={item}/></span><span className="photos-dup-keep"><input type="checkbox" checked={chosen.includes(item.id)} onChange={()=>toggle(group,item.id)} aria-label={`Keep ${item.name} in ${item.path||"All files"}`}/>Keep</span></label>
          <strong>{item.name}</strong><small>{item.width?`${item.width} × ${item.height} · `:""}{formatBytes(item.size)}</small><small>{new Date(item.takenAt).toLocaleDateString([],{dateStyle:"medium"})}{item.favorite?" · Favorite":""}</small><small className="photos-dup-path">{item.path||"All files"}</small>
        </li>)}</ul>
      </section>;})}
      {cursor&&<div className="photos-more"><button className="text-button" onClick={()=>{void load(cursor);}}>Load more</button></div>}
      {groups&&groups.length>0&&<p className="photos-hint photos-dup-hint">Kept photos receive the albums, favorite and description of the copies you trash.</p>}
    </div>
  </>;
}

function DeleteDialog({session,ids,empty,locked,onClose,onDeleted,onLocked}:{session:BrowserSession;ids:string[];empty:boolean;locked:boolean;onClose:()=>void;onDeleted:(removed:string[]|undefined,label:string)=>Promise<void>;onLocked:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  async function run(){
    setBusy(true);setError("");
    try{
      if(empty){let total=0;for(let round=0;round<100;round++){const r=await request<{deleted:number;remaining:boolean}>(`${base}/trash/empty`,{method:"POST"},session.csrfToken);total+=r.deleted;if(!r.remaining)break;}await onDeleted(undefined,`Deleted ${noun(total)} permanently`);}
      else{const r=await request<{deleted:string[]}>(`${base}/assets/delete`,{method:"POST",body:JSON.stringify({ids})},session.csrfToken);await onDeleted(r.deleted,`Deleted ${noun(ids.length)} permanently`);}
    }catch(cause){if(lockedError(cause))onLocked();else{setError(message(cause));setBusy(false);}}
  }
  return <Dialog title={empty?"Empty the trash?":"Delete permanently?"} onClose={onClose}>
    <p>{empty?"Every photo and video in the trash is deleted from Files with its versions.":`${ids.length===1?"This item is":`These ${ids.length} items are`} deleted from Files with ${ids.length===1?"its":"their"} versions${locked?", without going through the trash":""}.`} This cannot be undone.</p>
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button className="text-button" onClick={onClose}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{void run();}}>{busy?"Deleting…":empty?"Empty trash":"Delete permanently"}</button></div>
  </Dialog>;
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

function PhotosSettings({session,overview,folder,onChanged,onClose}:{session:BrowserSession;overview?:PhotoOverview;folder:LockedFolder;onChanged:()=>void;onClose:()=>void}){
  const [section,setSection]=useState("General"),saver=usePreferenceSaver(session);
  const saved=saver.preferences?.photosUploadFolder??overview?.uploadFolder??"pics",[draft,setDraft]=useState<string>(),value=draft??saved,valid=!!photoFolderParts(value);
  function saveFolder(event:FormEvent){event.preventDefault();const parts=photoFolderParts(value);if(!parts)return;const next=parts.join("/");setDraft(undefined);if(next!==saved){saver.change("photosUploadFolder",next,`Uploads go to ${next}`);setTimeout(onChanged,800);}}
  return <SettingsSheet title="Photos settings" icon="image" sections={["General","Privacy","Immich","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={onClose} notice={saver.notice} onNotice={saver.setNotice} footer="Photos keeps favorites, archive, descriptions, albums and stacks with your account.">
    <SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>
    {section==="General"&&<><section className="settings-group" aria-label="Library"><h3>Library</h3>
      <SettingRow label="Where photos come from" hint="Every picture and video you own in Files, including RAW, HEIC and videos. Shared files from others are not included." stacked><a className="secondary-button" href="/">Open Files</a></SettingRow>
      <SettingRow label="Upload folder" hint={`Uploads go to Files › ${saved} › year › month. Same-named files are kept with a number.`} state={saver.states.photosUploadFolder} stacked>
        <form className="photos-folder-form" onSubmit={saveFolder}><label><span className="sr-only">Upload folder</span><input value={value} maxLength={500} onChange={event=>setDraft(event.target.value)} aria-invalid={!valid} aria-describedby={valid?undefined:"photos-folder-error"}/></label><button className="secondary-button" disabled={!valid||value===saved||!saver.preferences}>Save</button></form>
        {!valid&&<p id="photos-folder-error" className="field-error" role="alert">Use folder names separated by /, like pics or Media/Camera.</p>}
      </SettingRow>
      <SettingRow label="Indexing" hint="New and changed files are picked up in the background">{overview?.indexing?<span className="photos-indexing"><span className="photos-spinner" aria-hidden="true"/>Working…</span>:<span className="photos-hint">Up to date</span>}</SettingRow></section>
      <section className="settings-group" aria-label="Media"><h3>Media</h3>
        <SettingRow label="Videos" hint="Videos your browser can’t play (HEVC, MKV and others) are converted to H.264 the first time you open them. Converted copies are kept in the cache and can be deleted." stacked><span className="photos-hint">Automatic</span></SettingRow>
        <SettingRow label="Live Photos, stacks and duplicates" hint="A still and its short MOV appear as one Live Photo. RAW+JPEG pairs and bursts stack under one cover. Duplicates finds copies of the same picture." stacked><span className="photos-hint">Automatic</span></SettingRow></section></>}
    {section==="Privacy"&&<section className="settings-group" aria-label="Locked folder and trash"><h3>Locked folder and trash</h3>
      <SettingRow label="Locked folder" hint="Locked photos live in Files › Locked folder. Your password opens it for 10 minutes on this device. They are hidden from the library, albums, search, sharing and WebDAV." stacked>{folder.unlocked?<button type="button" className="secondary-button" onClick={()=>{void folder.lock();}}><Icon name="lock"/>Lock now</button>:<span className="photos-hint">Locked</span>}</SettingRow>
      <SettingRow label="Trash" hint="The trash is Files › Deleted files. Nothing is removed until you delete it permanently or empty the trash. Deleting in the Locked folder is permanent." stacked><span className="photos-hint">Kept until deleted</span></SettingRow></section>}
    {section==="Privacy"&&<section className="settings-group" aria-label="Shared links"><h3>Shared links</h3><LinksList session={session}/></section>}
    {section==="Immich"&&<ImmichSettings session={session}/>}
    {section==="Keyboard shortcuts"&&<dl className="ui-shortcuts"><dt>Shift click</dt><dd>Select a photo</dd><dt>← / →</dt><dd>Previous or next photo</dd><dt>f</dt><dd>Favorite or unfavorite</dd><dt>Delete</dt><dd>Move the open photo to the trash</dd><dt>Escape</dt><dd>Close the photo</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd></dl>}
  </SettingsSheet>;
}
