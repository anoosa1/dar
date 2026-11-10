import {useEffect,useRef,useState} from "react";
import {request} from "./api.ts";
import {Icon} from "./icons.tsx";
import type {DarFilePickerPage,DarFileSelection} from "../shared/file-picker.ts";
import "./file-picker.css";
export type {DarFileSelection};

/** Inline Files browser shared by app forms. The caller owns its final action.
 * Folder mode selects the folder being viewed; file mode selects a file row.
 * Loading/access loss clears the selection, so callers cannot submit stale IDs.
 */
export function DarFilePicker({mode="folder",initialFolderId="",allowRoot=false,excludeIds=[],disabled=false,onChange}:{
  mode?:"file"|"folder";initialFolderId?:string;allowRoot?:boolean;excludeIds?:string[];disabled?:boolean;
  onChange:(selection:DarFileSelection|undefined)=>void;
}){
  const [parent,setParent]=useState(initialFolderId),[cursor,setCursor]=useState(""),[previous,setPrevious]=useState<string[]>([]),[page,setPage]=useState<DarFilePickerPage>(),[error,setError]=useState(""),[attempt,setAttempt]=useState(0),[query,setQuery]=useState(""),[selected,setSelected]=useState("");
  const changed=useRef(onChange);changed.current=onChange;
  useEffect(()=>{
    const abort=new AbortController();setPage(undefined);setError("");setSelected("");changed.current(undefined);
    const params=new URLSearchParams({parentId:parent,cursor,kind:mode});
    void request<DarFilePickerPage>(`/api/v1/files/picker?${params}`,{signal:abort.signal}).then(value=>{
      if(abort.signal.aborted)return;setPage(value);
      if(mode==="folder")changed.current(value.folder??(allowRoot?{id:"",name:"All files",kind:"folder"}:undefined));
    }).catch(cause=>{if(!abort.signal.aborted)setError(cause instanceof Error?cause.message:"Folders could not be loaded.");});
    return()=>abort.abort();
  },[parent,cursor,mode,allowRoot,attempt]);
  function browse(id:string){setParent(id);setCursor("");setPrevious([]);setQuery("");}
  const items=page?.items.filter(item=>!excludeIds.includes(item.id)&&item.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()))??[];
  return <section className="dar-file-picker" aria-label={mode==="folder"?"Dar folder picker":"Dar file picker"} aria-busy={!page&&!error}>
    <nav className="dar-picker-path" aria-label="Folder path"><button type="button" className="text-button" disabled={disabled||!parent} onClick={()=>browse("")}>All files</button>{page?.ancestors.map(part=><button key={part.id} type="button" className="text-button" disabled={disabled} onClick={()=>browse(part.id)}>/ {part.name}</button>)}{page?.folder&&<strong aria-current="location">/ {page.folder.name}</strong>}</nav>
    {error?<div role="alert"><p className="error">{error}</p><button type="button" className="secondary-button" onClick={()=>setAttempt(n=>n+1)}>Retry folders</button></div>:!page?<p role="status">Loading folders…</p>:<>
      <label>Filter this page<input type="search" value={query} disabled={disabled} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Enter")event.preventDefault();}}/></label>
      <ul>{items.map(item=><li key={item.id}><button type="button" className="dar-picker-row" disabled={disabled} aria-pressed={item.kind==="file"?selected===item.id:undefined} onClick={()=>{if(item.kind==="folder")browse(item.id);else{setSelected(item.id);changed.current(item);}}}><Icon name={item.kind==="folder"?"folder":"file"}/><span>{item.name}</span>{item.kind==="folder"&&<Icon name="chevron"/>}</button></li>)}</ul>
      {!items.length&&<p className="muted small">{query?"No matching items on this page.":"No items on this page."}</p>}
      {(previous.length>0||page.nextCursor)&&<div className="pagination"><button type="button" className="text-button" disabled={disabled||!previous.length} onClick={()=>{setCursor(previous.at(-1)??"");setPrevious(values=>values.slice(0,-1));}}>Previous folders</button><button type="button" className="text-button" disabled={disabled||!page.nextCursor} onClick={()=>{setPrevious(values=>[...values,cursor]);setCursor(page.nextCursor!);}}>Next folders</button></div>}
      {mode==="folder"&&<p className="dar-picker-selection" role="status">{page.folder?`Selected folder: ${page.folder.name}`:allowRoot?"Selected folder: All files":"Open the folder to use."}</p>}
    </>}
  </section>;
}
