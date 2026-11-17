import {useEffect,useId,useRef,useState,type FormEvent} from "react";
import {request,formatBytes,type BrowserSession} from "./api.ts";
import {localDateTime} from "./dates.ts";
import {Icon} from "./icons.tsx";
import {EditorPane} from "./pane.tsx";
import {Dialog} from "./dialog.tsx";
import {Diff,DiffStat,unifiedHunks,countLines,textLines} from "./diff.tsx";
import {historyLimits,gitPath,type HistoryOverview,type HistoryChange,type HistoryChangeset,type HistoryDetail,type HistoryBranch} from "../shared/history.ts";

/** Folder history in the folder details sheet (docs/spacetimedb/folder-history.md). */
const base=(folderId:string)=>`/api/v1/files/history/${folderId}`;
export function historyFailure(error:unknown){
  const text=error instanceof Error?error.message:"The operation could not be completed.";
  if(/nothingToCommit/.test(text))return "There are no changes to commit.";
  if(/uncommittedChanges/.test(text))return "Commit or snapshot the current changes before restoring.";
  if(/conflict/.test(text))return "The folder or its history changed meanwhile. Review the latest state and try again.";
  if(/operationTooLarge/.test(text))return `History supports up to ${historyLimits.files.toLocaleString()} files and ${historyLimits.depth} folder levels.`;
  if(/quotaExceeded/.test(text))return "There isn’t enough storage available for this history.";
  if(/forbidden/.test(text))return "You can view this history but not change it.";
  if(/notFound/.test(text))return "This history is no longer available, or you no longer have access.";
  if(/contentUnavailable/.test(text))return "Some content in this history is unavailable. Try again later.";
  return text;
}
const kindLabel:Record<HistoryChange["kind"],string>={added:"Added",modified:"Modified",deleted:"Deleted",renamed:"Renamed"};
const changesetLabel=(row:HistoryChangeset)=>row.kind==="snapshot"?"Snapshot":row.kind==="restore"?"Restore":row.kind==="merge"?"Merge":"Commit";
const when=(value:number)=>localDateTime(value,{dateStyle:"medium",timeStyle:"short"});
const plural=(count:number,one:string,many=one+"s")=>`${count.toLocaleString()} ${count===1?one:many}`;

/** Text that can be compared line by line in the browser. */
function textual(change:HistoryChange){
  const types=[change.mediaType,change.previousMediaType].filter(Boolean) as string[],sizes=[change.size??0,change.previousSize??0];
  const named=/\.(md|markdown|txt|csv|json|ya?ml|toml|xml|html?|css|[cm]?[jt]sx?|py|rb|go|rs|java|kt|swift|c|h|cpp|sh|ini|conf|env|sql|ics|vcf|svg|log)$/i.test(change.path);
  return sizes.every(size=>size<=512*1024)&&(named||types.length>0&&types.every(type=>/^text\/|json|xml|javascript|yaml/.test(type)));
}
function contentUrl(folderId:string,versionId?:string,fileId?:string){return versionId?`${base(folderId)}/versions/${versionId}/content`:fileId?`/api/v1/files/${fileId}/content`:undefined;}
async function fetchText(url:string|undefined,signal:AbortSignal){
  if(!url)return "";
  const response=await fetch(url,{signal});if(!response.ok)throw new Error("The content could not be loaded.");
  const text=await response.text();if(text.includes("\0"))throw new Error("binary");return text;
}

/** One path with its diff, loaded when expanded. */
function ChangeRow({folderId,change,live,selected,onSelect}:{folderId:string;change:HistoryChange;live:boolean;selected?:boolean;onSelect?:(value:boolean)=>void}){
  const [open,setOpen]=useState(false),[diff,setDiff]=useState<{patch:string;added:number;removed:number}|{note:string}|null>(null),[wrap,setWrap]=useState(false);
  useEffect(()=>{
    if(!open||diff)return;
    if(!textual(change)){
      const size=change.kind==="deleted"?`Was ${formatBytes(change.previousSize??0)}`:`${formatBytes(change.size??0)}${change.kind==="modified"&&change.previousSize!==undefined?`, was ${formatBytes(change.previousSize)}`:""}`;
      setDiff({note:`${size}. No text preview for this file type.`});return;
    }
    const abort=new AbortController();
    void Promise.all([fetchText(change.kind==="added"?undefined:contentUrl(folderId,change.previousVersionId),abort.signal),fetchText(change.kind==="deleted"?undefined:contentUrl(folderId,change.versionId,live?change.fileId:undefined),abort.signal)]).then(([old,next])=>{
      const a=textLines(old),b=textLines(next);setDiff({patch:unifiedHunks(a,b),...countLines(a,b)});
    }).catch(error=>{if(!abort.signal.aborted)setDiff({note:error instanceof Error&&error.message==="binary"?"No text preview for this file.":"The content could not be loaded. Close and open the file to try again."});});
    return()=>abort.abort();
  },[open]);
  return <section className="history-change" aria-label={change.path}>
    <header>
      {onSelect&&<input type="checkbox" checked={selected} aria-label={`Include ${change.path}`} onChange={event=>onSelect(event.target.checked)}/>}
      <button type="button" className="history-change-toggle" aria-expanded={open} onClick={()=>setOpen(value=>!value)}><Icon name="chevron"/><span className="history-change-path" title={change.path}>{change.path}{change.previousPath&&<small>from {change.previousPath}</small>}</span></button>
      <span className={`history-change-kind is-${change.kind}`}>{kindLabel[change.kind]}</span>
      {diff&&"patch" in diff&&<DiffStat added={diff.added} removed={diff.removed}/>}
      {open&&diff&&"patch" in diff&&<button type="button" className="icon-button" aria-label="Wrap lines" title="Wrap lines" aria-pressed={wrap} onClick={()=>setWrap(value=>!value)}><Icon name="menu"/></button>}
    </header>
    {open&&(!diff?<p className="history-note" role="status">Loading changes…</p>:"note" in diff?<p className="history-note">{diff.note}</p>:diff.patch?<Diff source={diff.patch} wrap={wrap} label={`Changes in ${change.path}`}/>:<p className="history-note">The content is unchanged; only the name or place changed.</p>)}
  </section>;
}

function CommitPane({folderId,status,session,onClose,onDone}:{folderId:string;status:NonNullable<HistoryOverview["status"]>;session:BrowserSession;onClose:()=>void;onDone:(message:string)=>void}){
  const [message,setMessage]=useState(""),[chosen,setChosen]=useState(()=>new Set(status.changes.map(change=>change.path))),[busy,setBusy]=useState(false),[error,setError]=useState("");
  async function submit(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{
      const all=chosen.size===status.changes.length;
      await request(`${base(folderId)}/commits`,{method:"POST",body:JSON.stringify({message,token:status.token,...all?{}:{paths:[...chosen]}})},session.csrfToken);
      onDone(`Committed ${plural(chosen.size,"change")}.`);
    }catch(cause){setError(historyFailure(cause));}finally{setBusy(false);}
  }
  return <EditorPane title="Commit changes" onClose={onClose} dirty={!!message.trim()}><form className="history-form" onSubmit={submit}>
    <label>Message<textarea name="message" required maxLength={historyLimits.messageBytes} rows={3} value={message} onChange={event=>setMessage(event.target.value)} placeholder="Describe what changed" autoFocus/></label>
    <div className="history-list-heading"><strong>{plural(chosen.size,"change")} selected</strong><button type="button" className="text-button" onClick={()=>setChosen(chosen.size===status.changes.length?new Set():new Set(status.changes.map(change=>change.path)))}>{chosen.size===status.changes.length?"Select none":"Select all"}</button></div>
    <div className="history-changes">{status.changes.map(change=><ChangeRow key={change.path} folderId={folderId} change={change} live selected={chosen.has(change.path)} onSelect={value=>setChosen(current=>{const next=new Set(current);if(value)next.add(change.path);else next.delete(change.path);return next;})}/>)}</div>
    {error&&<p className="error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="text-button">Cancel</button><button className="primary-button" disabled={busy||!chosen.size||!message.trim()}>{busy?"Committing…":"Commit"}</button></div>
  </form></EditorPane>;
}

function ChangesetPane({folderId,changeset,revision,canWrite,session,onClose,onDone}:{folderId:string;changeset:HistoryChangeset;revision:number;canWrite:boolean;session:BrowserSession;onClose:()=>void;onDone:(message:string)=>void}){
  const [detail,setDetail]=useState<HistoryDetail|null>(null),[error,setError]=useState(""),[confirm,setConfirm]=useState(false),[busy,setBusy]=useState(false),[attempt,setAttempt]=useState(0);
  useEffect(()=>{const abort=new AbortController();setError("");void request<HistoryDetail>(`${base(folderId)}/commits/${changeset.id}`,{signal:abort.signal}).then(setDetail).catch(cause=>{if(!abort.signal.aborted)setError(historyFailure(cause));});return()=>abort.abort();},[changeset.id,attempt]);
  async function restore(){
    setBusy(true);setError("");
    try{await request(`${base(folderId)}/restore`,{method:"POST",body:JSON.stringify({id:changeset.id,revision})},session.csrfToken);onDone(`Folder restored to “${changeset.message}”.`);}
    catch(cause){setError(historyFailure(cause));setConfirm(false);}finally{setBusy(false);}
  }
  return <EditorPane title={changesetLabel(changeset)} onClose={onClose}>
    <div className="history-summary"><h3>{changeset.message}</h3><p className="muted">{changeset.author} · {when(changeset.createdAt)} · {plural(changeset.fileCount,"file")}{changeset.sha&&<> · <code className="history-sha" title={changeset.sha}>{changeset.sha.slice(0,7)}</code></>}</p></div>
    {error&&<div className="error" role="alert">{error}{!detail&&<button type="button" className="text-button" onClick={()=>setAttempt(value=>value+1)}>Retry</button>}</div>}
    {!detail&&!error?<p className="history-note" role="status">Loading changes…</p>:detail&&<div className="history-changes">{detail.changes.map(change=><ChangeRow key={change.path} folderId={folderId} change={change} live={false}/>)}</div>}
    {canWrite&&<div className="dialog-actions"><button type="button" className="secondary-button" disabled={busy} onClick={()=>setConfirm(true)}><Icon name="history"/>Restore folder to this point</button></div>}
    {confirm&&<Dialog title="Restore this folder?" onClose={()=>setConfirm(false)}><p>Files return to how they were at “{changeset.message}”. Files added later move to deleted files. The restore is committed, so you can return to the current state from history.</p><div className="dialog-actions"><button className="text-button" onClick={()=>setConfirm(false)}>Cancel</button><button className="primary-button" disabled={busy} onClick={()=>{void restore();}}>{busy?"Restoring…":"Restore folder"}</button></div></Dialog>}
  </EditorPane>;
}

function BranchPane({folderId,branch,revision,canWrite,session,onClose,onDone}:{folderId:string;branch:HistoryBranch;revision:number;canWrite:boolean;session:BrowserSession;onClose:()=>void;onDone:(message:string)=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[conflicts,setConflicts]=useState<string[]>([]),[confirm,setConfirm]=useState(false);
  async function act(action:"merge"|"discard"){
    setBusy(true);setError("");setConflicts([]);
    try{
      const result=await request<{merged:boolean;conflicts?:string[]}>(`${base(folderId)}/branches/${action}`,{method:"POST",body:JSON.stringify({name:branch.name,revision})},session.csrfToken);
      if(result.conflicts?.length){setConflicts(result.conflicts);return;}
      onDone(action==="merge"?"Agent changes merged and committed.":"Agent branch discarded.");
    }catch(cause){setError(historyFailure(cause));}finally{setBusy(false);setConfirm(false);}
  }
  return <EditorPane title="Agent branch" onClose={onClose}>
    <div className="history-summary"><h3>{branch.message}</h3><p className="muted">{branch.name} · {branch.author} · {when(branch.modifiedAt)}</p></div>
    {conflicts.length>0&&<div className="error" role="alert"><p>These files changed after the agent started, so nothing was merged:</p><ul>{conflicts.map(path=><li key={path}>{path}</li>)}</ul><p>Commit the agent’s changes again from its thread, or discard this branch.</p></div>}
    {error&&<p className="error" role="alert">{error}</p>}
    <div className="history-changes">{branch.changes.map(change=><ChangeRow key={change.path} folderId={folderId} change={change} live={false}/>)}</div>
    {canWrite&&<div className="dialog-actions"><button type="button" className="text-button danger" disabled={busy} onClick={()=>setConfirm(true)}>Discard</button><button type="button" className="primary-button" disabled={busy} onClick={()=>{void act("merge");}}>{busy?"Merging…":"Merge into folder"}</button></div>}
    {confirm&&<Dialog title="Discard this branch?" onClose={()=>setConfirm(false)}><p>The agent’s proposed changes to {plural(branch.changes.length,"file")} are deleted. The folder is not changed.</p><div className="dialog-actions"><button className="text-button" onClick={()=>setConfirm(false)}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{void act("discard");}}>Discard branch</button></div></Dialog>}
  </EditorPane>;
}

/** Clone address and the branches and tags pushed besides main (docs/spacetimedb/folder-history.md#git). */
function GitCard({folderId,refs,username}:{folderId:string;refs:HistoryOverview["gitRefs"];username:string}){
  const url=`${location.origin}${gitPath(folderId)}`,[copied,setCopied]=useState(""),field=useId();
  return <div className="property-card history-git">
    <h3>Git</h3>
    <div className="history-clone"><label htmlFor={field}>Clone address</label><span className="history-clone-row"><input id={field} readOnly value={url} onFocus={event=>event.target.select()}/><button type="button" className="secondary-button" onClick={()=>{void navigator.clipboard.writeText(url).then(()=>setCopied("Address copied."),()=>setCopied("Select the address and copy it."));}}><Icon name="copy"/>Copy</button></span></div>
    {copied&&<p className="muted small" role="status">{copied}</p>}
    <p className="muted small">Sign in as {username} with an app password that has Files access. Pushing to main changes this folder; other branches and tags are kept without changing it.</p>
    {refs.length>0&&<ul className="history-refs" aria-label="Branches and tags">{refs.map(ref=>{const tag=ref.name.startsWith("refs/tags/");return <li key={ref.name}><Icon name={tag?"bookmark":"share"}/><span title={ref.name}>{ref.name.replace(/^refs\/(heads|tags)\//,"")}</span><small>{tag?"Tag":"Branch"}</small><code>{ref.sha.slice(0,7)}</code></li>;})}</ul>}
    <a className="text-button history-open-git" href={`/git?repo=${folderId}`}><Icon name="branch"/>Open in Git</a>
  </div>;
}

type Pane={type:"commit"}|{type:"changeset";row:HistoryChangeset}|{type:"branch";branch:HistoryBranch};
export function FolderHistory({folderId,revision,session}:{folderId:string;revision:number;session:BrowserSession}){
  const [data,setData]=useState<HistoryOverview|null>(null),[older,setOlder]=useState<HistoryChangeset[]>([]),[next,setNext]=useState<number|null>(null);
  const [error,setError]=useState(""),[notice,setNotice]=useState(""),[busy,setBusy]=useState(false),[pane,setPane]=useState<Pane|null>(null),[untrack,setUntrack]=useState(false);
  const sequence=useRef(0);
  async function load(){
    const current=++sequence.current;
    try{const value=await request<HistoryOverview>(base(folderId));if(current!==sequence.current)return;setData(value);setOlder([]);setNext(value.nextBefore);setError("");}
    catch(cause){if(current===sequence.current){setError(historyFailure(cause));}}
  }
  useEffect(()=>{setData(null);setNotice("");void load();},[folderId]);
  useEffect(()=>{if(data)void load();},[revision]);
  async function act(work:()=>Promise<unknown>,message:string){
    setBusy(true);setError("");setNotice("");
    try{await work();setNotice(message);await load();}catch(cause){setError(historyFailure(cause));}finally{setBusy(false);}
  }
  async function more(){
    if(next===null)return;
    try{const value=await request<HistoryOverview>(`${base(folderId)}?before=${next}`);setOlder(current=>[...current,...value.changesets]);setNext(value.nextBefore);}catch(cause){setError(historyFailure(cause));}
  }
  function done(message:string){setPane(null);setNotice(message);void load();}
  const post=(path:string,value:unknown)=>request(`${base(folderId)}${path}`,{method:"POST",body:JSON.stringify(value)},session.csrfToken);
  if(!data)return error?<div className="history-error" role="alert"><p className="error">{error}</p><button className="secondary-button" onClick={()=>{void load();}}>Retry</button></div>:<p className="muted" role="status">Loading history…</p>;
  const messages=<>{error&&<p className="error" role="alert">{error}</p>}{notice&&<p className="history-notice" role="status"><Icon name="check"/>{notice}</p>}</>;
  if(!data.tracked)return <div className="history-panel">{messages}
    <div className="empty-history"><Icon name="history"/><h3>Keep history for this folder</h3><p>Commit changes to files in this folder, see what changed and restore earlier states. Files keep working as usual; nothing is committed until you choose to.</p>
      {data.blocked==="locked"?<p className="muted">Folders in the Locked folder can’t keep history.</p>:data.blocked==="nested"?<p className="muted">A folder that contains this one already keeps history.</p>:data.blocked==="limit"?<p className="muted">You can keep history for up to {historyLimits.repos} folders.</p>:data.owner?<button className="primary-button" disabled={busy} onClick={()=>{void act(()=>post("",{action:"track"}),"History is on for this folder.");}}>Track history</button>:<p className="muted">Only the owner can turn on history.</p>}
    </div>
  </div>;
  const status=data.status!,changesets=[...data.changesets,...older];
  return <div className="history-panel">{messages}
    <div className="property-card history-status">
      <h3>Uncommitted changes</h3>
      {status.changes.length?<><ul className="history-pending">{status.changes.slice(0,6).map(change=><li key={change.path}><span className={`history-change-kind is-${change.kind}`}>{kindLabel[change.kind]}</span><span title={change.path}>{change.path}</span></li>)}</ul>
        {status.changes.length>6&&<p className="history-more">and {plural(status.changes.length-6,"more change")}</p>}
        {data.canWrite&&<div className="history-actions"><button className="primary-button" disabled={busy} onClick={()=>setPane({type:"commit"})}>Review and commit</button><button className="secondary-button" disabled={busy} title="Commit every change with an automatic message" onClick={()=>{void act(()=>post("/commits",{snapshot:true,token:status.token}),`Snapshot of ${plural(status.changes.length,"change")} committed.`);}}>Snapshot</button></div>}</>
        :<p className="history-clean"><Icon name="check"/>Everything is committed.</p>}
    </div>
    {data.branches.length>0&&<div className="property-card"><h3>Agent branches</h3>{data.branches.map(branch=><button key={branch.name} className="property property-action" onClick={()=>setPane({type:"branch",branch})}><Icon name="agent"/><div><span>{branch.message}</span><small>{plural(branch.changes.length,"file")} · {when(branch.modifiedAt)}</small></div></button>)}</div>}
    <h3 className="history-heading">History</h3>
    {changesets.length?<ol className="history-log">{changesets.map(row=><li key={row.id}><button className="history-entry" onClick={()=>setPane({type:"changeset",row})}><Icon name={row.kind==="restore"?"history":row.kind==="merge"?"agent":row.kind==="snapshot"?"clock":"check"}/><span><strong>{row.message}</strong><small>{row.author} · {when(row.createdAt)} · {plural(row.fileCount,"file")}</small></span></button></li>)}</ol>:<p className="muted">No commits yet. Commit the current files to start the history.</p>}
    {next!==null&&<button className="text-button" onClick={()=>{void more();}}>Show older</button>}
    <GitCard folderId={folderId} refs={data.gitRefs} username={session.user.username}/>
    <div className="history-footer">{changesets.length>0&&<a className="text-button" href={`${base(folderId)}/bundle`} download><Icon name="download"/>Download as Git bundle</a>}{data.owner&&<button className="text-button danger" disabled={busy} onClick={()=>setUntrack(true)}>Stop keeping history</button>}</div>
    {pane?.type==="commit"&&<CommitPane folderId={folderId} status={status} session={session} onClose={()=>setPane(null)} onDone={done}/>}
    {pane?.type==="changeset"&&<ChangesetPane key={pane.row.id} folderId={folderId} changeset={pane.row} revision={data.repo!.revision} canWrite={data.canWrite} session={session} onClose={()=>setPane(null)} onDone={done}/>}
    {pane?.type==="branch"&&<BranchPane key={pane.branch.name} folderId={folderId} branch={pane.branch} revision={data.repo!.revision} canWrite={data.canWrite} session={session} onClose={()=>setPane(null)} onDone={done}/>}
    {untrack&&<Dialog title="Stop keeping history?" onClose={()=>setUntrack(false)}><p>All commits and agent branches for “{data.folder.name}” are deleted. Files and their versions stay; content only history kept is released from your storage.</p><div className="dialog-actions"><button className="text-button" onClick={()=>setUntrack(false)}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setUntrack(false);void act(()=>post("",{action:"untrack",revision:data.repo!.revision}),"History is off for this folder.");}}>Delete history</button></div></Dialog>}
  </div>;
}
