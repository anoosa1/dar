import {useEffect,useRef,useState,type FormEvent} from "react";
import {request,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceHeader} from "../shell.tsx";
import {Icon} from "../icons.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {Select} from "../controls.tsx";
import {activeAgentRun,type AgentProject,type AgentThread,type AgentMessage,type AgentHistory,type AgentFolders,type AgentRun,type AgentProvider,type AgentChange} from "../../shared/agent.ts";
import "./workspace.css";

const base="/api/v1/agent";
type View="projects"|"conversation"|"changes"|"terminal";
function mergeMessages(current:AgentMessage[],incoming:AgentMessage[]){
  return [...new Map([...current,...incoming].map(message=>[message.id,message])).values()].sort((a,b)=>a.sequence-b.sequence);
}
function failure(error:unknown){
  if(error instanceof RequestError){
    if(error.status===409)return "This thread changed in another window. Refresh the conversation, then retry. Your draft is retained.";
    if(error.status===404)return "This item is unavailable, or your access changed. Refresh to continue.";
    if(error.status===507)return "Agent storage is full for this account. No new data was saved.";
  }
  return error instanceof Error?error.message:"The request could not be completed.";
}

/** The selected folder is a Files identity, never a physical blob/storage path. */
function ProjectEditor({session,onCreated,onClose}:{session:BrowserSession;onCreated:(project:AgentProject)=>void;onClose:()=>void}){
  const [name,setName]=useState(""),[kind,setKind]=useState<"git"|"files">("git"),[repository,setRepository]=useState("");
  const [folder,setFolder]=useState<{id:string;name:string}>(),[parent,setParent]=useState(""),[cursor,setCursor]=useState("");
  const [folders,setFolders]=useState<AgentFolders>(),[busy,setBusy]=useState(false),[error,setError]=useState(""),[folderError,setFolderError]=useState(""),[attempt,setAttempt]=useState(0);
  const id=useRef(crypto.randomUUID()),controller=useRef(new AbortController());
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{
    if(kind!=="files")return;
    const abort=new AbortController();setFolders(undefined);setFolderError("");
    void request<AgentFolders>(`${base}/folders?parentId=${encodeURIComponent(parent)}&cursor=${encodeURIComponent(cursor)}`,{signal:abort.signal}).then(setFolders).catch(e=>{if(!abort.signal.aborted)setFolderError(failure(e));});
    return()=>abort.abort();
  },[kind,parent,cursor,attempt]);
  async function create(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{const project=await request<AgentProject>(`${base}/projects`,{method:"POST",signal:controller.current.signal,body:JSON.stringify({id:id.current,name,sourceType:kind,sourceRef:kind==="git"?repository:folder?.id})},session.csrfToken);if(!controller.current.signal.aborted)onCreated(project);}
    catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  function browse(value:string){setParent(value);setCursor("");}
  return <EditorPane title="New project" onClose={onClose} dirty={!!name||!!repository||!!folder}><form className="agent-project-form" onSubmit={event=>{void create(event);}}>
    <label>Project name<input required maxLength={80} value={name} onChange={e=>setName(e.target.value)}/></label>
    <label>Project source<Select value={kind} onChange={e=>setKind(e.target.value as "git"|"files")}><option value="git">Git repository</option><option value="files">Dar Files folder</option></Select></label>
    {kind==="git"?<><label>Repository URL<input required type="text" autoCapitalize="none" spellCheck={false} placeholder="https://host.example/you/project.git" value={repository} maxLength={2048} onChange={e=>setRepository(e.target.value)}/></label><p className="muted small">Use a credential-free HTTPS URL on an administrator-enabled host. SSH project sources can be saved; SSH authentication is not connected yet.</p></>:<section className="agent-folder-picker" aria-label="Choose a Dar folder">
      <div className="agent-folder-heading"><strong>{folders?.folder?.name??"Dar Files"}</strong>{parent&&<button type="button" className="text-button" disabled={!folders} onClick={()=>browse(folders?.folder?.parentId??"")}>Up one folder</button>}</div>
      {folderError?<><p role="alert">{folderError}</p><button type="button" className="secondary-button" onClick={()=>setAttempt(v=>v+1)}>Retry folders</button></>:!folders?<p role="status">Loading folders…</p>:<>
        {folders.folder&&<button type="button" className="secondary-button" onClick={()=>{setFolder({id:folders.folder!.id,name:folders.folder!.name});if(!name)setName(folders.folder!.name);}}>Choose this folder</button>}
        {!folders.items.length&&<p className="muted small">No folders in this page. Create a project folder in Files if needed.</p>}
        <ul>{folders.items.map(row=><li key={row.id}><button type="button" onClick={()=>browse(row.id)}><Icon name="folder"/>{row.name}<Icon name="chevron"/></button></li>)}</ul>
        {folders.nextCursor&&<button type="button" className="text-button" onClick={()=>setCursor(folders.nextCursor!)}>Next page</button>}
      </>}
      {folder&&<p className="agent-folder-selection" role="status">Selected: {folder.name}</p>}
      <p className="muted small">Runs use an isolated copy of this folder. Review text edits before applying them through Files’ revision checks.</p>
    </section>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="secondary-button" onClick={onClose} disabled={busy}>Cancel</button><button className="primary-button" disabled={busy||kind==="files"&&!folder}>{busy?"Creating…":"Create project"}</button></div>
  </form></EditorPane>;
}

function ThreadEditor({project,session,onCreated,onClose}:{project:AgentProject;session:BrowserSession;onCreated:(thread:AgentThread)=>void;onClose:()=>void}){
  const [title,setTitle]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const id=useRef(crypto.randomUUID()),controller=useRef(new AbortController());useEffect(()=>()=>controller.current.abort(),[]);
  return <EditorPane title="New thread" onClose={onClose} dirty={!!title}><form className="agent-project-form" onSubmit={event=>{
    event.preventDefault();setBusy(true);setError("");
    void request<AgentThread>(`${base}/threads`,{method:"POST",signal:controller.current.signal,body:JSON.stringify({id:id.current,projectId:project.id,title})},session.csrfToken).then(row=>{if(!controller.current.signal.aborted)onCreated(row);}).catch(e=>{if(!controller.current.signal.aborted)setError(failure(e));}).finally(()=>{if(!controller.current.signal.aborted)setBusy(false);});
  }}><p className="muted">{project.name}</p><label>Thread title<input required maxLength={100} value={title} onChange={e=>setTitle(e.target.value)}/></label>{error&&<p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button className="primary-button" disabled={busy}>{busy?"Creating…":"Create thread"}</button></div></form></EditorPane>;
}

/** Draft and operation IDs survive reload; uncertain appends can be retried safely. */
function Conversation({thread,session,onChanged,draftCache,provider,run,onRun,historyEpoch}:{thread:AgentThread;session:BrowserSession;onChanged:(thread:AgentThread)=>void;draftCache:Map<string,string>;provider?:AgentProvider;run?:AgentRun;onRun:(id:string)=>void;historyEpoch:number}){
  const key=`dar.agent.draft.${session.user.id}.${thread.id}`;
  const [draft,setDraft]=useState(()=>{if(draftCache.has(key))return draftCache.get(key)!;try{return sessionStorage.getItem(key)??"";}catch{return "";}});
  const [history,setHistory]=useState<AgentHistory>(),[error,setError]=useState(""),[storageError,setStorageError]=useState(""),[busy,setBusy]=useState(false),[notice,setNotice]=useState(""),[reload,setReload]=useState(0),[loadingEarlier,setLoadingEarlier]=useState(false);
  const controller=useRef(new AbortController()),composer=useRef<HTMLTextAreaElement>(null),pending=useRef<{id:string;content:string}|undefined>(undefined),lastMessage=useRef<HTMLElement>(null),draftRef=useRef(draft);
  const operationKey=key+".operation";
  const [model,setModel]=useState(""),running=run?activeAgentRun(run.status):!!history?.runs.some(item=>activeAgentRun(item.status));
  useEffect(()=>{if(provider?.models.length)setModel(current=>provider.models.some(m=>m.id===current)?current:(provider.models.find(m=>m.isDefault)??provider.models[0]).id);},[provider]);
  draftRef.current=draft;
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{
    const abort=new AbortController();setError("");setHistory(undefined);
    void request<AgentHistory>(`${base}/threads/${thread.id}/messages`,{signal:abort.signal}).then(value=>{setHistory(value);onChanged(value.thread);if(value.runs[0])onRun(value.runs[0].id);}).catch(e=>{if(!abort.signal.aborted)setError(failure(e));});
    return()=>abort.abort();
    // onChanged updates the thread list; it does not change this history's identity.
  },[thread.id,reload,historyEpoch]);
  useEffect(()=>{
    draftCache.set(key,draft);
    try{if(draft)sessionStorage.setItem(key,draft);else sessionStorage.removeItem(key);setStorageError("");}
    catch{setStorageError("Draft recovery is unavailable in this browser. Save your message before leaving.");}
  },[draft,key]);
  useEffect(()=>{
    const protect=(event:BeforeUnloadEvent)=>{if(storageError&&draftRef.current)event.preventDefault();};window.addEventListener("beforeunload",protect);return()=>window.removeEventListener("beforeunload",protect);
  },[storageError]);
  async function save(execute=false){
    if(!draft.trim()||!history||busy||running)return;
    if(new TextEncoder().encode(draft).length>16384){setError("Keep each message within 16 KiB.");return;}
    setBusy(true);setError("");setNotice("");
    if(pending.current?.content!==draft){
      try{const saved=JSON.parse(sessionStorage.getItem(operationKey)??"null");pending.current=saved?.content===draft&&typeof saved.id==="string"?saved:undefined;}catch{}
      pending.current??={id:crypto.randomUUID(),content:draft};
      if(pending.current.content!==draft)pending.current={id:crypto.randomUUID(),content:draft};
    }
    const operation=pending.current!;
    try{sessionStorage.setItem(operationKey,JSON.stringify(operation));}catch{}
    try{
      if(execute){
        const runKey=key+".run-operation";let requestId={id:crypto.randomUUID(),messageId:operation.id,content:operation.content,model};
        try{const saved=JSON.parse(sessionStorage.getItem(runKey)??"null");if(saved?.content===draft&&saved.model===model&&typeof saved.id==="string"&&typeof saved.messageId==="string")requestId=saved;sessionStorage.setItem(runKey,JSON.stringify(requestId));}catch{}
        const value=await request<AgentRun>(`${base}/threads/${thread.id}/runs`,{method:"POST",signal:controller.current.signal,headers:{"if-match":`"${history.thread.revision}"`},body:JSON.stringify(requestId)},session.csrfToken);
        if(controller.current.signal.aborted)return;
        onRun(value.id);setReload(n=>n+1);setDraft("");draftCache.set(key,"");pending.current=undefined;
        try{sessionStorage.removeItem(operationKey);sessionStorage.removeItem(runKey);sessionStorage.removeItem(key);}catch{}
        setNotice("Task sent to Codex.");return;
      }
      const value=await request<{thread:AgentThread;message:AgentMessage}>(`${base}/threads/${thread.id}/messages`,{method:"POST",signal:controller.current.signal,headers:{"if-match":`"${history.thread.revision}"`},body:JSON.stringify(operation)},session.csrfToken);
      if(controller.current.signal.aborted)return;
      setHistory(current=>current?{...current,thread:value.thread,items:mergeMessages(current.items,[value.message])}:current);onChanged(value.thread);setDraft("");draftCache.set(key,"");pending.current=undefined;try{sessionStorage.removeItem(operationKey);sessionStorage.removeItem(key);}catch{}setNotice("Message saved. It has not been sent to a model.");composer.current?.focus();
      requestAnimationFrame(()=>lastMessage.current?.scrollIntoView({block:"nearest"}));
    }catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  async function earlier(){
    if(!history?.nextBefore||loadingEarlier)return;setLoadingEarlier(true);setError("");
    try{const value=await request<AgentHistory>(`${base}/threads/${thread.id}/messages?before=${history.nextBefore}`,{signal:controller.current.signal});if(!controller.current.signal.aborted)setHistory(current=>current?{...current,items:mergeMessages(current.items,value.items),nextBefore:value.nextBefore}:current);}
    catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setLoadingEarlier(false);}
  }
  return <>
    <div className="agent-conversation" aria-label="Conversation messages" aria-busy={!history&&!error}>
      {!history&&!error&&<p role="status">Loading conversation…</p>}
      {history?.nextBefore&&<button className="text-button" disabled={loadingEarlier} onClick={()=>{void earlier();}}>{loadingEarlier?"Loading…":"Load earlier messages"}</button>}
      {history&&!history.items.length&&<div className="agent-empty"><Icon name="agent"/><h2>What would you like to work on?</h2><p>{provider?.connected?"Describe a task for Codex in this project’s isolated workspace.":"Save the task and context here while Codex is unavailable."}</p></div>}
      {history?.items.map(message=><article className={`agent-message agent-message-${message.role}`} key={message.id} ref={lastMessage}><header><strong>{message.role==="assistant"?"Codex":"You"}</strong><time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleString()}</time></header><p>{message.content}</p>{message.role==="user"&&!history.runs.some(item=>item.messageId===message.id)&&<small>Saved · not sent to a model</small>}</article>)}
      {run&&<section className="agent-run-state" aria-label="Run status"><p role="status">Codex · {run.status}{run.document.truncated?" · output is truncated":""}</p>{run.document.error&&<p role="alert" className="error">{run.document.error}</p>}{run.document.activity.length>0&&<details open={running}><summary>Tool activity</summary><ul>{run.document.activity.map((item,i)=><li key={i}>{item}</li>)}</ul></details>}{run.document.output&&(run.status==="completed"?<details><summary>Full Codex response</summary><p className="agent-live-output">{run.document.output}</p></details>:<p className="agent-live-output">{run.document.output}</p>)}</section>}
    </div>
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void save(!!provider?.connected);}}>
      {error&&<div role="alert" className="error"><p>{error}</p><button type="button" className="text-button" disabled={busy} onClick={()=>setReload(n=>n+1)}>Refresh conversation</button></div>}
      {storageError&&<p role="alert" className="error">{storageError}</p>}
      <div className="agent-composer"><label className="sr-only" htmlFor="agent-message">Message</label><textarea id="agent-message" ref={composer} value={draft} disabled={busy} placeholder="Describe a task or add context…" onChange={event=>setDraft(event.target.value)} onKeyDown={event=>{if((event.ctrlKey||event.metaKey)&&event.key==="Enter"){event.preventDefault();void save(!!provider?.connected);}}}/><div className="agent-composer-controls">{provider?.connected?<Select aria-label="Codex model" value={model} onChange={event=>setModel(event.target.value)} disabled={busy||running}>{provider.models.map(item=><option key={item.id} value={item.id}>{item.name}</option>)}</Select>:<span>{provider?.configured?"Codex unavailable":"No provider connected"}</span>}{provider?.connected&&<button type="button" className="text-button" disabled={busy||running||!history||!draft.trim()} onClick={()=>{void save();}}>Save only</button>}<button className="primary-button" disabled={busy||running||!history||!draft.trim()||!!provider?.connected&&!model}>{busy?"Saving…":provider?.connected?"Run with Codex":"Save message"}</button></div></div>
      {provider?.connected&&<p className="agent-context-notice">Running sends this conversation and selected project contents to Codex. Commands have no network access; publishing is disabled.</p>}
      <div className="agent-composer-hint" role="status">{notice||"Draft kept in this tab · Ctrl / Cmd Enter to save"}</div>
    </form>
  </>;
}

function ChangeReview({run,session}:{run?:AgentRun;session:BrowserSession}){
  const [selected,setSelected]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState(""),[applied,setApplied]=useState<string[]>([]),[download,setDownload]=useState("");
  const controller=useRef(new AbortController());useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{setApplied([]);setSelected("");setError("");},[run?.id]);
  useEffect(()=>{if(!run?.document.changes.length){setDownload("");return;}const url=URL.createObjectURL(new Blob([run.document.changes.map(change=>change.diff).join("\n\n")],{type:"text/plain"}));setDownload(url);return()=>URL.revokeObjectURL(url);},[run?.document.changes]);
  const changes=run?.document.changes??[],change=changes.find(item=>item.path===selected)??changes[0];
  async function apply(change:AgentChange){
    if(change.content===undefined||!change.fileId||!change.expectedRevision)return;setBusy(true);setError("");
    try{
      const file=new File([change.content],change.path.split("/").at(-1)!,{type:change.mediaType??"text/plain"});
      const reservation=await request<{id:string}>("/api/v1/files/uploads",{method:"POST",signal:controller.current.signal,body:JSON.stringify({fileId:change.fileId,expectedRevision:change.expectedRevision,name:file.name,size:file.size,mediaType:file.type})},session.csrfToken);
      await request(`/api/v1/files/uploads/${reservation.id}`,{method:"PUT",signal:controller.current.signal,body:file},session.csrfToken);if(!controller.current.signal.aborted)setApplied(current=>[...current,change.path]);
    }catch(e){if(!controller.current.signal.aborted)setError(e instanceof RequestError&&e.status===409?"The file changed since this workspace was created. Your proposed edit is retained; open Files to resolve the conflict.":failure(e));}
    finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  return <>{!changes.length?<><Icon name="file"/><h3>No changes yet</h3><p>Changes from agent runs appear here for review.</p></>:<><nav aria-label="Changed files" className="agent-change-list">{changes.map(item=><button key={item.path} className={change?.path===item.path?"selected":""} aria-current={change?.path===item.path?"page":undefined} onClick={()=>setSelected(item.path)}><Icon name="file"/><span>{item.path}</span><small>{item.kind}</small></button>)}</nav>{change&&<><h3>{change.path}</h3><pre className="agent-diff">{change.diff}</pre>{change.content!==undefined&&change.fileId&&run?.status==="completed"&&<button className="primary-button" disabled={busy||applied.includes(change.path)} onClick={()=>{void apply(change);}}>{busy?"Applying…":applied.includes(change.path)?"Applied to Files":"Apply this edit to Files"}</button>}</>}{download&&<a href={download} download="agent-review.txt">Download review</a>}<p className="small muted">Edits stay in this thread’s working copy until applied. New files, deletions and Git publishing are not applied automatically.</p></>}{error&&<p role="alert" className="error">{error}</p>}</>;
}

export function AgentWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const initial=useRef(new URLSearchParams(location.search)).current;
  const [projects,setProjects]=useState<AgentProject[]>(),[projectId,setProjectId]=useState(initial.get("project")??""),[threads,setThreads]=useState<AgentThread[]>(),[threadId,setThreadId]=useState(initial.get("thread")??"");
  const [error,setError]=useState(""),[threadError,setThreadError]=useState(""),[reload,setReload]=useState(0),[view,setView]=useState<View>("conversation"),[editor,setEditor]=useState<"project"|"thread">(),[settings,setSettings]=useState(false),[review,setReview]=useState(true),[nav,setNav]=useState(true);
  const [provider,setProvider]=useState<AgentProvider>(),[providerEpoch,setProviderEpoch]=useState(0),[runId,setRunId]=useState(""),[run,setRun]=useState<AgentRun>(),[runError,setRunError]=useState(""),[historyEpoch,setHistoryEpoch]=useState(0);
  const completedSeen=useRef(new Set<string>());
  const panel=useRef<HTMLDivElement>(null),tabs=useRef<HTMLElement>(null),previousView=useRef(view),returnToTabs=useRef(false),draftCache=useRef(new Map<string,string>());
  const project=projects?.find(p=>p.id===projectId),thread=threads?.find(t=>t.id===threadId);
  useEffect(()=>{const controller=new AbortController();void request<AgentProvider>(`${base}/provider`,{signal:controller.signal}).then(setProvider).catch(e=>{if(!controller.signal.aborted)setProvider({configured:false,connected:false,models:[],message:failure(e)});});return()=>controller.abort();},[providerEpoch]);
  useEffect(()=>{setRunId("");setRun(undefined);setRunError("");},[threadId]);
  useEffect(()=>{
    if(!runId)return;const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    async function refresh(){try{
      const value=await request<AgentRun>(`${base}/runs/${runId}`,{signal:controller.signal});if(controller.signal.aborted)return;
      if(value.threadId!==threadId)return;setRun(value);setRunError("");
      if(activeAgentRun(value.status))timer=setTimeout(()=>{void refresh();},1000);
      else if(!completedSeen.current.has(value.id)){completedSeen.current.add(value.id);setHistoryEpoch(n=>n+1);}
    }catch(e){if(!controller.signal.aborted){setRunError(failure(e));timer=setTimeout(()=>{void refresh();},3000);}}}
    void refresh();return()=>{controller.abort();clearTimeout(timer);};
  },[runId,threadId]);
  useEffect(()=>{
    const controller=new AbortController();setError("");
    void request<{items:AgentProject[]}>(`${base}/projects`,{signal:controller.signal}).then(value=>{setProjects(value.items);setProjectId(current=>value.items.some(p=>p.id===current)?current:value.items[0]?.id??"");}).catch(e=>{if(!controller.signal.aborted)setError(failure(e));});return()=>controller.abort();
  },[reload]);
  useEffect(()=>{
    setThreads(undefined);setThreadError("");if(!projectId){setThreads([]);return;}
    const controller=new AbortController();void request<{items:AgentThread[]}>(`${base}/threads?projectId=${projectId}`,{signal:controller.signal}).then(value=>{setThreads(value.items);setThreadId(current=>value.items.some(t=>t.id===current)?current:value.items[0]?.id??"");}).catch(e=>{if(!controller.signal.aborted)setThreadError(failure(e));});return()=>controller.abort();
  },[projectId,reload]);
  useEffect(()=>{
    const url=new URL(location.href);if(projectId)url.searchParams.set("project",projectId);else url.searchParams.delete("project");if(threadId)url.searchParams.set("thread",threadId);else url.searchParams.delete("thread");history.replaceState(null,"",url);
  },[projectId,threadId]);
  useEffect(()=>{
    if(previousView.current!==view&&matchMedia("(max-width:800px)").matches){
      if(returnToTabs.current)tabs.current?.querySelector<HTMLButtonElement>("button[aria-current=page]")?.focus({preventScroll:true});
      else panel.current?.querySelector<HTMLElement>(`[data-agent-panel=${view}]`)?.focus({preventScroll:true});
    }
    returnToTabs.current=false;previousView.current=view;
  },[view]);
  const changed=(row:AgentThread)=>setThreads(current=>current?.map(t=>t.id===row.id?row:t));
  return <div className={`app-shell agent-shell agent-view-${view}${review?"":" agent-review-hidden"}${nav?"":" agent-nav-hidden"}`}>
    <WorkspaceHeader app="agent" session={session} onSignedOut={onSignedOut} query="" onQuery={()=>{}}/>
    <nav ref={tabs} className="agent-mobile-views" aria-label="Agent views">{(["projects","conversation","changes","terminal"] as const).map(value=><button key={value} aria-current={view===value?"page":undefined} onClick={()=>setView(value)}>{value[0].toUpperCase()+value.slice(1)}</button>)}</nav>
    <div className="agent-layout" ref={panel} onKeyDown={event=>{if(event.key==="Escape"&&view!=="conversation"&&!editor&&!settings){event.preventDefault();returnToTabs.current=true;setView("conversation");}}}>
      <aside className="agent-sidebar" data-agent-panel="projects" tabIndex={-1} aria-label="Projects and threads"><h1>Agent</h1><button className="primary-button ui-new" disabled={!project||!threads} onClick={()=>setEditor("thread")}><Icon name="plus"/>New thread</button><div className="agent-sidebar-heading"><h2>Projects</h2><button className="icon-button" aria-label="New project" disabled={!projects} onClick={()=>setEditor("project")}><Icon name="plus"/></button></div>
        {error?<div role="alert"><p>{error}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry projects</button></div>:!projects?<p role="status">Loading projects…</p>:!projects.length?<div className="agent-sidebar-empty"><p>No projects yet.</p><button className="text-button" onClick={()=>setEditor("project")}>Create a project</button></div>:<nav aria-label="Projects">{projects.map(row=><div key={row.id}><button className="agent-project-row" aria-expanded={row.id===projectId} onClick={()=>{setProjectId(row.id);if(row.id!==projectId)setThreadId("");}}><Icon name={row.sourceType==="files"?"folder":"share"}/><span>{row.name}</span></button>{row.id===projectId&&<div className="agent-thread-list">{threadError?<div role="alert"><p>{threadError}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry threads</button></div>:!threads?<p role="status">Loading threads…</p>:!threads.length?<p className="muted small">No threads yet.</p>:threads.map(item=><button key={item.id} className={item.id===threadId?"selected":""} aria-current={item.id===threadId?"page":undefined} onClick={()=>{setThreadId(item.id);setView("conversation");}}><Icon name="agent"/><span>{item.title}<small>{new Date(item.modifiedAt).toLocaleDateString()}</small></span></button>)}</div>}</div>)}</nav>}
        <button className="agent-settings-entry" onClick={()=>setSettings(true)}><Icon name="settings"/>Agent settings</button>
      </aside>
      <main className="agent-center" data-agent-panel="conversation" tabIndex={-1} aria-label="Agent conversation"><header className="agent-thread-heading"><button className="icon-button agent-desktop-toggle" aria-label={nav?"Hide projects":"Show projects"} aria-expanded={nav} onClick={()=>setNav(v=>!v)}><Icon name="menu"/></button><h2>{thread?.title??project?.name??"Agent"}</h2>{run&&activeAgentRun(run.status)&&<button className="secondary-button" disabled={run.status==="cancelling"} onClick={()=>{void request<AgentRun>(`${base}/runs/${run.id}/cancel`,{method:"POST"},session.csrfToken).then(setRun).catch(e=>setRunError(failure(e)));}}>{run.status==="cancelling"?"Stopping…":"Stop"}</button>}<button className="icon-button agent-desktop-toggle" aria-label={review?"Hide changes":"Show changes"} aria-expanded={review} onClick={()=>setReview(v=>!v)}><Icon name="file"/></button></header>
        {runError&&<p role="alert" className="error">{runError}</p>}
        {thread?<Conversation key={thread.id} thread={thread} session={session} onChanged={changed} draftCache={draftCache.current} provider={provider} run={run} onRun={setRunId} historyEpoch={historyEpoch}/>:<div className="agent-empty"><Icon name="agent"/><h2>{project?"Start a thread":"Choose a project"}</h2><p>{project?"Keep tasks and conversations together in this project.":"Use a Git repository or a folder in Dar Files."}</p><button className="primary-button" disabled={!projects||!!project&&!threads} onClick={()=>setEditor(project?"thread":"project")}>{project?"New thread":"New project"}</button></div>}
        <section className="agent-terminal" data-agent-panel="terminal" tabIndex={-1} aria-label="Terminal output"><details open={view==="terminal"||undefined}><summary>Terminal</summary>{run?.document.terminal?<pre>{run.document.terminal}</pre>:<p>No command output yet.</p>}</details></section>
      </main>
      <aside className="agent-review" data-agent-panel="changes" tabIndex={-1} aria-label="Changes"><h2>Changes</h2><div className="agent-review-content"><ChangeReview key={run?.id??"empty"} run={run} session={session}/>{project&&<section className="agent-project-source"><h3>Project source</h3><p>{project.sourceType==="git"?"Git repository":"Dar Files folder"}</p>{project.sourceType==="git"?<code>{project.sourceRef}</code>:<a href={`/?folder=${project.sourceRef}`}>Open folder in Files</a>}<p className="muted small">{project.sourceType==="git"?"Each thread keeps a private working copy.":"Files stay in Dar’s versioned storage."}</p></section>}</div></aside>
    </div>
    {editor==="project"&&<ProjectEditor session={session} onClose={()=>setEditor(undefined)} onCreated={row=>{setProjects(current=>[...current??[],row]);setProjectId(row.id);setThreadId("");setEditor(undefined);setView("conversation");}}/>}
    {editor==="thread"&&project&&<ThreadEditor project={project} session={session} onClose={()=>setEditor(undefined)} onCreated={row=>{setProjectId(row.projectId);setThreads(current=>[row,...(current??[]).filter(item=>item.projectId===row.projectId)]);setThreadId(row.id);setEditor(undefined);setView("conversation");}}/>}
    {settings&&<Dialog title="Agent settings" onClose={()=>setSettings(false)}><div className="ui-settings-dialog"><section><h3>Codex connection</h3><p role="status">{provider?.message??"Checking Codex…"}</p><button className="secondary-button" onClick={()=>setProviderEpoch(n=>n+1)}>Refresh connection</button><p className="muted small">Codex uses the existing ChatGPT login on the server. The administrator binds it to this Dar account; login tokens never enter the browser.</p><p className="muted small">Dar account ID: <code className="agent-settings-account">{session.user.id}</code></p><h3>Keyboard shortcuts</h3><dl className="ui-shortcuts"><dt>Ctrl / Cmd Enter</dt><dd>Run with Codex when connected; otherwise save the message</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps outside editors</dd><dt>Escape</dt><dd>Return from a phone panel or close settings</dd></dl></section></div></Dialog>}
  </div>;
}
