import { useState, type FormEvent } from "react";
import { request, type BrowserSession } from "./api.ts";
import { Icon } from "./icons.tsx";

export function Brand() { return <div className="brand"><span className="brand-mark" aria-hidden="true"></span><span>dar</span></div>; }
export function Auth({ initialized, onLogin }: { initialized: boolean; onLogin: (session: BrowserSession) => void }) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [recover, setRecover] = useState(false);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setBusy(true); setError("");
    const p = Object.fromEntries(new FormData(event.currentTarget));
    try { onLogin(await request<BrowserSession>(`/api/v1/auth/${initialized ? "login" : "setup"}`, { method: "POST", body: JSON.stringify(p) })); }
    catch (cause) { setError((cause as Error).message); }
    finally { setBusy(false); }
  }
  return <main className="auth-page">
    <header className="auth-header"><Brand /><span className="instance-label">Your personal cloud</span></header>
    <div className="auth-layout"><section className="auth-intro"><div className="intro-symbol"><Icon name="folder" /></div><span className="eyebrow">A SPACE OF YOUR OWN</span><h1>Everything important.<br />Right at home.</h1><p>A place for your files, on your terms.<br />Private, organized, and always yours.</p><div className="auth-proof"><Icon name="lock" /><span>Hosted by you. Made for your everyday.</span></div></section>
    <section className="auth-card" aria-labelledby="auth-title"><span className="auth-card-mark"><Icon name="lock" /></span><h2 id="auth-title">{recover ? "Recover your account" : initialized ? "Welcome back" : "Make yourself at home"}</h2>
      <p className="muted">{recover ? "Your instance administrator can reset your password using the local recovery command." : initialized ? "Sign in to your personal space." : "Create the administrator account for this instance."}</p>
      {recover ? <><p>Ask your administrator to follow the account recovery instructions in the operator guide. Recovery doesn’t require access to an email provider.</p><button className="primary-button full" onClick={() => setRecover(false)}>Back to sign in</button></> : <form onSubmit={submit}>
        {!initialized && <label>Display name<input name="displayName" autoComplete="name" maxLength={100} required autoFocus /></label>}
        <label>Username<input name="username" autoComplete="username" pattern="[a-zA-Z0-9][a-zA-Z0-9_.\-]{2,63}" maxLength={64} required autoFocus={initialized} /></label>
        <label>Password<input name="password" type="password" autoComplete={initialized ? "current-password" : "new-password"} minLength={initialized ? 1 : 15} maxLength={1024} required /></label>
        {!initialized && <><small className="field-hint">Use a passphrase of at least 15 characters.</small><label>Instance setup key<input name="bootstrapToken" type="password" autoComplete="off" required /><small className="field-hint">Available to the operator in the instance’s bootstrap-token file.</small></label></>}
        {error && <p role="alert" className="error">{error}</p>}
        <button className="primary-button full" type="submit" disabled={busy}>{busy ? "Signing in…" : initialized ? "Sign in" : "Create your account"}<Icon name="chevron" /></button>
        {initialized && <button className="text-button full" type="button" onClick={() => setRecover(true)}>Forgot your password?</button>}
      </form>}
      <div className="auth-card-footer"><Icon name="lock" /> Secure sign-in on your own instance</div>
    </section></div><footer className="auth-footer">A calmer home for your digital life.</footer>
  </main>;
}
