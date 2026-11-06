import React, { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { Auth } from "./auth.tsx";
import { Files } from "./files.tsx";
import { Contacts } from "./pim/contacts.tsx";
import { Calendar } from "./pim/calendar.tsx";
import { IdentitySettings,IdentityInteraction } from "./identity.tsx";
import { request, RequestError, type BrowserSession } from "./api.ts";
import "./styles.css";
import "./loading.css";
import "./pim/styles.css";
function App() {
  const [status, setStatus] = useState<{ initialized: boolean } | null>(null);
  const [session, setSession] = useState<BrowserSession | null>(null);
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
  if(session&&["/identity","/settings"].includes(location.pathname))return <IdentitySettings session={session} onSignedOut={signedOut}/>;
  return session ? location.pathname === "/contacts" ? <Contacts session={session} onSignedOut={signedOut} /> : location.pathname === "/calendar" ? <Calendar session={session} onSignedOut={signedOut} /> : <Files session={session} onSignedOut={signedOut} /> : <Auth initialized={status.initialized} onLogin={setSession} />;
}
createRoot(document.getElementById("root")!).render(<App />);
