import {useEffect,useRef,useState,type FormEvent} from "react";
import {request,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceHeader} from "../shell.tsx";
import {Icon} from "../icons.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {Select} from "../controls.tsx";
import {AppSettingsButton,usePreferences,savePreferences} from "../app-settings.tsx";
import {DarFilePicker,type DarFileSelection} from "../file-picker.tsx";
import {activeAgentRun,type AgentProject,type AgentThread,type AgentMessage,type AgentHistory,type AgentRun,type AgentProvider,type AgentChange} from "../../shared/agent.ts";
import "./workspace.css";

const base="/api/v1/agent";
type View="threads"|"conversation"|"changes"|"terminal";
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
  const [kind,setKind]=useState<"git"|"files">("files"),[repository,setRepository]=useState("");
  const [folder,setFolder]=useState<DarFileSelection>(),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const name=kind==="files"?folder?.name??"":repository.trim().replace(/\/+$/,"").split(/[/:]/).at(-1)?.replace(/\.git$/,"")??"";
  const id=useRef(crypto.randomUUID()),controller=useRef(new AbortController());
  useEffect(()=>()=>controller.current.abort(),[]);
  async function create(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{const project=await request<AgentProject>(`${base}/projects`,{method:"POST",signal:controller.current.signal,body:JSON.stringify({id:id.current,name,sourceType:kind,sourceRef:kind==="git"?repository:folder?.id})},session.csrfToken);if(!controller.current.signal.aborted)onCreated(project);}
    catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  return <EditorPane title="Add project" onClose={onClose} dirty={!!repository}><form className="agent-project-form" onSubmit={event=>{void create(event);}}>
    <label>Project source<Select value={kind} onChange={e=>setKind(e.target.value as "git"|"files")}><option value="git">Git repository</option><option value="files">Dar Files folder</option></Select></label>
    {kind==="git"?<><label>Repository URL<input required type="url" autoCapitalize="none" spellCheck={false} placeholder="https://github.com/you/project.git" value={repository} maxLength={2048} disabled={busy} onChange={e=>setRepository(e.target.value)}/></label><p className="muted small">Use a public HTTPS repository on an administrator-enabled host.</p></>:<DarFilePicker mode="folder" disabled={busy} onChange={setFolder}/>}
    {name&&<p className="muted small">Project: <strong>{name}</strong></p>}
    {error&&<p role="alert" className="error">{error}</p>}
    <div className="dialog-actions"><button type="button" className="secondary-button" onClick={onClose} disabled={busy}>Cancel</button><button className="primary-button" disabled={busy||!name||kind==="files"&&!folder}>{busy?"Adding…":"Add project"}</button></div>
  </form></EditorPane>;
}

function ThreadChooser({projects,busy,error,onChoose,onAdd,onClose}:{projects:AgentProject[];busy:boolean;error:string;onChoose:(project:AgentProject)=>void;onAdd:()=>void;onClose:()=>void}){
  return <EditorPane title="New thread" onClose={onClose}><div className="agent-project-chooser"><h3>Choose a project</h3>{projects.map(project=><button key={project.id} className="agent-project-choice" disabled={busy} onClick={()=>onChoose(project)}><Icon name={project.sourceType==="files"?"folder":"share"}/><span><strong>{project.name}</strong><small>{project.sourceType==="files"?"Dar Files":"Git repository"}</small></span><Icon name="chevron"/></button>)}<button className="text-button" disabled={busy} onClick={onAdd}><Icon name="plus"/>Add project</button>{busy&&<p role="status">Opening thread…</p>}{error&&<p className="error" role="alert">{error}</p>}</div></EditorPane>;
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
  useEffect(()=>{composer.current?.focus({preventScroll:true});},[]);
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
  const [error,setError]=useState(""),[threadError,setThreadError]=useState(""),[reload,setReload]=useState(0),[view,setView]=useState<View>("conversation"),[editor,setEditor]=useState<"project"|"thread">(),[settings,setSettings]=useState(false),[settingsSection,setSettingsSection]=useState("General"),[review,setReview]=useState(true),[nav,setNav]=useState(true);
  const [provider,setProvider]=useState<AgentProvider>(),[providerEpoch,setProviderEpoch]=useState(0),[checkingProvider,setCheckingProvider]=useState(true),[connectionError,setConnectionError]=useState(""),[runId,setRunId]=useState(""),[run,setRun]=useState<AgentRun>(),[runError,setRunError]=useState(""),[historyEpoch,setHistoryEpoch]=useState(0),[creatingThread,setCreatingThread]=useState(false),[createError,setCreateError]=useState(""),[settling,setSettling]=useState(false);
  const {preferences,loaded:preferencesLoaded,error:preferencesError}=usePreferences();
  const completedSeen=useRef(new Set<string>()),newThreadRequest=useRef<{id:string;projectId:string}|undefined>(undefined),lifetime=useRef(new AbortController());
  const panel=useRef<HTMLDivElement>(null),tabs=useRef<HTMLElement>(null),previousView=useRef(view),returnToTabs=useRef(false),draftCache=useRef(new Map<string,string>());
  const thread=threads?.find(t=>t.id===threadId),project=projects?.find(p=>p.id===(thread?.projectId??projectId));
  useEffect(()=>()=>lifetime.current.abort(),[]);
  useEffect(()=>{
    const controller=new AbortController();setCheckingProvider(true);setConnectionError("");
    void request<AgentProvider>(`${base}/provider`,{signal:controller.signal}).then(value=>{if(!controller.signal.aborted)setProvider(value);}).catch(e=>{if(!controller.signal.aborted)setConnectionError(failure(e));}).finally(()=>{if(!controller.signal.aborted)setCheckingProvider(false);});
    return()=>controller.abort();
  },[providerEpoch]);
  async function connectProvider(){
    setCheckingProvider(true);setConnectionError("");
    try{const value=await request<AgentProvider>(`${base}/provider/connect`,{method:"POST",signal:lifetime.current.signal},session.csrfToken);if(!lifetime.current.signal.aborted)setProvider(value);}
    catch(e){if(!lifetime.current.signal.aborted)setConnectionError(e instanceof Error?e.message:"Codex could not be connected.");}
    finally{if(!lifetime.current.signal.aborted)setCheckingProvider(false);}
  }
  useEffect(()=>{setRunId("");setRun(undefined);setRunError("");},[threadId]);
  useEffect(()=>{
    if(!runId)return;const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    async function refresh(){try{
      const value=await request<AgentRun>(`${base}/runs/${runId}`,{signal:controller.signal});if(controller.signal.aborted)return;
      if(value.threadId!==threadId)return;setRun(value);setRunError("");
      setThreads(current=>current?.map(item=>item.id===threadId?{...item,runStatus:value.status}:item));
      if(activeAgentRun(value.status))timer=setTimeout(()=>{void refresh();},1000);
      else if(!completedSeen.current.has(value.id)){completedSeen.current.add(value.id);setHistoryEpoch(n=>n+1);setReload(n=>n+1);}
    }catch(e){if(!controller.signal.aborted){setRunError(failure(e));timer=setTimeout(()=>{void refresh();},3000);}}}
    void refresh();return()=>{controller.abort();clearTimeout(timer);};
  },[runId,threadId]);
  useEffect(()=>{
    const controller=new AbortController();setError("");
    void request<{items:AgentProject[]}>(`${base}/projects`,{signal:controller.signal}).then(value=>{if(!controller.signal.aborted)setProjects(value.items);}).catch(e=>{if(!controller.signal.aborted)setError(failure(e));});
    return()=>controller.abort();
  },[reload]);
  useEffect(()=>{
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    async function refresh(){try{
      const value=await request<{items:AgentThread[]}>(`${base}/threads`,{signal:controller.signal});if(controller.signal.aborted)return;
      setThreads(value.items);setThreadError("");setThreadId(current=>value.items.some(t=>t.id===current)?current:"");
      timer=setTimeout(()=>{void refresh();},value.items.some(item=>activeAgentRun(item.runStatus??""))?3000:15000);
    }catch(e){if(!controller.signal.aborted){setThreadError(failure(e));timer=setTimeout(()=>{void refresh();},5000);}}}
    void refresh();const focus=()=>{clearTimeout(timer);void refresh();};window.addEventListener("focus",focus);
    return()=>{controller.abort();clearTimeout(timer);window.removeEventListener("focus",focus);};
  },[reload]);
  useEffect(()=>{
    const url=new URL(location.href),selectedProject=thread?.projectId??projectId;
    if(selectedProject)url.searchParams.set("project",selectedProject);else url.searchParams.delete("project");
    if(threadId)url.searchParams.set("thread",threadId);else url.searchParams.delete("thread");history.replaceState(null,"",url);
  },[projectId,threadId,thread?.projectId]);
  useEffect(()=>{
    if(previousView.current!==view&&matchMedia("(max-width:800px)").matches){
      if(returnToTabs.current)tabs.current?.querySelector<HTMLButtonElement>("button[aria-current=page]")?.focus({preventScroll:true});
      else panel.current?.querySelector<HTMLElement>(`[data-agent-panel=${view}]`)?.focus({preventScroll:true});
    }
    returnToTabs.current=false;previousView.current=view;
  },[view]);
  const changed=(row:AgentThread)=>setThreads(current=>current?.map(t=>t.id===row.id?{...t,...row}:t));
  const isWorking=(row:AgentThread)=>activeAgentRun(row.id===threadId&&run?run.status:row.runStatus??"");
  const isSettled=(row:AgentThread)=>!isWorking(row)&&!!preferences.agentSettledThreads?.some(item=>item.id===row.id&&item.revision===row.revision);
  async function settle(row:AgentThread){
    if(!preferencesLoaded||settling||isWorking(row))return;setSettling(true);setThreadError("");
    const entries=(preferences.agentSettledThreads??[]).filter(item=>item.id!==row.id);
    if(!isSettled(row))entries.push({id:row.id,revision:row.revision});
    try{await savePreferences({...preferences,agentSettledThreads:entries},session);}
    catch(e){setThreadError(failure(e));}finally{setSettling(false);}
  }
  function chooseNewThread(){newThreadRequest.current=undefined;setCreateError("");setEditor("thread");}
  async function createThread(destination:AgentProject){
    if(creatingThread)return;setCreatingThread(true);setCreateError("");
    if(newThreadRequest.current?.projectId!==destination.id)newThreadRequest.current={id:crypto.randomUUID(),projectId:destination.id};
    try{
      const row=await request<AgentThread>(`${base}/threads`,{method:"POST",signal:lifetime.current.signal,body:JSON.stringify(newThreadRequest.current)},session.csrfToken);
      if(lifetime.current.signal.aborted)return;
      setThreads(current=>[row,...(current??[]).filter(item=>item.id!==row.id)]);setProjectId(row.projectId);setThreadId(row.id);setEditor(undefined);setView("conversation");newThreadRequest.current=undefined;
    }catch(e){if(!lifetime.current.signal.aborted){setCreateError(failure(e));setEditor("thread");}}
    finally{if(!lifetime.current.signal.aborted)setCreatingThread(false);}
  }
  function rows(items:AgentThread[]){return <div className="agent-thread-list">{items.map(item=><button key={item.id} className={item.id===threadId?"selected":""} aria-current={item.id===threadId?"page":undefined} onClick={()=>{setThreadId(item.id);setProjectId(item.projectId);setView("conversation");}}><Icon name="agent"/><span>{item.title}<small>{projects?.find(p=>p.id===item.projectId)?.name??"Project"}{isWorking(item)?" · Working":""}</small></span></button>)}</div>;}
  const active=threads?.filter(row=>!isWorking(row)&&!isSettled(row))??[],working=threads?.filter(isWorking)??[],settled=threads?.filter(isSettled)??[];
  return <div className={`app-shell agent-shell agent-view-${view}${review?"":" agent-review-hidden"}${nav?"":" agent-nav-hidden"}`}>
    <WorkspaceHeader app="agent" session={session} onSignedOut={onSignedOut} query="" onQuery={()=>{}}/>
    <nav ref={tabs} className="agent-mobile-views" aria-label="Agent views">{(["threads","conversation","changes","terminal"] as const).map(value=><button key={value} aria-current={view===value?"page":undefined} onClick={()=>setView(value)}>{value[0].toUpperCase()+value.slice(1)}</button>)}</nav>
    <div className="agent-layout" ref={panel} onKeyDown={event=>{if(event.key==="Escape"&&view!=="conversation"&&!editor&&!settings){event.preventDefault();returnToTabs.current=true;setView("conversation");}}}>
      <aside className="agent-sidebar" data-agent-panel="threads" tabIndex={-1} aria-label="Threads"><h1 className="sr-only">Agent</h1><button className="primary-button ui-new" disabled={!projects} onClick={chooseNewThread}><Icon name="plus"/>New thread</button>
        {error&&<div role="alert"><p>{error}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry projects</button></div>}
        {threadError&&<div role="alert"><p>{threadError}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry threads</button></div>}
        {preferencesError&&<p role="alert" className="error">{preferencesError}</p>}
        <div className="agent-thread-shelves">{!threads?<p role="status">Loading threads…</p>:<>{rows(active)}{!threads.length&&<p className="muted small">Start a thread to work on a task.</p>}<details open><summary>Working <span>{working.length}</span></summary>{working.length?rows(working):<p className="muted small">No threads running.</p>}</details><details><summary>Settled <span>{settled.length}</span></summary>{settled.length?rows(settled):<p className="muted small">No settled threads.</p>}</details></>}</div>
        <div className="agent-sidebar-footer"><AppSettingsButton app="agent" onClick={()=>setSettings(true)}/></div>
      </aside>
      <main className="agent-center" data-agent-panel="conversation" tabIndex={-1} aria-label="Agent conversation"><header className="agent-thread-heading"><button className="icon-button agent-desktop-toggle" aria-label={nav?"Hide threads":"Show threads"} aria-expanded={nav} onClick={()=>setNav(v=>!v)}><Icon name="menu"/></button><h2>{thread?.title??"Agent"}</h2>{thread&&!isWorking(thread)&&<button className="text-button" disabled={settling||!preferencesLoaded} onClick={()=>{void settle(thread);}}>{isSettled(thread)?"Restore thread":"Settle thread"}</button>}{run&&activeAgentRun(run.status)&&<button className="secondary-button" disabled={run.status==="cancelling"} onClick={()=>{void request<AgentRun>(`${base}/runs/${run.id}/cancel`,{method:"POST"},session.csrfToken).then(setRun).catch(e=>setRunError(failure(e)));}}>{run.status==="cancelling"?"Stopping…":"Stop"}</button>}<button className="icon-button agent-desktop-toggle" aria-label={review?"Hide changes":"Show changes"} aria-expanded={review} onClick={()=>setReview(v=>!v)}><Icon name="file"/></button></header>
        {runError&&<p role="alert" className="error">{runError}</p>}
        {thread?<Conversation key={thread.id} thread={thread} session={session} onChanged={changed} draftCache={draftCache.current} provider={provider} run={run} onRun={setRunId} historyEpoch={historyEpoch}/>:<div className="agent-empty"><Icon name="agent"/><h2>What would you like to work on?</h2><p>Start a thread in a Git repository or a folder in Dar Files.</p><button className="primary-button" disabled={!projects} onClick={chooseNewThread}>New thread</button></div>}
        <section className="agent-terminal" data-agent-panel="terminal" tabIndex={-1} aria-label="Terminal output"><details open={view==="terminal"||undefined}><summary>Terminal</summary>{run?.document.terminal?<pre>{run.document.terminal}</pre>:<p>No command output yet.</p>}</details></section>
      </main>
      <aside className="agent-review" data-agent-panel="changes" tabIndex={-1} aria-label="Changes"><h2>Changes</h2><div className="agent-review-content"><ChangeReview key={run?.id??"empty"} run={run} session={session}/>{project&&<section className="agent-project-source"><h3>Project source</h3><p>{project.sourceType==="git"?"Git repository":"Dar Files folder"}</p>{project.sourceType==="git"?<code>{project.sourceRef}</code>:<a href={`/?folder=${project.sourceRef}`}>Open folder in Files</a>}</section>}</div></aside>
    </div>
    {editor==="project"&&<ProjectEditor session={session} onClose={()=>setEditor("thread")} onCreated={row=>{setProjects(current=>[...current??[],row]);void createThread(row);}}/>}
    {editor==="thread"&&projects&&<ThreadChooser projects={projects} busy={creatingThread} error={createError} onClose={()=>setEditor(undefined)} onChoose={row=>{void createThread(row);}} onAdd={()=>setEditor("project")}/>}
    {settings&&<Dialog title="Agent settings" onClose={()=>setSettings(false)}><div className="ui-settings-dialog"><nav aria-label="Settings sections">{["General","Keyboard shortcuts"].map(section=><button key={section} className={settingsSection===section?"active":""} aria-current={settingsSection===section?"page":undefined} onClick={()=>setSettingsSection(section)}>{section}</button>)}</nav><section>{settingsSection==="General"?<><h3>Codex connection</h3><p role="status">{checkingProvider?"Checking Codex…":provider?.message??"Connection could not be checked."}</p>{connectionError&&<p className="error" role="alert">{connectionError}</p>}<div className="agent-connection-actions">{session.user.administrator&&!provider?.configured&&<button className="primary-button" disabled={checkingProvider} onClick={()=>{void connectProvider();}}>Connect server Codex</button>}<button className="secondary-button" disabled={checkingProvider} onClick={()=>setProviderEpoch(n=>n+1)}>{checkingProvider?"Checking…":"Refresh connection"}</button></div><p className="muted small">Uses the Codex ChatGPT login available to the Linux account running Dar. Connect links it to your Dar account. Login tokens stay on the server.</p><p className="muted small">Dar account ID: <code className="agent-settings-account">{session.user.id}</code></p></>:<><h3>Keyboard shortcuts</h3><dl className="ui-shortcuts"><dt>Ctrl / Cmd Enter</dt><dd>Run with Codex when connected; otherwise save the message</dd><dt>Ctrl / Cmd K</dt><dd>Search all apps outside editors</dd><dt>Escape</dt><dd>Return from a phone panel or close settings</dd></dl></>}</section></div></Dialog>}
  </div>;
}
