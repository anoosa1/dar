import {useEffect,useRef,useState} from "react";
import {request} from "./api.ts";
import {Icon} from "./icons.tsx";
import {FileTile} from "./file-types.tsx";
import type {DarFilePickerPage,DarFileSelection} from "../shared/file-picker.ts";
export type {DarFileSelection};

/** Inline Files browser shared by app forms. The caller owns its final action.
 * Folder mode: the destination is the highlighted folder row, otherwise the folder being
 * viewed; a row's chevron (or double-click) opens it. File mode selects a file row.
 * Loading/access loss clears the selection, so callers cannot submit stale IDs.
 */
export function DarFilePicker({mode="folder",initialFolderId="",allowRoot=false,excludeIds=[],disabled=false,onChange}:{
  mode?:"file"|"folder";initialFolderId?:string;allowRoot?:boolean;excludeIds?:string[];disabled?:boolean;
  onChange:(selection:DarFileSelection|undefined)=>void;
}){
  const [parent,setParent]=useState(initialFolderId),[cursor,setCursor]=useState(""),[previous,setPrevious]=useState<string[]>([]),[page,setPage]=useState<DarFilePickerPage>(),[error,setError]=useState(""),[attempt,setAttempt]=useState(0),[query,setQuery]=useState(""),[selected,setSelected]=useState<DarFileSelection>();
  const changed=useRef(onChange);changed.current=onChange;
  const here=(value:DarFilePickerPage|undefined)=>value?.folder?{id:value.folder.id,name:value.folder.name,kind:"folder" as const}:allowRoot?{id:"",name:"All files",kind:"folder" as const}:undefined;
  useEffect(()=>{
    const abort=new AbortController();setPage(undefined);setError("");setSelected(undefined);changed.current(undefined);
    const params=new URLSearchParams({parentId:parent,cursor,kind:mode});
    void request<DarFilePickerPage>(`/api/v1/files/picker?${params}`,{signal:abort.signal}).then(value=>{
      if(abort.signal.aborted)return;setPage(value);
      if(mode==="folder")changed.current(here(value));
    }).catch(cause=>{if(!abort.signal.aborted)setError(cause instanceof Error?cause.message:"Folders could not be loaded.");});
    return()=>abort.abort();
  },[parent,cursor,mode,allowRoot,attempt]);
  function browse(id:string){setParent(id);setCursor("");setPrevious([]);setQuery("");}
  function choose(item:DarFileSelection){
    // Choosing the highlighted folder again returns the destination to the folder being viewed.
    const next=selected?.id===item.id&&mode==="folder"?undefined:item;
    setSelected(next);changed.current(next??(mode==="folder"?here(page):undefined));
  }
  const items=page?.items.filter(item=>!excludeIds.includes(item.id)&&item.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))??[];
  const up=page?.folder?page.ancestors.at(-1)?.id??"":undefined,destination=selected??(mode==="folder"?here(page):undefined);
  const noun=mode==="folder"?"folders":"items";
  return <section className="dar-file-picker" aria-label={mode==="folder"?"Dar folder picker":"Dar file picker"} aria-busy={!page&&!error}>
    <div className="dar-picker-head">
      <button type="button" className="icon-button" aria-label="Up one folder" title="Up one folder" disabled={disabled||up===undefined} onClick={()=>browse(up??"")}><Icon name="back"/></button>
      <nav className="dar-picker-path" aria-label="Folder path">
        {page?.folder?<button type="button" disabled={disabled} onClick={()=>browse("")}>All files</button>:<strong aria-current="location">All files</strong>}
        {page?.ancestors.map(part=><span key={part.id}><Icon name="chevron"/><button type="button" disabled={disabled} onClick={()=>browse(part.id)}>{part.name}</button></span>)}
        {page?.folder&&<span><Icon name="chevron"/><strong aria-current="location">{page.folder.name}</strong></span>}
      </nav>
    </div>
    <div className="dar-picker-search"><Icon name="search"/><input type="search" aria-label={`Filter ${noun} in this folder`} placeholder={`Filter ${noun}`} value={query} disabled={disabled||!page} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Enter")event.preventDefault();}}/></div>
    {error?<div className="dar-picker-state" role="alert"><p className="error">{error}</p><button type="button" className="secondary-button" onClick={()=>setAttempt(n=>n+1)}>Retry</button></div>
      :!page?<p className="dar-picker-state muted" role="status">Loading {noun}…</p>
      :<>
        <ul className="dar-picker-list" aria-label={page.folder?.name??"All files"}>{items.map(item=>{
          const open=()=>browse(item.id),pressed=selected?.id===item.id;
          return <li key={item.id} className={pressed?"is-selected":""}>
            <button type="button" className="dar-picker-row" disabled={disabled} aria-pressed={item.kind==="file"||mode==="folder"?pressed:undefined}
              onClick={()=>{if(item.kind==="folder"&&mode==="file")open();else choose(item);}} onDoubleClick={()=>{if(item.kind==="folder")open();}}
              onKeyDown={event=>{if(item.kind==="folder"&&(event.key==="ArrowRight"||event.key==="Enter"&&event.shiftKey)){event.preventDefault();open();}}}>
              <FileTile entry={{kind:item.kind,name:item.name,mediaType:""}}/><span>{item.name}</span>{pressed&&<Icon name="check"/>}
            </button>
            {item.kind==="folder"&&<button type="button" className="icon-button dar-picker-open" aria-label={`Open ${item.name}`} title="Open folder" disabled={disabled} onClick={open}><Icon name="chevron"/></button>}
          </li>;})}
        </ul>
        {!items.length&&<p className="dar-picker-state muted">{query?`No matching ${noun} here.`:`No ${noun} here.`}</p>}
        {(previous.length>0||page.nextCursor)&&<div className="dar-picker-pages"><button type="button" className="text-button" disabled={disabled||!previous.length} onClick={()=>{setCursor(previous.at(-1)??"");setPrevious(values=>values.slice(0,-1));}}>Previous</button><button type="button" className="text-button" disabled={disabled||!page.nextCursor} onClick={()=>{setPrevious(values=>[...values,cursor]);setCursor(page.nextCursor!);}}>Next</button></div>}
      </>}
    <p className="dar-picker-selection" role="status">{destination?<><Icon name={destination.kind==="folder"?"folder":"file"}/><span>{mode==="folder"?"Destination":"Selected"}: <strong>{destination.name}</strong></span></>:<span>{mode==="folder"?"Open a folder to use it.":"Choose a file."}</span>}</p>
  </section>;
}
