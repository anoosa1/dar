import {useEffect,useId,useRef,useState} from "react";
import {request} from "./api.ts";
import {Icon} from "./icons.tsx";
import type {SearchResults,SearchResult,SearchGroup} from "../shared/search.ts";
export function navigateSearchResult(href:string){const event=new CustomEvent("dar:navigate",{cancelable:true,detail:{href}});if(window.dispatchEvent(event))location.assign(href);}
export function GlobalSearch(){
  const [query,setQuery]=useState(""),[open,setOpen]=useState(false),[result,setResult]=useState<SearchResults>({items:[],truncated:false}),[loading,setLoading]=useState(false),[error,setError]=useState(""),[retry,setRetry]=useState(0),[active,setActive]=useState(-1);
  const input=useRef<HTMLInputElement>(null),root=useRef<HTMLDivElement>(null),id=useId();
  useEffect(()=>{const shortcut=(event:KeyboardEvent)=>{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==="k"&&!event.defaultPrevented&&!document.querySelector("dialog[open],[data-editor-pane]")){event.preventDefault();input.current?.focus();input.current?.select();}};const outside=(event:PointerEvent)=>{if(!root.current?.contains(event.target as Node))setOpen(false);};document.addEventListener("keydown",shortcut);document.addEventListener("pointerdown",outside);return()=>{document.removeEventListener("keydown",shortcut);document.removeEventListener("pointerdown",outside);};},[]);
  useEffect(()=>{
    const controller=new AbortController();setActive(-1);setResult({items:[],truncated:false});setError("");if(!query.trim()){setLoading(false);return;}
    setLoading(true);const timer=setTimeout(()=>{void request<SearchResults>(`/api/v1/search?q=${encodeURIComponent(query)}`,{signal:controller.signal}).then(value=>{if(!controller.signal.aborted)setResult(value);}).catch(e=>{if(!controller.signal.aborted)setError(e.message);}).finally(()=>{if(!controller.signal.aborted)setLoading(false);});},180);
    return()=>{clearTimeout(timer);controller.abort();};
  },[query,retry]);
  useEffect(()=>{if(active>=0)root.current?.querySelector(`#${CSS.escape(id+"-"+active)}`)?.scrollIntoView({block:"nearest"});},[active]);
  const order:SearchGroup[]=["Files","Contacts","Calendar","Notes","Tasks","Applications"],items=order.flatMap(group=>result.items.filter(item=>item.group===group));
  function choose(item:SearchResult){setOpen(false);navigateSearchResult(item.href);}
  return <div className="global-search-wrap" ref={root}><div className="global-search"><Icon name="search"/><input ref={input} type="search" role="combobox" aria-label="Search all apps" aria-autocomplete="list" aria-haspopup="listbox" aria-controls={id} aria-expanded={open&&!!query.trim()} aria-activedescendant={open&&active>=0?id+"-"+active:undefined} placeholder="Search all apps…" maxLength={200} value={query} onFocus={()=>setOpen(true)} onChange={event=>{setQuery(event.target.value);setOpen(true);}} onKeyDown={event=>{
    if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setQuery("");setOpen(false);setActive(-1);}
    else if(["ArrowDown","ArrowUp"].includes(event.key)&&items.length){event.preventDefault();setOpen(true);setActive(old=>old<0?(event.key==="ArrowDown"?0:items.length-1):(old+(event.key==="ArrowDown"?1:-1)+items.length)%items.length);}
    else if(event.key==="Enter"&&active>=0&&items[active]){event.preventDefault();choose(items[active]);}else if(event.key==="Tab")setOpen(false);
  }}/>{query?<button className="icon-button" aria-label="Clear search" onClick={()=>{setQuery("");input.current?.focus();}}><Icon name="close"/></button>:<kbd className="search-shortcut" aria-hidden="true">{navigator.platform.includes("Mac")?"⌘ K":"Ctrl K"}</kbd>}</div>
  {open&&query.trim()&&<div className="global-search-results"><div role="status" aria-live="polite">{loading?"Searching all apps…":error?"Search could not be completed.":!items.length?"No matches across your apps.":result.truncated?"Showing the first matches. Refine your search for more.":""}</div>{error&&<div className="error"><p role="alert">{error}</p><button className="text-button" onClick={()=>setRetry(n=>n+1)}>Retry search</button></div>}<div role="listbox" id={id} aria-label="Search results">{order.map(group=>{const rows=items.filter(item=>item.group===group);return rows.length?<div role="group" aria-label={group} key={group}><h2>{group}</h2>{rows.map(item=>{const index=items.indexOf(item);return <a key={item.id} href={item.href} role="option" aria-selected={active===index} id={id+"-"+index} tabIndex={-1} className="global-search-result" onMouseMove={()=>setActive(index)} onClick={event=>{event.preventDefault();choose(item);}}><Icon name={group==="Files"?"file":group==="Tasks"?"check":group==="Calendar"?"clock":group==="Contacts"?"share":"grid"}/><span><strong>{item.title}</strong><small>{item.detail}</small></span></a>;})}</div>:null;})}</div></div>}
  </div>;
}
