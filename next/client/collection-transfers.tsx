import {useEffect,useRef,useState} from "react";
import {Select} from "./controls.tsx";
import {request,type BrowserSession} from "./api.ts";
import type {PimCollection} from "../shared/pim.ts";

const transfers={
  contacts:{endpoint:"/api/v1/contacts/books",noun:"Address book",plural:"address books",format:"vCard",accept:".vcf,text/vcard"},
  calendar:{endpoint:"/api/v1/calendar/calendars",noun:"Calendar",plural:"calendars",format:"ICS",accept:".ics,text/calendar"},
  notes:{endpoint:"/api/v1/notes/notebooks",noun:"Notebook",plural:"notebooks",format:"Markdown or notebook JSON",accept:".md,.markdown,.json"},
  tasks:{endpoint:"/api/v1/tasks/lists",noun:"Task list",plural:"task lists",format:"ICS",accept:".ics,text/calendar"},
};
export function CollectionTransfers({session,app="calendar"}:{session:BrowserSession;app?:keyof typeof transfers}){
  const {endpoint,noun,plural,format,accept}=transfers[app];
  const [collections,setCollections]=useState<PimCollection[]>([]),[selected,setSelected]=useState(""),[error,setError]=useState(""),[busy,setBusy]=useState(false),[loaded,setLoaded]=useState(false);
  const [results,setResults]=useState<{index:number;error?:string}[]>(),[content,setContent]=useState(""),file=useRef<HTMLInputElement>(null),abort=useRef(new AbortController());
  useEffect(()=>{const controller=new AbortController();abort.current=controller;setLoaded(false);void request<{items:PimCollection[]}>(endpoint,{signal:controller.signal}).then(result=>{if(!controller.signal.aborted){setCollections(result.items);setSelected(result.items[0]?.id??"");setLoaded(true);}}).catch(e=>{if(!controller.signal.aborted){setError(e.message);setLoaded(true);}});return()=>controller.abort();},[endpoint]);
  const collection=collections.find(row=>row.id===selected);
  async function importFile(){
    const source=file.current?.files?.[0];if((!source&&!content)||!collection)return;
    setBusy(true);setError("");setResults(undefined);
    try{
      if((source?.size??new Blob([content]).size)>2*1024*1024)throw Error("Import exceeds 2 MiB.");
      const value=source?await source.text():content,signal=abort.current.signal;
      let items:{error?:string}[];
      if(app==="notes"&&(!source||!/\.json$/i.test(source.name))){
        await request("/api/v1/notes/items",{method:"POST",signal,body:JSON.stringify({collectionId:collection.id,title:source?.name.replace(/\.(md|markdown)$/i,"")??"Imported note",content:value})},session.csrfToken);items=[{}];
      }else{
        const result=await request<{items:{error?:string}[]}>(`${endpoint}/${collection.id}/import`,{method:"POST",signal,body:JSON.stringify(app==="notes"?JSON.parse(value):{content:value})},session.csrfToken);items=result.items;
      }
      if(!signal.aborted)setResults(items.map((row,index)=>({...row,index})));
    }catch(e){if(!abort.current.signal.aborted)setError((e as Error).message);}
    finally{if(!abort.current.signal.aborted)setBusy(false);}
  }
  return <section><h3>Import and export</h3>{error&&<p role="alert" className="error">{error}</p>}{!loaded?<p role="status">Loading {plural}…</p>:!collections.length?<p>No {plural} available.</p>:<>
    <label>{noun}<Select aria-label={noun+" for import or export"} value={selected} onChange={event=>{setSelected(event.target.value);setResults(undefined);}} disabled={busy}>{collections.map(row=><option key={row.id} value={row.id}>{row.name}</option>)}</Select></label>
    {collection&&<a className="secondary-button" href={`${endpoint}/${collection.id}/export`}>Export {app==="notes"?"notebook":format}</a>}
    <h4>Import {format}</h4><label>{format} file<input type="file" ref={file} accept={accept} disabled={busy||collection?.role==="read"}/></label>
    <label>Or paste {app==="notes"?"Markdown":"content"}<textarea rows={6} value={content} disabled={busy||collection?.role==="read"} onChange={e=>setContent(e.target.value)}/></label>
    <button type="button" className="primary-button" disabled={busy||!collection||collection.role==="read"} onClick={()=>{void importFile();}}>{busy?"Importing…":"Import "+format}</button>
    {app==="tasks"&&<p className="field-hint">Task lists share calendars with Calendar. ICS transfers include both events and tasks.</p>}
    {collection?.role==="read"&&<p className="field-hint">Import requires write access to this {noun.toLowerCase()}.</p>}
    {results&&<div role="status"><p>{results.filter(row=>!row.error).length} imported; {results.filter(row=>row.error).length} failed.</p>{results.filter(row=>row.error).map(row=><p key={row.index}>Item {row.index+1}: {row.error}</p>)}</div>}
  </>}</section>;
}
