import {useEffect,useId,useRef,type KeyboardEvent,type ReactNode} from "react";
import {createPortal} from "react-dom";
import {Icon,type IconName} from "./icons.tsx";

/** Settings sheet B (docs/spacetimedb/settings-revamp-mock-b.html): each control saves itself. */
export type SaveState="saving"|"saved"|"error";
export interface SettingsNotice {message:string;undo?:()=>void}

/** A right sheet in place of the details pane (a bottom sheet on phones). Closing never discards. */
export function SettingsSheet({title,icon="settings",sections,section,onSection,onClose,notice,onNotice,footer="Changes save to your account as you make them.",children}:{title:string;icon?:IconName;sections:string[];section:string;onSection:(section:string)=>void;onClose:()=>void;notice?:SettingsNotice|null;onNotice?:(notice:null)=>void;footer?:ReactNode;children:ReactNode}){
  const ref=useRef<HTMLElement>(null),tabs=useRef<HTMLDivElement>(null),id=useId(),panel=useId(),close=useRef(onClose);close.current=onClose;
  useEffect(()=>{
    const previous=document.activeElement as HTMLElement|null;
    (tabs.current?.querySelector<HTMLElement>('[aria-selected="true"]')??ref.current)?.focus();
    return()=>{if(previous?.isConnected&&previous.getBoundingClientRect().width)previous.focus({preventScroll:true});else document.querySelector<HTMLElement>("main")?.focus({preventScroll:true});};
  },[]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>onNotice?.(null),8000);return()=>clearTimeout(timer);},[notice]);
  function escape(event:KeyboardEvent){
    if(event.key!=="Escape"||event.defaultPrevented)return;
    // Open menus and selects close first.
    if((event.target as HTMLElement).closest('[aria-expanded="true"],[role="listbox"],[role="menu"]'))return;
    event.preventDefault();close.current();
  }
  function move(event:KeyboardEvent){
    const index=sections.indexOf(section),last=sections.length-1;
    const next={ArrowRight:index+1,ArrowLeft:index-1,Home:0,End:last}[event.key];if(next===undefined)return;
    event.preventDefault();const target=sections[(next+sections.length)%sections.length];onSection(target);
    requestAnimationFrame(()=>tabs.current?.querySelector<HTMLElement>(`[data-section="${CSS.escape(target)}"]`)?.focus());
  }
  return createPortal(<aside ref={ref} role="dialog" aria-modal="false" aria-labelledby={id} tabIndex={-1} data-settings-sheet="" className="editing-pane settings-sheet" onKeyDown={escape}>
    <span className="settings-sheet-handle" aria-hidden="true"/>
    <div className="settings-sheet-heading"><span className="settings-sheet-icon" aria-hidden="true"><Icon name={icon}/></span><h2 id={id}>{title}</h2><button type="button" className="icon-button" aria-label="Close settings" onClick={onClose}><Icon name="close"/></button></div>
    {sections.length>1&&<div ref={tabs} role="tablist" aria-label={`${title} sections`} className="settings-tabs" onKeyDown={move}>{sections.map(item=><button key={item} type="button" role="tab" data-section={item} aria-selected={item===section} aria-controls={panel} tabIndex={item===section?0:-1} onClick={()=>onSection(item)}>{item}</button>)}</div>}
    <div id={panel} role={sections.length>1?"tabpanel":undefined} aria-label={section} className="settings-sheet-content">{children}</div>
    <div className="settings-sheet-footer"><span>{footer}</span></div>
    <div className="settings-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast"><span>{notice.message}</span>{notice.undo&&<button type="button" className="text-button" onClick={()=>{const undo=notice.undo!;onNotice?.(null);undo();}}>Undo</button>}</div>}</div>
  </aside>,document.body);
}

/** Visual save state beside a row; the sheet's status region announces results. */
export function SaveStatus({state}:{state?:SaveState}){
  if(!state)return null;
  return <span className={`settings-save-state is-${state}`} aria-hidden="true">{state==="saving"?<><span className="settings-spinner"/>Saving…</>:state==="saved"?<><Icon name="check"/>Saved</>:"Not saved"}</span>;
}

/** Label and short explanation on the left, the control on the right (stacked when wide). */
export function SettingRow({label,hint,state,stacked=false,children}:{label:string;hint?:ReactNode;state?:SaveState;stacked?:boolean;children:ReactNode}){
  return <div className={`settings-row ${stacked?"is-stacked":""}`}><div className="settings-row-text"><span className="settings-row-label">{label}</span>{hint&&<small>{hint}</small>}</div><SaveStatus state={state}/><div className="settings-row-control">{children}</div></div>;
}

/** An on/off row: the whole row is the switch's label. */
export function SettingSwitch({label,hint,state,checked,disabled,onChange}:{label:string;hint?:ReactNode;state?:SaveState;checked:boolean;disabled?:boolean;onChange:(checked:boolean)=>void}){
  const id=useId();
  return <label className="settings-row settings-switch"><span className="settings-row-text"><span id={id+"label"} className="settings-row-label">{label}</span>{hint&&<small id={id+"hint"}>{hint}</small>}</span><SaveStatus state={state}/><span className="ui-switch"><input type="checkbox" role="switch" aria-labelledby={id+"label"} aria-describedby={hint?id+"hint":undefined} checked={checked} disabled={disabled} onChange={event=>onChange(event.target.checked)}/></span></label>;
}

/** Two or three choices as a segmented radio group with native arrow-key behavior. */
export function Segmented<T extends string|number>({label,value,options,onChange,disabled}:{label:string;value:T;options:readonly (readonly [T,string])[];onChange:(value:T)=>void;disabled?:boolean}){
  const name=useId();
  return <div role="radiogroup" aria-label={label} className="ui-segmented">{options.map(([option,text])=><label key={String(option)} className={option===value?"is-checked":""}><input type="radio" name={name} checked={option===value} disabled={disabled} onChange={()=>onChange(option)}/><span>{text}</span></label>)}</div>;
}
