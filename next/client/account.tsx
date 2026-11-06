import { useEffect, useState, type FormEvent } from "react";
import { request, type BrowserSession } from "./api.ts";
interface Session { id: string; createdAt: number; current: boolean }
interface Password { id: string; label: string; scope: string; expiresAt: number }
export function AccountSecurity({ session, onSignedOut }: { session: BrowserSession; onSignedOut: () => void }) {
  const [sessions, setSessions] = useState<Session[]>([]), [passwords, setPasswords] = useState<Password[]>([]);
  const [error, setError] = useState(""), [secret, setSecret] = useState<{ secret: string; davUrl: string } | null>(null);
  const [busy, setBusy] = useState(false);
  async function refresh() {
    const [a, b] = await Promise.all([request<{ items: Session[] }>("/api/v1/auth/sessions"), request<{ items: Password[] }>("/api/v1/auth/app-passwords")]);
    setSessions(a.items); setPasswords(b.items);
  }
  useEffect(() => { void refresh().catch(cause => setError(cause.message)); }, []);
  async function run(work: () => Promise<void>) {
    setError(""); setBusy(true);
    try { await work(); } catch (cause) { setError((cause as Error).message); } finally { setBusy(false); }
  }
  function changePassword(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); const values = Object.fromEntries(new FormData(event.currentTarget));
    void run(async () => { await request("/api/v1/auth/password", { method: "POST", body: JSON.stringify(values) }, session.csrfToken); onSignedOut(); });
  }
  function createPassword(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); const values = Object.fromEntries(new FormData(event.currentTarget));
    void run(async () => { setSecret(await request("/api/v1/auth/app-passwords", { method: "POST", body: JSON.stringify(values) }, session.csrfToken)); await refresh(); });
  }
  return <div className="account-security">
    {error && <p role="alert" className="error">{error}</p>}
    <h3>Password</h3><form onSubmit={changePassword}><label>Current password<input name="currentPassword" type="password" autoComplete="current-password" required /></label><label>New password<input name="password" type="password" autoComplete="new-password" minLength={15} required /></label><p className="muted small">Use at least 15 characters. Changing it revokes browser sessions and app passwords.</p><button className="secondary-button" disabled={busy}>Change password</button></form>
    <h3>Signed-in sessions</h3>{sessions.map(item => <div className="grant" key={item.id}><span>{item.current ? "This browser session" : "Another browser session"}<small>Signed in {new Date(item.createdAt).toLocaleString()}</small></span>{!item.current && <button className="text-button danger" disabled={busy} onClick={() => { void run(async () => { await request(`/api/v1/auth/sessions/${item.id}`, { method: "DELETE" }, session.csrfToken); await refresh(); }); }}>Sign out</button>}</div>)}
    <h3>App passwords</h3><p className="muted small">Connect a WebDAV client using a separate, revocable password. Your account password cannot be used for WebDAV.</p>
    <form onSubmit={createPassword}><label>Device or application name<input name="label" required maxLength={100} placeholder="For example, my laptop" /></label><label>App access<select name="scope"><option value="read">Files · Read only</option><option value="write">Files · Read and write</option><option value="contacts:read">Contacts · Read only</option><option value="contacts:write">Contacts · Read and write</option><option value="calendar:read">Calendar · Read only</option><option value="calendar:write">Calendar · Read and write</option></select></label><button className="secondary-button" disabled={busy}>Create app password</button></form>
    {secret && <section className="new-credential"><label>App password<input readOnly value={secret.secret} onFocus={event => event.target.select()} /></label><label>WebDAV address<input readOnly value={secret.davUrl} onFocus={event => event.target.select()} /></label><p className="muted small">Username: {session.user.username}. Save the password now; it won’t be shown again.</p><button className="text-button" onClick={() => setSecret(null)}>I saved it</button></section>}
    {passwords.map(item => <div className="grant" key={item.id}><span><strong>{item.label}</strong><small>{item.scope.startsWith("contacts:") ? "Contacts" : item.scope.startsWith("calendar:") ? "Calendar" : "Files"} · {item.scope.endsWith("write") ? "Read and write" : "Read only"} · Expires {new Date(item.expiresAt).toLocaleDateString()}</small></span><button className="text-button danger" disabled={busy} onClick={() => { void run(async () => { await request(`/api/v1/auth/app-passwords/${item.id}`, { method: "DELETE" }, session.csrfToken); setSecret(null); await refresh(); }); }}>Revoke</button></div>)}
  </div>;
}
