import {useEffect,useState,type ReactNode} from "react";
import {request,type BrowserSession} from "./api.ts";
import {usePreferenceSaver,SaveProblems} from "./app-settings.tsx";
import {Avatar} from "./shell.tsx";
import {Icon,type IconName} from "./icons.tsx";
import type {ManagedApp} from "../shared/app-settings.ts";
import type {InstanceAppearance} from "./instance-appearance.tsx";

type Data=Record<string,any>;
const plural=(count:number,one:string,many=one+"s")=>`${count} ${count===1?one:many}`;
const themes=[["system","Follow system"],["latte","Light"],["mocha","Dark"]] as const;

/** One card per section, showing its current value; the title opens the section. */
function Card({icon,title,label=title,onOpen,current=false,children}:{icon:IconName;title:string;label?:string;onOpen:()=>void;current?:boolean;children?:ReactNode}){
  return <article className={`settings-overview-card ${current?"is-current":""}`}><button type="button" className="settings-overview-open" aria-label={`Open ${label}`} onClick={onOpen}><Icon name={icon}/><span>{title}</span><Icon name="chevron"/></button>{children}</article>;
}

/** Settings home (settings sheet B): current values at a glance, theme changes in place. */
export function SettingsOverview({session,connections,clients,users,groups,credentials,status,onOpen}:{session:BrowserSession;connections:number;clients:number;users:number;groups:number;credentials:Data[];status?:Data;onOpen:(section:string)=>void}){
  const admin=session.user.administrator,saver=usePreferenceSaver(session),preferences=saver.preferences;
  const [passwords,setPasswords]=useState<number>(),[apps,setApps]=useState<ManagedApp[]>(),[logo,setLogo]=useState<InstanceAppearance>(),[now,setNow]=useState(Date.now());
  useEffect(()=>{
    const controller=new AbortController(),signal=controller.signal,quiet=()=>{};
    void request<{items:unknown[]}>("/api/v1/auth/app-passwords",{signal}).then(value=>setPasswords(value.items.length)).catch(quiet);
    if(admin){void request<{items:ManagedApp[]}>("/api/v1/admin/installedApps",{signal}).then(value=>setApps(value.items)).catch(quiet);void request<InstanceAppearance>("/api/v1/appearance",{signal}).then(setLogo).catch(quiet);}
    const timer=setInterval(()=>setNow(Date.now()),30000);
    return()=>{controller.abort();clearInterval(timer);};
  },[admin]);
  const zone=preferences?.timezone??"UTC",theme=preferences?.appearance??"system",activeCredentials=credentials.filter(c=>!c.revoked&&Date.parse(c.expiresAt)>now).length;
  let clock="";try{clock=new Intl.DateTimeFormat(undefined,{timeZone:zone,hour:"numeric",minute:"2-digit",timeZoneName:"short"}).format(now);}catch{}
  return <section className="settings-overview" aria-labelledby="settings-overview-heading">
    <h1 id="settings-overview-heading" className="sr-only">Overview</h1>
    <div className="settings-overview-me"><Avatar name={session.user.displayName}/><div><h2>{session.user.displayName}</h2><p className="muted">@{session.user.username} · {admin?"Administrator":"Member"}</p></div><button type="button" className="secondary-button" onClick={()=>onOpen("Profile")}>Edit profile</button></div>
    <SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>{void saver.reload();}}/>
    <h2 className="settings-overview-heading">Personal</h2>
    <div className="settings-overview-grid">
      <Card icon="palette" title="Appearance" onOpen={()=>onOpen("Appearance")}><p>{themes.find(([id])=>id===theme)?.[1]}{saver.states.appearance==="saving"?" · Saving…":saver.states.appearance==="saved"?" · Saved":""}</p><div className="settings-overview-themes" role="radiogroup" aria-label="Color theme">{themes.map(([id,label])=><button key={id} type="button" role="radio" aria-checked={theme===id} aria-label={label} disabled={!preferences} className={`theme-swatch swatch-${id}`} onClick={()=>saver.change("appearance",id,"Appearance saved",false)}/>)}</div></Card>
      <Card icon="lock" title="Security" onOpen={()=>onOpen("Account")}><p>Password and sessions<br/>{passwords===undefined?"Checking app passwords…":plural(passwords,"app password")}</p></Card>
      <Card icon="globe" title="Regional settings" onOpen={()=>onOpen("Regional settings")}><p>{zone.replaceAll("_"," ")}{clock&&<><br/>{clock} now</>}</p></Card>
      <Card icon="plug" title="Connected applications" onOpen={()=>onOpen("Connected applications")}><p>{connections?`${plural(connections,"application")} can use your Dar identity`:"No applications connected"}</p></Card>
    </div>
    {admin&&<><h2 className="settings-overview-heading">Administration</h2>
    <div className="settings-overview-grid">
      <Card icon="grid" title="Apps" onOpen={()=>onOpen("Installed apps")}><p>{apps?`${apps.filter(app=>app.enabled).length} of ${plural(apps.length,"app")} enabled`:"Checking apps…"}</p></Card>
      <Card icon="contacts" title="Directory" onOpen={()=>onOpen("Directory")}><p>{plural(users,"person","people")} · {plural(groups,"group")}</p></Card>
      <Card icon="plug" title="Applications" onOpen={()=>onOpen("Applications")}><p>{clients?plural(clients,"registered application"):"No registered applications"}</p></Card>
      <Card icon="refresh" title="SCIM" onOpen={()=>onOpen("SCIM")}><p>{activeCredentials?plural(activeCredentials,"active credential"):"No active credentials"}</p></Card>
      <Card icon="key" title="Signing keys" onOpen={()=>onOpen("Signing keys")}><p>{status?.keys?.rotatedAt?`Rotated ${new Date(status.keys.rotatedAt).toLocaleDateString()}`:"Active key"}</p></Card>
      <Card icon="palette" title="Sign-in appearance" label="Administration appearance" onOpen={()=>onOpen("Instance appearance")}><p>{logo?logo.logoUrl?"Instance logo":"Dar logo":"Checking logo…"}</p></Card>
    </div></>}
  </section>;
}
