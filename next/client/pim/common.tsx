import {EditorPane} from "../pane.tsx";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { DbConnection } from "../module_bindings/index.ts";
import { request, type BrowserSession } from "../api.ts";
import {WorkspaceHeader} from "../shell.tsx";
import {ConnectionStatus, type ConnectionState} from "../connection-status.tsx";
import {Select} from "../controls.tsx";
import {AppSettingsButton,AppSettings} from "../app-settings.tsx";
import { Dialog } from "../dialog.tsx";
import type { PimCollection, PimResource } from "../../shared/pim.ts";
export const etag = (row: { id: string; revision: number }) => `"${row.id}:${row.revision}"`;
export interface Item<T> extends PimResource { data: T }
export type {Preferences} from "../../shared/preferences.ts";
export function usePim(session: BrowserSession, onSignedOut: () => void) {
  const [epoch, setEpoch] = useState(0), [connectionState, setConnectionState] = useState<ConnectionState>("connecting");
  useEffect(() => {
    let disposed = false, db: DbConnection | undefined;
    const tab = crypto.randomUUID();
    async function token() { return await request<{ token: string; databaseUrl: string; databaseName: string }>(`/api/v1/auth/token?tab=${tab}`, { method: "POST" }, session.csrfToken); }
    void token().then(config => {
      if (disposed) return;
      db = DbConnection.builder().withUri(config.databaseUrl.replace(/^http/, "ws")).withDatabaseName(config.databaseName).withToken(config.token).withAutomaticReconnect().withTokenProvider(async () => (await token()).token)
        .onConnect(c => {
          const changed = () => { if (!disposed) setEpoch(n => n + 1); };
          c.db.myPimChanges.onInsert(changed); c.db.myPimChanges.onUpdate(changed);
          c.db.me.onDelete(() => { if (!disposed) onSignedOut(); });
          c.subscriptionBuilder().onApplied(() => { if (!disposed) { setConnectionState("ready"); changed(); } }).onError(() => { if (!disposed) setConnectionState("error"); }).subscribe(["SELECT * FROM me", "SELECT * FROM my_pim_changes"]);
        }).onAutomaticReconnect(() => { if (!disposed) setConnectionState("reconnecting"); })
        .onDisconnect((_ctx, _error, next) => { if (!disposed) setConnectionState(next ? "reconnecting" : "error"); }).onConnectError(() => { if (!disposed) setConnectionState("error"); }).build();
    }).catch(() => { if (!disposed) setConnectionState("error"); });
    return () => { disposed = true; db?.disconnect(); };
  }, [session]);
  return { epoch, connectionState, refresh: () => setEpoch(n => n + 1) };
}
export function PimShell({ kind, session, onSignedOut, query, onQuery, sidebar, children, details, connectionState, mainClassName="" }: { kind: "contacts" | "calendar"|"notes"|"tasks"; session: BrowserSession; onSignedOut: () => void; query: string; onQuery: (query: string) => void; sidebar: ReactNode; children: ReactNode; details?: ReactNode; connectionState: ConnectionState;mainClassName?:string }) {
  const [navigation, setNavigation] = useState(false), [settings, setSettings] = useState(false);
  // Suppress initial empty states, but keep editors mounted during reconnects.
  const main = useRef<HTMLElement>(null), hadDetails = useRef(false), hasConnected = useRef(false);
  if (connectionState === "ready") hasConnected.current = true;
  useEffect(() => { if (hadDetails.current && !details && document.activeElement === document.body) main.current?.focus(); hadDetails.current = !!details; }, [!!details]);
  return <div className={`app-shell pim-shell ${details ? "has-pim-details" : ""}`}>
    <WorkspaceHeader app={kind} session={session} onSignedOut={onSignedOut} query={query} onQuery={onQuery} onNavigation={()=>setNavigation(!navigation)}/>
    <div className="pim-workspace">{navigation && <button className="nav-scrim" aria-label="Close navigation" onClick={() => setNavigation(false)} />}<aside className={`sidebar ${navigation ? "is-open" : ""}`} onClick={e => { if ((e.target as HTMLElement).closest("button") && !(e.target as HTMLElement).closest(".ui-select") && (!(e.target as HTMLElement).closest(".ui-menu")||(e.target as HTMLElement).closest('[role="menuitem"]'))) setNavigation(false); }}>{sidebar}<AppSettingsButton app={kind} onClick={()=>setSettings(true)}/></aside><main ref={main} tabIndex={-1} className={`pim-main ${mainClassName}`}><ConnectionStatus state={connectionState}/>{hasConnected.current && children}</main>{details && <aside className="pim-details" aria-label={`${({contacts:"Contact",calendar:"Event",notes:"Note",tasks:"Task"})[kind]} details`}>{details}</aside>}</div>
    {settings&&<AppSettings app={kind} session={session} onClose={()=>setSettings(false)}/>}
  </div>;
}
export function CollectionEditor({ collection, title, onSave, onClose }: { collection?: PimCollection; title: string; onSave: (name: string, color: string) => Promise<void>; onClose: () => void }) {
  const [error, setError] = useState(""), [busy, setBusy] = useState(false);
  return <EditorPane title={title} onClose={onClose}><form onSubmit={e => { e.preventDefault(); const data = new FormData(e.currentTarget); setBusy(true); setError(""); void onSave(String(data.get("name")), String(data.get("color"))).then(onClose).catch(cause => setError(cause.message)).finally(() => setBusy(false)); }}><label>Name<input name="name" autoFocus required maxLength={255} defaultValue={collection?.name} /></label><label>Color<input name="color" type="color" defaultValue={collection?.color ?? "#2563eb"} /></label>{error && <p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy}>Save</button></div></form></EditorPane>;
}
export function Sharing({ collection, base, session, onClose, onChanged }: { collection: PimCollection; base: string; session: BrowserSession; onClose: () => void; onChanged: () => void }) {
  const [grants, setGrants] = useState<{ id: string; username: string; role: string }[]>([]), [current, setCurrent] = useState(collection), [error, setError] = useState("");
  async function refresh() { const [row, list] = await Promise.all([request<PimCollection>(base), request<{ items: typeof grants }>(base + "/grants")]); setCurrent(row); setGrants(list.items); onChanged(); }
  useEffect(() => { void refresh().catch(cause => setError(cause.message)); }, []);
  return <Dialog title={`Share ${collection.name}`} onClose={onClose}>{error && <p role="alert" className="error">{error}</p>}<form onSubmit={e => { e.preventDefault(); const values = Object.fromEntries(new FormData(e.currentTarget)); void request(base + "/grants", { method: "POST", headers: { "if-match": etag(current) }, body: JSON.stringify(values) }, session.csrfToken).then(refresh).catch(cause => setError(cause.message)); }}><label>Username<input name="username" required /></label><label>Permission<Select name="role"><option value="read">Can view</option><option value="write">Can edit</option></Select></label><button className="primary-button">Share</button></form>{grants.map(grant => <div className="grant" key={grant.id}><span>@{grant.username}<small>{grant.role === "write" ? "Can edit" : "Can view"}</small></span><button className="text-button danger" onClick={() => { void request(base + "/grants", { method: "DELETE", headers: { "if-match": etag(current) }, body: JSON.stringify({ id: grant.id }) }, session.csrfToken).then(refresh).catch(cause => setError(cause.message)); }}>Revoke</button></div>)}</Dialog>;
}
