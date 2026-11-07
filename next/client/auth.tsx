import { useEffect, useState, type FormEvent } from "react";
import { request, type BrowserSession } from "./api.ts";

export function Brand() { return <div className="brand"><span className="brand-mark" aria-hidden="true"></span><span>dar</span></div>; }
export function Auth({ initialized, onLogin }: { initialized: boolean; onLogin: (session: BrowserSession) => void }) {
  const [logo,setLogo]=useState<string|null>(null);
  useEffect(()=>{const controller=new AbortController();void request<{logoUrl:string|null}>("/api/v1/appearance",{signal:controller.signal}).then(value=>setLogo(value.logoUrl)).catch(()=>{});return()=>controller.abort();},[]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setBusy(true); setError("");
    const p = Object.fromEntries(new FormData(event.currentTarget));
    try { onLogin(await request<BrowserSession>(`/api/v1/auth/${initialized ? "login" : "setup"}`, { method: "POST", body: JSON.stringify(p) })); }
    catch (cause) { setError((cause as Error).message); }
    finally { setBusy(false); }
  }
  return <main className="auth-page">
    <section className="auth-card" aria-label={initialized ? "Sign in" : "Set up Dar"}>
      <div className="signin-brand">{logo?<img src={logo} alt="Dar" onError={()=>setLogo(null)}/>:<Brand/>}</div>
      {initialized && <h1>Sign in</h1>}
      {!initialized && <h1>Set up Dar</h1>}
      <form onSubmit={submit}>
        {!initialized && <label>Display name<input name="displayName" autoComplete="name" maxLength={100} required autoFocus /></label>}
        <label>Username<input name="username" autoComplete="username" pattern="[a-zA-Z0-9][a-zA-Z0-9_.\-]{2,63}" maxLength={64} required autoFocus={initialized} /></label>
        <label>Password<input name="password" type="password" autoComplete={initialized ? "current-password" : "new-password"} minLength={initialized ? 1 : 15} maxLength={1024} required /></label>
        {!initialized && <><small className="field-hint">Use a passphrase of at least 15 characters.</small><label>Instance setup key<input name="bootstrapToken" type="password" autoComplete="off" required /><small className="field-hint">Available to the operator in the instance’s bootstrap-token file.</small></label></>}
        {error && <p role="alert" className="error">{error}</p>}
        <button className="primary-button full" type="submit" disabled={busy}>{busy ? "Signing in…" : initialized ? "Sign in" : "Create your account"}</button>
      </form>
    </section>
  </main>;
}
