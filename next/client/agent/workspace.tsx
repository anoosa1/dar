import {Fragment,useEffect,useLayoutEffect,useRef,useState,type FormEvent} from "react";
import {request,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceAccount} from "../shell.tsx";
import {AppSwitcher} from "../apps.tsx";
import {Icon} from "../icons.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {ActionMenu,Select} from "../controls.tsx";
import {Markdown} from "../pim/markdown.tsx";
import {AppSettingsButton,usePreferences,savePreferences} from "../app-settings.tsx";
import {DarFilePicker,type DarFileSelection} from "../file-picker.tsx";
import {activeAgentRun,type AgentProject,type AgentThread,type AgentMessage,type AgentHistory,type AgentRun,type AgentProvider,type AgentChange} from "../../shared/agent.ts";
import "./workspace.css";

const base="/api/v1/agent";
type View="threads"|"conversation"|"details"|"changes"|"terminal";
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
  const [query,setQuery]=useState("");
  const matches=projects.filter(project=>project.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  return <Dialog title="New thread" onClose={onClose}><div className="agent-project-chooser"><label className="sr-only" htmlFor="agent-project-search">Find a project</label><input id="agent-project-search" type="search" placeholder="Find a project…" value={query} onChange={event=>setQuery(event.target.value)}/><h3>Choose a project</h3>{matches.map(project=><button key={project.id} className="agent-project-choice" disabled={busy} onClick={()=>onChoose(project)}><Icon name={project.sourceType==="files"?"folder":"share"}/><span><strong>{project.name}</strong><small>{project.sourceType==="files"?"Dar Files":"Git repository"}</small></span><Icon name="chevron"/></button>)}{!matches.length&&<p className="muted">{projects.length?"No matching projects.":"Add a project to start working."}</p>}<button className="text-button" disabled={busy} onClick={onAdd}><Icon name="plus"/>Add project</button>{busy&&<p role="status">Opening thread…</p>}{error&&<p className="error" role="alert">{error}</p>}</div></Dialog>;
}

type ScrollPosition={top:number;following:boolean};
type Layout={nav:boolean;details:boolean;review:boolean;terminal:boolean};
function savedLayout(userId:string):Layout{
  const defaults:Layout={nav:true,details:true,review:false,terminal:false};
  try{const value=JSON.parse(localStorage.getItem(`dar.agent.layout.${userId}`)??"{}");for(const key of Object.keys(defaults) as (keyof Layout)[])if(typeof value?.[key]==="boolean")defaults[key]=value[key];}catch{}
  return defaults;
}

/** Existing run data stays truthful: a work summary, never invented tool events. */
function RunActivity({run}:{run:AgentRun}){
  const running=activeAgentRun(run.status),seconds=Math.max(1,Math.round((run.modifiedAt-run.createdAt)/1000));
  return <section className="agent-run-state" aria-label="Run status"><span className="sr-only" role="status">Codex · {run.status}</span><details key={`${run.id}-${running}`} open={running}><summary><Icon name={running?"clock":run.status==="completed"?"check":"info"}/>{running?run.status==="cancelling"?"Stopping…":"Working…":run.status==="completed"?`Worked for ${seconds}s`:`Codex · ${run.status}`}<Icon name="chevron"/></summary>{run.document.activity.length?<ul>{run.document.activity.map((item,i)=><li key={i}><Icon name="chevron"/><code>{item}</code></li>)}</ul>:<p className="muted">No tool activity recorded.</p>}</details>{run.document.truncated&&<p role="status">Output is truncated.</p>}{run.document.error&&<p role="alert" className="error">{run.document.error}</p>}</section>;
}

/** Draft and operation IDs survive reload; uncertain appends can be retried safely. */
function Conversation({thread,session,onChanged,draftCache,positions,provider,run,onRun,onStop,historyEpoch,findOpen,onCloseFind}:{thread:AgentThread;session:BrowserSession;onChanged:(thread:AgentThread)=>void;draftCache:Map<string,string>;positions:Map<string,ScrollPosition>;provider?:AgentProvider;run?:AgentRun;onRun:(id:string)=>void;onStop:()=>void;historyEpoch:number;findOpen:boolean;onCloseFind:()=>void}){
  const key=`dar.agent.draft.${session.user.id}.${thread.id}`;
  const [draft,setDraft]=useState(()=>{if(draftCache.has(key))return draftCache.get(key)!;try{return sessionStorage.getItem(key)??"";}catch{return "";}});
  const [history,setHistory]=useState<AgentHistory>(),[error,setError]=useState(""),[storageError,setStorageError]=useState(""),[busy,setBusy]=useState(false),[notice,setNotice]=useState(""),[reload,setReload]=useState(0),[loadingEarlier,setLoadingEarlier]=useState(false);
  const controller=useRef(new AbortController()),composer=useRef<HTMLTextAreaElement>(null),pending=useRef<{id:string;content:string}|undefined>(undefined),draftRef=useRef(draft);
  const viewport=useRef<HTMLDivElement>(null),content=useRef<HTMLDivElement>(null),following=useRef(true),positioned=useRef(false),prepend=useRef<{height:number;top:number}|undefined>(undefined),findInput=useRef<HTMLInputElement>(null);
  const [atEnd,setAtEnd]=useState(true),[find,setFind]=useState(""),[match,setMatch]=useState(0),[copyError,setCopyError]=useState("");
  const operationKey=key+".operation";
  const [model,setModel]=useState(()=>{try{return sessionStorage.getItem(key+".model")??"";}catch{return "";}}),running=run?activeAgentRun(run.status):!!history?.runs.some(item=>activeAgentRun(item.status));
  useEffect(()=>{if(provider?.models.length)setModel(current=>provider.models.some(m=>m.id===current)?current:(provider.models.find(m=>m.isDefault)??provider.models[0]).id);},[provider]);
  draftRef.current=draft;
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{composer.current?.focus({preventScroll:true});},[]);
  useEffect(()=>{if(model)try{sessionStorage.setItem(key+".model",model);}catch{}},[model,key]);
  useLayoutEffect(()=>{const field=composer.current;if(field){field.rows=Math.min(7,Math.max(2,draft.split("\n").reduce((rows,line)=>rows+Math.max(1,Math.ceil(line.length/75)),0)));}},[draft]);
  useEffect(()=>{if(findOpen)findInput.current?.focus();else setFind("");},[findOpen]);
  useLayoutEffect(()=>{
    const element=viewport.current;if(!element)return;
    return()=>{if(positioned.current)positions.set(thread.id,{top:element.scrollTop,following:following.current});};
  },[positions,thread.id]);
  useLayoutEffect(()=>{
    const element=viewport.current;if(!element||!history)return;
    if(prepend.current){element.scrollTop=prepend.current.top+element.scrollHeight-prepend.current.height;prepend.current=undefined;}
    else if(!positioned.current){const saved=positions.get(thread.id);following.current=saved?.following??true;element.scrollTop=following.current?element.scrollHeight:saved?.top??0;positioned.current=true;}
    else if(following.current)element.scrollTop=element.scrollHeight;
    setAtEnd(element.scrollHeight-element.scrollTop-element.clientHeight<48);
  },[history,run?.document.output,run?.document.activity.length,positions,thread.id]);
  useEffect(()=>{
    const element=viewport.current,node=content.current;if(!element||!node)return;
    const observer=new ResizeObserver(()=>{if(following.current&&positioned.current)element.scrollTop=element.scrollHeight;});observer.observe(node);observer.observe(element);return()=>observer.disconnect();
  },[]);
  useEffect(()=>{
    const abort=new AbortController();setError("");
    void request<AgentHistory>(`${base}/threads/${thread.id}/messages`,{signal:abort.signal}).then(value=>{if(abort.signal.aborted)return;setHistory(value);onChanged(value.thread);if(value.runs[0])onRun(value.runs[0].id);}).catch(e=>{if(!abort.signal.aborted)setError(failure(e));});
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
      following.current=true;requestAnimationFrame(()=>{if(viewport.current)viewport.current.scrollTop=viewport.current.scrollHeight;});
    }catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  async function earlier(){
    if(!history?.nextBefore||loadingEarlier)return;setLoadingEarlier(true);setError("");
    try{const value=await request<AgentHistory>(`${base}/threads/${thread.id}/messages?before=${history.nextBefore}`,{signal:controller.current.signal});if(!controller.current.signal.aborted){const element=viewport.current;if(element)prepend.current={height:element.scrollHeight,top:element.scrollTop};setHistory(current=>current?{...current,items:mergeMessages(current.items,value.items),nextBefore:value.nextBefore}:current);}}
    catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setLoadingEarlier(false);}
  }
  const matches=history?.items.filter(message=>find.trim()&&message.content.toLocaleLowerCase().includes(find.trim().toLocaleLowerCase()))??[],matchId=matches[match%Math.max(1,matches.length)]?.id;
  const truncatedResponse=!!run&&!!history?.items.some(message=>message.role==="assistant"&&message.content.endsWith("\n\n[Full response is in the run output.]")&&run.document.output.startsWith(message.content.slice(0,-"\n\n[Full response is in the run output.]".length)));
  const responseSaved=truncatedResponse||!!run&&!!history?.items.some(message=>message.role==="assistant"&&message.content===run.document.output);
  useEffect(()=>{if(matchId){following.current=false;viewport.current?.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(matchId)}"]`)?.scrollIntoView({block:"center"});}},[matchId]);
  async function copy(text:string){try{await navigator.clipboard.writeText(text);setNotice("Copied to clipboard.");setCopyError("");}catch{setCopyError("Could not copy. Select the message text to copy it.");}}
  return <>
    {findOpen&&<div className="agent-find"><Icon name="search"/><input ref={findInput} aria-label="Find in thread" type="search" placeholder="Find in loaded messages…" value={find} onChange={event=>{setFind(event.target.value);setMatch(0);}} onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();event.stopPropagation();onCloseFind();composer.current?.focus();}else if(event.key==="Enter"){event.preventDefault();setMatch(value=>(value+(event.shiftKey?-1:1)+matches.length)%Math.max(1,matches.length));}}}/><span role="status">{matches.length?`${match%matches.length+1} / ${matches.length}`:find?"No matches":"Loaded messages"}</span><button className="icon-button" aria-label="Next match" disabled={!matches.length} onClick={()=>setMatch(value=>(value+1)%matches.length)}><Icon name="chevron"/></button><button className="icon-button" aria-label="Close find" onClick={()=>{onCloseFind();composer.current?.focus();}}><Icon name="close"/></button></div>}
    <div className="agent-timeline-wrap">
    <div ref={viewport} className="agent-conversation" aria-label="Conversation messages" aria-busy={!history&&!error} onScroll={()=>{const element=viewport.current;if(!element)return;const end=element.scrollHeight-element.scrollTop-element.clientHeight<48;following.current=end;setAtEnd(end);}}><div className="agent-timeline-content" ref={content}>
      {!history&&!error&&<p role="status">Loading conversation…</p>}
      {history?.nextBefore&&<button className="text-button" disabled={loadingEarlier} onClick={()=>{void earlier();}}>{loadingEarlier?"Loading…":"Load earlier messages"}</button>}
      {history&&!history.items.length&&<div className="agent-empty"><Icon name="agent"/><h2>What would you like to work on?</h2><p>{provider?.connected?"Describe a task for Codex in this project’s isolated workspace.":"Save the task and context here while Codex is unavailable."}</p></div>}
      {history?.items.map(message=><Fragment key={message.id}><article data-message-id={message.id} data-match={message.id===matchId||undefined} className={`agent-message agent-message-${message.role}`}><div className="agent-message-body">{message.role==="assistant"?<Markdown source={message.content}/>:<p>{message.content}</p>}</div><div className="agent-message-meta"><time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleTimeString([],{hour:"2-digit",minute:"2-digit"})}</time><button className="icon-button" aria-label={message.role==="user"?"Copy message":"Copy response"} title="Copy" onClick={()=>{void copy(message.content);}}><Icon name="copy"/></button></div>{message.role==="user"&&!history.runs.some(item=>item.messageId===message.id)&&<small>Saved · not sent to a model</small>}</article>{run?.messageId===message.id&&<RunActivity run={run}/>}</Fragment>)}
      {run?.document.output&&(running||!responseSaved)&&<article className="agent-message agent-message-assistant agent-live-output"><Markdown source={run.document.output}/></article>}
      {run?.document.output&&truncatedResponse&&<details className="agent-full-response"><summary>Full Codex response</summary><Markdown source={run.document.output}/></details>}
    </div></div>
    {!atEnd&&<button className="agent-jump secondary-button" onClick={()=>{following.current=true;if(viewport.current)viewport.current.scrollTop=viewport.current.scrollHeight;setAtEnd(true);}}>Jump to latest <Icon name="download"/></button>}
    </div>
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void save(!!provider?.connected);}}>
      {error&&<div role="alert" className="error"><p>{error}</p><button type="button" className="text-button" disabled={busy} onClick={()=>setReload(n=>n+1)}>Refresh conversation</button></div>}
      {storageError&&<p role="alert" className="error">{storageError}</p>}
      {copyError&&<p role="alert" className="error">{copyError}</p>}
      <div className="agent-composer"><label className="sr-only" htmlFor="agent-message">Message</label><textarea id="agent-message" ref={composer} value={draft} disabled={busy} placeholder="Ask anything…" onChange={event=>setDraft(event.target.value)} onKeyDown={event=>{if(!event.nativeEvent.isComposing&&event.key==="Enter"&&!event.shiftKey&&!event.altKey){event.preventDefault();void save(!!provider?.connected);}}}/><div className="agent-composer-controls"><span className="agent-draft-label">{running?"Draft your next message":"Shift Enter for a new line"}</span>{provider?.connected?<Select aria-label="Codex model" value={model} onChange={event=>setModel(event.target.value)} disabled={busy||running}>{provider.models.map(item=><option key={item.id} value={item.id}>{item.name}</option>)}</Select>:<span>{provider?.configured?"Codex unavailable":"No provider connected"}</span>}{provider?.connected&&<ActionMenu label="Message actions" actions={[{label:"Save only",icon:"file",disabled:busy||running||!history||!draft.trim(),run:()=>{void save();}}]}><Icon name="more"/></ActionMenu>}{running?<button type="button" className="agent-send agent-stop" aria-label={run?.status==="cancelling"?"Stopping…":"Stop"} title="Stop generation" disabled={run?.status==="cancelling"} onClick={onStop}><span aria-hidden="true">■</span></button>:<button className="agent-send" aria-label={busy?"Saving…":provider?.connected?"Run with Codex":"Save message"} title={provider?.connected?"Run with Codex":"Save message"} disabled={busy||!history||!draft.trim()||!!provider?.connected&&!model}><Icon name="send"/></button>}</div></div>
      <div className="agent-composer-hint" role="status">{notice}</div>
    </form>
  </>;
}

/** Render the recorded patch, retaining its original markers and hunk boundaries. */
function Diff({source,wrap}:{source:string;wrap:boolean}){
  let oldLine=1,newLine=1,inHunk=false;
  return <div className={`agent-diff${wrap?" agent-diff-wrap":""}`} role="region" aria-label="File diff" tabIndex={0}>{source.split("\n").map((line,index)=>{
    let oldNumber:number|undefined,newNumber:number|undefined,kind="";
    if(line.startsWith("@@")){const hunk=/^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);oldLine=hunk?Number(hunk[1]):1;newLine=hunk?Number(hunk[2]):1;inHunk=true;kind="hunk";}
    else if(inHunk){if(line.startsWith("-")){kind="removed";oldNumber=oldLine++;}else if(line.startsWith("+")){kind="added";newNumber=newLine++;}else if(line.startsWith(" ")){oldNumber=oldLine++;newNumber=newLine++;}}
    return <div key={index} className={`agent-diff-line ${kind}`}><span className="agent-diff-number" aria-hidden="true">{oldNumber}</span><span className="agent-diff-number" aria-hidden="true">{newNumber}</span><code className="agent-diff-text">{line||" "}</code></div>;
  })}</div>;
}

function ChangeReview({run,session}:{run?:AgentRun;session:BrowserSession}){
  const [selected,setSelected]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState(""),[applied,setApplied]=useState<string[]>([]),[download,setDownload]=useState("");
  const [wrap,setWrap]=useState(false),[collapsed,setCollapsed]=useState(false),[copied,setCopied]=useState(false);
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
  return <>{!changes.length?<><Icon name="file"/><h3>No changes yet</h3><p>Changes from agent runs appear here for review.</p></>:<><p className="agent-review-scope">{changes.length} changed {changes.length===1?"file":"files"} · Latest run</p><nav aria-label="Changed files" className="agent-change-list">{changes.map(item=><button key={item.path} className={change?.path===item.path?"selected":""} aria-current={change?.path===item.path?"page":undefined} onClick={()=>{setSelected(item.path);setCollapsed(false);setCopied(false);}}><Icon name="file"/><span>{item.path}</span><small>{item.kind}</small></button>)}</nav>{change&&<><h3>{change.path}</h3><div className="agent-review-tools"><button className="secondary-button" aria-pressed={wrap} onClick={()=>setWrap(value=>!value)}>Wrap lines</button><button className="text-button" aria-expanded={!collapsed} onClick={()=>setCollapsed(value=>!value)}>{collapsed?"Expand diff":"Collapse diff"}</button><button className="text-button" onClick={()=>{void navigator.clipboard.writeText(change.diff).then(()=>setCopied(true)).catch(()=>setError("Could not copy the diff. Use Download review instead."));}}>{copied?"Copied":"Copy diff"}</button></div>{!collapsed&&<Diff source={change.diff} wrap={wrap}/>}<span className="sr-only" role="status">{copied?"Diff copied.":""}</span>{change.content!==undefined&&change.fileId&&run?.status==="completed"&&<button className="primary-button" disabled={busy||applied.includes(change.path)} onClick={()=>{void apply(change);}}>{busy?"Applying…":applied.includes(change.path)?"Applied to Files":"Apply this edit to Files"}</button>}</>}{download&&<a href={download} download="agent-review.txt">Download review</a>}<p className="small muted">Edits stay in this thread’s working copy until applied. New files, deletions and Git publishing are not applied automatically.</p></>}{error&&<p role="alert" className="error">{error}</p>}</>;
}

export function AgentWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const initial=useRef(new URLSearchParams(location.search)).current;
  const layout=useRef(savedLayout(session.user.id)).current;
  const [projects,setProjects]=useState<AgentProject[]>(),[projectId,setProjectId]=useState(initial.get("project")??""),[threads,setThreads]=useState<AgentThread[]>(),[threadId,setThreadId]=useState(initial.get("thread")??"");
  const [error,setError]=useState(""),[threadError,setThreadError]=useState(""),[reload,setReload]=useState(0),[view,setView]=useState<View>("conversation"),[editor,setEditor]=useState<"project"|"thread">(),[settings,setSettings]=useState(false),[settingsSection,setSettingsSection]=useState("General"),[review,setReview]=useState(layout.review),[nav,setNav]=useState(layout.nav),[details,setDetails]=useState(layout.details),[terminal,setTerminal]=useState(layout.terminal),[query,setQuery]=useState(""),[findOpen,setFindOpen]=useState(false),[maximized,setMaximized]=useState(false),[layoutError,setLayoutError]=useState("");
  const [provider,setProvider]=useState<AgentProvider>(),[providerEpoch,setProviderEpoch]=useState(0),[checkingProvider,setCheckingProvider]=useState(true),[connectionError,setConnectionError]=useState(""),[runId,setRunId]=useState(""),[storedRun,setRun]=useState<AgentRun>(),[runError,setRunError]=useState(""),[historyEpoch,setHistoryEpoch]=useState(0),[creatingThread,setCreatingThread]=useState(false),[createError,setCreateError]=useState(""),[settling,setSettling]=useState(false);
  const {preferences,loaded:preferencesLoaded,error:preferencesError}=usePreferences();
  const completedSeen=useRef(new Set<string>()),newThreadRequest=useRef<{id:string;projectId:string}|undefined>(undefined),lifetime=useRef(new AbortController());
  const panel=useRef<HTMLDivElement>(null),tabs=useRef<HTMLElement>(null),previousView=useRef(view),returnToTabs=useRef(false),draftCache=useRef(new Map<string,string>()),positions=useRef(new Map<string,ScrollPosition>()),search=useRef<HTMLInputElement>(null);
  const thread=threads?.find(t=>t.id===threadId),project=projects?.find(p=>p.id===(thread?.projectId??projectId));
  const run=storedRun?.threadId===threadId?storedRun:undefined;
  useEffect(()=>()=>lifetime.current.abort(),[]);
  useEffect(()=>{try{localStorage.setItem(`dar.agent.layout.${session.user.id}`,JSON.stringify({nav,details,review,terminal}));setLayoutError("");}catch{setLayoutError("Layout changes could not be saved in this browser.");}},[nav,details,review,terminal,session.user.id]);
  useEffect(()=>{
    const shortcut=(event:KeyboardEvent)=>{if(!(event.ctrlKey||event.metaKey)||event.defaultPrevented||document.querySelector("dialog[open],[data-editor-pane]"))return;
      if(event.key.toLowerCase()==="k"){event.preventDefault();setNav(true);setView("threads");requestAnimationFrame(()=>search.current?.focus());}
      else if(event.key.toLowerCase()==="f"&&threadId){event.preventDefault();setView("conversation");setFindOpen(true);}
    };document.addEventListener("keydown",shortcut);return()=>document.removeEventListener("keydown",shortcut);
  },[threadId]);
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
  function openReview(){setReview(true);setView("changes");}
  function closeReview(){setReview(false);setMaximized(false);setView("conversation");requestAnimationFrame(()=>panel.current?.querySelector<HTMLButtonElement>('[aria-label="Show changes"]')?.focus());}
  function stop(){if(!runId||run&&(!activeAgentRun(run.status)||run.threadId!==threadId))return;void request<AgentRun>(`${base}/runs/${runId}/cancel`,{method:"POST",signal:lifetime.current.signal},session.csrfToken).then(value=>{if(!lifetime.current.signal.aborted)setRun(value);}).catch(e=>{if(!lifetime.current.signal.aborted)setRunError(failure(e));});}
  async function copyThread(item:AgentThread){try{const url=new URL(location.href);url.searchParams.set("thread",item.id);url.searchParams.set("project",item.projectId);await navigator.clipboard.writeText(url.href);}catch{setThreadError("Could not copy the thread link. Open the thread and copy its address.");}}
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
  function rows(items:AgentThread[]){return <div className="agent-thread-list">{items.map(item=><div className={`agent-thread-row${item.id===threadId?" selected":""}`} key={item.id}><button className="agent-thread-select" aria-current={item.id===threadId?"page":undefined} onClick={()=>{setThreadId(item.id);setProjectId(item.projectId);setFindOpen(false);setView("conversation");}}><span className="agent-thread-project"><Icon name={isWorking(item)?"clock":"folder"}/>{projects?.find(p=>p.id===item.projectId)?.name??"Project"}{isWorking(item)&&<span className="agent-working-dot" aria-label="Working"/>}</span><strong>{item.title}</strong></button><ActionMenu label={`Actions for ${item.title}`} actions={[{label:isSettled(item)?"Restore thread":"Settle thread",icon:"check",disabled:settling||!preferencesLoaded||isWorking(item),run:()=>{void settle(item);}},{label:"Copy thread link",icon:"share",run:()=>{void copyThread(item);}}]}><Icon name="more"/></ActionMenu></div>)}</div>;}
  const filtered=threads?.filter(row=>`${row.title} ${projects?.find(project=>project.id===row.projectId)?.name??""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()))??[];
  const active=filtered.filter(row=>!isWorking(row)&&!isSettled(row)),working=filtered.filter(isWorking),settled=filtered.filter(isSettled);
  return <div className={`app-shell agent-shell agent-view-${view}${review?"":" agent-review-hidden"}${nav?"":" agent-nav-hidden"}${details?"":" agent-details-hidden"}${maximized&&review?" agent-review-maximized":""}`}>
    <nav ref={tabs} className="agent-mobile-views" aria-label="Agent views">{(["threads","conversation","details","changes","terminal"] as const).map(value=><button key={value} aria-current={view===value?"page":undefined} onClick={()=>setView(value)}>{value[0].toUpperCase()+value.slice(1)}</button>)}</nav>
    <div className="agent-layout" ref={panel} onKeyDown={event=>{if(event.key==="Escape"&&view!=="conversation"&&!editor&&!settings){event.preventDefault();returnToTabs.current=true;setView("conversation");}}}>
      <aside className="agent-sidebar" data-agent-panel="threads" tabIndex={-1} aria-label="Threads"><h1 className="sr-only">Agent</h1><div className="agent-brand"><a href="/" aria-label="Dar home"><Icon name="agent"/>Dar</a><AppSwitcher active="agent" administrator={session.user.administrator}/></div><div className="agent-search-row"><label className="agent-thread-search"><Icon name="search"/><span className="sr-only">Search threads</span><input ref={search} type="search" placeholder="Search" value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();setQuery("");}}}/></label><button className="icon-button" aria-label="New thread" title="New thread" disabled={!projects} onClick={chooseNewThread}><Icon name="plus"/></button></div>
        {error&&<div role="alert"><p>{error}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry projects</button></div>}
        {threadError&&<div role="alert"><p>{threadError}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry threads</button></div>}
        {preferencesError&&<p role="alert" className="error">{preferencesError}</p>}
        <div className="agent-thread-shelves">{!threads?<p role="status">Loading threads…</p>:<>{rows(active)}{!threads.length&&<p className="muted small">Start a thread to work on a task.</p>}{threads.length>0&&!filtered.length&&<p role="status" className="muted small">No matching threads.</p>}<details open><summary>Working <span>{working.length}</span></summary>{rows(working)}</details><details open={query?true:undefined}><summary>Settled <span>{settled.length}</span></summary>{rows(settled)}</details></>}</div>
        <div className="agent-sidebar-footer"><AppSettingsButton app="agent" onClick={()=>setSettings(true)}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/></div>
      </aside>
      <main className="agent-center" data-agent-panel="conversation" tabIndex={-1} aria-label="Agent conversation"><header className="agent-thread-heading"><button className="icon-button agent-desktop-toggle" aria-label={nav?"Hide threads":"Show threads"} aria-expanded={nav} onClick={()=>setNav(v=>!v)}><Icon name="menu"/></button><div className="agent-collapsed-controls"><AppSwitcher active="agent" administrator={session.user.administrator}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/><button className="icon-button" aria-label="Agent settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div>{project&&<span className="agent-heading-project"><Icon name="folder"/>{project.name}<span>/</span></span>}<h2>{thread?.title??"New thread"}</h2>{thread&&<ActionMenu label="Thread actions" actions={[{label:"Find in thread",icon:"search",run:()=>setFindOpen(true)},{label:isSettled(thread)?"Restore thread":"Settle thread",icon:"check",disabled:settling||!preferencesLoaded||isWorking(thread),run:()=>{void settle(thread);}},{label:"Copy thread link",icon:"share",run:()=>{void copyThread(thread);}}]}><Icon name="more"/></ActionMenu>}<div className="agent-panel-controls"><button className="icon-button agent-desktop-toggle" aria-label="Toggle thread details" aria-expanded={details} onClick={()=>setDetails(value=>!value)}><Icon name="info"/></button><button className="icon-button agent-desktop-toggle" aria-label="Toggle command output" aria-expanded={terminal} onClick={()=>setTerminal(value=>!value)}><Icon name="list"/></button><button className="icon-button agent-desktop-toggle" aria-label={review?"Hide changes":"Show changes"} aria-expanded={review} onClick={()=>review?closeReview():openReview()}><Icon name="file"/></button></div></header>
        {runError&&<p role="alert" className="error">{runError}</p>}
        {layoutError&&<p role="alert" className="error">{layoutError}</p>}
        <div className="agent-stage"><div className="agent-chat">{thread?<Conversation key={thread.id} thread={thread} session={session} onChanged={changed} draftCache={draftCache.current} positions={positions.current} provider={provider} run={run} onRun={setRunId} onStop={stop} historyEpoch={historyEpoch} findOpen={findOpen} onCloseFind={()=>setFindOpen(false)}/>:<div className="agent-empty"><Icon name="agent"/><h2>What would you like to work on?</h2><p>Start a thread in a Git repository or a folder in Dar Files.</p><button className="primary-button" disabled={!projects} onClick={chooseNewThread}>New thread</button></div>}</div>
        {thread&&<aside className="agent-context" data-agent-panel="details" tabIndex={-1} aria-label="Thread details"><div className="agent-context-card"><div className="agent-context-project"><Icon name="folder"/><strong>{project?.name??"Project"}</strong><span>{project?.sourceType==="files"?"Dar Files":"Git"}</span></div>{project?.sourceType==="files"?<a href={`/?folder=${project.sourceRef}`}><Icon name="folder"/>Open folder in Files<Icon name="chevron"/></a>:project&&<div className="agent-source-ref"><Icon name="share"/><code>{project.sourceRef}</code></div>}<div className="agent-workspace-label"><Icon name="lock"/>Isolated working copy</div><button onClick={openReview}><Icon name="file"/>Changes<span>{run?.document.changes.length??0}</span><Icon name="chevron"/></button><div className="agent-context-provider"><span className={provider?.connected?"agent-connected":""}/>{checkingProvider?"Checking Codex…":provider?.connected?"Codex connected":"Codex unavailable"}<button className="icon-button" aria-label="Connection settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div></div></aside>}</div>
        <section className={`agent-terminal${terminal?" is-open":""}`} data-agent-panel="terminal" tabIndex={-1} aria-label="Terminal output"><div className="agent-output-heading"><span>Command output</span><span className="muted">Read only</span><button className="icon-button" aria-label="Close command output" onClick={()=>{setTerminal(false);setView("conversation");}}><Icon name="close"/></button></div>{run?.document.terminal?<pre>{run.document.terminal}</pre>:<p>No command output yet.</p>}</section>
      </main>
      <aside className="agent-review" data-agent-panel="changes" tabIndex={-1} aria-label="Changes"><header className="agent-review-heading"><h2>Changes</h2><button className="icon-button agent-desktop-toggle" aria-label={maximized?"Restore review size":"Maximize review"} onClick={()=>setMaximized(value=>!value)}><Icon name="grid"/></button><button className="icon-button" aria-label="Close changes" onClick={closeReview}><Icon name="close"/></button></header><div className="agent-review-content"><ChangeReview key={run?.id??"empty"} run={run} session={session}/></div></aside>
    </div>
    {editor==="project"&&<ProjectEditor session={session} onClose={()=>setEditor("thread")} onCreated={row=>{setProjects(current=>[...current??[],row]);void createThread(row);}}/>}
    {editor==="thread"&&projects&&<ThreadChooser projects={projects} busy={creatingThread} error={createError} onClose={()=>setEditor(undefined)} onChoose={row=>{void createThread(row);}} onAdd={()=>setEditor("project")}/>}
    {settings&&<Dialog title="Agent settings" onClose={()=>setSettings(false)}><div className="ui-settings-dialog"><nav aria-label="Settings sections">{["General","Keyboard shortcuts"].map(section=><button key={section} className={settingsSection===section?"active":""} aria-current={settingsSection===section?"page":undefined} onClick={()=>setSettingsSection(section)}>{section}</button>)}</nav><section>{settingsSection==="General"?<><h3>Codex connection</h3><p role="status">{checkingProvider?"Checking Codex…":provider?.message??"Connection could not be checked."}</p>{connectionError&&<p className="error" role="alert">{connectionError}</p>}<div className="agent-connection-actions">{session.user.administrator&&!provider?.configured&&<button className="primary-button" disabled={checkingProvider} onClick={()=>{void connectProvider();}}>Connect server Codex</button>}<button className="secondary-button" disabled={checkingProvider} onClick={()=>setProviderEpoch(n=>n+1)}>{checkingProvider?"Checking…":"Refresh connection"}</button></div><p className="muted small">Runs send the conversation and selected project contents to Codex. Commands have no network access; publishing is disabled.</p><p className="muted small">Uses the Codex ChatGPT login available to the Linux account running Dar. Connect links it to your Dar account. Login tokens stay on the server.</p><p className="muted small">Dar account ID: <code className="agent-settings-account">{session.user.id}</code></p><h3>Workspace</h3><p className="muted small">Panel visibility is remembered for this account in this browser. Drafts and selected models are kept in this tab.</p></>:<><h3>Keyboard shortcuts</h3><dl className="ui-shortcuts"><dt>Enter / Ctrl / Cmd Enter</dt><dd>Run with Codex when connected; otherwise save the message</dd><dt>Shift Enter</dt><dd>Insert a new line</dd><dt>Ctrl / Cmd K</dt><dd>Search threads</dd><dt>Ctrl / Cmd F</dt><dd>Find in loaded thread messages</dd><dt>Escape</dt><dd>Return from a phone panel or close settings</dd></dl></>}</section></div></Dialog>}
  </div>;
}
