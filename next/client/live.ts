import {useEffect,useSyncExternalStore} from "react";
import {DbConnection} from "./module_bindings/index.ts";
import {request,type BrowserSession} from "./api.ts";

/** "open" covers the first connection and every automatic reconnect. */
export type LiveStatus="connecting"|"open"|"reconnecting"|"error";
interface Live {key:string;status:LiveStatus;connection:DbConnection|null;users:number;timer?:ReturnType<typeof setTimeout>;close:()=>void}

const listeners=new Set<()=>void>();
let live:Live|undefined,snapshot:{key:string;status:LiveStatus;connection:DbConnection|null}={key:"",status:"connecting",connection:null};
function publish(){snapshot=live?{key:live.key,status:live.status,connection:live.connection}:{key:"",status:"connecting",connection:null};for(const listener of listeners)listener();}
const waiting={status:"connecting" as LiveStatus,connection:null};

function open(session:BrowserSession):Live{
  const tab=crypto.randomUUID();let db:DbConnection|undefined,closed=false;
  const token=()=>request<{token:string;databaseUrl:string;databaseName:string}>(`/api/v1/auth/token?tab=${tab}`,{method:"POST"},session.csrfToken);
  const state:Live={key:session.csrfToken,status:"connecting",connection:null,users:0,close:()=>{closed=true;db?.disconnect();}};
  const set=(status:LiveStatus,connection=state.connection)=>{if(closed||live!==state)return;state.status=status;state.connection=connection;publish();};
  void token().then(config=>{
    if(closed)return;
    db=DbConnection.builder().withUri(config.databaseUrl.replace(/^http/,"ws")).withDatabaseName(config.databaseName).withToken(config.token)
      .withAutomaticReconnect().withTokenProvider(async()=>(await token()).token)
      .onConnect(connection=>set("open",connection)).onAutomaticReconnect(connection=>set("open",connection))
      .onDisconnect((_ctx,_error,next)=>set(next?"reconnecting":"error")).onConnectError(()=>set("error")).build();
  }).catch(()=>set("error"));
  return state;
}

/**
 * The page's live database connection. Files, Contacts, Calendar, Notes and Tasks
 * share one connection and keep it for a minute after the last of them closes, so
 * switching between apps does not reconnect. Each app adds its own subscriptions
 * and table callbacks and removes them when it closes.
 */
export function useLiveConnection(session:BrowserSession){
  useEffect(()=>{
    if(live&&live.key!==session.csrfToken){live.close();live=undefined;}
    if(!live){live=open(session);publish();}
    const current=live;current.users++;clearTimeout(current.timer);
    return()=>{if(--current.users)return;current.timer=setTimeout(()=>{if(current.users||live!==current)return;current.close();live=undefined;publish();},60_000);};
  },[session.csrfToken]);
  const current=useSyncExternalStore(listener=>{listeners.add(listener);return()=>{listeners.delete(listener);};},()=>snapshot);
  // Never hand one account's connection to another session.
  return current.key===session.csrfToken?current:waiting;
}

interface Shared {queries:string[];listeners:Set<{applied:()=>void;error:()=>void}>;applied:boolean;timer?:ReturnType<typeof setTimeout>;handle?:{isActive:()=>boolean;unsubscribe:()=>void}}
const subscriptions=new WeakMap<DbConnection,Map<string,Shared>>();
/**
 * Subscribes an app to its queries on the shared connection; returns the function
 * that ends it. Apps asking for the same queries share one subscription, and an
 * ended one is kept a few seconds, so the next app is subscribed before the last
 * one lets go (overlapping unsubscribe and subscribe requests could leave the next
 * app waiting). Every attached app hears each apply, including reconnect replays.
 */
export function subscribe(connection:DbConnection,queries:string[],onApplied:()=>void,onError:()=>void){
  let byQuery=subscriptions.get(connection);if(!byQuery)subscriptions.set(connection,byQuery=new Map());
  const key=queries.join("\n"),listener={applied:onApplied,error:onError};let entry=byQuery.get(key);
  if(!entry){
    const created:Shared={queries,listeners:new Set(),applied:false};entry=created;byQuery.set(key,created);
    created.handle=connection.subscriptionBuilder()
      .onApplied(()=>{created.applied=true;if(byQuery!.get(key)!==created){end(created);return;}for(const item of [...created.listeners])item.applied();})
      .onError(()=>{created.applied=false;if(byQuery!.get(key)===created)byQuery!.delete(key);for(const item of [...created.listeners])item.error();})
      .subscribe(queries);
  }else{
    clearTimeout(entry.timer);
    const shared=entry;if(shared.applied)queueMicrotask(()=>{if(shared.listeners.has(listener))onApplied();});
  }
  const current=entry;current.listeners.add(listener);
  return()=>{
    current.listeners.delete(listener);if(current.listeners.size)return;
    clearTimeout(current.timer);
    current.timer=setTimeout(()=>{if(current.listeners.size||byQuery!.get(key)!==current)return;byQuery!.delete(key);end(current);},5_000);
  };
}
function end(entry:Shared){try{if(entry.handle?.isActive())entry.handle.unsubscribe();}catch{/* already ending */}}
