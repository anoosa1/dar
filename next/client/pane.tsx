import {useEffect,useId,useRef,useState,type ReactNode} from "react";
import {createPortal} from "react-dom";
import {Icon} from "./icons.tsx";

/** Non-modal workspace editor. Its form owns validation and persistence. */
export function EditorPane({title,onClose,children,dirty:formDirty}:{title:string;onClose:()=>void;children:ReactNode;dirty?:boolean}){
  const ref=useRef<HTMLElement>(null),id=useId(),dirty=useRef(false),[discard,setDiscard]=useState(false);
  if(formDirty!==undefined)dirty.current=formDirty;
  function close(){if(dirty.current)setDiscard(true);else onClose();}
  useEffect(()=>{
    const previous=document.activeElement as HTMLElement|null;
    (ref.current?.querySelector<HTMLElement>('[autofocus],input:not([type=hidden]),textarea')??ref.current)?.focus();
    const protect=(event:BeforeUnloadEvent)=>{if(dirty.current)event.preventDefault();};window.addEventListener("beforeunload",protect);
    return()=>{window.removeEventListener("beforeunload",protect);if(previous?.isConnected&&previous.getBoundingClientRect().width)previous.focus({preventScroll:true});else document.querySelector<HTMLElement>(".topbar .mobile-menu,main")?.focus({preventScroll:true});};
  },[]);
  return createPortal(<aside ref={ref} tabIndex={-1} data-editor-pane="" className="editing-pane" aria-labelledby={id} onClickCapture={event=>{if((event.target as HTMLElement).closest('[role="option"]'))dirty.current=true;}} onChangeCapture={()=>{dirty.current=true;}} onKeyDown={event=>{if(event.key==="Escape"&&!event.defaultPrevented){event.preventDefault();close();}}}>
    <div className="editing-pane-heading"><h2 id={id}>{title}</h2><button type="button" className="icon-button" aria-label="Close editor" onClick={close}><Icon name="close"/></button></div>
    {discard&&<div className="draft-banner" role="alert"><p>Discard unsaved changes?</p><button type="button" className="text-button" onClick={()=>setDiscard(false)}>Keep editing</button><button type="button" className="text-button danger" onClick={onClose}>Discard changes</button></div>}
    <div className="editing-pane-content" onClickCapture={event=>{const button=(event.target as HTMLElement).closest("button");if(button?.textContent?.trim()==="Cancel"){event.preventDefault();event.stopPropagation();close();}}}>{children}</div>
  </aside>,document.body);
}
