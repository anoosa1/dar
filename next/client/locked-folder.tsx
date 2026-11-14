import {useCallback,useEffect,useRef,useState,type FormEvent} from "react";
import {request,type BrowserSession} from "./api.ts";
import {Icon} from "./icons.tsx";
import {Dialog} from "./dialog.tsx";
import {lockedUnlockMinutes,type LockedFolderState} from "../shared/locked-folder.ts";
import "./locked-folder.css";

const endpoint="/api/v1/files/locked";
/**
 * The account's Locked folder for this browser session. It relocks by itself when the
 * unlock expires; Files and Photos share it.
 */
export function useLockedFolder(session:BrowserSession){
  const [state,setState]=useState<LockedFolderState>(),[error,setError]=useState("");
  const refresh=useCallback(async()=>{try{const value=await request<LockedFolderState>(endpoint);setState(value);setError("");return value;}catch(cause){setError((cause as Error).message);return undefined;}},[]);
  useEffect(()=>{void refresh();},[refresh]);
  useEffect(()=>{if(!state?.unlockedUntil)return;const timer=setTimeout(()=>{void refresh();},Math.max(0,state.unlockedUntil-Date.now())+500);return()=>clearTimeout(timer);},[state,refresh]);
  const unlock=async(password:string)=>{const value=await request<LockedFolderState>(`${endpoint}/unlock`,{method:"POST",body:JSON.stringify({password})},session.csrfToken);setState(value);return value;};
  const lock=async()=>{const value=await request<LockedFolderState>(`${endpoint}/lock`,{method:"POST"},session.csrfToken);setState(value);return value;};
  const unlocked=!!state&&state.unlockedUntil>Date.now();
  return {state,unlocked,error,refresh,unlock,lock};
}
export type LockedFolder=ReturnType<typeof useLockedFolder>;

/** Password form shown in place of locked content. */
export function UnlockForm({folder,title="Locked folder",description,onUnlocked,compact=false}:{folder:LockedFolder;title?:string;description:string;onUnlocked?:(state:LockedFolderState)=>void;compact?:boolean}){
  const [password,setPassword]=useState(""),[error,setError]=useState(""),[busy,setBusy]=useState(false),input=useRef<HTMLInputElement>(null);
  useEffect(()=>{if(!compact)input.current?.focus({preventScroll:true});},[compact]);
  async function submit(event:FormEvent){
    event.preventDefault();if(!password)return setError("Enter your password.");setBusy(true);setError("");
    try{const value=await folder.unlock(password);setPassword("");onUnlocked?.(value);}catch(cause){setError((cause as Error).message);input.current?.focus();}finally{setBusy(false);}
  }
  return <form className={`locked-unlock${compact?" is-compact":""}`} onSubmit={event=>{void submit(event);}}>
    {!compact&&<span className="locked-unlock-icon" aria-hidden="true"><Icon name="lock"/></span>}
    {!compact&&<h2>{title}</h2>}
    <p>{description}</p>
    <label>Password<input ref={input} type="password" autoComplete="current-password" value={password} onChange={event=>{setPassword(event.target.value);setError("");}} aria-invalid={!!error} aria-describedby={error?"locked-unlock-error":undefined}/></label>
    {error&&<p id="locked-unlock-error" role="alert" className="error">{error}</p>}
    <button className="primary-button" disabled={busy}><Icon name="lock"/>{busy?"Unlocking…":"Unlock"}</button>
    <small className="field-hint">Stays open on this device for {lockedUnlockMinutes} minutes, or until you lock it.</small>
  </form>;
}
/** Asks for the password before an action that needs the Locked folder, then runs it. */
export function UnlockDialog({folder,action,onClose,onUnlocked}:{folder:LockedFolder;action:string;onClose:()=>void;onUnlocked:(state:LockedFolderState)=>void}){
  return <Dialog title="Unlock the Locked folder" onClose={onClose}><UnlockForm compact folder={folder} description={`Enter your password to ${action}.`} onUnlocked={onUnlocked}/></Dialog>;
}
