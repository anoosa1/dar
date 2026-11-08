import { useEffect, useRef, useId, useState, type ReactNode } from "react";
import { Icon } from "./icons.tsx";
import {createPortal} from "react-dom";
export function Dialog({ title, onClose, children,dirty=false }: { title: string; onClose: () => void; children: ReactNode;dirty?:boolean }) {
  const ref = useRef<HTMLDialogElement>(null);
  const [discard,setDiscard]=useState(false),unsaved=useRef(dirty);unsaved.current=dirty;
  function close(){if(unsaved.current)setDiscard(true);else onClose();}
  useEffect(()=>{const protect=(event:BeforeUnloadEvent)=>{if(unsaved.current)event.preventDefault();};window.addEventListener("beforeunload",protect);return()=>window.removeEventListener("beforeunload",protect);},[]);
  const titleId=useId();
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    ref.current?.showModal();
    return () => {const rect=previous?.getBoundingClientRect();if(previous&&previous!==document.body&&previous.isConnected&&rect&&rect.width&&rect.height&&rect.right>0&&rect.left<innerWidth&&rect.bottom>0&&rect.top<innerHeight)previous.focus();else{const navigation=document.querySelector<HTMLElement>(".topbar .mobile-menu"),visible=navigation?.getBoundingClientRect();if(visible?.width)navigation?.focus();else document.querySelector<HTMLElement>("main")?.focus();}};
  }, []);
  return createPortal(<dialog ref={ref} className="dialog" aria-labelledby={titleId} onCancel={event => { event.preventDefault(); close(); }}>
    <div className="dialog-heading"><h2 id={titleId}>{title}</h2><button className="icon-button" aria-label="Close dialog" onClick={close}><Icon name="close" /></button></div>{discard&&<div className="draft-banner" role="alert"><p>Discard unsaved changes?</p><div className="dialog-actions"><button className="text-button" onClick={()=>setDiscard(false)}>Keep editing</button><button className="text-button danger" onClick={onClose}>Discard changes</button></div></div>}{children}
  </dialog>,document.body);
}
