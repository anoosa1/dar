import { useEffect, useRef, useState, type FormEvent } from "react";
import { DbConnection } from "./module_bindings/index.ts";
import type { FileEntries as FileEntry, Usage as MyUsage } from "./module_bindings/types.ts";
import { Brand } from "./auth.tsx";
import { AccountSecurity } from "./account.tsx";
import { Icon, type IconName } from "./icons.tsx";
import { Dialog } from "./dialog.tsx";
import { formatBytes, request, readProcedure, type BrowserSession } from "./api.ts";

type Section = "files" | "favorites" | "shared" | "trash";
interface Details {
  entry: FileEntry; role: "owner" | "read" | "write";
  ancestors: { id: string; name: string }[];
  versions: { id: string; createdAt: number; size: number; blobId: string }[];
  grants: { id: string; username: string; role: string }[];
  publicShares: { id: string; expiresAt: number }[];
}
type Modal = { type: "folder" | "rename" | "move" | "copy" | "delete"; entry?: FileEntry } | { type: "account" | "user" };
interface Transfer { id: string; name: string; progress: number; status: "uploading" | "complete" | "error" | "cancelled"; message?: string }
const sections: { id: Section; label: string; icon: IconName }[] = [
  { id: "files", label: "All files", icon: "folder" }, { id: "favorites", label: "Favorites", icon: "star" },
  { id: "shared", label: "Shared with you", icon: "share" }, { id: "trash", label: "Deleted files", icon: "trash" },
];
function readLocation(): { section: Section; parentId: string } {
  const parameters = new URLSearchParams(location.search);
  const section = parameters.get("section") as Section;
  const id = parameters.get("folder") ?? "";
  return { section: sections.some(item => item.id === section) ? section : "files", parentId: /^[a-f0-9-]{36}$/.test(id) ? id : "" };
}
function date(value: number) { return new Date(value).toLocaleDateString(undefined, { month: "short", day: "numeric" }); }
function failure(error: unknown) {
  const text = error instanceof Error ? error.message : "The operation could not be completed.";
  if (/conflict/i.test(text)) return "This item changed or that name is already in use. Refresh the details and try again.";
  if (/forbidden|unauthenticated/i.test(text)) return "Your access changed. Refresh the page or sign in again.";
  if (/invalidName/i.test(text)) return "Choose a name without slashes or control characters.";
  if (/quotaExceeded/i.test(text)) return "There isn’t enough storage available for this upload.";
  if (/notFound/i.test(text)) return "This item is no longer available, or you no longer have access.";
  return text;
}

export function Files({ session, onSignedOut }: { session: BrowserSession; onSignedOut: () => void }) {
  const [connection, setConnection] = useState<DbConnection | null>(null);
  const [connectionEpoch, setConnectionEpoch] = useState(0);
  const [rows, setRows] = useState<FileEntry[]>([]), [usage, setUsage] = useState<MyUsage | null>(null);
  const [state, setState] = useState("Connecting…"), [error, setError] = useState(""), [notice, setNotice] = useState("");
  const initial = useRef(readLocation()).current;
  const [section, setSection] = useState<Section>(initial.section), [parentId, setParentId] = useState(initial.parentId);
  const [loading, setLoading] = useState(true);
  const [path, setPath] = useState<{ id: string; name: string }[]>([]);
  const [query, setQuery] = useState(""), [cursor, setCursor] = useState("");
  const [previous, setPrevious] = useState<string[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set()), [details, setDetails] = useState<Details | null>(null);
  const [tab, setTab] = useState<"details" | "versions" | "sharing">("details");
  const [grid, setGrid] = useState(false), [sidebar, setSidebar] = useState(false), [menu, setMenu] = useState(false);
  const [modal, setModal] = useState<Modal | null>(null), [busy, setBusy] = useState(false);
  const [targetPath, setTargetPath] = useState<{ id: string; name: string }[]>([]);
  const [folders, setFolders] = useState<{ id: string; name: string }[]>([]);
  const [pickerQuery, setPickerQuery] = useState(""), [pickerCursor, setPickerCursor] = useState("");
  const [pickerNext, setPickerNext] = useState(""), [pickerPrevious, setPickerPrevious] = useState<string[]>([]);
  const [transfers, setTransfers] = useState<Transfer[]>([]), [publicLink, setPublicLink] = useState("");
  const [preview, setPreview] = useState<FileEntry | null>(null), [textPreview, setTextPreview] = useState("");
  const uploadInput = useRef<HTMLInputElement>(null), replacementInput = useRef<HTMLInputElement>(null);
  const uploadRequests = useRef(new Map<string, XMLHttpRequest>());
  const uploadReservations = useRef(new Map<string, AbortController>());
  const detailSequence = useRef(0), active = useRef(true);
  const navigationReady = useRef(false);
  const currentDetails = useRef(details); currentDetails.current = details;
  const visible = rows.slice(0, 200), hasMore = rows.length > 200;

  useEffect(() => {
    let disposed = false; let db: DbConnection | undefined;
    const tabId = crypto.randomUUID();
    const getToken = () => request<{ token: string; databaseUrl: string; databaseName: string }>(`/api/v1/auth/token?tab=${tabId}`, { method: "POST" }, session.csrfToken);
    void getToken().then(config => {
      if (disposed) return;
      db = DbConnection.builder().withUri(config.databaseUrl.replace(/^http/, "ws")).withDatabaseName(config.databaseName).withToken(config.token)
        .withAutomaticReconnect().withTokenProvider(async () => (await getToken()).token)
        .onConnect(c => {
          const synchronize = () => {
            if (disposed) return;
            if (navigationReady.current) setRows([...c.db.myFiles.iter()]);
            setUsage([...c.db.myUsage.iter()][0] ?? null);
          };
          c.db.myFiles.onInsert(synchronize); c.db.myFiles.onDelete(synchronize); c.db.myFiles.onUpdate(synchronize);
          c.db.myUsage.onInsert(synchronize); c.db.myUsage.onDelete(synchronize); c.db.myUsage.onUpdate(synchronize);
          c.db.me.onDelete(() => { if (!disposed) { setRows([]); setDetails(null); onSignedOut(); } });
          c.subscriptionBuilder().onApplied(() => { if (!disposed) { synchronize(); setState(""); setConnection(c); setConnectionEpoch(value => value + 1); } })
            .onError(() => { if (!disposed) setError("Your files could not be loaded. Try refreshing the page."); })
            .subscribe(["SELECT * FROM me", "SELECT * FROM my_files", "SELECT * FROM my_usage"]);
        })
        .onAutomaticReconnect(() => { if (!disposed) setState("Refreshing your files…"); })
        .onDisconnect((_ctx, _error, next) => { if (!disposed) setState(next ? "Connection lost. Reconnecting…" : "Disconnected. Refresh to reconnect."); })
        .onConnectError(() => { if (!disposed) setError("The live connection could not be established. Refresh to try again."); }).build();
    }).catch(cause => { if (!disposed) setError(failure(cause)); });
    return () => { disposed = true; db?.disconnect(); };
  }, [session]);

  useEffect(() => {
    if (!connection) return;
    let cancelled = false;
    navigationReady.current = false; setLoading(true);
    setSelected(new Set()); setDetails(null); detailSequence.current++;
    const timer = setTimeout(() => {
      void connection.reducers.browse({ parentId, section, query, cursor }).then(() => {
        if (!cancelled) { navigationReady.current = true; setRows([...connection.db.myFiles.iter()]); setLoading(false); }
      }).catch(cause => { if (!cancelled) { setError(failure(cause)); setRows([]); setLoading(false); } });
    }, query ? 180 : 0);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [connection, connectionEpoch, parentId, section, query, cursor]);
  useEffect(() => {
    const pop = () => {
      const value = readLocation(); setSection(value.section); setParentId(value.parentId); setPath([]);
      setQuery(""); setCursor(""); setPrevious([]); setSelected(new Set()); setDetails(null); detailSequence.current++;
    };
    window.addEventListener("popstate", pop); return () => window.removeEventListener("popstate", pop);
  }, []);
  useEffect(() => {
    if (!connection || !parentId || path.at(-1)?.id === parentId) return;
    let cancelled = false;
    void connection.procedures.fileDetails({ id: parentId }).then(value => {
      const detail = readProcedure<Details>(value);
      if (!cancelled) setPath([...detail.ancestors, { id: detail.entry.id, name: detail.entry.name }]);
    }).catch(cause => { if (!cancelled) setError(failure(cause)); });
    return () => { cancelled = true; };
  }, [connection, parentId]);
  useEffect(() => { active.current = true; return () => { active.current = false; for (const req of uploadRequests.current.values()) req.abort(); for (const controller of uploadReservations.current.values()) controller.abort(); }; }, []);
  useEffect(() => {
    const current = currentDetails.current;
    if (!current || !connection) return;
    const row = rows.find(item => item.id === current.entry.id);
    if (row && row.revision !== current.entry.revision) void showDetails(row);
    else if (!row) { setDetails(null); detailSequence.current++; }
  }, [rows]);
  useEffect(() => {
    if (modal?.type !== "move" && modal?.type !== "copy" || !connection) return;
    let current = true;
    void connection.procedures.folderChoices({ parentId: targetPath.at(-1)?.id ?? "", query: pickerQuery, cursor: pickerCursor }).then(value => { if (current) { const result = readProcedure<{ items: { id: string; name: string }[]; nextCursor: string }>(value); setFolders(result.items); setPickerNext(result.nextCursor); } }).catch(cause => { if (current) setError(failure(cause)); });
    return () => { current = false; };
  }, [modal, targetPath, connection, pickerQuery, pickerCursor]);
  useEffect(() => {
    setTextPreview("");
    if (!preview || !/^(text\/|application\/json)/.test(preview.mediaType) || preview.size > 1024 * 1024) return;
    const abort = new AbortController();
    void fetch(`/api/v1/files/${preview.id}/content`, { signal: abort.signal }).then(r => { if (!r.ok) throw new Error(); return r.text(); }).then(setTextPreview).catch(() => { if (!abort.signal.aborted) setTextPreview("This preview is unavailable. You can still download the file."); });
    return () => abort.abort();
  }, [preview]);

  async function perform(work: () => Promise<unknown>, message = "") {
    setError(""); setBusy(true);
    try { await work(); if (message) setNotice(message); return true; }
    catch (cause) { setError(failure(cause)); return false; }
    finally { setBusy(false); }
  }
  async function showDetails(entry: FileEntry) {
    if (!connection) return;
    const sequence = ++detailSequence.current;
    try {
      const value = readProcedure<Details>(await connection.procedures.fileDetails({ id: entry.id }));
      if (sequence === detailSequence.current && active.current) setDetails(value);
    } catch (cause) { if (sequence === detailSequence.current) { setDetails(null); setError(failure(cause)); } }
  }
  function navigate(next: Section, id = "", crumbs: { id: string; name: string }[] = []) {
    const parameters = new URLSearchParams(); if (next !== "files") parameters.set("section", next); if (id) parameters.set("folder", id);
    history.pushState(null, "", `${location.pathname}${parameters.size ? `?${parameters}` : ""}`);
    setLoading(true);
    setSection(next); setParentId(id); setPath(crumbs); setSelected(new Set()); setDetails(null); detailSequence.current++;
    setQuery(""); setCursor(""); setPrevious([]); setSidebar(false); setError("");
  }
  async function openEntry(entry: FileEntry) {
    if (entry.trashed) { select(entry); return; }
    if (entry.kind !== "folder") { setPreview(entry); return; }
    if (!connection || entry.trashed) return;
    try {
      const d = readProcedure<Details>(await connection.procedures.fileDetails({ id: entry.id }));
      navigate("files", entry.id, [...d.ancestors, { id: entry.id, name: entry.name }]);
    } catch (cause) { setError(failure(cause)); }
  }
  function select(entry: FileEntry) { setSelected(new Set([entry.id])); setPublicLink(""); void showDetails(entry); }
  function toggle(entry: FileEntry) {
    setSelected(previous => { const next = new Set(previous); if (next.has(entry.id)) next.delete(entry.id); else next.add(entry.id); return next; });
  }
  async function logout() { await perform(async () => { await request("/api/v1/auth/logout", { method: "POST" }, session.csrfToken); connection?.disconnect(); onSignedOut(); }); }
  function openModal(value: Modal) { setError(""); setModal(value); setTargetPath([]); setPickerQuery(""); setPickerCursor(""); setPickerPrevious([]); setMenu(false); }
  function pickPath(value: { id: string; name: string }[]) { setTargetPath(value); setPickerQuery(""); setPickerCursor(""); setPickerPrevious([]); }
  async function submitModal(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (!modal) return;
    const p = Object.fromEntries(new FormData(event.currentTarget));
    const success = await perform(async () => {
      if (modal.type === "account") {
        await request("/api/v1/auth/password", { method: "POST", body: JSON.stringify(p) }, session.csrfToken); onSignedOut(); return;
      }
      if (modal.type === "user") { await request("/api/v1/auth/users", { method: "POST", body: JSON.stringify(p) }, session.csrfToken); return; }
      if (!connection) throw new Error("Wait for the connection to return.");
      if (modal.type === "folder") await connection.reducers.createFolder({ id: crypto.randomUUID(), parentId, name: String(p.name) });
      const entry = "entry" in modal ? modal.entry : undefined;
      if (entry && modal.type === "rename") await connection.reducers.renameEntry({ id: entry.id, name: String(p.name), expectedRevision: entry.revision });
      if (entry && modal.type === "move") await connection.reducers.moveEntry({ id: entry.id, parentId: targetPath.at(-1)?.id ?? "", expectedRevision: entry.revision });
      if (entry && modal.type === "copy") await connection.reducers.copyEntry({ id: entry.id, newId: crypto.randomUUID(), name: String(p.name), parentId: targetPath.at(-1)?.id ?? "", expectedRevision: entry.revision });
      if (modal.type === "delete") {
        const targets = entry ? [entry] : visible.filter(item => selected.has(item.id));
        for (const item of targets) await connection.reducers.deleteEntry({ id: item.id, expectedRevision: item.revision });
        setSelected(new Set()); setDetails(null);
      }
    }, modal.type === "user" ? "Account created. You can now share files with this user." : "Changes saved.");
    if (success) setModal(null);
  }
  async function trashSelection() {
    if (!connection) return;
    const targets = visible.filter(item => selected.has(item.id));
    await perform(async () => {
      for (const entry of targets) await connection.reducers.trashEntry({ id: entry.id, expectedRevision: entry.revision });
      setSelected(new Set()); setDetails(null);
    }, `${targets.length} item${targets.length === 1 ? "" : "s"} moved to deleted files.`);
  }
  async function upload(files: FileList | null, replace?: FileEntry) {
    if (!files?.length) return;
    for (const file of [...files]) {
      if (!active.current) break;
      const transferId = crypto.randomUUID();
      const controller = new AbortController(); uploadReservations.current.set(transferId, controller);
      const update = (change: Partial<Transfer>) => { if (active.current) setTransfers(current => current.map(t => t.id === transferId ? { ...t, ...change } : t)); };
      setTransfers(current => [...current, { id: transferId, name: file.name, progress: 0, status: "uploading" }]);
      let uploadId: string | undefined;
      try {
        const reservation = await request<{ id: string }>("/api/v1/files/uploads", { method: "POST", signal: controller.signal, body: JSON.stringify({ parentId: replace?.parentId ?? parentId, name: file.name, size: file.size, mediaType: file.type || "application/octet-stream", ...(replace ? { fileId: replace.id, expectedRevision: replace.revision } : {}) }) }, session.csrfToken);
        uploadId = reservation.id;
        if (!active.current || controller.signal.aborted) throw new Error("Upload cancelled.");
        await new Promise<void>((resolve, reject) => {
          const xhr = new XMLHttpRequest(); uploadRequests.current.set(transferId, xhr);
          xhr.open("PUT", `/api/v1/files/uploads/${reservation.id}`); xhr.setRequestHeader("x-csrf-token", session.csrfToken);
          xhr.upload.onprogress = event => update({ progress: event.lengthComputable ? event.loaded / event.total : 0 });
          xhr.onload = () => xhr.status >= 200 && xhr.status < 300 ? resolve() : reject(new Error((() => { try { return JSON.parse(xhr.responseText).error.message; } catch { return "The upload could not be completed."; } })()));
          xhr.onerror = () => reject(new Error("The connection was interrupted. Try the upload again."));
          xhr.onabort = () => reject(new Error("Upload cancelled.")); xhr.send(file);
        });
        update({ progress: 1, status: "complete" });
      } catch (cause) {
        const message = controller.signal.aborted ? "Upload cancelled." : failure(cause); update({ status: /cancelled/.test(message) ? "cancelled" : "error", message });
        if (uploadId && active.current) await request(`/api/v1/files/uploads/${uploadId}`, { method: "DELETE", signal: AbortSignal.timeout(5000) }, session.csrfToken).catch(() => {});
      } finally { uploadRequests.current.delete(transferId); uploadReservations.current.delete(transferId); }
    }
  }
  function cancelUpload(id: string) { uploadReservations.current.get(id)?.abort(); uploadRequests.current.get(id)?.abort(); }
  async function share(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); if (!details || !connection) return;
    const p = Object.fromEntries(new FormData(event.currentTarget));
    const entry = details.entry;
    if (await perform(() => connection.reducers.grantAccess({ id: crypto.randomUUID(), fileId: entry.id, username: String(p.username), role: String(p.role), expectedRevision: entry.revision }), "Sharing updated.")) await showDetails(entry);
  }
  async function makePublicLink() {
    if (!details || !connection) return;
    await perform(async () => {
      const bytes = crypto.getRandomValues(new Uint8Array(32));
      const token = btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
      const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token)))].map(n => n.toString(16).padStart(2, "0")).join("");
      await connection.reducers.createPublicShare({ id: crypto.randomUUID(), fileId: details.entry.id, digest, expiresAt: Date.now() + 7 * 86400_000, expectedRevision: details.entry.revision });
      setPublicLink(`${location.origin}/public/files/${token}`); await showDetails(details.entry);
    });
  }
  const connected = !!connection && !state;
  const title = parentId ? path.at(-1)?.name ?? "Folder" : sections.find(item => item.id === section)!.label;
  const chosen = details?.entry;
  const owner = details?.role === "owner", canWrite = owner || details?.role === "write";
  const initials = session.user.displayName.split(/\s+/).map(word => word[0]).join("").slice(0, 2).toUpperCase();

  return <div className="app-shell" onDragOver={event => { if (event.dataTransfer.types.includes("Files")) event.preventDefault(); }} onDrop={event => { if (event.dataTransfer.files.length) { event.preventDefault(); if (section === "files") void upload(event.dataTransfer.files); } }}>
    <header className="topbar"><div className="topbar-brand"><button className="icon-button mobile-menu" aria-label="Open navigation" onClick={() => setSidebar(!sidebar)}><Icon name="menu" /></button><Brand /><span className="app-name">Files</span></div>
      <label className="global-search"><Icon name="search" /><input aria-label="Search filenames starting with" placeholder="Search filenames…" value={query} onChange={event => { setQuery(event.target.value); setCursor(""); setPrevious([]); }} /><span className="search-scope">Current folder · prefix</span>{query && <button className="icon-button" aria-label="Clear search" onClick={() => setQuery("")}><Icon name="close" /></button>}</label>
      <div className="account-area"><button className="avatar" aria-label="Open account menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{initials}</button>{menu && <div className="account-menu"><strong>{session.user.displayName}</strong><span className="muted">@{session.user.username}</span><button onClick={() => openModal({ type: "account" })}>Account security</button>{session.user.administrator && <button onClick={() => openModal({ type: "user" })}>Create user</button>}<button onClick={() => { void logout(); }}>Sign out</button></div>}</div>
    </header>
    <div className="workspace">
      {sidebar && <button className="nav-scrim" aria-label="Close navigation" onClick={() => setSidebar(false)} />}
      <aside className={`sidebar ${sidebar ? "is-open" : ""}`} aria-label="Files navigation"><div className="sidebar-label">YOUR SPACE</div><button className="new-button" disabled={!connected || section !== "files"} onClick={() => openModal({ type: "folder" })}><Icon name="plus" />New folder</button><nav>{sections.map(item => <button className={`nav-item ${section === item.id ? "active" : ""}`} aria-current={section === item.id ? "page" : undefined} key={item.id} onClick={() => navigate(item.id)}><Icon name={item.icon} />{item.label}</button>)}</nav>
        <div className="storage"><Icon name="folder" /><strong>Your storage</strong><progress max={10 * 1024 ** 3} value={usage?.used ?? 0} aria-label="Storage used" /><span>{formatBytes(usage?.used ?? 0)} of 10 GB used</span><small>Files stay on your instance.</small></div>
        <div className="sidebar-footer"><Icon name="lock" /> Your files. Your place.</div>
      </aside>
      <main className="files-main">
        <div className="breadcrumb" aria-label="Breadcrumb"><button onClick={() => navigate("files")}>All files</button>{path.map((part, index) => <span key={part.id}><Icon name="chevron" /><button onClick={() => navigate("files", part.id, path.slice(0, index + 1))}>{part.name}</button></span>)}</div>
        <div className="page-heading"><div><span className="eyebrow">{section === "shared" ? "A SPACE TOGETHER" : "PERSONAL SPACE"}</span><h1>{title}</h1><p>{section === "trash" ? "Deleted items stay here until you remove them permanently." : section === "shared" ? "Files and folders other people have shared with you." : "A little room for everything that matters."}</p></div><div className="heading-actions"><button className="secondary-button" disabled={!connected || section !== "files"} onClick={() => uploadInput.current?.click()}><Icon name="upload" />Upload files</button></div></div>
        {state && <div className="connection-state" role="status"><Icon name="refresh" />{state}</div>}
        {error && <div className="error-banner" role="alert"><span>{error}</span><button className="icon-button" aria-label="Dismiss error" onClick={() => setError("")}><Icon name="close" /></button></div>}
        {notice && <div className="notice" role="status"><Icon name="check" /><span>{notice}</span><button className="icon-button" aria-label="Dismiss notification" onClick={() => setNotice("")}><Icon name="close" /></button></div>}
        <div className="file-toolbar"><div>{selected.size ? <><strong>{selected.size} selected</strong><button className="text-button" onClick={() => setSelected(new Set())}>Clear</button>{section === "trash" ? <button className="text-button danger" onClick={() => openModal({ type: "delete" })}>Delete permanently</button> : <button className="icon-button" aria-label="Move selected items to deleted files" disabled={!connected || busy || visible.some(entry => selected.has(entry.id) && entry.ownerId !== session.user.id)} onClick={() => { void trashSelection(); }}><Icon name="trash" /></button>}</> : <span>{visible.length}{hasMore ? "+" : ""} items{query ? ` starting with “${query}”` : ""}</span>}</div><div className="view-switch" aria-label="File display"><button className={`icon-button ${!grid ? "selected" : ""}`} aria-label="List view" aria-pressed={!grid} onClick={() => setGrid(false)}><Icon name="list" /></button><button className={`icon-button ${grid ? "selected" : ""}`} aria-label="Grid view" aria-pressed={grid} onClick={() => setGrid(true)}><Icon name="grid" /></button></div></div>
        {loading ? <div className="file-skeleton" aria-busy="true" aria-label="Loading files">{[1, 2, 3, 4].map(id => <div key={id}><span /><span /></div>)}</div> : !visible.length ? <div className="empty-state"><span className="empty-icon"><Icon name={section === "trash" ? "trash" : section === "shared" ? "share" : query ? "search" : "folder"} /></span><h2>{query ? "No matching files" : section === "trash" ? "Nothing in deleted files" : section === "shared" ? "A little quiet here" : section === "favorites" ? "Keep your favorites close" : "A home for your files"}</h2><p>{query ? "Search matches the beginning of a filename in this folder." : section === "files" ? "Drop your first file here, or upload something to get started." : section === "favorites" ? "Mark a file or folder as a favorite to find it here." : section === "shared" ? "When someone shares with you, it will appear here." : "Items you delete will appear here for recovery."}</p>{section === "files" && !query && <button className="primary-button" disabled={!connected} onClick={() => uploadInput.current?.click()}><Icon name="upload" />Upload your first file</button>}</div> : grid ? <div className="file-grid">{visible.map(entry => <article key={entry.id} className={`file-card ${selected.has(entry.id) ? "is-selected" : ""}`}><button className="card-open" onClick={() => { void openEntry(entry); }} aria-label={`Open ${entry.name}`}><Icon name={entry.kind === "folder" ? "folder" : "file"} className={entry.kind === "folder" ? "folder-icon" : ""} /></button><div className="card-caption"><input type="checkbox" checked={selected.has(entry.id)} aria-label={`Select ${entry.name}`} onChange={() => toggle(entry)} /><button className="name-button" onClick={() => select(entry)}>{entry.name}</button><button className="icon-button" aria-label={`Details for ${entry.name}`} onClick={() => select(entry)}><Icon name="more" /></button></div><p>{entry.kind === "folder" ? "Folder" : formatBytes(entry.size)} · {date(entry.modifiedAt)}</p></article>)}</div> : <div className="table-scroll"><table className="file-table"><thead><tr><th className="checkbox-cell"><input type="checkbox" aria-label="Select all visible files" checked={visible.length > 0 && visible.every(entry => selected.has(entry.id))} onChange={event => setSelected(event.target.checked ? new Set(visible.map(entry => entry.id)) : new Set())} /></th><th>Name <span aria-hidden="true">↑</span></th><th className="size-cell">Size</th><th className="date-cell">Modified</th><th><span className="sr-only">Actions</span></th></tr></thead><tbody>{visible.map(entry => <tr key={entry.id} className={selected.has(entry.id) || chosen?.id === entry.id ? "is-selected" : ""}><td className="checkbox-cell"><input type="checkbox" checked={selected.has(entry.id)} aria-label={`Select ${entry.name}`} onChange={() => toggle(entry)} /></td><td><div className="file-name"><Icon name={entry.kind === "folder" ? "folder" : "file"} className={entry.kind === "folder" ? "folder-icon" : "document-icon"} /><button className="name-button" onClick={() => entry.kind === "folder" && !entry.trashed ? void openEntry(entry) : select(entry)} onDoubleClick={() => { if (entry.kind === "file" && !entry.trashed) setPreview(entry); }}>{entry.name}</button>{entry.favorite && <Icon name="star" className="favorite-icon" />}</div></td><td className="size-cell muted">{entry.kind === "folder" ? "—" : formatBytes(entry.size)}</td><td className="date-cell muted">{date(entry.modifiedAt)}</td><td><button className="icon-button" aria-label={`Details for ${entry.name}`} onClick={() => select(entry)}><Icon name="more" /></button></td></tr>)}</tbody></table></div>}
        {(previous.length > 0 || hasMore) && <div className="pagination"><button className="secondary-button" disabled={!previous.length} onClick={() => { setCursor(previous.at(-1) ?? ""); setPrevious(previous.slice(0, -1)); }}>Previous</button><span>Page {previous.length + 1}</span><button className="secondary-button" disabled={!hasMore} onClick={() => { setPrevious([...previous, cursor]); setCursor(visible.at(-1)!.listKey); }}>Next</button></div>}
        <div className="drop-hint"><Icon name="upload" /> Drag files into this folder to upload</div>
      </main>
      {details && <aside className="details-panel" aria-label="File details"><div className="details-heading"><span>File details</span><button className="icon-button" aria-label="Close details" onClick={() => { setDetails(null); detailSequence.current++; }}><Icon name="close" /></button></div><div className="details-preview"><Icon name={chosen!.kind === "folder" ? "folder" : "file"} /></div><h2>{chosen!.name}</h2><p className="muted details-summary">{chosen!.kind === "folder" ? "Folder" : formatBytes(chosen!.size)} · {date(chosen!.modifiedAt)}</p>
        <div className="details-tabs" role="tablist" aria-label="File information">{(["details", "versions", "sharing"] as const).map(id => <button role="tab" aria-selected={tab === id} key={id} onClick={() => setTab(id)}>{id[0].toUpperCase() + id.slice(1)}</button>)}</div>
        <div className="details-content" role="tabpanel" aria-label={tab}>
          {tab === "details" && <><dl><dt>Modified</dt><dd>{new Date(chosen!.modifiedAt).toLocaleString()}</dd><dt>Access</dt><dd>{owner ? "You own this item" : canWrite ? "You can edit" : "You can view"}</dd><dt>Revision</dt><dd>{chosen!.revision}</dd></dl><div className="detail-actions">
            {chosen!.trashed ? <>{owner && <><button className="secondary-button" disabled={busy} onClick={() => { void perform(() => connection!.reducers.restoreEntry({ id: chosen!.id, expectedRevision: chosen!.revision }), "Item restored."); }}><Icon name="refresh" />Restore</button><button className="text-button danger" onClick={() => openModal({ type: "delete", entry: chosen })}><Icon name="trash" />Delete permanently</button></>}</> : <>
            {chosen!.kind === "file" && <><a className="secondary-button" href={`/api/v1/files/${chosen!.id}/content`}><Icon name="download" />Download</a><button className="secondary-button" onClick={() => setPreview(chosen!)}><Icon name="file" />Preview</button></>}
            {owner && <button className="text-button" disabled={busy} onClick={() => { void perform(() => connection!.reducers.favoriteEntry({ id: chosen!.id, favorite: !chosen!.favorite, expectedRevision: chosen!.revision })); }}><Icon name="star" />{chosen!.favorite ? "Remove favorite" : "Add to favorites"}</button>}
            {canWrite && <button className="text-button" onClick={() => openModal({ type: "rename", entry: chosen })}>Rename</button>}
            {owner && <button className="text-button" onClick={() => openModal({ type: "move", entry: chosen })}>Move to…</button>}
            <button className="text-button" onClick={() => openModal({ type: "copy", entry: chosen })}>Copy to…</button>
            {canWrite && chosen!.kind === "file" && <button className="text-button" onClick={() => replacementInput.current?.click()}>Upload new version</button>}
            {owner && <button className="text-button danger" disabled={busy} onClick={() => { void perform(() => connection!.reducers.trashEntry({ id: chosen!.id, expectedRevision: chosen!.revision }), "Moved to deleted files."); }}><Icon name="trash" />Move to deleted files</button>}
            </>}
          </div></>}
          {tab === "versions" && <>{details.versions.length ? details.versions.map(version => <div className="version" key={version.id}><Icon name="clock" /><div><strong>{new Date(version.createdAt).toLocaleString()}</strong><span>{formatBytes(version.size)}{version.blobId === chosen!.blobId ? " · Current content" : ""}</span></div>{!chosen!.trashed && canWrite && version.blobId !== chosen!.blobId && <button className="text-button" disabled={busy} onClick={() => { void perform(() => connection!.reducers.restoreVersion({ id: chosen!.id, versionId: version.id, expectedRevision: chosen!.revision }), "Version restored."); }}>Restore</button>}</div>) : <p className="muted">Folders don’t have content versions.</p>}</>}
          {tab === "sharing" && <>{owner && !chosen!.trashed ? <><h3>Share with a person</h3><form className="share-form" onSubmit={share}><label>Username<input name="username" required maxLength={64} placeholder="Enter a username" /></label><label>Permission<select name="role"><option value="read">Can view</option><option value="write">Can edit</option></select></label><button className="primary-button" disabled={busy}>Share</button></form>{details.grants.map(grant => <div className="grant" key={grant.id}><span><strong>@{grant.username}</strong><small>{grant.role === "write" ? "Can edit" : "Can view"}</small></span><button className="icon-button" aria-label={`Revoke access for ${grant.username}`} onClick={() => { void perform(async () => { await connection!.reducers.revokeGrant({ id: grant.id }); await showDetails(chosen!); }); }}><Icon name="close" /></button></div>)}{chosen!.kind === "file" && <><h3>Public download link</h3><p className="muted small">Anyone with the link can download this file. New links expire after seven days.</p><button className="secondary-button" disabled={busy} onClick={() => { void makePublicLink(); }}><Icon name="share" />Create link</button>{publicLink && <label>Copy this link<input readOnly value={publicLink} onFocus={e => e.target.select()} /><small className="field-hint">The secret link is shown only here. Save it before closing.</small></label>}{details.publicShares.map(link => <div className="grant" key={link.id}><span>Expires {date(link.expiresAt)}</span><button className="text-button danger" onClick={() => { void perform(async () => { await connection!.reducers.revokePublicShare({ id: link.id }); setPublicLink(""); await showDetails(chosen!); }); }}>Revoke</button></div>)}</>}</> : <p className="muted">{chosen!.trashed ? "Restore this item before sharing it." : "Only the owner can manage sharing."}</p>}</>}
        </div>
      </aside>}
    </div>
    <input ref={uploadInput} className="sr-only" type="file" multiple tabIndex={-1} aria-label="Choose files to upload" onChange={event => { void upload(event.target.files); event.target.value = ""; }} />
    <input ref={replacementInput} className="sr-only" type="file" tabIndex={-1} aria-label="Choose a new file version" onChange={event => { void upload(event.target.files, chosen); event.target.value = ""; }} />
    {transfers.length > 0 && <section className="transfer-panel" aria-label="Uploads"><div className="transfer-heading"><strong>Uploads</strong><button className="icon-button" aria-label="Dismiss finished uploads" onClick={() => setTransfers(t => t.filter(item => item.status === "uploading"))}><Icon name="close" /></button></div>{transfers.map(transfer => <div className="transfer" key={transfer.id}><Icon name={transfer.status === "complete" ? "check" : "upload"} /><div><strong>{transfer.name}</strong><span role="status">{transfer.status === "uploading" ? `Uploading · ${Math.round(transfer.progress * 100)}%` : transfer.status === "complete" ? "Uploaded" : transfer.message}</span>{transfer.status === "uploading" && <progress value={transfer.progress} max={1} aria-label={`Upload progress for ${transfer.name}`} />}</div>{transfer.status === "uploading" && <button className="icon-button" aria-label={`Cancel upload ${transfer.name}`} onClick={() => cancelUpload(transfer.id)}><Icon name="close" /></button>}</div>)}</section>}
    {modal && <Dialog title={modal.type === "folder" ? "New folder" : modal.type === "rename" ? "Rename item" : modal.type === "move" ? "Move item" : modal.type === "copy" ? "Copy item" : modal.type === "delete" ? "Delete permanently?" : modal.type === "user" ? "Create a user" : "Account security"} onClose={() => setModal(null)}>{modal.type === "account" ? <AccountSecurity session={session} onSignedOut={onSignedOut} /> : <form onSubmit={submitModal}>
      {["folder", "rename", "copy"].includes(modal.type) && <label>Name<input name="name" required maxLength={255} defaultValue={"entry" in modal ? modal.type === "copy" ? `${modal.entry!.name} copy` : modal.entry?.name : ""} autoFocus /></label>}
      {(modal.type === "move" || modal.type === "copy") && <div className="folder-picker"><label>Search folders<input type="search" value={pickerQuery} placeholder="Filename starts with…" onChange={event => { setPickerQuery(event.target.value); setPickerCursor(""); setPickerPrevious([]); }} onKeyDown={event => { if (event.key === "Enter") event.preventDefault(); }} /></label><div className="picker-path"><button type="button" className="text-button" onClick={() => pickPath([])}>All files</button>{targetPath.map((part, i) => <button key={part.id} type="button" className="text-button" onClick={() => pickPath(targetPath.slice(0, i + 1))}>/ {part.name}</button>)}</div>{folders.filter(folder => folder.id !== modal.entry?.id).map(folder => <button key={folder.id} type="button" className="picker-folder" onClick={() => pickPath([...targetPath, folder])}><Icon name="folder" />{folder.name}<Icon name="chevron" /></button>)}{!folders.length && <p className="muted">No matching folders here.</p>}{(pickerPrevious.length > 0 || pickerNext) && <div className="pagination"><button type="button" className="text-button" disabled={!pickerPrevious.length} onClick={() => { setPickerCursor(pickerPrevious.at(-1) ?? ""); setPickerPrevious(pickerPrevious.slice(0, -1)); }}>Previous</button><button type="button" className="text-button" disabled={!pickerNext} onClick={() => { setPickerPrevious([...pickerPrevious, pickerCursor]); setPickerCursor(pickerNext); }}>Next</button></div>}</div>}
      {modal.type === "delete" && <p>This permanently removes {"entry" in modal && modal.entry ? `“${modal.entry.name}”` : `${selected.size} selected items`} and their versions. This cannot be undone.</p>}
      {modal.type === "user" && <><label>Display name<input name="displayName" required maxLength={100} autoFocus /></label><label>Username<input name="username" required pattern="[a-zA-Z0-9][a-zA-Z0-9_.\-]{2,63}" maxLength={64} /></label><label>Initial password<input name="password" type="password" autoComplete="new-password" required minLength={15} /></label></>}
      {error && <p className="error" role="alert">{error}</p>}<div className="dialog-actions"><button type="button" className="text-button" onClick={() => setModal(null)}>Cancel</button><button className={modal.type === "delete" ? "danger-button" : "primary-button"} disabled={busy}>{busy ? "Saving…" : modal.type === "folder" ? "Create folder" : modal.type === "delete" ? "Delete permanently" : modal.type === "move" ? "Move here" : modal.type === "copy" ? "Copy here" : "Save"}</button></div>
    </form>}</Dialog>}
    {preview && <Dialog title={preview.name} onClose={() => setPreview(null)}><div className="preview-content">{/^image\/(png|jpeg|gif|webp)$/.test(preview.mediaType) ? <img src={`/api/v1/files/${preview.id}/content?preview=1`} alt={preview.name} /> : /^(text\/|application\/json)/.test(preview.mediaType) && preview.size <= 1024 * 1024 ? <pre>{textPreview || "Loading preview…"}</pre> : <div className="empty-state"><Icon name="file" /><p>A browser preview isn’t available for this file.</p></div>}</div><a className="primary-button" href={`/api/v1/files/${preview.id}/content`}><Icon name="download" />Download</a></Dialog>}
  </div>;
}
