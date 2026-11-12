import {useAppearance} from "./appearance.tsx";
import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Auth } from "./auth.tsx";
import { Files } from "./files.tsx";
import { Contacts } from "./pim/contacts.tsx";
import { Calendar } from "./pim/calendar.tsx";
import { IdentitySettings,IdentityInteraction } from "./identity.tsx";
import {Workspace} from "./pim/workspace.tsx";
import {InstalledAppPage} from "./installed-apps.tsx";
import "./installed-apps.css";
import { request, RequestError, type BrowserSession } from "./api.ts";
import "./styles.css";
import "./loading.css";
import "./shared-ui.css";
import "./pim/styles.css";
import "./ui-layout.css";
import "./theme.css";
import "./calendar-layout.css";
import "./contacts-layout.css";
import "./app-launcher.css";
import "./control-theme.css";
import {useAppSettings} from "./app-availability.tsx";
import {WorkspaceHeader} from "./shell.tsx";
import {AgentWorkspace} from "./agent/workspace.tsx";
// The shared frame is last so it supersedes the earlier per-surface frame rules.
import "./frame.css";
import "./settings-sheet.css";
function OptionalWorkspace({kind,session,onSignedOut}:{kind:"notes"|"tasks"|"agent";session:BrowserSession;onSignedOut:()=>void}){
  const {settings,error}=useAppSettings();
  if(settings&&!settings.disabled.includes(kind))return kind==="agent"?<AgentWorkspace session={session} onSignedOut={onSignedOut}/>:<Workspace kind={kind} session={session} onSignedOut={onSignedOut}/>;
  return <div className="app-shell"><WorkspaceHeader app={kind} session={session} onSignedOut={onSignedOut} query="" onQuery={()=>{}}/><main className="startup-state">{error?<><h1>Unable to load apps</h1><p role="alert">{error}</p><button className="primary-button" onClick={()=>location.reload()}>Try again</button></>:settings?<><h1>App disabled</h1><p>This app has been disabled by an administrator.</p></>:<p role="status">Opening app…</p>}<a href="/">Return to Files</a></main></div>;
}
function App() {
  const [status, setStatus] = useState<{ initialized: boolean } | null>(null);
  const [session, setSession] = useState<BrowserSession | null>(null);
  useAppearance(session);
  const [ready, setReady] = useState(false), [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const value = await request<{ initialized: boolean }>("/api/v1/auth/status");
        if (!active) return;
        setStatus(value);
        if (value.initialized) { try { const loggedIn = await request<BrowserSession>("/api/v1/auth/session"); if (active) setSession(loggedIn); } catch (cause) { if (!(cause instanceof RequestError) || cause.status !== 401) throw cause; } }
      } catch { if (active) setError("Your instance could not be reached. Check the connection and try again."); }
      finally { if (active) setReady(true); }
    })();
    return () => { active = false; };
  }, []);
  if (error) return <main className="startup-state"><h1>Unable to connect</h1><p role="alert">{error}</p><button className="primary-button" onClick={() => location.reload()}>Try again</button></main>;
  if (!ready || !status) return <main className="startup-state" aria-busy="true"><div className="brand-mark" /><p role="status">Opening your space…</p></main>;
  const signedOut = () => { setSession(null); setStatus({ initialized: true }); };
  if(location.pathname.startsWith("/identity/interaction/"))return <IdentityInteraction session={session} onLogin={setSession}/>;
  if(session&&location.pathname.startsWith("/apps/"))return <InstalledAppPage id={location.pathname.split("/")[2]} session={session} onSignedOut={signedOut}/>;
  if(session&&["/identity","/settings"].includes(location.pathname))return <IdentitySettings session={session} onSignedOut={signedOut}/>;
  if(session&&location.pathname==="/agent")return <OptionalWorkspace kind="agent" session={session} onSignedOut={signedOut}/>;
  if(session&&["/notes","/tasks"].includes(location.pathname))return <OptionalWorkspace kind={location.pathname==="/notes"?"notes":"tasks"} session={session} onSignedOut={signedOut}/>;
  return session ? location.pathname === "/contacts" ? <Contacts session={session} onSignedOut={signedOut} /> : location.pathname === "/calendar" ? <Calendar session={session} onSignedOut={signedOut} /> : <Files session={session} onSignedOut={signedOut} /> : <Auth initialized={status.initialized} onLogin={setSession} />;
}
createRoot(document.getElementById("root")!).render(<App />);
