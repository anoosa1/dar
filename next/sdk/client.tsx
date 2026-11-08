import {useEffect,useRef,type ReactNode} from "react";
import {WorkspaceHeader,useMobileNavigation} from "../client/shell.tsx";
import type {BrowserSession} from "../client/api.ts";
import type {AppRecord} from "../shared/installed-apps.ts";
export {useState,useEffect,useMemo,useRef,useCallback,createElement,Fragment} from "react";
export {jsx,jsxs} from "react/jsx-runtime";
export type {JSX} from "react/jsx-runtime";
export {createRoot} from "react-dom/client";
export {Select,ActionMenu} from "../client/controls.tsx";
export {EditorPane} from "../client/pane.tsx";
export {Dialog} from "../client/dialog.tsx";
export {Icon} from "../client/icons.tsx";
export type {AppRecord};
export interface AppHost {
  id:string;session:BrowserSession;signal:AbortSignal;onError:(error:unknown)=>void;
  data:{get:<T>(key:string)=>Promise<AppRecord<T>|null>;put:<T>(key:string,value:T,revision:number,schemaVersion?:number)=>Promise<AppRecord<T>>;delete:(key:string,revision:number)=>Promise<void>;list:(cursor?:string)=>Promise<{items:AppRecord[];cursor:string|null}>};
  regional:()=>Promise<{timezone?:string}>;
}
export interface AppModule {mount:(element:HTMLElement,host:AppHost)=>()=>void}
/** External apps reuse Dar's shell and bounded panes, including phone focus return. */
export function AppWorkspace({host,query,onQuery,sidebar,children,details,searchLabel="Search app"}:{host:AppHost;query:string;onQuery:(value:string)=>void;sidebar:ReactNode;children:ReactNode;details?:ReactNode;searchLabel?:string}){
  const nav=useMobileNavigation(),panel=useRef<HTMLElement>(null),origin=useRef<HTMLElement|null>(null),wasOpen=useRef(false);
  useEffect(()=>{if(details&&!wasOpen.current&&matchMedia("(max-width:800px)").matches)(panel.current?.querySelector<HTMLElement>("button")??panel.current)?.focus();if(!details&&wasOpen.current)origin.current?.focus();wasOpen.current=!!details;},[!!details]);
  return <div className={`app-shell extension-shell ${details?"has-extension-details":""}`}>
    <WorkspaceHeader app={`app:${host.id}`} session={host.session} onSignedOut={()=>location.assign("/")} query={query} onQuery={onQuery} searchLabel={searchLabel} onNavigation={()=>nav.setOpen(!nav.open)} navigationOpen={nav.open} navigationId={nav.id}/>
    <div className="extension-workspace">{nav.open&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>nav.setOpen(false)}/>}
      <aside id={nav.id} ref={nav.ref} className={`sidebar ${nav.open?"is-open":""}`} onClick={event=>{if((event.target as HTMLElement).closest("button"))nav.setOpen(false);}}>{sidebar}</aside>
      <main tabIndex={-1} onClickCapture={event=>{origin.current=(event.target as HTMLElement).closest<HTMLElement>("button")??origin.current;}}>{children}</main>
      {details&&<aside ref={panel} tabIndex={-1} className="extension-details" aria-label="Day details">{details}</aside>}
    </div>
  </div>;
}
