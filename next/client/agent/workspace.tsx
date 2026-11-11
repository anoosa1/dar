import {Fragment,useEffect,useLayoutEffect,useMemo,useRef,useState,type FormEvent,type KeyboardEvent as ReactKeyboardEvent} from "react";
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
  return <Dialog title="Choose project" onClose={onClose}><div className="agent-project-chooser"><label className="sr-only" htmlFor="agent-project-search">Find a project</label><input id="agent-project-search" type="search" placeholder="Find a project…" value={query} onChange={event=>setQuery(event.target.value)}/><h3>Projects</h3>{matches.map(project=><button key={project.id} className="agent-project-choice" aria-label={`${project.name} ${project.sourceType==="files"?"Dar Files":"Git repository"}`} disabled={busy} onClick={()=>onChoose(project)}><ProjectMark name={project.name}/><span><strong>{project.name}</strong><small>{project.sourceType==="files"?"Dar Files":"Git repository"}</small></span><Icon name="chevron"/></button>)}{!matches.length&&<p className="muted">{projects.length?"No matching projects.":"Add a project to start working."}</p>}<button className="text-button" disabled={busy} onClick={onAdd}><Icon name="plus"/>Add project</button>{error&&<p className="error" role="alert">{error}</p>}</div></Dialog>;
}

type ScrollPosition={top:number;following:boolean};
type Layout={nav:boolean;details:boolean;review:boolean;terminal:boolean};
function savedLayout(userId:string):Layout{
  const defaults:Layout={nav:true,details:true,review:false,terminal:false};
  try{const value=JSON.parse(localStorage.getItem(`dar.agent.layout.${userId}`)??"{}");for(const key of Object.keys(defaults) as (keyof Layout)[])if(typeof value?.[key]==="boolean")defaults[key]=value[key];}catch{}
  return defaults;
}

/** Ticks only while a caller needs live labels; idle rows do not schedule work. */
function useNow(active:boolean,interval=1000){
  const [now,setNow]=useState(Date.now);
  useEffect(()=>{if(!active)return;setNow(Date.now());const timer=setInterval(()=>setNow(Date.now()),interval);return()=>clearInterval(timer);},[active,interval]);
  return now;
}
function duration(ms:number){
  const seconds=Math.max(1,Math.round(ms/1000));if(seconds<60)return `${seconds}s`;
  const minutes=Math.floor(seconds/60);if(minutes<60)return `${minutes}m ${String(seconds%60).padStart(2,"0")}s`;
  return `${Math.floor(minutes/60)}h ${String(minutes%60).padStart(2,"0")}m`;
}
function ago(time:number,now:number){
  const minutes=Math.floor((now-time)/60000);if(minutes<1)return "now";if(minutes<60)return `${minutes}m`;
  const hours=Math.floor(minutes/60);if(hours<24)return `${hours}h`;const days=Math.floor(hours/24);if(days<7)return `${days}d`;
  return days<365?`${Math.floor(days/7)}w`:`${Math.floor(days/365)}y`;
}
/** Deterministic project tint from Dar theme tokens; no per-project inline colors. */
function ProjectMark({name}:{name:string}){
  let hash=0;for(const char of name)hash=(hash*31+char.codePointAt(0)!)>>>0;
  return <span className="agent-project-mark" data-tint={hash%5} aria-hidden="true">{[...name.trim()][0]?.toLocaleUpperCase()??"?"}</span>;
}
type DiffLine={kind:" "|"-"|"+";text:string};
/** Recorded source diffs hold whole old/new contents. A trailing empty element is the final newline, not a line. */
function fullContents(diff:string){
  const lines=diff.split("\n"),header=/^@@ (added|modified|deleted) @@$/.exec(lines[2]??"");
  if(!lines[0]?.startsWith("--- ")||!lines[1]?.startsWith("+++ ")||!header)return undefined;
  let index=3;const old:string[]=[],next:string[]=[];
  while(index<lines.length&&lines[index].startsWith("-"))old.push(lines[index++].slice(1));
  if(lines[index]==="")index++;
  while(index<lines.length&&lines[index].startsWith("+"))next.push(lines[index++].slice(1));
  if(index<lines.length&&!(index===lines.length-1&&lines[index]===""))return undefined;
  if(old.at(-1)==="")old.pop();if(next.at(-1)==="")next.pop();
  return {head:lines.slice(0,2),old,next};
}
/** Minimal line diff. Large middles fall back to replacement to bound browser work. */
function lineDiff(old:string[],next:string[]):DiffLine[]{
  let start=0,endOld=old.length,endNew=next.length;
  while(start<endOld&&start<endNew&&old[start]===next[start])start++;
  while(endOld>start&&endNew>start&&old[endOld-1]===next[endNew-1]){endOld--;endNew--;}
  const a=old.slice(start,endOld),b=next.slice(start,endNew),middle:DiffLine[]=[];
  if(a.length*b.length>2_000_000)middle.push(...a.map(text=>({kind:"-" as const,text})),...b.map(text=>({kind:"+" as const,text})));
  else{
    const width=b.length+1,table=new Uint32Array((a.length+1)*width);
    for(let i=a.length-1;i>=0;i--)for(let j=b.length-1;j>=0;j--)table[i*width+j]=a[i]===b[j]?table[(i+1)*width+j+1]+1:Math.max(table[(i+1)*width+j],table[i*width+j+1]);
    let i=0,j=0;while(i<a.length||j<b.length){
      if(i<a.length&&j<b.length&&a[i]===b[j]){middle.push({kind:" ",text:a[i]});i++;j++;}
      else if(i<a.length&&(j===b.length||table[(i+1)*width+j]>=table[i*width+j+1]))middle.push({kind:"-",text:a[i++]});
      else middle.push({kind:"+",text:b[j++]});
    }
  }
  return [...old.slice(0,start).map(text=>({kind:" " as const,text})),...middle,...old.slice(endOld).map(text=>({kind:" " as const,text}))];
}
/** Render a recorded whole-file patch as unified hunks with three context lines. */
function unifiedPatch(diff:string){
  const contents=fullContents(diff);if(!contents)return diff;
  const lines=lineDiff(contents.old,contents.next),changed=lines.flatMap((line,index)=>line.kind===" "?[]:[index]);
  if(!changed.length)return contents.head.join("\n");
  const ranges:[number,number][]=[];for(const index of changed){const from=Math.max(0,index-3),to=Math.min(lines.length,index+4),last=ranges.at(-1);if(last&&from<=last[1])last[1]=to;else ranges.push([from,to]);}
  const output=[...contents.head];let oldLine=1,newLine=1,cursor=0;
  for(const [from,to] of ranges){
    for(;cursor<from;cursor++){if(lines[cursor].kind!=="+")oldLine++;if(lines[cursor].kind!=="-")newLine++;}
    const slice=lines.slice(from,to),oldCount=slice.filter(line=>line.kind!=="+").length,newCount=slice.filter(line=>line.kind!=="-").length;
    output.push(`@@ -${oldCount?oldLine:oldLine-1},${oldCount} +${newCount?newLine:newLine-1},${newCount} @@`,...slice.map(line=>line.kind+line.text));
  }
  return output.join("\n");
}
function diffCounts(diff:string){
  const contents=fullContents(diff),lines:DiffLine[]=contents?lineDiff(contents.old,contents.next):diff.split("\n").filter(line=>!line.startsWith("+++")&&!line.startsWith("---")).map(line=>({kind:line[0]==="+"||line[0]==="-"?line[0]:" ",text:""}));
  return {added:lines.filter(line=>line.kind==="+").length,removed:lines.filter(line=>line.kind==="-").length};
}
function DiffStat({added,removed}:{added:number;removed:number}){
  return <span className="agent-diff-stat"><span className="agent-added">+{added}</span><span className="agent-removed">−{removed}</span></span>;
}

/** Compact searchable model menu. Escape and outside presses close it and return focus. */
function ModelPicker({models,value,onChange,disabled}:{models:AgentProvider["models"];value:string;onChange:(id:string)=>void;disabled?:boolean}){
  const [open,setOpen]=useState(false),[query,setQuery]=useState(""),[active,setActive]=useState(0),root=useRef<HTMLDivElement>(null),trigger=useRef<HTMLButtonElement>(null),search=useRef<HTMLInputElement>(null),list=useRef<HTMLUListElement>(null);
  const shown=models.filter(model=>`${model.name} ${model.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())),selected=models.find(model=>model.id===value);
  useEffect(()=>{if(!open)return;setQuery("");setActive(Math.max(0,models.findIndex(model=>model.id===value)));requestAnimationFrame(()=>(search.current??list.current)?.focus());
    const dismiss=(event:PointerEvent)=>{if(!root.current?.contains(event.target as Node))setOpen(false);};document.addEventListener("pointerdown",dismiss);return()=>document.removeEventListener("pointerdown",dismiss);},[open]);
  useEffect(()=>{list.current?.querySelector("[data-active]")?.scrollIntoView({block:"nearest"});},[active,open]);
  function choose(id:string){onChange(id);setOpen(false);trigger.current?.focus();}
  function keys(event:ReactKeyboardEvent){
    if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setOpen(false);trigger.current?.focus();}
    else if(event.key==="ArrowDown"||event.key==="ArrowUp"){event.preventDefault();setActive(index=>shown.length?(index+(event.key==="ArrowDown"?1:-1)+shown.length)%shown.length:0);}
    else if(event.key==="Enter"&&shown[active]){event.preventDefault();choose(shown[active].id);}
  }
  return <div className="agent-model-picker" ref={root}><button type="button" ref={trigger} className="agent-model-trigger" aria-label={`Codex model: ${selected?.name??"none"}`} aria-haspopup="listbox" aria-expanded={open} disabled={disabled} onClick={()=>setOpen(value=>!value)}><Icon name="agent"/><span>{selected?.name??"Choose model"}</span><Icon name="chevron"/></button>
    {open&&<div className="agent-model-popover" onKeyDown={keys}>{models.length>6&&<input ref={search} type="search" aria-label="Search models" placeholder="Search models" value={query} onChange={event=>{setQuery(event.target.value);setActive(0);}}/>}<ul ref={list} role="listbox" aria-label="Codex models" tabIndex={-1} aria-activedescendant={shown[active]?`agent-model-${active}`:undefined}>{shown.map((model,index)=><li key={model.id} id={`agent-model-${index}`} role="option" aria-selected={model.id===value} data-active={index===active||undefined} onPointerEnter={()=>setActive(index)} onClick={()=>choose(model.id)}><span>{model.name}</span>{model.isDefault&&<small>Default</small>}{model.id===value&&<Icon name="check"/>}</li>)}{!shown.length&&<li className="muted" role="presentation">No matching models</li>}</ul></div>}
  </div>;
}

/** Existing run data stays truthful: recorded commands and edits, never invented tool events. */
function WorkLog({run,onReview}:{run:AgentRun;onReview:()=>void}){
  const running=activeAgentRun(run.status),now=useNow(running),[open,setOpen]=useState(running);
  useEffect(()=>setOpen(running),[running]);
  const spent=duration((running?now:run.modifiedAt)-run.createdAt);
  const label=running?run.status==="cancelling"?`Stopping · ${spent}`:`Working for ${spent}`:run.status==="completed"?`Worked for ${spent}`:run.status==="cancelled"?`Stopped after ${spent}`:run.status==="failed"?`Failed after ${spent}`:`Interrupted after ${spent}`;
  const entries=run.document.activity,changes=run.document.changes;
  return <section className={`agent-work${running?" is-running":""}`} aria-label="Work log"><span className="sr-only" role="status">Codex · {run.status}</span>
    <button type="button" className="agent-work-toggle" aria-expanded={open} onClick={()=>setOpen(value=>!value)}>{running&&<span className="agent-pulse" aria-hidden="true"/>}<span>{label}</span>{(entries.length>0||!running)&&<Icon name="chevron"/>}</button>
    {open&&<ol className="agent-work-entries">{entries.map((item,i)=>item==="Editing workspace files"?<li key={i}><Icon name="pencil"/><span>Edited workspace files</span></li>:<li key={i}><Icon name="terminal"/><span>Ran <code>{item}</code></span></li>)}{!entries.length&&<li className="muted"><Icon name="info"/><span>{running?"Waiting for the first tool step…":"No commands or edits were recorded."}</span></li>}</ol>}
    {run.document.truncated&&<p className="agent-work-note" role="status">Output is truncated.</p>}{run.document.error&&<p role="alert" className="error agent-work-note">{run.document.error}</p>}
    {!running&&changes.length>0&&<div className="agent-changed-card"><div className="agent-changed-heading"><strong>{changes.length} {changes.length===1?"file":"files"} changed</strong><DiffStat {...changes.map(change=>diffCounts(change.diff)).reduce((sum,item)=>({added:sum.added+item.added,removed:sum.removed+item.removed}),{added:0,removed:0})}/><button type="button" className="text-button" onClick={onReview}>Review changes</button></div><ul>{changes.map(change=><li key={change.path}><Icon name={change.kind==="deleted"?"trash":change.kind==="added"?"plus":"file"}/><span title={change.path}>{change.path}</span><DiffStat {...diffCounts(change.diff)}/></li>)}</ul></div>}
  </section>;
}

/** Draft and operation IDs survive reload; uncertain appends can be retried safely. */
function Conversation({thread,project,session,onChanged,draftCache,positions,provider,run,onRun,onStop,onReview,historyEpoch,findOpen,onCloseFind,autoStart,onStarted,checkingProvider}:{thread:AgentThread;project?:AgentProject;session:BrowserSession;onChanged:(thread:AgentThread)=>void;draftCache:Map<string,string>;positions:Map<string,ScrollPosition>;provider?:AgentProvider;run?:AgentRun;onRun:(id:string)=>void;onStop:()=>void;onReview:()=>void;historyEpoch:number;findOpen:boolean;onCloseFind:()=>void;autoStart?:boolean;onStarted:()=>void;checkingProvider:boolean}){
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
  /** A draft composed before its thread existed is submitted once, after history confirms the revision. */
  const started=useRef(false);
  useEffect(()=>{if(autoStart===undefined||started.current||!history||!draft.trim()||autoStart&&checkingProvider)return;started.current=true;onStarted();void save(autoStart&&!!provider?.connected);},[history,autoStart,provider,checkingProvider]);
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
      {history&&!history.items.length&&autoStart===undefined&&<div className="agent-empty agent-hero"><h2>What should we build in <strong>{project?.name??"this project"}</strong>?</h2><p>{provider?.connected?"Codex works in an isolated copy of this project.":"Codex is unavailable. Messages are saved here until it is connected."}</p></div>}
      {history?.items.map(message=><Fragment key={message.id}><article data-message-id={message.id} data-match={message.id===matchId||undefined} className={`agent-message agent-message-${message.role}`}><div className="agent-message-body">{message.role==="assistant"?<Markdown source={message.content}/>:<p>{message.content}</p>}</div>{message.role==="user"&&!history.runs.some(item=>item.messageId===message.id)&&<small className="agent-saved-label">Saved · not sent to a model</small>}<div className="agent-message-meta">{message.role==="user"&&<time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit"})}</time>}<button className="icon-button" aria-label={message.role==="user"?"Copy message":"Copy response"} title="Copy" onClick={()=>{void copy(message.content);}}><Icon name="copy"/></button>{message.role==="assistant"&&<time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit"})}</time>}</div></article>{run?.messageId===message.id&&<WorkLog run={run} onReview={onReview}/>}</Fragment>)}
      {run?.document.output&&(running||!responseSaved)&&<article className="agent-message agent-message-assistant agent-live-output"><Markdown source={run.document.output}/></article>}
      {run?.document.output&&truncatedResponse&&<details className="agent-full-response"><summary>Full Codex response</summary><Markdown source={run.document.output}/></details>}
    </div></div>
    {!atEnd&&<button className="agent-jump secondary-button" onClick={()=>{following.current=true;if(viewport.current)viewport.current.scrollTop=viewport.current.scrollHeight;setAtEnd(true);}}>Jump to latest <Icon name="download"/></button>}
    </div>
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void save(!!provider?.connected);}}>
      {error&&<div role="alert" className="error"><p>{error}</p><button type="button" className="text-button" disabled={busy} onClick={()=>setReload(n=>n+1)}>Refresh conversation</button></div>}
      {storageError&&<p role="alert" className="error">{storageError}</p>}
      {copyError&&<p role="alert" className="error">{copyError}</p>}
      <div className="agent-composer"><label className="sr-only" htmlFor="agent-message">Message</label><textarea id="agent-message" ref={composer} value={draft} disabled={busy} placeholder={running?"Prepare a follow-up…":provider?.connected?"Ask Codex to plan, change or explain…":"Write a message to save…"} onChange={event=>setDraft(event.target.value)} onKeyDown={event=>{if(!event.nativeEvent.isComposing&&event.key==="Enter"&&!event.shiftKey&&!event.altKey){event.preventDefault();void save(!!provider?.connected);}}}/><div className="agent-composer-controls">{provider?.connected?<ModelPicker models={provider.models} value={model} onChange={setModel} disabled={busy||running}/>:<span className="agent-provider-state"><span className="agent-status-dot"/>{provider?.configured?"Codex unavailable":"No provider connected"}</span>}{provider?.connected&&<ActionMenu label="Message actions" actions={[{label:"Save only",icon:"file",disabled:busy||running||!history||!draft.trim(),run:()=>{void save();}}]}><Icon name="more"/></ActionMenu>}{running?<button type="button" className="agent-send agent-stop" aria-label={run?.status==="cancelling"?"Stopping…":"Stop"} title="Stop generation" disabled={run?.status==="cancelling"} onClick={onStop}><span aria-hidden="true"/></button>:<button className="agent-send" aria-label={busy?"Saving…":provider?.connected?"Run with Codex":"Save message"} title={provider?.connected?"Run with Codex (Enter)":"Save message (Enter)"} disabled={busy||!history||!draft.trim()||!!provider?.connected&&!model}><Icon name="send"/></button>}</div></div>
      <div className="agent-composer-hint" role="status">{notice}</div>
    </form>
  </>;
}

/** Compose before a thread exists. The thread is created with a stable ID only on submit. */
function DraftComposer({session,project,provider,checkingProvider,draftCache,onChooseProject,onCreated}:{session:BrowserSession;project:AgentProject;provider?:AgentProvider;checkingProvider:boolean;draftCache:Map<string,string>;onChooseProject:()=>void;onCreated:(thread:AgentThread,execute:boolean)=>void}){
  const key=`dar.agent.draft.${session.user.id}.new`;
  const [draft,setDraft]=useState(()=>{try{return sessionStorage.getItem(key)??"";}catch{return "";}}),[model,setModel]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const pending=useRef<{id:string;projectId:string}|undefined>(undefined),composer=useRef<HTMLTextAreaElement>(null),controller=useRef(new AbortController());
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{composer.current?.focus({preventScroll:true});},[project.id]);
  useEffect(()=>{try{if(draft)sessionStorage.setItem(key,draft);else sessionStorage.removeItem(key);}catch{}},[draft,key]);
  useEffect(()=>{if(provider?.models.length)setModel(current=>provider.models.some(m=>m.id===current)?current:(provider.models.find(m=>m.isDefault)??provider.models[0]).id);},[provider]);
  useLayoutEffect(()=>{const field=composer.current;if(field)field.rows=Math.min(9,Math.max(3,draft.split("\n").reduce((rows,line)=>rows+Math.max(1,Math.ceil(line.length/75)),0)));},[draft]);
  async function start(execute:boolean){
    if(!draft.trim()||busy||execute&&!model)return;
    if(new TextEncoder().encode(draft).length>16384){setError("Keep each message within 16 KiB.");return;}
    setBusy(true);setError("");
    if(pending.current?.projectId!==project.id)pending.current={id:crypto.randomUUID(),projectId:project.id};
    try{
      const row=await request<AgentThread>(`${base}/threads`,{method:"POST",signal:controller.current.signal,body:JSON.stringify(pending.current)},session.csrfToken);
      if(controller.current.signal.aborted)return;
      const target=`dar.agent.draft.${session.user.id}.${row.id}`;draftCache.set(target,draft);
      try{sessionStorage.setItem(target,draft);if(model)sessionStorage.setItem(target+".model",model);sessionStorage.removeItem(key);}catch{}
      pending.current=undefined;onCreated(row,execute);
    }catch(e){if(!controller.current.signal.aborted){setError(failure(e));setBusy(false);}}
  }
  const execute=!!provider?.connected;
  return <div className="agent-draft"><div className="agent-hero"><h2>What should we build in <button type="button" className="agent-hero-project" aria-label={`Project: ${project.name}. Change project`} onClick={onChooseProject}><ProjectMark name={project.name}/>{project.name}<Icon name="chevron"/></button>?</h2></div>
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void start(execute);}}>{error&&<p role="alert" className="error">{error}</p>}
      <div className="agent-composer"><label className="sr-only" htmlFor="agent-message">Message</label><textarea id="agent-message" ref={composer} value={draft} disabled={busy} placeholder={execute?"Ask Codex to plan, change or explain…":"Write a message to save…"} onChange={event=>setDraft(event.target.value)} onKeyDown={event=>{if(!event.nativeEvent.isComposing&&event.key==="Enter"&&!event.shiftKey&&!event.altKey){event.preventDefault();void start(execute);}}}/><div className="agent-composer-controls">{execute?<ModelPicker models={provider!.models} value={model} onChange={setModel} disabled={busy}/>:<span className="agent-provider-state"><span className="agent-status-dot"/>{checkingProvider?"Checking Codex…":provider?.configured?"Codex unavailable":"No provider connected"}</span>}{execute&&<ActionMenu label="Message actions" actions={[{label:"Save only",icon:"file",disabled:busy||!draft.trim(),run:()=>{void start(false);}}]}><Icon name="more"/></ActionMenu>}<button className="agent-send" aria-label={busy?"Starting…":execute?"Run with Codex":"Save message"} title={execute?"Run with Codex (Enter)":"Save message (Enter)"} disabled={busy||!draft.trim()||execute&&!model||checkingProvider&&!provider}><Icon name="send"/></button></div></div>
      <div className="agent-composer-hint" role="status">{busy?"Starting thread…":""}</div>
    </form></div>;
}

/** Render the recorded patch, retaining its original markers and hunk boundaries. */
function Diff({source,wrap}:{source:string;wrap:boolean}){
  let oldLine=1,newLine=1,inHunk=false;
  return <div className={`agent-diff${wrap?" agent-diff-wrap":""}`} role="region" aria-label="File diff" tabIndex={0}>{source.split("\n").filter(line=>!line.startsWith("--- ")&&!line.startsWith("+++ ")).map((line,index)=>{
    let oldNumber:number|undefined,newNumber:number|undefined,kind="";
    if(line.startsWith("@@")){const hunk=/^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);oldLine=hunk?Number(hunk[1]):1;newLine=hunk?Number(hunk[2]):1;inHunk=true;kind="hunk";}
    else if(inHunk){if(line.startsWith("-")){kind="removed";oldNumber=oldLine++;}else if(line.startsWith("+")){kind="added";newNumber=newLine++;}else if(line.startsWith(" ")){oldNumber=oldLine++;newNumber=newLine++;}}
    return <div key={index} className={`agent-diff-line ${kind}`}><span className="agent-diff-number" aria-hidden="true">{oldNumber}</span><span className="agent-diff-number" aria-hidden="true">{newNumber}</span><code className="agent-diff-text">{(kind==="hunk"||!inHunk?line:line.slice(1))||" "}</code></div>;
  })}</div>;
}

/** Stacked per-file review. Hunks are derived for display; Apply writes the recorded content only. */
function ChangeReview({run,session}:{run?:AgentRun;session:BrowserSession}){
  const [busy,setBusy]=useState(""),[error,setError]=useState(""),[applied,setApplied]=useState<string[]>([]),[download,setDownload]=useState("");
  const [wrap,setWrap]=useState(false),[collapsed,setCollapsed]=useState<string[]>([]),[notice,setNotice]=useState("");
  const controller=useRef(new AbortController());useEffect(()=>()=>controller.current.abort(),[]);
  const changes=run?.document.changes??[];
  const patches=useMemo(()=>changes.map(change=>({change,patch:unifiedPatch(change.diff),counts:diffCounts(change.diff)})),[changes]);
  useEffect(()=>{if(!patches.length){setDownload("");return;}const url=URL.createObjectURL(new Blob([patches.map(item=>item.patch).join("\n\n")],{type:"text/plain"}));setDownload(url);return()=>URL.revokeObjectURL(url);},[patches]);
  const total=patches.reduce((sum,item)=>({added:sum.added+item.counts.added,removed:sum.removed+item.counts.removed}),{added:0,removed:0}),allCollapsed=patches.length>0&&collapsed.length===patches.length;
  async function apply(change:AgentChange){
    if(change.content===undefined||!change.fileId||!change.expectedRevision)return;setBusy(change.path);setError("");
    try{
      const file=new File([change.content],change.path.split("/").at(-1)!,{type:change.mediaType??"text/plain"});
      const reservation=await request<{id:string}>("/api/v1/files/uploads",{method:"POST",signal:controller.current.signal,body:JSON.stringify({fileId:change.fileId,expectedRevision:change.expectedRevision,name:file.name,size:file.size,mediaType:file.type})},session.csrfToken);
      await request(`/api/v1/files/uploads/${reservation.id}`,{method:"PUT",signal:controller.current.signal,body:file},session.csrfToken);if(!controller.current.signal.aborted)setApplied(current=>[...current,change.path]);
    }catch(e){if(!controller.current.signal.aborted)setError(e instanceof RequestError&&e.status===409?"The file changed since this workspace was created. Your proposed edit is retained; open Files to resolve the conflict.":failure(e));}
    finally{if(!controller.current.signal.aborted)setBusy("");}
  }
  async function copy(text:string,label:string){try{await navigator.clipboard.writeText(text);setNotice(label);}catch{setError("Could not copy. Use Download review instead.");}}
  if(!changes.length)return <div className="agent-review-empty"><Icon name="file"/><h3>No changes yet</h3><p>Files Codex edits in this thread appear here for review.</p></div>;
  return <>
    <div className="agent-review-toolbar"><span className="agent-review-scope"><strong>{changes.length} {changes.length===1?"file":"files"}</strong><DiffStat {...total}/><span>· Latest run</span></span><button className="icon-button" aria-label="Wrap lines" title="Wrap lines" aria-pressed={wrap} onClick={()=>setWrap(value=>!value)}><Icon name="menu"/></button><button className="icon-button" aria-label={allCollapsed?"Expand diff":"Collapse diff"} title={allCollapsed?"Expand all":"Collapse all"} onClick={()=>setCollapsed(allCollapsed?[]:changes.map(change=>change.path))}><Icon name={allCollapsed?"grid":"list"}/></button>{download&&<a className="icon-button" href={download} download="agent-review.diff" aria-label="Download review" title="Download review"><Icon name="download"/></a>}</div>
    {patches.map(({change,patch,counts})=>{const open=!collapsed.includes(change.path);return <section key={change.path} className="agent-review-file" aria-label={change.path}>
      <header><button className="agent-review-file-toggle" aria-expanded={open} aria-label={`${open?"Collapse":"Expand"} ${change.path}`} onClick={()=>setCollapsed(current=>open?[...current,change.path]:current.filter(path=>path!==change.path))}><Icon name="chevron"/><span className="agent-review-path" title={change.path}>{change.path}</span></button><span className={`agent-change-kind is-${change.kind}`}>{change.kind==="added"?"New":change.kind==="deleted"?"Deleted":"Modified"}</span><DiffStat {...counts}/><button className="icon-button" aria-label={`Copy diff for ${change.path}`} title="Copy diff" onClick={()=>{void copy(patch,"Diff copied.");}}><Icon name="copy"/></button></header>
      {open&&<Diff source={patch} wrap={wrap}/>}
      {open&&change.content!==undefined&&change.fileId&&run?.status==="completed"&&<div className="agent-review-apply"><span>Edits stay in this thread’s working copy until applied.</span><button className="primary-button" disabled={!!busy||applied.includes(change.path)} onClick={()=>{void apply(change);}}>{busy===change.path?"Applying…":applied.includes(change.path)?"Applied to Files":"Apply this edit to Files"}</button></div>}
    </section>;})}
    <p className="agent-review-note">New files, deletions and Git publishing are not applied automatically.</p>
    <span className="sr-only" role="status">{notice}</span>{error&&<p role="alert" className="error">{error}</p>}
  </>;
}

export function AgentWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const initial=useRef(new URLSearchParams(location.search)).current;
  const layout=useRef(savedLayout(session.user.id)).current;
  const [projects,setProjects]=useState<AgentProject[]>(),[projectId,setProjectId]=useState(initial.get("project")??""),[threads,setThreads]=useState<AgentThread[]>(),[threadId,setThreadId]=useState(initial.get("thread")??"");
  const [error,setError]=useState(""),[threadError,setThreadError]=useState(""),[reload,setReload]=useState(0),[view,setView]=useState<View>("conversation"),[editor,setEditor]=useState<"project"|"thread">(),[settings,setSettings]=useState(false),[settingsSection,setSettingsSection]=useState("General"),[review,setReview]=useState(layout.review),[nav,setNav]=useState(layout.nav),[details,setDetails]=useState(layout.details),[terminal,setTerminal]=useState(layout.terminal),[query,setQuery]=useState(""),[findOpen,setFindOpen]=useState(false),[maximized,setMaximized]=useState(false),[layoutError,setLayoutError]=useState("");
  const [provider,setProvider]=useState<AgentProvider>(),[providerEpoch,setProviderEpoch]=useState(0),[checkingProvider,setCheckingProvider]=useState(true),[connectionError,setConnectionError]=useState(""),[runId,setRunId]=useState(""),[storedRun,setRun]=useState<AgentRun>(),[runError,setRunError]=useState(""),[historyEpoch,setHistoryEpoch]=useState(0),[creatingThread,setCreatingThread]=useState(false),[createError,setCreateError]=useState(""),[settling,setSettling]=useState(false);
  const {preferences,loaded:preferencesLoaded,error:preferencesError}=usePreferences();
  const completedSeen=useRef(new Set<string>()),autoStart=useRef(new Map<string,boolean>()),lifetime=useRef(new AbortController()),now=useNow(true,30000);
  const panel=useRef<HTMLDivElement>(null),tabs=useRef<HTMLElement>(null),previousView=useRef(view),returnToTabs=useRef(false),draftCache=useRef(new Map<string,string>()),positions=useRef(new Map<string,ScrollPosition>()),search=useRef<HTMLInputElement>(null);
  const thread=threads?.find(t=>t.id===threadId),project=projects?.find(p=>p.id===(thread?.projectId??projectId))??(thread?undefined:projects?.[0]);
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
  function chooseNewThread(){if(!projects)return;setCreateError("");if(!projects.length){setEditor("project");return;}setThreadId("");setProjectId(project?.id??projects[0].id);setFindOpen(false);setView("conversation");requestAnimationFrame(()=>document.getElementById("agent-message")?.focus());}
  function started(row:AgentThread,execute:boolean){autoStart.current.set(row.id,execute);setThreads(current=>[row,...(current??[]).filter(item=>item.id!==row.id)]);setProjectId(row.projectId);setThreadId(row.id);setView("conversation");}
  function openReview(){setReview(true);setView("changes");}
  function closeReview(){setReview(false);setMaximized(false);setView("conversation");requestAnimationFrame(()=>panel.current?.querySelector<HTMLButtonElement>('[aria-label="Show changes"]')?.focus());}
  function stop(){if(!runId||run&&(!activeAgentRun(run.status)||run.threadId!==threadId))return;void request<AgentRun>(`${base}/runs/${runId}/cancel`,{method:"POST",signal:lifetime.current.signal},session.csrfToken).then(value=>{if(!lifetime.current.signal.aborted)setRun(value);}).catch(e=>{if(!lifetime.current.signal.aborted)setRunError(failure(e));});}
  async function copyThread(item:AgentThread){try{const url=new URL(location.href);url.searchParams.set("thread",item.id);url.searchParams.set("project",item.projectId);await navigator.clipboard.writeText(url.href);}catch{setThreadError("Could not copy the thread link. Open the thread and copy its address.");}}
  function rows(items:AgentThread[]){return <div className="agent-thread-list">{items.map(item=>{const name=projects?.find(p=>p.id===item.projectId)?.name??"Project",working=isWorking(item),settledRow=isSettled(item);
    return <div className={`agent-thread-row${item.id===threadId?" selected":""}${settledRow?" settled":""}`} key={item.id}><button className="agent-thread-select" aria-current={item.id===threadId?"page":undefined} title={`${name} / ${item.title}`} onClick={()=>{setThreadId(item.id);setProjectId(item.projectId);setFindOpen(false);setView("conversation");}}><ProjectMark name={name}/><span className="sr-only">{name}: </span><span className="agent-thread-title">{item.title}</span></button>
      <span className="agent-thread-meta">{working?<span className="agent-thread-status"><span className="agent-pulse" aria-hidden="true"/>{item.id===threadId&&run?duration(Date.now()-run.createdAt):"Working"}</span>:<time dateTime={new Date(item.modifiedAt).toISOString()} title={new Date(item.modifiedAt).toLocaleString()}>{ago(item.modifiedAt,now)}</time>}</span>
      <span className="agent-thread-actions">{!working&&<button className="icon-button" aria-label={settledRow?`Restore ${item.title}`:`Settle ${item.title}`} title={settledRow?"Restore thread":"Settle thread"} disabled={settling||!preferencesLoaded} onClick={()=>{void settle(item);}}><Icon name={settledRow?"undo":"check"}/></button>}<ActionMenu label={`Actions for ${item.title}`} actions={[{label:settledRow?"Restore thread":"Settle thread",icon:"check",disabled:settling||!preferencesLoaded||working,run:()=>{void settle(item);}},{label:"Copy thread link",icon:"share",run:()=>{void copyThread(item);}}]}><Icon name="more"/></ActionMenu></span></div>;})}</div>;}
  const filtered=threads?.filter(row=>`${row.title} ${projects?.find(project=>project.id===row.projectId)?.name??""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()))??[];
  const active=filtered.filter(row=>!isWorking(row)&&!isSettled(row)),working=filtered.filter(isWorking),settled=filtered.filter(isSettled);
  return <div className={`app-shell agent-shell agent-view-${view}${review?"":" agent-review-hidden"}${nav?"":" agent-nav-hidden"}${details?"":" agent-details-hidden"}${maximized&&review?" agent-review-maximized":""}`}>
    <nav ref={tabs} className="agent-mobile-views" aria-label="Agent views">{(["threads","conversation","details","changes","terminal"] as const).map(value=><button key={value} aria-current={view===value?"page":undefined} onClick={()=>setView(value)}>{value[0].toUpperCase()+value.slice(1)}</button>)}</nav>
    <div className="agent-layout" ref={panel} onKeyDown={event=>{if(event.key==="Escape"&&view!=="conversation"&&!editor&&!settings){event.preventDefault();returnToTabs.current=true;setView("conversation");}}}>
      <aside className="agent-sidebar" data-agent-panel="threads" tabIndex={-1} aria-label="Threads"><h1 className="sr-only">Agent</h1><div className="agent-brand"><a href="/" aria-label="Dar home"><Icon name="agent"/>Dar</a><AppSwitcher active="agent" administrator={session.user.administrator}/></div><div className="agent-search-row"><label className="agent-thread-search"><Icon name="search"/><span className="sr-only">Search threads</span><input ref={search} type="search" placeholder="Search" value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();setQuery("");}}}/></label><button className="icon-button" aria-label="New thread" title="New thread" disabled={!projects} onClick={chooseNewThread}><Icon name="plus"/></button></div>
        {error&&<div role="alert"><p>{error}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry projects</button></div>}
        {threadError&&<div role="alert"><p>{threadError}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry threads</button></div>}
        {preferencesError&&<p role="alert" className="error">{preferencesError}</p>}
        <div className="agent-thread-shelves">{!threads?<p role="status">Loading threads…</p>:<>{rows(active)}{!threads.length&&<p className="muted small">No threads yet.</p>}{threads.length>0&&!filtered.length&&<p role="status" className="muted small">No matching threads.</p>}<div className="agent-shelves">{working.length>0&&<details open><summary>Working <span>{working.length}</span></summary>{rows(working)}</details>}<details open={query?true:undefined}><summary>Settled <span>{settled.length}</span></summary>{rows(settled)}</details></div></>}</div>
        <div className="agent-sidebar-footer"><AppSettingsButton app="agent" onClick={()=>setSettings(true)}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/></div>
      </aside>
      <main className="agent-center" data-agent-panel="conversation" tabIndex={-1} aria-label="Agent conversation"><header className="agent-thread-heading"><button className="icon-button agent-desktop-toggle" aria-label={nav?"Hide threads":"Show threads"} aria-expanded={nav} onClick={()=>setNav(v=>!v)}><Icon name="menu"/></button><div className="agent-collapsed-controls"><AppSwitcher active="agent" administrator={session.user.administrator}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/><button className="icon-button" aria-label="Agent settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div>{project&&<span className="agent-heading-project"><ProjectMark name={project.name}/>{project.name}<span>/</span></span>}<h2>{thread?.title??"New thread"}</h2>{thread&&<ActionMenu label="Thread actions" actions={[{label:"Find in thread",icon:"search",run:()=>setFindOpen(true)},{label:isSettled(thread)?"Restore thread":"Settle thread",icon:"check",disabled:settling||!preferencesLoaded||isWorking(thread),run:()=>{void settle(thread);}},{label:"Copy thread link",icon:"share",run:()=>{void copyThread(thread);}}]}><Icon name="more"/></ActionMenu>}{thread&&<div className="agent-panel-controls"><button className="icon-button agent-desktop-toggle" aria-label="Toggle thread details" aria-expanded={details} onClick={()=>setDetails(value=>!value)}><Icon name="info"/></button><button className="icon-button agent-desktop-toggle" aria-label="Toggle command output" aria-expanded={terminal} onClick={()=>setTerminal(value=>!value)}><Icon name="list"/></button><button className="icon-button agent-desktop-toggle" aria-label={review?"Hide changes":"Show changes"} aria-expanded={review} onClick={()=>review?closeReview():openReview()}><Icon name="file"/></button></div>}</header>
        {runError&&<p role="alert" className="error">{runError}</p>}
        {layoutError&&<p role="alert" className="error">{layoutError}</p>}
        <div className="agent-stage"><div className="agent-chat">{thread?<Conversation key={thread.id} thread={thread} project={project} session={session} onChanged={changed} draftCache={draftCache.current} positions={positions.current} provider={provider} checkingProvider={checkingProvider} run={run} onRun={setRunId} onStop={stop} onReview={openReview} historyEpoch={historyEpoch} findOpen={findOpen} onCloseFind={()=>setFindOpen(false)} autoStart={autoStart.current.get(thread.id)} onStarted={()=>autoStart.current.delete(thread.id)}/>:project?<DraftComposer key={project.id} session={session} project={project} provider={provider} checkingProvider={checkingProvider} draftCache={draftCache.current} onChooseProject={()=>setEditor("thread")} onCreated={started}/>:<div className="agent-empty agent-hero">{!projects?<p role="status">Loading projects…</p>:<><h2>Add a project to start</h2><p>Work in a Git repository or a folder in Dar Files. Codex uses an isolated copy.</p><button className="primary-button" onClick={()=>setEditor("project")}><Icon name="plus"/>Add project</button></>}</div>}</div>
        {thread&&<aside className="agent-context" data-agent-panel="details" tabIndex={-1} aria-label="Thread details"><div className="agent-context-card"><div className="agent-context-project"><ProjectMark name={project?.name??"Project"}/><strong>{project?.name??"Project"}</strong><span>{project?.sourceType==="files"?"Dar Files":"Git"}</span></div>{project?.sourceType==="files"?<a href={`/?folder=${project.sourceRef}`}><Icon name="folder"/>Open folder in Files<Icon name="chevron"/></a>:project&&<div className="agent-source-ref"><Icon name="share"/><code>{project.sourceRef}</code></div>}<div className="agent-workspace-label"><Icon name="lock"/>Isolated working copy</div><button onClick={openReview}><Icon name="file"/>Changes{run?.document.changes.length?<DiffStat {...run.document.changes.map(change=>diffCounts(change.diff)).reduce((sum,item)=>({added:sum.added+item.added,removed:sum.removed+item.removed}),{added:0,removed:0})}/>:<span>0</span>}<Icon name="chevron"/></button><div className="agent-context-provider"><span className={provider?.connected?"agent-connected":""}/>{checkingProvider?"Checking Codex…":provider?.connected?"Codex connected":"Codex unavailable"}<button className="icon-button" aria-label="Connection settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div></div></aside>}</div>
        <section className={`agent-terminal${terminal?" is-open":""}`} data-agent-panel="terminal" tabIndex={-1} aria-label="Terminal output"><div className="agent-output-heading"><span>Command output</span><span className="muted">Read only</span><button className="icon-button" aria-label="Close command output" onClick={()=>{setTerminal(false);setView("conversation");}}><Icon name="close"/></button></div>{run?.document.terminal?<pre>{run.document.terminal}</pre>:<p>No command output yet.</p>}</section>
      </main>
      <aside className="agent-review" data-agent-panel="changes" tabIndex={-1} aria-label="Changes"><header className="agent-review-heading"><h2>Changes</h2><button className="icon-button agent-desktop-toggle" aria-label={maximized?"Restore review size":"Maximize review"} onClick={()=>setMaximized(value=>!value)}><Icon name="grid"/></button><button className="icon-button" aria-label="Close changes" onClick={closeReview}><Icon name="close"/></button></header><div className="agent-review-content"><ChangeReview key={run?.id??"empty"} run={run} session={session}/></div></aside>
    </div>
    {editor==="project"&&<ProjectEditor session={session} onClose={()=>setEditor(projects?.length?"thread":undefined)} onCreated={row=>{setProjects(current=>[...current??[],row]);setProjectId(row.id);setThreadId("");setEditor(undefined);setView("conversation");}}/>}
    {editor==="thread"&&projects&&<ThreadChooser projects={projects} busy={false} error={createError} onClose={()=>setEditor(undefined)} onChoose={row=>{setProjectId(row.id);setThreadId("");setEditor(undefined);setView("conversation");}} onAdd={()=>setEditor("project")}/>}
    {settings&&<Dialog title="Agent settings" onClose={()=>setSettings(false)}><div className="ui-settings-dialog"><nav aria-label="Settings sections">{["General","Keyboard shortcuts"].map(section=><button key={section} className={settingsSection===section?"active":""} aria-current={settingsSection===section?"page":undefined} onClick={()=>setSettingsSection(section)}>{section}</button>)}</nav><section>{settingsSection==="General"?<><h3>Codex connection</h3><p role="status">{checkingProvider?"Checking Codex…":provider?.message??"Connection could not be checked."}</p>{connectionError&&<p className="error" role="alert">{connectionError}</p>}<div className="agent-connection-actions">{session.user.administrator&&!provider?.configured&&<button className="primary-button" disabled={checkingProvider} onClick={()=>{void connectProvider();}}>Connect server Codex</button>}<button className="secondary-button" disabled={checkingProvider} onClick={()=>setProviderEpoch(n=>n+1)}>{checkingProvider?"Checking…":"Refresh connection"}</button></div><p className="muted small">Runs send the conversation and selected project contents to Codex. Commands have no network access; publishing is disabled.</p><p className="muted small">Uses the Codex ChatGPT login available to the Linux account running Dar. Connect links it to your Dar account. Login tokens stay on the server.</p><p className="muted small">Dar account ID: <code className="agent-settings-account">{session.user.id}</code></p><h3>Workspace</h3><p className="muted small">Panel visibility is remembered for this account in this browser. Drafts and selected models are kept in this tab.</p></>:<><h3>Keyboard shortcuts</h3><dl className="ui-shortcuts"><dt>Enter / Ctrl / Cmd Enter</dt><dd>Run with Codex when connected; otherwise save the message</dd><dt>Shift Enter</dt><dd>Insert a new line</dd><dt>Ctrl / Cmd K</dt><dd>Search threads</dd><dt>Ctrl / Cmd F</dt><dd>Find in loaded thread messages</dd><dt>Escape</dt><dd>Return from a phone panel or close settings</dd></dl></>}</section></div></Dialog>}
  </div>;
}
