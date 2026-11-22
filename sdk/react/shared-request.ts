import {useSyncExternalStore} from "react";
import {request} from "./api.ts";

/** Window focus refreshes data older than this; the named events always refresh. */
const fresh=15_000;
export interface SharedState<T>{value?:T;error:string}
/**
 * One request per page for data several components read. Every reader shares the
 * latest result; the named events force a refresh and window focus refreshes stale
 * data while anything still reads it.
 */export function sharedRequest<T>(url:string,events:string[]){
  let state:SharedState<T>={error:""},sequence=0,fetchedAt=0,receivedAt=0,controller:AbortController|undefined,listening=false;
  const listeners=new Set<()=>void>();
  const publish=(next:SharedState<T>)=>{state=next;for(const listener of listeners)listener();};
  function refresh(){
    controller?.abort();controller=new AbortController();
    const id=++sequence,signal=controller.signal;fetchedAt=Date.now();
    void request<T>(url,{signal}).then(value=>{if(id===sequence){receivedAt=Date.now();publish({value,error:""});}}).catch((e:Error)=>{if(id===sequence&&!signal.aborted)publish({...state,error:e.message});});
  }
  const stale=()=>{if(listeners.size&&Date.now()-fetchedAt>fresh)refresh();};
  function subscribe(listener:()=>void){
    if(!listening){listening=true;for(const name of events)window.addEventListener(name,()=>{if(listeners.size)refresh();});window.addEventListener("focus",stale);}
    listeners.add(listener);if(listeners.size===1)stale();
    // Without readers (for example after signing out) nothing is kept for the next account.
    // Readers replaced within the same render keep the result.
    return()=>{listeners.delete(listener);setTimeout(()=>{if(!listeners.size){controller?.abort();sequence++;fetchedAt=0;state={error:""};}});};
  }
  const read=()=>state;
  /** Milliseconds since the latest value arrived, or Infinity before any. */
  const age=()=>state.value===undefined?Infinity:Date.now()-receivedAt;
  return {refresh,read,age,subscribe,use:()=>useSyncExternalStore(subscribe,read)};
}
