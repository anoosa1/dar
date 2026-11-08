import { useEffect, useRef, useId, type ReactNode } from "react";
import { Icon } from "./icons.tsx";
import {createPortal} from "react-dom";
export function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId=useId();
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    ref.current?.showModal();
    return () => {const rect=previous?.getBoundingClientRect();if(previous&&previous!==document.body&&previous.isConnected&&rect&&rect.width&&rect.height&&rect.right>0&&rect.left<innerWidth&&rect.bottom>0&&rect.top<innerHeight)previous.focus();else{const navigation=document.querySelector<HTMLElement>(".topbar .mobile-menu"),visible=navigation?.getBoundingClientRect();if(visible?.width)navigation?.focus();else document.querySelector<HTMLElement>("main")?.focus();}};
  }, []);
  return createPortal(<dialog ref={ref} className="dialog" aria-labelledby={titleId} onCancel={event => { event.preventDefault(); onClose(); }}>
    <div className="dialog-heading"><h2 id={titleId}>{title}</h2><button className="icon-button" aria-label="Close dialog" onClick={onClose}><Icon name="close" /></button></div>{children}
  </dialog>,document.body);
}
