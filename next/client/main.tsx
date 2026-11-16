import {useAppearance} from "./appearance.tsx";
import React, { Fragment, Suspense, use, useEffect, useLayoutEffect, useRef, useState, type ComponentType } from "react";
import { interceptLinks, setNavigator } from "./navigation.ts";
import { createRoot } from "react-dom/client";
import { request, RequestError, type BrowserSession } from "./api.ts";
// Every stylesheet is bundled here, in cascade order, so one cached app CSS file
// styles every app while each app's code loads only on its own page.
import "./file-picker.css";
import "./global-search.css";
import "./locked-folder.css";
import "./date-popover.css";
import "./identity.css";
import "./pim/workspace.css";
import "./installed-apps.css";
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
import "./diff.css";
import "./history.css";
import "./agent/workspace.css";
import "./news/news.css";
import "./photos/photos.css";
import "./books/books.css";
// The shared frame is last so it supersedes the earlier per-surface frame rules.
import "./frame.css";
import "./settings-sheet.css";
// The apps revamp layer sits on top of the frame and settings sheet.
import "./apps.css";
import {useAppSettings} from "./app-availability.tsx";
import {WorkspaceHeader} from "./shell.tsx";

/**
 * A page whose code loads on first use; `preload` starts the download early. Once
 * loaded it renders without suspending (React.lazy would suspend once and hold the
 * page back behind the fallback).
 */
function page<P extends object>(load:()=>Promise<ComponentType<P>>){
  let loaded:ComponentType<P>|undefined,pending:Promise<unknown>|undefined;
  const preload=()=>pending??=load().then(component=>{loaded=component;});
  function Page(props:P){if(!loaded)use(preload());const Loaded=loaded!;return <Loaded {...props}/>;}
  return Object.assign(Page,{preload});
}
const Auth=page(()=>import("./auth.tsx").then(m=>m.Auth));
const Files=page(()=>import("./files.tsx").then(m=>m.Files));
const Contacts=page(()=>import("./pim/contacts.tsx").then(m=>m.Contacts));
const Calendar=page(()=>import("./pim/calendar.tsx").then(m=>m.Calendar));
const IdentitySettings=page(()=>import("./identity.tsx").then(m=>m.IdentitySettings));
const IdentityInteraction=page(()=>import("./identity.tsx").then(m=>m.IdentityInteraction));
const Workspace=page(()=>import("./pim/workspace.tsx").then(m=>m.Workspace));
const Notes=page(()=>import("./notes.tsx").then(m=>m.Notes));
const InstalledAppPage=page(()=>import("./installed-apps.tsx").then(m=>m.InstalledAppPage));
const AgentWorkspace=page(()=>import("./agent/workspace.tsx").then(m=>m.AgentWorkspace));
const NewsWorkspace=page(()=>import("./news/workspace.tsx").then(m=>m.NewsWorkspace));
const PhotosWorkspace=page(()=>import("./photos/workspace.tsx").then(m=>m.PhotosWorkspace));
const BooksWorkspace=page(()=>import("./books/workspace.tsx").then(m=>m.BooksWorkspace));
const PublicPhotos=page(()=>import("./photos/public.tsx").then(m=>m.PublicPhotos));

function pageFor(path:string){
  return path.startsWith("/s/")?PublicPhotos:path.startsWith("/identity/interaction/")?IdentityInteraction:path.startsWith("/apps/")?InstalledAppPage
    :["/identity","/settings"].includes(path)?IdentitySettings:path==="/news"?NewsWorkspace:path==="/photos"?PhotosWorkspace:path==="/books"?BooksWorkspace:path==="/agent"?AgentWorkspace
    :["/notes","/tasks"].includes(path)?Workspace:path==="/contacts"?Contacts:path==="/calendar"?Calendar:Files;
}
/** Built-in and installed apps open in place; shared links and sign-in pages load normally. */
const inPlace=new Set(["/","/contacts","/calendar","/notes","/tasks","/news","/photos","/books","/agent","/settings","/identity"]);
const opensInPlace=(path:string)=>inPlace.has(path)||/^\/apps\/[^/]+$/.test(path);
const path=location.pathname;
// Start downloading this page's code while the session is checked.
pageFor(path).preload();

const opening=<main className="startup-state" aria-busy="true"><div className="brand-mark" /><p role="status">Opening your space…</p></main>;
function OptionalWorkspace({kind,session,onSignedOut}:{kind:"notes"|"tasks"|"agent"|"news"|"photos"|"books";session:BrowserSession;onSignedOut:()=>void}){
  const {settings,error}=useAppSettings();
  if(settings&&!settings.disabled.includes(kind))return kind==="agent"?<AgentWorkspace session={session} onSignedOut={onSignedOut}/>:kind==="news"?<NewsWorkspace session={session} onSignedOut={onSignedOut}/>:kind==="photos"?<PhotosWorkspace session={session} onSignedOut={onSignedOut}/>:kind==="books"?<BooksWorkspace session={session} onSignedOut={onSignedOut}/>:kind==="notes"?<Notes session={session} onSignedOut={onSignedOut}/>:<Workspace kind={kind} session={session} onSignedOut={onSignedOut}/>;
  return <div className="app-shell"><WorkspaceHeader app={kind} session={session} onSignedOut={onSignedOut} query="" onQuery={()=>{}}/><main className="startup-state">{error?<><h1>Unable to load apps</h1><p role="alert">{error}</p><button className="primary-button" onClick={()=>location.reload()}>Try again</button></>:settings?<><h1>App disabled</h1><p>This app has been disabled by an administrator.</p></>:<p role="status">Opening app…</p>}<a href="/">Return to Files</a></main></div>;
}
function App() {
  const [status, setStatus] = useState<{ initialized: boolean } | null>(null);
  const [session, setSession] = useState<BrowserSession | null>(null);
  useAppearance(session);
  const [ready, setReady] = useState(false), [error, setError] = useState("");
  // Each visit to an app mounts it afresh, as a page load would.
  const [route, setRoute] = useState({ path: location.pathname, visit: 0 }), fromRail = useRef(false);
  useEffect(() => {
    const pop = () => setRoute(current => current.path === location.pathname ? current : { path: location.pathname, visit: current.visit + 1 });
    window.addEventListener("popstate", pop); return () => window.removeEventListener("popstate", pop);
  }, []);
  useEffect(() => {
    if (!session) return;
    let latest = 0;
    const stop = interceptLinks(), release = setNavigator({ handles: url => opensInPlace(url.pathname), open: url => {
      const request = ++latest;
      // Show the next app once its code is loaded, so the page never flashes empty.
      void pageFor(url.pathname).preload().then(() => {
        if (request !== latest) return;
        fromRail.current = !!document.activeElement?.closest(".app-rail");
        history.pushState(null, "", url.pathname + url.search + url.hash);
        setRoute(current => ({ path: url.pathname, visit: current.visit + 1 })); window.scrollTo(0, 0);
      }, () => location.assign(url.href));
    } });
    return () => { stop(); release(); };
  }, [session]);
  useLayoutEffect(() => { if (fromRail.current) { fromRail.current = false; document.querySelector<HTMLElement>(".app-rail [aria-current=page]")?.focus(); } }, [route.visit]);
  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        // Both checks start together; the session only counts once the instance is initialized.
        const signedIn = request<BrowserSession>("/api/v1/auth/session").catch(cause => { if (cause instanceof RequestError && cause.status === 401) return null; throw cause; });
        signedIn.catch(() => {});
        const value = await request<{ initialized: boolean }>("/api/v1/auth/status");
        if (!active) return;
        setStatus(value);
        if (value.initialized) { const loggedIn = await signedIn; if (active && loggedIn) setSession(loggedIn); }
      } catch { if (active) setError("Your instance could not be reached. Check the connection and try again."); }
      finally { if (active) setReady(true); }
    })();
    return () => { active = false; };
  }, []);
  if (error) return <main className="startup-state"><h1>Unable to connect</h1><p role="alert">{error}</p><button className="primary-button" onClick={() => location.reload()}>Try again</button></main>;
  if (!ready || !status) return opening;
  const signedOut = () => { setSession(null); setStatus({ initialized: true }); };
  return <Fragment key={route.visit}>{screen(route.path, status, session, setSession, signedOut)}</Fragment>;
}
function screen(path: string, status: { initialized: boolean }, session: BrowserSession | null, setSession: (session: BrowserSession) => void, signedOut: () => void) {
  if(path.startsWith("/identity/interaction/"))return <IdentityInteraction session={session} onLogin={setSession}/>;
  if(!session)return <Auth initialized={status.initialized} onLogin={setSession} />;
  if(path.startsWith("/apps/"))return <InstalledAppPage id={path.split("/")[2]} session={session} onSignedOut={signedOut}/>;
  if(["/identity","/settings"].includes(path))return <IdentitySettings session={session} onSignedOut={signedOut}/>;
  if(path==="/news")return <OptionalWorkspace kind="news" session={session} onSignedOut={signedOut}/>;
  if(path==="/photos")return <OptionalWorkspace kind="photos" session={session} onSignedOut={signedOut}/>;
  if(path==="/books")return <OptionalWorkspace kind="books" session={session} onSignedOut={signedOut}/>;
  if(path==="/agent")return <OptionalWorkspace kind="agent" session={session} onSignedOut={signedOut}/>;
  if(["/notes","/tasks"].includes(path))return <OptionalWorkspace kind={path==="/notes"?"notes":"tasks"} session={session} onSignedOut={signedOut}/>;
  return path === "/contacts" ? <Contacts session={session} onSignedOut={signedOut} /> : path === "/calendar" ? <Calendar session={session} onSignedOut={signedOut} /> : <Files session={session} onSignedOut={signedOut} />;
}
// Photos shared links (/s/<token>) are for visitors without an account.
createRoot(document.getElementById("root")!).render(<Suspense fallback={opening}>{path.startsWith("/s/")?<PublicPhotos/>:<App />}</Suspense>);
