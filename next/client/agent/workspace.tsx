import {Fragment,useEffect,useLayoutEffect,useMemo,useRef,useState,type FormEvent,type KeyboardEvent as ReactKeyboardEvent} from "react";
import {request,RequestError,type BrowserSession} from "../api.ts";
import {WorkspaceAccount} from "../shell.tsx";
import {AppSwitcher} from "../apps.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {EditorPane} from "../pane.tsx";
import {Dialog} from "../dialog.tsx";
import {ActionMenu,Select} from "../controls.tsx";
import {Markdown} from "../pim/markdown.tsx";
import {AppSettingsButton,usePreferences,savePreferences} from "../app-settings.tsx";
import type {Preferences} from "../../shared/preferences.ts";
import {DarFilePicker,type DarFileSelection} from "../file-picker.tsx";
import {activeAgentRun,type AgentProject,type AgentThread,type AgentMessage,type AgentHistory,type AgentRun,type AgentProvider,type AgentChange,type AgentModel,type AgentStep,type AgentRunSummary,type AgentAttachment,type AgentLimits,agentImageTypes,agentAttachmentLimits} from "../../shared/agent.ts";
import {ThreadTerminal} from "./terminal.tsx";
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
type Layout={nav:boolean;details:boolean;review:boolean;terminal:boolean;sidebarWidth:number;reviewWidth:number};
function savedLayout(userId:string):Layout{
  const defaults:Layout={nav:true,details:true,review:false,terminal:false,sidebarWidth:256,reviewWidth:560};
  try{const value=JSON.parse(localStorage.getItem(`dar.agent.layout.${userId}`)??"{}");for(const key of Object.keys(defaults) as (keyof Layout)[])if(typeof value?.[key]===typeof defaults[key])(defaults as Record<string,unknown>)[key]=value[key];}catch{}
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

const effortLabel=(value:string)=>value==="xhigh"?"Extra high":value[0].toUpperCase()+value.slice(1);
/** Compact searchable model menu with the model's declared reasoning levels. Escape and outside presses close it. */
function ModelPicker({models,value,onChange,effort,onEffort,disabled}:{models:AgentModel[];value:string;onChange:(id:string)=>void;effort:string;onEffort:(value:string)=>void;disabled?:boolean}){
  const [open,setOpen]=useState(false),[query,setQuery]=useState(""),[active,setActive]=useState(0),root=useRef<HTMLDivElement>(null),trigger=useRef<HTMLButtonElement>(null),search=useRef<HTMLInputElement>(null),list=useRef<HTMLUListElement>(null);
  const shown=models.filter(model=>`${model.name} ${model.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())),selected=models.find(model=>model.id===value);
  useEffect(()=>{if(!open)return;setQuery("");setActive(Math.max(0,models.findIndex(model=>model.id===value)));requestAnimationFrame(()=>(search.current??list.current)?.focus());
    const dismiss=(event:PointerEvent)=>{if(!root.current?.contains(event.target as Node))setOpen(false);};document.addEventListener("pointerdown",dismiss);return()=>document.removeEventListener("pointerdown",dismiss);},[open]);
  useEffect(()=>{list.current?.querySelector("[data-active]")?.scrollIntoView({block:"nearest"});},[active,open]);
  function choose(id:string){onChange(id);if(!selected?.efforts?.length||!models.find(model=>model.id===id)?.efforts?.length){setOpen(false);trigger.current?.focus();}}
  function keys(event:ReactKeyboardEvent){
    if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setOpen(false);trigger.current?.focus();}
    else if((event.key==="ArrowDown"||event.key==="ArrowUp")&&!(event.target as HTMLElement).closest(".agent-effort")){event.preventDefault();setActive(index=>shown.length?(index+(event.key==="ArrowDown"?1:-1)+shown.length)%shown.length:0);}
    else if(event.key==="Enter"&&shown[active]&&!(event.target as HTMLElement).closest("button")){event.preventDefault();onChange(shown[active].id);setOpen(false);trigger.current?.focus();}
  }
  return <div className="agent-model-picker" ref={root}><button type="button" ref={trigger} className="agent-model-trigger" aria-label={`Codex model: ${selected?.name??"none"}${effort?`, ${effortLabel(effort)} reasoning`:""}`} aria-haspopup="dialog" aria-expanded={open} disabled={disabled} onClick={()=>setOpen(value=>!value)}><Icon name="agent"/><span>{selected?.name??"Choose model"}</span>{effort&&<span className="agent-effort-chip">{effortLabel(effort)}</span>}<Icon name="chevron"/></button>
    {open&&<div className="agent-model-popover" role="dialog" aria-label="Model and reasoning" onKeyDown={keys}>{models.length>6&&<input ref={search} type="search" aria-label="Search models" placeholder="Search models" value={query} onChange={event=>{setQuery(event.target.value);setActive(0);}}/>}<ul ref={list} role="listbox" aria-label="Codex models" tabIndex={-1} aria-activedescendant={shown[active]?`agent-model-${active}`:undefined}>{shown.map((model,index)=><li key={model.id} id={`agent-model-${index}`} role="option" aria-selected={model.id===value} data-active={index===active||undefined} onPointerEnter={()=>setActive(index)} onClick={()=>choose(model.id)}><span>{model.name}</span>{model.isDefault&&<small>Default</small>}{model.id===value&&<Icon name="check"/>}</li>)}{!shown.length&&<li className="muted" role="presentation">No matching models</li>}</ul>
      {selected?.efforts?.length?<div className="agent-effort" role="radiogroup" aria-label="Reasoning"><span>Reasoning</span><div>{selected.efforts.map(level=><button key={level} type="button" role="radio" aria-checked={effort===level} onClick={()=>onEffort(level)} onKeyDown={event=>{if(event.key==="ArrowRight"||event.key==="ArrowLeft"){event.preventDefault();const all=selected.efforts!,next=all[(all.indexOf(effort)+(event.key==="ArrowRight"?1:-1)+all.length)%all.length];onEffort(next);(event.currentTarget.parentElement?.children[all.indexOf(next)] as HTMLElement|undefined)?.focus();}}}>{effortLabel(level)}</button>)}</div></div>:null}
    </div>}
  </div>;
}
/** Keeps a selected model/effort valid for the current catalog; effort is cleared when unsupported. */
function useModelChoice(provider:AgentProvider|undefined,storageKey:string){
  const [model,setModel]=useState(()=>{try{return sessionStorage.getItem(storageKey+".model")??"";}catch{return "";}});
  const [effort,setEffort]=useState(()=>{try{return sessionStorage.getItem(storageKey+".effort")??"";}catch{return "";}});
  useEffect(()=>{if(provider?.models.length)setModel(current=>provider.models.some(m=>m.id===current)?current:(provider.models.find(m=>m.isDefault)??provider.models[0]).id);},[provider]);
  const selected=provider?.models.find(item=>item.id===model);
  useEffect(()=>{if(!selected)return;setEffort(current=>selected.efforts?.includes(current)?current:selected.defaultEffort??"");},[selected]);
  useEffect(()=>{try{if(model)sessionStorage.setItem(storageKey+".model",model);if(effort)sessionStorage.setItem(storageKey+".effort",effort);else sessionStorage.removeItem(storageKey+".effort");}catch{}},[model,effort,storageKey]);
  return {model,setModel,effort:selected?.efforts?.includes(effort)?effort:"",setEffort};
}

function StepEntry({step}:{step:AgentStep}){
  const [open,setOpen]=useState(false),failed=step.status==="failed",running=step.status==="running";
  if(step.kind==="reasoning")return <li className="agent-step agent-step-reasoning"><Icon name="agent"/><span><em>{step.label}</em></span></li>;
  if(step.kind==="plan")return <li className="agent-step"><Icon name="list"/><span><span className="agent-step-title">Plan</span><ul className="agent-plan">{step.items?.map((item,index)=><li key={index} data-done={item.done||undefined}><span aria-hidden="true">{item.done?"✓":"○"}</span><span className="sr-only">{item.done?"Done: ":"To do: "}</span><span>{item.text}</span></li>)}</ul></span></li>;
  if(step.kind==="edit")return <li className="agent-step"><Icon name="pencil"/><span>{failed?"Failed to edit":running?"Editing":"Edited"} {step.label.split("\n").map((file,index)=><code key={index} className="agent-step-file">{file}</code>)}</span></li>;
  if(step.kind==="question")return <li className="agent-step"><Icon name="info"/><span>Asked <em>{step.label}</em>{step.items?.map((item,index)=><span key={index} className="agent-step-answer">{item.text}</span>)}</span></li>;
  if(step.kind==="tool"&&failed&&step.label.startsWith("Declined:"))return <li className="agent-step is-failed"><Icon name="lock"/><span>{step.label} <small className="muted">· Dar keeps commands inside the sandbox</small></span></li>;
  if(step.kind==="command")return <li className={`agent-step${failed?" is-failed":""}`}><Icon name="terminal"/><span><button type="button" className="agent-step-command" aria-expanded={open} disabled={!step.output} onClick={()=>setOpen(value=>!value)}>{running?"Running":"Ran"} <code>{step.label}</code>{step.exitCode!==undefined&&step.exitCode!==0&&<span className="agent-step-exit">exit {step.exitCode}</span>}{running&&<span className="agent-pulse" aria-hidden="true"/>}</button>{open&&step.output&&<pre className="agent-step-output">{step.output}</pre>}</span></li>;
  return <li className={`agent-step${failed?" is-failed":""}`}><Icon name={step.kind==="search"?"search":"plug"}/><span>{step.kind==="search"?"Searched":"Called"} <code>{step.label}</code></span></li>;
}
const tokens=(value:number)=>value>=1000?`${(value/1000).toFixed(value>=10000?0:1)}k`:String(value);
/** Existing run data stays truthful: recorded steps and edits, never invented tool events. */
function WorkLog({run,onReview}:{run:AgentRun;onReview:()=>void}){
  const running=activeAgentRun(run.status),now=useNow(running),[open,setOpen]=useState(running);
  useEffect(()=>setOpen(running),[running]);
  const spent=duration((running?now:run.modifiedAt)-run.createdAt),usage=run.document.usage;
  const label=running?run.status==="cancelling"?`Stopping · ${spent}`:`Working for ${spent}`:run.status==="completed"?`Worked for ${spent}`:run.status==="cancelled"?`Stopped after ${spent}`:run.status==="failed"?`Failed after ${spent}`:`Interrupted after ${spent}`;
  const steps=run.document.steps,entries=run.document.activity,changes=run.document.changes,count=steps?.length??entries.length;
  return <section className={`agent-work${running?" is-running":""}`} aria-label="Work log"><span className="sr-only" role="status">Codex · {run.status}</span>
    <button type="button" className="agent-work-toggle" aria-expanded={open} onClick={()=>setOpen(value=>!value)}>{running&&<span className="agent-pulse" aria-hidden="true"/>}<span>{label}</span>{!running&&count>0&&<span className="agent-work-count">· {count} {count===1?"step":"steps"}</span>}{usage&&!running&&<span className="agent-work-count" title={`${usage.input} input tokens (${usage.cached} cached), ${usage.output} output tokens`}>· {tokens(usage.input+usage.output)} tokens</span>}{(count>0||!running)&&<Icon name="chevron"/>}</button>
    {open&&<ol className="agent-work-entries">{steps?steps.map(step=><StepEntry key={step.id} step={step}/>):entries.map((item,i)=>item==="Editing workspace files"?<li key={i}><Icon name="pencil"/><span>Edited workspace files</span></li>:<li key={i}><Icon name="terminal"/><span>Ran <code>{item}</code></span></li>)}{!count&&<li className="muted"><Icon name="info"/><span>{running?"Waiting for the first step…":"No commands or edits were recorded."}</span></li>}</ol>}
    {run.document.truncated&&<p className="agent-work-note" role="status">Output is truncated.</p>}{run.document.error&&<p role="alert" className="error agent-work-note">{run.document.error}</p>}
    {!running&&changes.length>0&&<div className="agent-changed-card"><div className="agent-changed-heading"><strong>{changes.length} {changes.length===1?"file":"files"} changed</strong><DiffStat {...changes.map(change=>diffCounts(change.diff)).reduce((sum,item)=>({added:sum.added+item.added,removed:sum.removed+item.removed}),{added:0,removed:0})}/><button type="button" className="text-button" onClick={onReview}>Review changes</button></div><ul>{changes.map(change=><li key={change.path}><Icon name={change.kind==="deleted"?"trash":change.kind==="added"?"plus":"file"}/><span title={change.path}>{change.path}</span><DiffStat {...diffCounts(change.diff)}/></li>)}</ul></div>}
  </section>;
}
/** Earlier turns load their saved run document only when opened. */
function EarlierRun({summary}:{summary:AgentRunSummary}){
  const [run,setRun]=useState<AgentRun>(),[open,setOpen]=useState(false),[error,setError]=useState("");
  useEffect(()=>{if(!open||run)return;const abort=new AbortController();void request<AgentRun>(`${base}/runs/${summary.id}`,{signal:abort.signal}).then(setRun).catch(e=>{if(!abort.signal.aborted)setError(failure(e));});return()=>abort.abort();},[open,run,summary.id]);
  const spent=duration(summary.modifiedAt-summary.createdAt),label=summary.status==="completed"?`Worked for ${spent}`:summary.status==="cancelled"?`Stopped after ${spent}`:summary.status==="failed"?`Failed after ${spent}`:`Interrupted after ${spent}`;
  if(open&&run)return <div className="agent-earlier-run"><WorkLogOpen run={run} onClose={()=>setOpen(false)}/></div>;
  return <section className="agent-work" aria-label="Earlier work log"><button type="button" className="agent-work-toggle" aria-expanded={false} onClick={()=>setOpen(true)}><span>{label}</span><Icon name="chevron"/></button>{open&&!run&&!error&&<p className="agent-work-note" role="status">Loading…</p>}{error&&<p className="error agent-work-note" role="alert">{error}</p>}</section>;
}
function WorkLogOpen({run,onClose}:{run:AgentRun;onClose:()=>void}){
  const spent=duration(run.modifiedAt-run.createdAt),steps=run.document.steps;
  return <section className="agent-work" aria-label="Earlier work log"><button type="button" className="agent-work-toggle" aria-expanded={true} onClick={onClose}><span>{run.status==="completed"?"Worked":"Ran"} for {spent}</span><Icon name="chevron"/></button><ol className="agent-work-entries">{steps?steps.map(step=><StepEntry key={step.id} step={step}/>):run.document.activity.map((item,i)=><li key={i}><Icon name="terminal"/><span>Ran <code>{item}</code></span></li>)}{!(steps?.length??run.document.activity.length)&&<li className="muted"><Icon name="info"/><span>No commands or edits were recorded.</span></li>}</ol>{run.document.changes.length>0&&<p className="agent-work-note">{run.document.changes.length} {run.document.changes.length===1?"file":"files"} changed in this turn.</p>}</section>;
}

type Picture={id:string;file:File;url:string;status:"uploading"|"ready"|"failed";error?:string};
/** Images upload as soon as they are added; a run references only uploaded IDs. */
function usePictures(threadId:string|undefined,session:BrowserSession){
  const [pictures,setPictures]=useState<Picture[]>([]),controller=useRef(new AbortController()),current=useRef(pictures);current.current=pictures;
  useEffect(()=>()=>{controller.current.abort();for(const item of current.current)URL.revokeObjectURL(item.url);},[]);
  const upload=(item:Picture,thread:string)=>{void request<AgentAttachment>(`${base}/threads/${thread}/attachments/${item.id}?name=${encodeURIComponent(item.file.name.replace(/[\x00-\x1f/\\]/g,"_").slice(0,200)||"image")}`,{method:"PUT",signal:controller.current.signal,headers:{"content-type":item.file.type},body:item.file},session.csrfToken)
    .then(()=>setPictures(list=>list.map(row=>row.id===item.id?{...row,status:"ready"}:row)))
    .catch(e=>{if(!controller.current.signal.aborted)setPictures(list=>list.map(row=>row.id===item.id?{...row,status:"failed",error:failure(e)}:row));});};
  function add(files:File[]){
    const accepted=files.filter(file=>(agentImageTypes as readonly string[]).includes(file.type)&&file.size<=agentAttachmentLimits.bytes).slice(0,Math.max(0,agentAttachmentLimits.count-current.current.length));
    const rows=accepted.map(file=>({id:crypto.randomUUID(),file,url:URL.createObjectURL(file),status:threadId?"uploading" as const:"ready" as const}));
    setPictures(list=>[...list,...rows]);if(threadId)for(const row of rows)upload(row,threadId);
    return accepted.length===files.length?"":`Only PNG, JPEG, GIF or WebP images up to 8 MiB, at most ${agentAttachmentLimits.count} per message.`;
  }
  function remove(id:string){setPictures(list=>{const row=list.find(item=>item.id===id);if(row)URL.revokeObjectURL(row.url);return list.filter(item=>item.id!==id);});}
  function clear(){for(const item of current.current)URL.revokeObjectURL(item.url);setPictures([]);}
  return {pictures,add,remove,clear,busy:pictures.some(item=>item.status==="uploading"),failed:pictures.some(item=>item.status==="failed")};
}
function PictureTray({pictures,onRemove}:{pictures:Picture[];onRemove:(id:string)=>void}){
  if(!pictures.length)return null;
  return <ul className="agent-pictures" aria-label="Attached images">{pictures.map(item=><li key={item.id} data-status={item.status}><img src={item.url} alt={item.file.name}/>{item.status==="uploading"&&<span className="agent-picture-state" role="status">Uploading…</span>}{item.status==="failed"&&<span className="agent-picture-state error" role="alert" title={item.error}>Failed</span>}<button type="button" className="icon-button" aria-label={`Remove ${item.file.name}`} onClick={()=>onRemove(item.id)}><Icon name="close"/></button></li>)}</ul>;
}
function AttachButton({onFiles,disabled}:{onFiles:(files:File[])=>void;disabled?:boolean}){
  const input=useRef<HTMLInputElement>(null);
  return <><button type="button" className="icon-button agent-attach" aria-label="Attach images" title="Attach images" disabled={disabled} onClick={()=>input.current?.click()}><Icon name="plus"/></button><input ref={input} type="file" accept={agentImageTypes.join(",")} multiple hidden onChange={event=>{onFiles([...event.target.files??[]]);event.target.value="";}}/></>;
}
/** Remaining model context from the latest turn's reported input size. */
function ContextMeter({used,window}:{used?:number;window?:number}){
  if(!used||!window)return null;const share=Math.min(1,used/window),percent=Math.round(share*100);
  return <span className="agent-context-meter" role="img" aria-label={`Context ${percent}% used: ${tokens(used)} of ${tokens(window)} tokens`} title={`${tokens(used)} of ${tokens(window)} tokens in the last request`} data-level={share>.85?"high":share>.6?"medium":undefined}><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="8"/><circle cx="10" cy="10" r="8" pathLength="100" strokeDasharray={`${percent} 100`}/></svg><span>{percent}%</span></span>;
}
/** One blocking question from the running turn. Secret answers are never echoed back into the log. */
function QuestionCard({run,session}:{run:AgentRun;session:BrowserSession}){
  const pending=run.document.pending!,[answers,setAnswers]=useState<Record<string,string>>({}),[other,setOther]=useState<Record<string,string>>({}),[busy,setBusy]=useState(false),[error,setError]=useState("");
  useEffect(()=>{setAnswers({});setOther({});setError("");},[pending.id]);
  const value=(id:string)=>answers[id]==="\u0000other"?other[id]?.trim()??"":answers[id]??"";
  const ready=pending.questions.every(question=>value(question.id));
  async function submit(event:FormEvent){
    event.preventDefault();if(!ready||busy)return;setBusy(true);setError("");
    try{await request(`${base}/runs/${run.id}/answer`,{method:"POST",body:JSON.stringify({requestId:pending.id,answers:Object.fromEntries(pending.questions.map(question=>[question.id,[value(question.id)]]))})},session.csrfToken);}
    catch(e){setError(failure(e));setBusy(false);}
  }
  return <form className="agent-question" aria-label="Codex is asking" onSubmit={event=>{void submit(event);}}><div className="agent-question-title"><Icon name="info"/><strong>Codex needs your input</strong></div>
    {pending.questions.map(question=><fieldset key={question.id}><legend>{question.header&&<small>{question.header}</small>}<span>{question.question}</span></legend>
      {question.options.map(option=><label key={option.label}><input type="radio" name={`q-${question.id}`} checked={answers[question.id]===option.label} onChange={()=>setAnswers(current=>({...current,[question.id]:option.label}))}/><span><strong>{option.label}</strong>{option.description&&<small>{option.description}</small>}</span></label>)}
      {(question.other||!question.options.length)&&<label className="agent-question-other">{question.options.length>0&&<input type="radio" name={`q-${question.id}`} checked={answers[question.id]==="\u0000other"} onChange={()=>setAnswers(current=>({...current,[question.id]:"\u0000other"}))}/>}<input type={question.secret?"password":"text"} aria-label={question.options.length?"Other answer":question.question} placeholder={question.options.length?"Other…":"Your answer"} value={other[question.id]??""} maxLength={2000} onFocus={()=>setAnswers(current=>({...current,[question.id]:"\u0000other"}))} onChange={event=>setOther(current=>({...current,[question.id]:event.target.value}))}/></label>}
    </fieldset>)}
    {error&&<p role="alert" className="error">{error}</p>}<div className="agent-question-actions"><button className="primary-button" disabled={!ready||busy}>{busy?"Sending…":"Send answer"}</button></div></form>;
}

/** Overview of the loaded timeline: one mark per message, the visible range, click to jump. */
function Minimap({viewport,content,version}:{viewport:React.RefObject<HTMLDivElement|null>;content:React.RefObject<HTMLDivElement|null>;version:string}){
  const [marks,setMarks]=useState<{top:number;height:number;role:string;id:string}[]>([]),[view,setView]=useState({top:0,height:1}),rail=useRef<HTMLDivElement>(null);
  useEffect(()=>{
    const element=viewport.current,node=content.current;if(!element||!node)return;
    const measure=()=>{const total=Math.max(1,element.scrollHeight);setMarks([...node.querySelectorAll<HTMLElement>("[data-message-id]")].map(item=>({top:item.offsetTop/total,height:Math.max(.004,item.offsetHeight/total),role:item.classList.contains("agent-message-user")?"user":"assistant",id:item.dataset.messageId!})));setView({top:element.scrollTop/total,height:Math.min(1,element.clientHeight/total)});};
    measure();const observer=new ResizeObserver(measure);observer.observe(node);observer.observe(element);element.addEventListener("scroll",measure,{passive:true});
    return()=>{observer.disconnect();element.removeEventListener("scroll",measure);};
  },[viewport,content,version]);
  // Positions are measurements, applied as custom properties rather than markup styles.
  useLayoutEffect(()=>{const node=rail.current;if(!node)return;const items=node.querySelectorAll<HTMLElement>(".agent-minimap-mark");marks.forEach((mark,index)=>{items[index]?.style.setProperty("--top",String(mark.top));items[index]?.style.setProperty("--height",String(mark.height));});node.style.setProperty("--view-top",String(view.top));node.style.setProperty("--view-height",String(view.height));});
  if(marks.length<4||view.height>=1)return null;
  const jump=(fraction:number)=>{const element=viewport.current;if(element)element.scrollTop=fraction*element.scrollHeight-element.clientHeight/2;};
  return <div className="agent-minimap" ref={rail} aria-hidden="true" onPointerDown={event=>{const box=rail.current!.getBoundingClientRect();jump((event.clientY-box.top)/box.height);}}>{marks.map(mark=><span key={mark.id} data-role={mark.role} className="agent-minimap-mark"/>)}<span className="agent-minimap-view"/></div>;
}
export type SlashCommand={name:string;hint:string;argument?:string[];run:(argument:string)=>void};
type Suggestion={key:string;label:string;detail?:string;apply:()=>void};
/** Prompt editor: / commands at the start, @ workspace paths, prompt history and the chosen send key. */
function PromptInput({inputRef,value,onChange,onSubmit,disabled,placeholder,sendKey,threadId,prompts,commands,onFiles}:{inputRef:React.RefObject<HTMLTextAreaElement|null>;value:string;onChange:(value:string)=>void;onSubmit:()=>void;disabled?:boolean;placeholder:string;sendKey:"enter"|"modEnter";threadId?:string;prompts:string[];commands:SlashCommand[];onFiles?:(files:File[])=>void}){
  const [caret,setCaret]=useState(0),[active,setActive]=useState(0),[files,setFiles]=useState<{query:string;items:string[];source:string}>(),[dismissed,setDismissed]=useState(""),historyIndex=useRef(-1),stash=useRef("");
  const before=value.slice(0,caret),slash=/^\/([a-z-]*)(?: ([a-z]*))?$/.exec(value),mention=/(?:^|\s)@([^\s@]*)$/.exec(before);
  const trigger=slash?"slash:"+value:mention?"mention:"+mention[1]+":"+caret:"";
  useEffect(()=>{if(!mention||!threadId){setFiles(undefined);return;}const query=mention[1],abort=new AbortController(),timer=setTimeout(()=>{void request<{source:string;items:string[]}>(`${base}/threads/${threadId}/files?query=${encodeURIComponent(query)}`,{signal:abort.signal}).then(value=>setFiles({query,...value})).catch(()=>{});},120);return()=>{clearTimeout(timer);abort.abort();};},[mention?.[1],threadId]);
  let suggestions:Suggestion[]=[],heading="";
  if(slash&&dismissed!==trigger){
    const command=commands.find(item=>item.name===slash[1]);
    if(command?.argument&&slash[2]!==undefined){heading=`/${command.name}`;suggestions=command.argument.filter(item=>item.startsWith(slash[2]??"")).map(item=>({key:item,label:effortLabel(item),apply:()=>{command.run(item);onChange("");}}));}
    else{heading="Commands";suggestions=commands.filter(item=>item.name.startsWith(slash[1])).map(item=>({key:item.name,label:"/"+item.name,detail:item.hint,apply:()=>{if(item.argument){onChange(`/${item.name} `);setCaret(item.name.length+2);}else{item.run("");onChange("");}}}));}
  }else if(mention&&dismissed!==trigger){
    heading=threadId?files?.source==="none"?"Files appear after the first run":"Files":"Files are available once the thread starts";
    suggestions=(files?.query===mention[1]?files.items:[]).slice(0,12).map(path=>({key:path,label:path.split("/").at(-1)!,detail:path.includes("/")?path:undefined,apply:()=>{const start=caret-mention[1].length-1,next=value.slice(0,start)+"@"+path+" "+value.slice(caret);onChange(next);const position=start+path.length+2;setCaret(position);requestAnimationFrame(()=>{inputRef.current?.setSelectionRange(position,position);});}}));
  }
  const open=!!heading;
  useEffect(()=>setActive(0),[trigger,suggestions.length]);
  function keys(event:ReactKeyboardEvent<HTMLTextAreaElement>){
    if(event.nativeEvent.isComposing)return;
    if(open&&suggestions.length&&(event.key==="ArrowDown"||event.key==="ArrowUp")){event.preventDefault();setActive(index=>(index+(event.key==="ArrowDown"?1:-1)+suggestions.length)%suggestions.length);return;}
    if(open&&suggestions[active]&&(event.key==="Tab"||event.key==="Enter"&&!event.shiftKey)){event.preventDefault();suggestions[active].apply();return;}
    if(open&&event.key==="Escape"){event.preventDefault();event.stopPropagation();setDismissed(trigger);return;}
    const field=event.currentTarget;
    if(event.key==="ArrowUp"&&prompts.length&&field.selectionStart===0&&field.selectionEnd===0&&(historyIndex.current>=0||!value)){event.preventDefault();if(historyIndex.current<0)stash.current=value;historyIndex.current=Math.min(prompts.length-1,historyIndex.current+1);onChange(prompts[prompts.length-1-historyIndex.current]);return;}
    if(event.key==="ArrowDown"&&historyIndex.current>=0&&field.selectionStart===value.length){event.preventDefault();historyIndex.current--;onChange(historyIndex.current<0?stash.current:prompts[prompts.length-1-historyIndex.current]);return;}
    const modifier=event.ctrlKey||event.metaKey;
    if(event.key==="Enter"&&!event.shiftKey&&!event.altKey&&(sendKey==="enter"||modifier)){event.preventDefault();if(slash){const command=commands.find(item=>item.name===slash[1]);if(command&&(!command.argument||command.argument.includes(slash[2]??""))){command.run(slash[2]??"");onChange("");return;}}historyIndex.current=-1;onSubmit();}
  }
  return <div className="agent-prompt">{open&&<div className="agent-suggestions" role="listbox" id="agent-suggestions" aria-label={heading}><div className="agent-suggestions-heading">{heading}</div>{suggestions.map((item,index)=><div key={item.key} id={`agent-suggestion-${index}`} role="option" aria-selected={index===active} data-active={index===active||undefined} onPointerDown={event=>event.preventDefault()} onPointerEnter={()=>setActive(index)} onClick={()=>item.apply()}><strong>{item.label}</strong>{item.detail&&<small>{item.detail}</small>}</div>)}{!suggestions.length&&<div className="agent-suggestions-empty">{slash?"No matching commands":threadId&&files?.source!=="none"?"No matching files":""}</div>}</div>}
    <label className="sr-only" htmlFor="agent-message">Message</label><textarea id="agent-message" ref={inputRef} value={value} disabled={disabled} placeholder={placeholder} role="combobox" aria-expanded={open} aria-controls={open?"agent-suggestions":undefined} aria-autocomplete="list" aria-activedescendant={open&&suggestions[active]?`agent-suggestion-${active}`:undefined} onChange={event=>{historyIndex.current=-1;setDismissed("");onChange(event.target.value);setCaret(event.target.selectionStart);}} onSelect={event=>setCaret(event.currentTarget.selectionStart)} onKeyDown={keys} onPaste={event=>{const files=[...event.clipboardData.files].filter(file=>file.type.startsWith("image/"));if(files.length&&onFiles){event.preventDefault();onFiles(files);}}}/></div>;
}

/** Draft and operation IDs survive reload; uncertain appends can be retried safely. */
function Conversation({thread,project,session,onChanged,draftCache,positions,provider,run,onRun,onStop,onReview,historyEpoch,findOpen,onCloseFind,autoStart,onStarted,checkingProvider,sendKey,commands,initialFiles,onForked}:{thread:AgentThread;project?:AgentProject;session:BrowserSession;onChanged:(thread:AgentThread)=>void;draftCache:Map<string,string>;positions:Map<string,ScrollPosition>;provider?:AgentProvider;run?:AgentRun;onRun:(id:string)=>void;onStop:()=>void;onReview:()=>void;historyEpoch:number;findOpen:boolean;onCloseFind:()=>void;autoStart?:boolean;onStarted:()=>void;checkingProvider:boolean;sendKey:"enter"|"modEnter";commands:(controls:{setModel:(id:string)=>void;setEffort:(value:string)=>void;efforts:string[];find:()=>void})=>SlashCommand[];initialFiles?:File[];onForked:(thread:AgentThread)=>void}){
  const key=`dar.agent.draft.${session.user.id}.${thread.id}`;
  const [draft,setDraft]=useState(()=>{if(draftCache.has(key))return draftCache.get(key)!;try{return sessionStorage.getItem(key)??"";}catch{return "";}});
  const [history,setHistory]=useState<AgentHistory>(),[error,setError]=useState(""),[storageError,setStorageError]=useState(""),[busy,setBusy]=useState(false),[notice,setNotice]=useState(""),[reload,setReload]=useState(0),[loadingEarlier,setLoadingEarlier]=useState(false);
  const controller=useRef(new AbortController()),composer=useRef<HTMLTextAreaElement>(null),pending=useRef<{id:string;content:string}|undefined>(undefined),draftRef=useRef(draft);
  const viewport=useRef<HTMLDivElement>(null),content=useRef<HTMLDivElement>(null),following=useRef(true),positioned=useRef(false),prepend=useRef<{height:number;top:number}|undefined>(undefined),findInput=useRef<HTMLInputElement>(null);
  const [atEnd,setAtEnd]=useState(true),[find,setFind]=useState(""),[match,setMatch]=useState(0),[copyError,setCopyError]=useState("");
  const operationKey=key+".operation";
  const {model,setModel,effort,setEffort}=useModelChoice(provider,key),running=run?activeAgentRun(run.status):!!history?.runs.some(item=>activeAgentRun(item.status));
  const [queued,setQueued]=useState(()=>{try{return sessionStorage.getItem(key+".queued")??"";}catch{return "";}});
  const slashCommands=[...commands({setModel,setEffort,efforts:provider?.models.find(item=>item.id===model)?.efforts??[],find:()=>{}}),{name:"plan",hint:"Toggle plan mode for the next turn",run:()=>setPlan(value=>!value)},{name:"review",hint:"Ask Codex to review uncommitted changes",run:()=>{void save(true,"Review the uncommitted changes in this working copy.","review");}},{name:"compact",hint:"Compact Codex's conversation context",run:()=>{void save(true,"Compact the conversation context.","compact");}}];
  useEffect(()=>{const listen=(event:Event)=>{const detail=(event as CustomEvent<{thread:string;mode:"review"|"compact"}>).detail;if(detail.thread===thread.id)void save(true,detail.mode==="review"?"Review the uncommitted changes in this working copy.":"Compact the conversation context.",detail.mode);};window.addEventListener("dar:agent-special",listen);return()=>window.removeEventListener("dar:agent-special",listen);});
  async function fork(message:AgentMessage){
    try{const row=await request<AgentThread>(`${base}/threads/${thread.id}/fork`,{method:"POST",signal:controller.current.signal,body:JSON.stringify({id:crypto.randomUUID(),sequence:message.sequence})},session.csrfToken);onForked(row);}
    catch(e){if(!controller.current.signal.aborted)setError(failure(e));}
  }
  const images=usePictures(thread.id,session),[plan,setPlan]=useState(()=>{try{return sessionStorage.getItem(key+".mode")==="plan";}catch{return false;}}),steerOperation=useRef<{id:string;content:string}|undefined>(undefined);
  useEffect(()=>{try{if(plan)sessionStorage.setItem(key+".mode","plan");else sessionStorage.removeItem(key+".mode");}catch{}},[plan,key]);
  useEffect(()=>{if(initialFiles?.length)images.add(initialFiles);},[]);
  function addImages(files:File[]){const message=images.add(files);if(message)setError(message);}
  useEffect(()=>{try{if(queued)sessionStorage.setItem(key+".queued",queued);else sessionStorage.removeItem(key+".queued");}catch{}},[queued,key]);
  draftRef.current=draft;
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{composer.current?.focus({preventScroll:true});},[]);
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
  async function save(execute=false,text=draft,special?:"review"|"compact"){
    const fromQueue=text!==draft&&!special;
    if(!text.trim()||!history||busy)return;
    if(running){if(execute&&!fromQueue)await steer(text);return;}
    if(!special&&images.pictures.length&&(!execute||images.busy||images.failed)){setError(!execute?"Images are sent with a Codex run, not saved on their own.":images.failed?"Remove images that failed to upload.":"Wait for images to finish uploading.");return;}
    if(new TextEncoder().encode(text).length>16384){setError("Keep each message within 16 KiB.");return;}
    setBusy(true);setError("");setNotice("");
    if(pending.current?.content!==text){
      try{const saved=JSON.parse(sessionStorage.getItem(operationKey)??"null");pending.current=saved?.content===text&&typeof saved.id==="string"?saved:undefined;}catch{}
      pending.current??={id:crypto.randomUUID(),content:text};
      if(pending.current.content!==text)pending.current={id:crypto.randomUUID(),content:text};
    }
    const operation=pending.current!;
    try{sessionStorage.setItem(operationKey,JSON.stringify(operation));}catch{}
    try{
      if(execute){
        const runKey=key+".run-operation";const attachments=fromQueue||special?[]:images.pictures.map(item=>item.id);let requestId:{id:string;messageId:string;content:string;model:string;effort?:string;mode?:string;attachments?:string[]}={id:crypto.randomUUID(),messageId:operation.id,content:operation.content,model,...effort?{effort}:{},...special?{mode:special}:plan?{mode:"plan"}:{},...attachments.length?{attachments}:{}};
        try{const saved=JSON.parse(sessionStorage.getItem(runKey)??"null");if(saved?.content===text&&saved.model===model&&(saved.effort??"")===effort&&(saved.mode??"")===(special??(plan?"plan":""))&&JSON.stringify(saved.attachments??[])===JSON.stringify(attachments)&&typeof saved.id==="string"&&typeof saved.messageId==="string")requestId=saved;sessionStorage.setItem(runKey,JSON.stringify(requestId));}catch{}
        const value=await request<AgentRun>(`${base}/threads/${thread.id}/runs`,{method:"POST",signal:controller.current.signal,headers:{"if-match":`"${history.thread.revision}"`},body:JSON.stringify(requestId)},session.csrfToken);
        if(controller.current.signal.aborted)return;
        onRun(value.id);setReload(n=>n+1);pending.current=undefined;if(!fromQueue&&!special)images.clear();if(fromQueue)setQueued("");else if(!special){setDraft("");draftCache.set(key,"");}
        try{sessionStorage.removeItem(operationKey);sessionStorage.removeItem(runKey);if(!fromQueue&&!special)sessionStorage.removeItem(key);}catch{}
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
  useEffect(()=>{if(autoStart===undefined||started.current||!history||!draft.trim()||autoStart&&checkingProvider||images.busy)return;started.current=true;onStarted();void save(autoStart&&!!provider?.connected);},[history,autoStart,provider,checkingProvider,images.busy]);
  /** A queued follow-up is sent once, after the reloaded history shows the previous run completed. */
  const latest=history?.runs[0];
  useEffect(()=>{if(queued&&!running&&!busy&&latest?.status==="completed"&&provider?.connected)void save(true,queued);},[queued,running,busy,latest?.id,latest?.status,provider?.connected]);
  /** Sends text into the running turn. The same ID is reused if the acknowledgement is lost. */
  async function steer(text:string){
    if(!run||!activeAgentRun(run.status))return;
    if(steerOperation.current?.content!==text)steerOperation.current={id:crypto.randomUUID(),content:text};
    setBusy(true);setError("");
    try{
      const value=await request<{thread:AgentThread;message:AgentMessage}>(`${base}/runs/${run.id}/steer`,{method:"POST",signal:controller.current.signal,body:JSON.stringify(steerOperation.current)},session.csrfToken);
      if(controller.current.signal.aborted)return;steerOperation.current=undefined;
      setHistory(current=>current?{...current,thread:value.thread,items:mergeMessages(current.items,[value.message])}:current);onChanged(value.thread);setDraft("");draftCache.set(key,"");try{sessionStorage.removeItem(key);}catch{}setNotice("Sent to the running turn.");
      following.current=true;
    }catch(e){if(!controller.current.signal.aborted)setError(failure(e));}finally{if(!controller.current.signal.aborted)setBusy(false);}
  }
  function queueFollowUp(){if(!draft.trim())return;if(queued){setError("One follow-up is already queued. Edit or remove it first.");return;}setQueued(draft);setDraft("");draftCache.set(key,"");setNotice("Queued. It is sent when Codex finishes.");}
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
      {history?.items.map(message=><Fragment key={message.id}><article data-message-id={message.id} data-match={message.id===matchId||undefined} className={`agent-message agent-message-${message.role}`}><div className="agent-message-body">{message.role==="assistant"?<Markdown source={message.content}/>:<p>{message.content}</p>}</div>{message.role==="user"&&!history.runs.some(item=>item.messageId===message.id||message.createdAt>=item.createdAt&&message.createdAt<=item.modifiedAt&&item.messageId!==message.id&&history.items.some(other=>other.id===item.messageId&&other.sequence<message.sequence))&&!run?.document.steered?.includes(message.id)&&<small className="agent-saved-label">Saved · not sent to a model</small>}{message.role==="user"&&run?.messageId===message.id&&run.document.attachments?.length?<div className="agent-message-images">{run.document.attachments.map(item=><a key={item.id} href={`${base}/threads/${thread.id}/attachments/${item.id}`} target="_blank" rel="noreferrer"><img src={`${base}/threads/${thread.id}/attachments/${item.id}`} alt={item.name} loading="lazy"/></a>)}</div>:null}{message.role==="user"&&run?.messageId===message.id&&run.document.mode==="plan"&&<small className="agent-saved-label">Plan mode</small>}<div className="agent-message-meta">{message.role==="user"&&<time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit"})}</time>}<button className="icon-button" aria-label={message.role==="user"?"Copy message":"Copy response"} title="Copy" onClick={()=>{void copy(message.content);}}><Icon name="copy"/></button><button className="icon-button" aria-label="Fork from here" title="Fork a new thread from this message" disabled={running} onClick={()=>{void fork(message);}}><Icon name="share"/></button>{message.role==="assistant"&&<time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleTimeString([],{hour:"numeric",minute:"2-digit"})}</time>}</div></article>{run?.messageId===message.id?<WorkLog run={run} onReview={onReview}/>:history.runs.filter(item=>item.messageId===message.id).map(item=><EarlierRun key={item.id} summary={item}/>)}</Fragment>)}
      {run?.document.output&&(running||!responseSaved)&&<article className="agent-message agent-message-assistant agent-live-output"><Markdown source={run.document.output}/></article>}
      {run?.document.output&&truncatedResponse&&<details className="agent-full-response"><summary>Full Codex response</summary><Markdown source={run.document.output}/></details>}
    </div></div>
    <Minimap viewport={viewport} content={content} version={`${history?.items.length}-${run?.status}-${run?.document.steps?.length}`}/>
    {!atEnd&&<button className="agent-jump secondary-button" onClick={()=>{following.current=true;if(viewport.current)viewport.current.scrollTop=viewport.current.scrollHeight;setAtEnd(true);}}>Jump to latest <Icon name="download"/></button>}
    </div>
    {run?.document.pending&&activeAgentRun(run.status)&&<div className="agent-question-area"><QuestionCard run={run} session={session}/></div>}
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void save(!!provider?.connected);}}>
      {error&&<div role="alert" className="error"><p>{error}</p><button type="button" className="text-button" disabled={busy} onClick={()=>setReload(n=>n+1)}>Refresh conversation</button></div>}
      {storageError&&<p role="alert" className="error">{storageError}</p>}
      {copyError&&<p role="alert" className="error">{copyError}</p>}
      {queued&&<div className="agent-queued" role="status"><Icon name="clock"/><span><strong>Queued follow-up</strong><span>{queued}</span></span>{!running&&latest?.status!=="completed"&&<button type="button" className="text-button" disabled={busy||!provider?.connected} onClick={()=>{void save(true,queued);}}>Send now</button>}<button type="button" className="text-button" onClick={()=>{if(!draft.trim())setDraft(queued);setQueued("");composer.current?.focus();}}>Edit</button><button type="button" className="icon-button" aria-label="Remove queued follow-up" onClick={()=>setQueued("")}><Icon name="close"/></button></div>}
      <div className={`agent-composer${plan?" is-plan":""}`} onDragOver={event=>{if([...event.dataTransfer.items].some(item=>item.type.startsWith("image/"))){event.preventDefault();event.currentTarget.dataset.drop="true";}}} onDragLeave={event=>{delete event.currentTarget.dataset.drop;}} onDrop={event=>{delete event.currentTarget.dataset.drop;const files=[...event.dataTransfer.files].filter(file=>file.type.startsWith("image/"));if(files.length){event.preventDefault();addImages(files);}}}><PictureTray pictures={images.pictures} onRemove={images.remove}/><PromptInput onFiles={provider?.connected?addImages:undefined} inputRef={composer} value={draft} onChange={setDraft} onSubmit={()=>{void save(!!provider?.connected);}} disabled={busy} sendKey={sendKey} threadId={thread.id} prompts={history?.items.filter(item=>item.role==="user").map(item=>item.content)??[]} commands={slashCommands} placeholder={running?"Steer the running turn, or queue a follow-up…":provider?.connected?"Ask Codex to plan, change or explain… @ files, / commands":"Write a message to save…"}/><div className="agent-composer-controls">{provider?.connected&&<AttachButton onFiles={addImages} disabled={busy||running||images.pictures.length>=agentAttachmentLimits.count}/>}{provider?.connected?<ModelPicker models={provider.models} value={model} onChange={setModel} effort={effort} onEffort={setEffort} disabled={busy}/>:<span className="agent-provider-state"><span className="agent-status-dot"/>{provider?.configured?"Codex unavailable":"No provider connected"}</span>}{provider?.connected&&<button type="button" className="agent-plan-toggle" aria-pressed={plan} title="Plan mode: Codex plans and asks before changing files" onClick={()=>setPlan(value=>!value)}><Icon name="list"/>Plan</button>}{provider?.connected&&<ActionMenu label="Message actions" actions={[{label:"Save only",icon:"file",disabled:busy||running||!history||!draft.trim(),run:()=>{void save();}}]}><Icon name="more"/></ActionMenu>}<ContextMeter used={run?.document.contextUsed} window={run?.document.contextWindow}/>{running&&provider?.connected&&<button type="button" className="agent-queue-button" disabled={busy||!draft.trim()||!!queued} onClick={queueFollowUp}>Queue</button>}{running&&provider?.connected&&<button className="agent-send" aria-label="Steer the running turn" title={`Send into the running turn (${sendKey==="enter"?"Enter":"Ctrl Enter"})`} disabled={busy||!draft.trim()}><Icon name="send"/></button>}{running?<button type="button" className="agent-send agent-stop" aria-label={run?.status==="cancelling"?"Stopping…":"Stop"} title="Stop generation" disabled={run?.status==="cancelling"} onClick={onStop}><span aria-hidden="true"/></button>:<button className="agent-send" aria-label={busy?"Saving…":provider?.connected?"Run with Codex":"Save message"} title={provider?.connected?`Run with Codex (${sendKey==="enter"?"Enter":"Ctrl Enter"})`:"Save message"} disabled={busy||!history||!draft.trim()||!!provider?.connected&&!model}><Icon name="send"/></button>}</div></div>
      <div className="agent-composer-hint" role="status">{notice}</div>
    </form>
  </>;
}

/** Compose before a thread exists. The thread is created with a stable ID only on submit. */
function DraftComposer({session,project,provider,checkingProvider,draftCache,onChooseProject,onCreated,sendKey,commands}:{session:BrowserSession;project:AgentProject;provider?:AgentProvider;checkingProvider:boolean;draftCache:Map<string,string>;onChooseProject:()=>void;onCreated:(thread:AgentThread,execute:boolean,files:File[])=>void;sendKey:"enter"|"modEnter";commands:(controls:{setModel:(id:string)=>void;setEffort:(value:string)=>void;efforts:string[];find:()=>void})=>SlashCommand[]}){
  const key=`dar.agent.draft.${session.user.id}.new`;
  const [draft,setDraft]=useState(()=>{try{return sessionStorage.getItem(key)??"";}catch{return "";}}),{model,setModel,effort,setEffort}=useModelChoice(provider,key),[busy,setBusy]=useState(false),[error,setError]=useState("");
  const images=usePictures(undefined,session),[plan,setPlan]=useState(false);
  const slashCommands=[...commands({setModel,setEffort,efforts:provider?.models.find(item=>item.id===model)?.efforts??[],find:()=>{}}).filter(item=>!["find","review","terminal","settle","rename"].includes(item.name)),{name:"plan",hint:"Toggle plan mode for the first turn",run:()=>setPlan(value=>!value)}];
  function addImages(files:File[]){const message=images.add(files);if(message)setError(message);}
  const pending=useRef<{id:string;projectId:string}|undefined>(undefined),composer=useRef<HTMLTextAreaElement>(null),controller=useRef(new AbortController());
  useEffect(()=>()=>controller.current.abort(),[]);
  useEffect(()=>{composer.current?.focus({preventScroll:true});},[project.id]);
  useEffect(()=>{try{if(draft)sessionStorage.setItem(key,draft);else sessionStorage.removeItem(key);}catch{}},[draft,key]);
  useLayoutEffect(()=>{const field=composer.current;if(field)field.rows=Math.min(9,Math.max(3,draft.split("\n").reduce((rows,line)=>rows+Math.max(1,Math.ceil(line.length/75)),0)));},[draft]);
  async function start(execute:boolean){
    if(!draft.trim()||busy||execute&&!model)return;
    if(images.pictures.length&&!execute){setError("Images are sent with a Codex run, not saved on their own.");return;}
    if(new TextEncoder().encode(draft).length>16384){setError("Keep each message within 16 KiB.");return;}
    setBusy(true);setError("");
    if(pending.current?.projectId!==project.id)pending.current={id:crypto.randomUUID(),projectId:project.id};
    try{
      const row=await request<AgentThread>(`${base}/threads`,{method:"POST",signal:controller.current.signal,body:JSON.stringify(pending.current)},session.csrfToken);
      if(controller.current.signal.aborted)return;
      const target=`dar.agent.draft.${session.user.id}.${row.id}`;draftCache.set(target,draft);
      try{sessionStorage.setItem(target,draft);if(model)sessionStorage.setItem(target+".model",model);if(effort)sessionStorage.setItem(target+".effort",effort);if(plan)sessionStorage.setItem(target+".mode","plan");sessionStorage.removeItem(key);}catch{}
      pending.current=undefined;onCreated(row,execute,images.pictures.map(item=>item.file));
    }catch(e){if(!controller.current.signal.aborted){setError(failure(e));setBusy(false);}}
  }
  const execute=!!provider?.connected;
  return <div className="agent-draft"><div className="agent-hero"><h2>What should we build in <button type="button" className="agent-hero-project" aria-label={`Project: ${project.name}. Change project`} onClick={onChooseProject}><ProjectMark name={project.name}/>{project.name}<Icon name="chevron"/></button>?</h2></div>
    <form className="agent-composer-area" onSubmit={event=>{event.preventDefault();void start(execute);}}>{error&&<p role="alert" className="error">{error}</p>}
      <div className={`agent-composer${plan?" is-plan":""}`} onDragOver={event=>{if(execute&&[...event.dataTransfer.items].some(item=>item.type.startsWith("image/")))event.preventDefault();}} onDrop={event=>{const files=[...event.dataTransfer.files].filter(file=>file.type.startsWith("image/"));if(execute&&files.length){event.preventDefault();addImages(files);}}}><PictureTray pictures={images.pictures} onRemove={images.remove}/><PromptInput onFiles={execute?addImages:undefined} inputRef={composer} value={draft} onChange={setDraft} onSubmit={()=>{void start(execute);}} disabled={busy} sendKey={sendKey} prompts={[]} commands={slashCommands} placeholder={execute?"Ask Codex to plan, change or explain… / commands":"Write a message to save…"}/><div className="agent-composer-controls">{execute&&<AttachButton onFiles={addImages} disabled={busy||images.pictures.length>=agentAttachmentLimits.count}/>}{execute?<ModelPicker models={provider!.models} value={model} onChange={setModel} effort={effort} onEffort={setEffort} disabled={busy}/>:<span className="agent-provider-state"><span className="agent-status-dot"/>{checkingProvider?"Checking Codex…":provider?.configured?"Codex unavailable":"No provider connected"}</span>}{execute&&<button type="button" className="agent-plan-toggle" aria-pressed={plan} title="Plan mode: Codex plans and asks before changing files" onClick={()=>setPlan(value=>!value)}><Icon name="list"/>Plan</button>}{execute&&<ActionMenu label="Message actions" actions={[{label:"Save only",icon:"file",disabled:busy||!draft.trim(),run:()=>{void start(false);}}]}><Icon name="more"/></ActionMenu>}<button className="agent-send" aria-label={busy?"Starting…":execute?"Run with Codex":"Save message"} title={execute?"Run with Codex (Enter)":"Save message (Enter)"} disabled={busy||!draft.trim()||execute&&!model||checkingProvider&&!provider}><Icon name="send"/></button></div></div>
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

type GitState={available:boolean;branch?:string;changed?:number;commits?:{hash:string;subject:string}[]};
/** Local Git in the working copy. Dar performs these actions; publishing stays disabled. */
function GitCard({threadId,session,epoch}:{threadId:string;session:BrowserSession;epoch:number}){
  const [state,setState]=useState<GitState>(),[dialog,setDialog]=useState<"branch"|"commit">(),[value,setValue]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState("");
  useEffect(()=>{const abort=new AbortController();void request<GitState>(`${base}/threads/${threadId}/git`,{signal:abort.signal}).then(setState).catch(()=>{if(!abort.signal.aborted)setState({available:false});});return()=>abort.abort();},[threadId,epoch]);
  if(!state?.available)return null;
  async function submit(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{setState(await request<GitState>(`${base}/threads/${threadId}/git`,{method:"POST",body:JSON.stringify(dialog==="branch"?{action:"branch",name:value}:{action:"commit",message:value})},session.csrfToken));setDialog(undefined);}
    catch(e){setError(failure(e));}finally{setBusy(false);}
  }
  return <><div className="agent-context-git"><Icon name="share"/><span className="agent-git-branch" title={state.branch}>{state.branch}</span><span>{state.changed?`${state.changed} changed`:"Clean"}</span><ActionMenu label="Git actions" actions={[{label:"New branch…",icon:"plus",run:()=>{setValue("");setError("");setDialog("branch");}},{label:"Commit changes…",icon:"check",disabled:!state.changed,run:()=>{setValue("");setError("");setDialog("commit");}}]}><Icon name="more"/></ActionMenu></div>
    {dialog&&<Dialog title={dialog==="branch"?"New branch":"Commit changes"} onClose={()=>{if(!busy)setDialog(undefined);}}><form className="agent-git-form" onSubmit={event=>{void submit(event);}}><label>{dialog==="branch"?"Branch name":"Commit message"}{dialog==="branch"?<input autoFocus required value={value} maxLength={100} spellCheck={false} placeholder="feature/greeting" onChange={event=>setValue(event.target.value)}/>:<textarea autoFocus required value={value} maxLength={500} rows={3} onChange={event=>setValue(event.target.value)}/>}</label>
      <p className="muted small">{dialog==="branch"?`Created from ${state.branch} in this thread’s working copy.`:`Commits ${state.changed} changed ${state.changed===1?"file":"files"} on ${state.branch}.`} Pushing and pull requests are disabled: Dar workspaces have no network access or Git credentials.</p>
      {error&&<p role="alert" className="error">{error}</p>}<div className="dialog-actions"><button type="button" className="secondary-button" disabled={busy} onClick={()=>setDialog(undefined)}>Cancel</button><button className="primary-button" disabled={busy||!value.trim()}>{busy?"Working…":dialog==="branch"?"Create branch":"Commit"}</button></div></form></Dialog>}</>;
}
/** Snooze presets in local time. Custom times use the dialog below. */
function snoozePresets(now=new Date()){
  const at=(days:number,hour:number)=>{const date=new Date(now);date.setDate(date.getDate()+days);date.setHours(hour,0,0,0);return date.getTime();};
  const evening=at(0,18)>now.getTime()+30*60000?at(0,18):at(1,18),monday=at((8-now.getDay())%7||7,9);
  return [{label:"In 1 hour",until:now.getTime()+3600000},{label:"In 3 hours",until:now.getTime()+3*3600000},{label:"This evening",until:evening},{label:"Tomorrow morning",until:at(1,9)},{label:"Next week",until:monday}];
}
const wakeLabel=(until:number)=>new Date(until).toLocaleString([],{weekday:"short",hour:"numeric",minute:"2-digit"});
function markdownThread(thread:AgentThread,project:string,items:AgentMessage[]){
  return [`# ${thread.title}`,"",`Project: ${project}`,"",...items.flatMap(item=>[`## ${item.role==="user"?"You":"Codex"} · ${new Date(item.createdAt).toLocaleString()}`,"",item.content,""])].join("\n");
}
type SharedThread={shareId:string;thread:{id:string;title:string;revision:number;modifiedAt:number};owner:string;project:string};
type ShareRow={id:string;username:string;displayName:string;createdAt:number};
/** Owner-side read-only grants to other Dar accounts. */
function ShareDialog({thread,session,onClose}:{thread:AgentThread;session:BrowserSession;onClose:()=>void}){
  const [rows,setRows]=useState<ShareRow[]>(),[username,setUsername]=useState(""),[busy,setBusy]=useState(false),[error,setError]=useState(""),operation=useRef(crypto.randomUUID());
  useEffect(()=>{const abort=new AbortController();void request<{items:ShareRow[]}>(`${base}/threads/${thread.id}/shares`,{signal:abort.signal}).then(value=>setRows(value.items)).catch(e=>{if(!abort.signal.aborted)setError(failure(e));});return()=>abort.abort();},[thread.id]);
  async function add(event:FormEvent){
    event.preventDefault();setBusy(true);setError("");
    try{const row=await request<ShareRow>(`${base}/threads/${thread.id}/shares`,{method:"POST",body:JSON.stringify({id:operation.current,username:username.trim()})},session.csrfToken);setRows(current=>[...(current??[]).filter(item=>item.id!==row.id),row]);setUsername("");operation.current=crypto.randomUUID();}
    catch(e){setError(e instanceof RequestError&&e.status===404?"No active Dar account has that username.":e instanceof RequestError&&e.status===400?"Enter another person’s username.":failure(e));}finally{setBusy(false);}
  }
  async function remove(row:ShareRow){try{await request(`${base}/threads/${thread.id}/shares/${row.id}`,{method:"DELETE"},session.csrfToken);setRows(current=>current?.filter(item=>item.id!==row.id));}catch(e){setError(failure(e));}}
  return <Dialog title="Share thread" onClose={onClose}><div className="agent-share"><p className="muted small">People you add can read this thread’s messages. They cannot send messages, see run output or open the working copy.</p>
    <form onSubmit={event=>{void add(event);}}><label className="sr-only" htmlFor="agent-share-user">Username</label><input id="agent-share-user" autoFocus autoCapitalize="none" spellCheck={false} placeholder="Dar username" value={username} maxLength={100} onChange={event=>setUsername(event.target.value)}/><button className="primary-button" disabled={busy||!username.trim()}>{busy?"Sharing…":"Share"}</button></form>
    {error&&<p role="alert" className="error">{error}</p>}
    {!rows?<p role="status" className="muted small">Loading…</p>:rows.length?<ul aria-label="People with access">{rows.map(row=><li key={row.id}><span className="avatar" aria-hidden="true">{(row.displayName||row.username)[0]?.toLocaleUpperCase()}</span><span><strong>{row.displayName||row.username}</strong><small>{row.username} · read only</small></span><button className="text-button" onClick={()=>{void remove(row);}}>Remove</button></li>)}</ul>:<p className="muted small">Only you can see this thread.</p>}
  </div></Dialog>;
}
/** Grantee view: messages only, no composer, runs or working copy. */
function SharedConversation({item}:{item:SharedThread}){
  const [items,setItems]=useState<AgentMessage[]>(),[next,setNext]=useState<number|null>(null),[error,setError]=useState(""),[loading,setLoading]=useState(false);
  useEffect(()=>{const abort=new AbortController();setItems(undefined);setError("");void request<{items:AgentMessage[];nextBefore:number|null}>(`${base}/shared/${item.thread.id}/messages`,{signal:abort.signal}).then(value=>{setItems(value.items);setNext(value.nextBefore);}).catch(e=>{if(!abort.signal.aborted)setError(failure(e));});return()=>abort.abort();},[item.thread.id,item.thread.revision]);
  async function earlier(){if(!next)return;setLoading(true);try{const value=await request<{items:AgentMessage[];nextBefore:number|null}>(`${base}/shared/${item.thread.id}/messages?before=${next}`);setItems(current=>mergeMessages(current??[],value.items));setNext(value.nextBefore);}catch(e){setError(failure(e));}finally{setLoading(false);}}
  return <><div className="agent-shared-banner" role="note"><Icon name="lock"/>Shared by <strong>{item.owner}</strong> · read only</div><div className="agent-timeline-wrap"><div className="agent-conversation" aria-label="Conversation messages" aria-busy={!items&&!error}><div className="agent-timeline-content">
    {error&&<p role="alert" className="error">{error}</p>}{!items&&!error&&<p role="status">Loading conversation…</p>}
    {next&&<button className="text-button" disabled={loading} onClick={()=>{void earlier();}}>{loading?"Loading…":"Load earlier messages"}</button>}
    {items?.map(message=><article key={message.id} data-message-id={message.id} className={`agent-message agent-message-${message.role}`}><div className="agent-message-body">{message.role==="assistant"?<Markdown source={message.content}/>:<p>{message.content}</p>}</div><div className="agent-message-meta"><time dateTime={new Date(message.createdAt).toISOString()}>{new Date(message.createdAt).toLocaleString([],{month:"short",day:"numeric",hour:"numeric",minute:"2-digit"})}</time></div></article>)}
    {items&&!items.length&&<p className="muted">No messages yet.</p>}
  </div></div></div></>;
}
type PaletteItem={key:string;group:string;label:string;detail?:string;mark?:string;icon?:IconName;run:()=>void};
/** Ctrl/Cmd+K: one searchable list of actions, threads and projects. */
function CommandPalette({items,onClose}:{items:PaletteItem[];onClose:()=>void}){
  const [query,setQuery]=useState(""),[active,setActive]=useState(0),list=useRef<HTMLDivElement>(null);
  const words=query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean),shown=items.filter(item=>words.every(word=>`${item.label} ${item.detail??""} ${item.group}`.toLocaleLowerCase().includes(word))).slice(0,60);
  useEffect(()=>setActive(0),[query]);
  useEffect(()=>{list.current?.querySelector("[data-active]")?.scrollIntoView({block:"nearest"});},[active]);
  function choose(item:PaletteItem){onClose();requestAnimationFrame(()=>item.run());}
  return <Dialog title="Command palette" onClose={onClose}><div className="agent-palette"><input autoFocus type="search" role="combobox" aria-expanded={true} aria-controls="agent-palette-list" aria-activedescendant={shown[active]?`agent-palette-${active}`:undefined} aria-label="Search commands, threads and projects" placeholder="Search commands, threads and projects…" value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="ArrowDown"||event.key==="ArrowUp"){event.preventDefault();setActive(index=>shown.length?(index+(event.key==="ArrowDown"?1:-1)+shown.length)%shown.length:0);}else if(event.key==="Enter"&&shown[active]){event.preventDefault();choose(shown[active]);}}}/>
    <div id="agent-palette-list" role="listbox" aria-label="Results" ref={list}>{shown.map((item,index)=><Fragment key={item.key}>{(index===0||shown[index-1].group!==item.group)&&<div className="agent-palette-group" role="presentation">{item.group}</div>}<div id={`agent-palette-${index}`} role="option" aria-selected={index===active} data-active={index===active||undefined} onPointerEnter={()=>setActive(index)} onClick={()=>choose(item)}>{item.mark!==undefined?<ProjectMark name={item.mark}/>:<Icon name={item.icon??"chevron"}/>}<span>{item.label}</span>{item.detail&&<small>{item.detail}</small>}</div></Fragment>)}{!shown.length&&<p className="muted">No results</p>}</div></div></Dialog>;
}
/** Pointer and keyboard resizable boundary; widths are remembered with the layout. */
function Resizer({label,value,min,max,onChange,side}:{label:string;value:number;min:number;max:number;onChange:(value:number)=>void;side:"left"|"right"}){
  return <div className={`agent-resizer agent-resizer-${side}`} role="separator" aria-orientation="vertical" aria-label={label} aria-valuenow={value} aria-valuemin={min} aria-valuemax={max} tabIndex={0}
    onKeyDown={event=>{if(event.key==="ArrowLeft"||event.key==="ArrowRight"){event.preventDefault();const step=(event.key==="ArrowRight")===(side==="right")?16:-16;onChange(Math.min(max,Math.max(min,value+step)));}}}
    onDoubleClick={()=>onChange(side==="right"?256:Math.round(innerWidth*.4))}
    onPointerDown={event=>{event.preventDefault();const start=event.clientX,initial=value,target=event.currentTarget;target.setPointerCapture(event.pointerId);
      const move=(next:PointerEvent)=>onChange(Math.min(max,Math.max(min,initial+(side==="right"?1:-1)*(next.clientX-start))));
      const end=()=>{target.removeEventListener("pointermove",move);target.removeEventListener("pointerup",end);target.removeEventListener("pointercancel",end);};
      target.addEventListener("pointermove",move);target.addEventListener("pointerup",end);target.addEventListener("pointercancel",end);}}/>;
}

export function AgentWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const initial=useRef(new URLSearchParams(location.search)).current;
  const layout=useRef(savedLayout(session.user.id)).current;
  const [projects,setProjects]=useState<AgentProject[]>(),[projectId,setProjectId]=useState(initial.get("project")??""),[threads,setThreads]=useState<AgentThread[]>(),[threadId,setThreadId]=useState(initial.get("thread")??"");
  const [error,setError]=useState(""),[threadError,setThreadError]=useState(""),[reload,setReload]=useState(0),[view,setView]=useState<View>("conversation"),[editor,setEditor]=useState<"project"|"thread">(),[settings,setSettings]=useState(false),[settingsSection,setSettingsSection]=useState("General"),[review,setReview]=useState(layout.review),[nav,setNav]=useState(layout.nav),[details,setDetails]=useState(layout.details),[terminal,setTerminal]=useState(layout.terminal),[query,setQuery]=useState(""),[findOpen,setFindOpen]=useState(false),[maximized,setMaximized]=useState(false),[layoutError,setLayoutError]=useState("");
  const [shared,setShared]=useState<SharedThread[]>([]),[sharedId,setSharedId]=useState(""),[sharing,setSharing]=useState<AgentThread>(),[snoozing,setSnoozing]=useState<AgentThread>(),[snoozeAt,setSnoozeAt]=useState(""),[limits,setLimits]=useState<AgentLimits>(),[drawerTab,setDrawerTab]=useState<"output"|"shell">("output"),[sidebarWidth,setSidebarWidth]=useState(layout.sidebarWidth),[reviewWidth,setReviewWidth]=useState(layout.reviewWidth),[palette,setPalette]=useState(false),[renaming,setRenaming]=useState(""),[renameValue,setRenameValue]=useState(""),[deleting,setDeleting]=useState<AgentThread>(),[deleteBusy,setDeleteBusy]=useState(false),[deleteError,setDeleteError]=useState(""),[collapsedGroups,setCollapsedGroups]=useState<string[]>([]),[permission,setPermission]=useState(()=>typeof Notification==="undefined"?"unsupported":Notification.permission);
  const shell=useRef<HTMLDivElement>(null),statuses=useRef(new Map<string,string>()),selectedRef=useRef("");
  const [provider,setProvider]=useState<AgentProvider>(),[providerEpoch,setProviderEpoch]=useState(0),[checkingProvider,setCheckingProvider]=useState(true),[connectionError,setConnectionError]=useState(""),[runId,setRunId]=useState(""),[storedRun,setRun]=useState<AgentRun>(),[runError,setRunError]=useState(""),[historyEpoch,setHistoryEpoch]=useState(0),[creatingThread,setCreatingThread]=useState(false),[createError,setCreateError]=useState(""),[settling,setSettling]=useState(false);
  const {preferences,loaded:preferencesLoaded,error:preferencesError}=usePreferences();
  const completedSeen=useRef(new Set<string>()),autoStart=useRef(new Map<string,boolean>()),startFiles=useRef(new Map<string,File[]>()),lifetime=useRef(new AbortController()),now=useNow(true,30000);
  const panel=useRef<HTMLDivElement>(null),tabs=useRef<HTMLElement>(null),previousView=useRef(view),returnToTabs=useRef(false),draftCache=useRef(new Map<string,string>()),positions=useRef(new Map<string,ScrollPosition>()),search=useRef<HTMLInputElement>(null);
  const thread=threads?.find(t=>t.id===threadId),project=projects?.find(p=>p.id===(thread?.projectId??projectId))??(thread?undefined:projects?.[0]);
  const run=storedRun?.threadId===threadId?storedRun:undefined;
  useEffect(()=>()=>lifetime.current.abort(),[]);
  // Dynamic panel widths are measurements, applied as custom properties rather than markup styles.
  useLayoutEffect(()=>{shell.current?.style.setProperty("--agent-sidebar-width",`${sidebarWidth}px`);shell.current?.style.setProperty("--agent-review-width",`${reviewWidth}px`);},[sidebarWidth,reviewWidth]);
  useEffect(()=>{try{localStorage.setItem(`dar.agent.layout.${session.user.id}`,JSON.stringify({nav,details,review,terminal,sidebarWidth,reviewWidth}));setLayoutError("");}catch{setLayoutError("Layout changes could not be saved in this browser.");}},[nav,details,review,terminal,sidebarWidth,reviewWidth,session.user.id]);
  useEffect(()=>{
    const shortcut=(event:KeyboardEvent)=>{if(!(event.ctrlKey||event.metaKey)||event.defaultPrevented||document.querySelector("dialog[open],[data-editor-pane]"))return;
      if(event.key.toLowerCase()==="k"){event.preventDefault();setPalette(true);}
      else if(event.key.toLowerCase()==="o"&&event.shiftKey){event.preventDefault();chooseNewThread();}
      else if(event.key.toLowerCase()==="b"&&!event.shiftKey){event.preventDefault();setNav(value=>!value);}
      else if(event.key.toLowerCase()==="f"&&threadId){event.preventDefault();setView("conversation");setFindOpen(true);}
    };document.addEventListener("keydown",shortcut);return()=>document.removeEventListener("keydown",shortcut);
  },[threadId,projects,project?.id]);
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
  const snoozed=preferences.agentSnoozed??[],snoozedUntil=(row:AgentThread)=>snoozed.find(item=>item.id===row.id)?.until,isSnoozed=(row:AgentThread)=>(snoozedUntil(row)??0)>now,woke=(row:AgentThread)=>{const until=snoozedUntil(row);return until!==undefined&&until<=now;};
  const snooze=(row:AgentThread,until:number|undefined)=>{if(until!==undefined&&row.id===threadId)setThreadId("");return updatePreferences({agentSnoozed:[...snoozed.filter(item=>item.id!==row.id&&item.until>Date.now()-7*86400000),...until===undefined?[]:[{id:row.id,until}]].slice(-128)});};
  /** Woken threads notify once per browser and keep a Woke badge until opened or dismissed. */
  const wakeNotified=useRef(new Set<string>());
  useEffect(()=>{for(const row of threads??[]){const until=snoozedUntil(row);if(until===undefined||until>now||wakeNotified.current.has(row.id+until))continue;wakeNotified.current.add(row.id+until);if(preferences.agentNotify&&typeof Notification!=="undefined"&&Notification.permission==="granted"&&Date.now()-until<120000){try{new Notification("Snoozed thread is back",{body:row.title,tag:"wake-"+row.id});}catch{}}}},[now,threads,snoozed]);
  useEffect(()=>{if(thread&&woke(thread))void snooze(thread,undefined);},[thread?.id,now]);
  useEffect(()=>{if(!provider?.connected||!(settings||details))return;const abort=new AbortController();void request<AgentLimits>(`${base}/provider/limits`,{signal:abort.signal}).then(setLimits).catch(()=>{});return()=>abort.abort();},[provider?.connected,settings,details,historyEpoch]);
  async function exportThread(row:AgentThread,mode:"copy"|"download"){
    try{
      let items:AgentMessage[]=[],before:number|null=null;
      do{const page:AgentHistory=await request<AgentHistory>(`${base}/threads/${row.id}/messages${before?`?before=${before}`:""}`,{signal:lifetime.current.signal});items=[...page.items,...items];before=page.nextBefore;}while(before);
      const text=markdownThread(row,projects?.find(item=>item.id===row.projectId)?.name??"Project",items);
      if(mode==="copy")await navigator.clipboard.writeText(text);
      else{const url=URL.createObjectURL(new Blob([text],{type:"text/markdown"})),link=document.createElement("a");link.href=url;link.download=`${row.title.replace(/[^\w .-]+/g,"").trim().slice(0,80)||"thread"}.md`;link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}
    }catch(e){setThreadError(failure(e));}
  }
  useEffect(()=>{const abort=new AbortController();let timer:ReturnType<typeof setTimeout>;const load=()=>{void request<{items:SharedThread[]}>(`${base}/shared`,{signal:abort.signal}).then(value=>{setShared(value.items);setSharedId(current=>value.items.some(item=>item.thread.id===current)?current:"");}).catch(()=>{}).finally(()=>{if(!abort.signal.aborted)timer=setTimeout(load,30000);});};load();return()=>{abort.abort();clearTimeout(timer);};},[reload]);
  const sharedItem=shared.find(item=>item.thread.id===sharedId);
  useEffect(()=>{if(threadId)setSharedId("");},[threadId]);
  const pinned=preferences.agentPinnedThreads??[],archived=preferences.agentArchivedThreads??[],sendKey=preferences.agentSendKey??"enter",grouping=preferences.agentGrouping??"recent";
  async function updatePreferences(patch:Partial<Preferences>){setThreadError("");try{await savePreferences({...preferences,...patch},session);}catch(e){setThreadError(failure(e));}}
  const togglePin=(row:AgentThread)=>updatePreferences({agentPinnedThreads:pinned.includes(row.id)?pinned.filter(id=>id!==row.id):[row.id,...pinned].slice(0,128)});
  const toggleArchive=(row:AgentThread)=>{const restoring=archived.includes(row.id);if(!restoring&&row.id===threadId)setThreadId("");return updatePreferences({agentArchivedThreads:restoring?archived.filter(id=>id!==row.id):[row.id,...archived].slice(0,128),agentPinnedThreads:pinned.filter(id=>id!==row.id)});};
  /** Read markers are per browser: a thread is unread when its revision passed the last one viewed here. */
  const seenKey=`dar.agent.seen.${session.user.id}`,[seen,setSeen]=useState<Record<string,number>|undefined>(()=>{try{return JSON.parse(localStorage.getItem(seenKey)??"null")??undefined;}catch{return undefined;}});
  useEffect(()=>{if(!threads)return;setSeen(current=>{const next={...current??Object.fromEntries(threads.map(row=>[row.id,row.revision]))};if(thread)next[thread.id]=thread.revision;for(const id of Object.keys(next))if(!threads.some(row=>row.id===id))delete next[id];try{localStorage.setItem(seenKey,JSON.stringify(next));}catch{}return next;});},[threads,thread?.id,thread?.revision]);
  const unread=(row:AgentThread)=>!!seen&&row.id!==threadId&&!isWorking(row)&&row.revision>(seen[row.id]??0);
  selectedRef.current=threadId;
  useEffect(()=>{
    for(const row of threads??[]){
      const before=statuses.current.get(row.id),after=row.runStatus??"";statuses.current.set(row.id,after);
      if(before&&activeAgentRun(before)&&!activeAgentRun(after)&&preferences.agentNotify&&typeof Notification!=="undefined"&&Notification.permission==="granted"&&(document.hidden||row.id!==selectedRef.current)){
        try{const note=new Notification(after==="completed"?"Codex finished":`Codex ${after}`,{body:row.title,tag:row.id});note.onclick=()=>{window.focus();setThreadId(row.id);setProjectId(row.projectId);note.close();};}catch{}
      }
    }
  },[threads,preferences.agentNotify]);
  function startRename(row:AgentThread){setRenaming(row.id);setRenameValue(row.title);}
  async function commitRename(row:AgentThread){
    const title=renameValue.trim();setRenaming("");if(!title||title===row.title)return;
    changed({...row,title});
    try{const value=await request<AgentThread>(`${base}/threads/${row.id}`,{method:"PATCH",signal:lifetime.current.signal,body:JSON.stringify({title})},session.csrfToken);changed(value);}
    catch(e){changed(row);setThreadError(failure(e));}
  }
  async function removeThread(row:AgentThread){
    setDeleteBusy(true);setDeleteError("");
    try{
      await request(`${base}/threads/${row.id}`,{method:"DELETE",signal:lifetime.current.signal},session.csrfToken);
      setThreads(current=>current?.filter(item=>item.id!==row.id));if(row.id===threadId)setThreadId("");setDeleting(undefined);
      try{sessionStorage.removeItem(`dar.agent.draft.${session.user.id}.${row.id}`);}catch{}
      if(pinned.includes(row.id)||archived.includes(row.id))void updatePreferences({agentPinnedThreads:pinned.filter(id=>id!==row.id),agentArchivedThreads:archived.filter(id=>id!==row.id)});
    }catch(e){setDeleteError(failure(e));}finally{setDeleteBusy(false);}
  }
  const slashCommands=({setEffort,efforts}:{setModel:(id:string)=>void;setEffort:(value:string)=>void;efforts:string[];find:()=>void}):SlashCommand[]=>[
    {name:"new",hint:"Start a new thread",run:()=>chooseNewThread()},
    {name:"model",hint:"Choose the Codex model",run:()=>requestAnimationFrame(()=>document.querySelector<HTMLButtonElement>(".agent-model-trigger")?.click())},
    ...efforts.length?[{name:"effort",hint:`Reasoning: ${efforts.join(", ")}`,argument:efforts,run:(value:string)=>setEffort(value)}]:[],
    {name:"changes",hint:"Open the changes panel",run:()=>openReview()},
    {name:"terminal",hint:"Show command output",run:()=>{setDrawerTab("output");setTerminal(true);}},
    {name:"shell",hint:"Open an interactive terminal in the working copy",run:()=>{setDrawerTab("shell");setTerminal(true);}},
    {name:"find",hint:"Find in this thread",run:()=>setFindOpen(true)},
    {name:"rename",hint:"Rename this thread",run:()=>{if(thread)startRename(thread);}},
    {name:"settle",hint:"Settle or restore this thread",run:()=>{if(thread)void settle(thread);}},
    {name:"settings",hint:"Open Agent settings",run:()=>setSettings(true)},
  ];
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
  function started(row:AgentThread,execute:boolean,files:File[]){autoStart.current.set(row.id,execute);if(files.length)startFiles.current.set(row.id,files);setThreads(current=>[row,...(current??[]).filter(item=>item.id!==row.id)]);setProjectId(row.projectId);setThreadId(row.id);setView("conversation");}
  function openReview(){setReview(true);setView("changes");}
  function closeReview(){setReview(false);setMaximized(false);setView("conversation");requestAnimationFrame(()=>panel.current?.querySelector<HTMLButtonElement>('[aria-label="Show changes"]')?.focus());}
  function stop(){if(!runId||run&&(!activeAgentRun(run.status)||run.threadId!==threadId))return;void request<AgentRun>(`${base}/runs/${runId}/cancel`,{method:"POST",signal:lifetime.current.signal},session.csrfToken).then(value=>{if(!lifetime.current.signal.aborted)setRun(value);}).catch(e=>{if(!lifetime.current.signal.aborted)setRunError(failure(e));});}
  async function copyThread(item:AgentThread){try{const url=new URL(location.href);url.searchParams.set("thread",item.id);url.searchParams.set("project",item.projectId);await navigator.clipboard.writeText(url.href);}catch{setThreadError("Could not copy the thread link. Open the thread and copy its address.");}}
  function rows(items:AgentThread[]){return <div className="agent-thread-list">{items.map(item=>{const name=projects?.find(p=>p.id===item.projectId)?.name??"Project",working=isWorking(item),settledRow=isSettled(item),isPinned=pinned.includes(item.id),isArchived=archived.includes(item.id),fresh=unread(item);
    const actions=[{label:isPinned?"Unpin thread":"Pin thread",icon:"pin" as const,disabled:!preferencesLoaded,run:()=>{void togglePin(item);}},{label:"Rename thread",icon:"pencil" as const,run:()=>startRename(item)},{label:settledRow?"Restore thread":"Settle thread",icon:"check" as const,disabled:settling||!preferencesLoaded||working,run:()=>{void settle(item);}},{label:"Copy thread link",icon:"share" as const,run:()=>{void copyThread(item);}},snoozedUntil(item)!==undefined?{label:"Unsnooze thread",icon:"clock" as const,run:()=>{void snooze(item,undefined);}}:{label:"Snooze",icon:"clock" as const,disabled:!preferencesLoaded||working,items:[...snoozePresets().map(preset=>({label:preset.label,run:()=>{void snooze(item,preset.until);}})),{label:"Custom…",run:()=>{setSnoozeAt("");setSnoozing(item);}}]},{label:"Export as Markdown",icon:"download" as const,run:()=>{void exportThread(item,"download");}},{label:isArchived?"Unarchive thread":"Archive thread",icon:"archive" as const,disabled:!preferencesLoaded||working,run:()=>{void toggleArchive(item);}},{label:"Delete thread",icon:"trash" as const,disabled:working,run:()=>{setDeleteError("");setDeleting(item);}}];
    if(renaming===item.id)return <div className="agent-thread-row selected" key={item.id}><ProjectMark name={name}/><input className="agent-rename" autoFocus aria-label="Thread title" value={renameValue} maxLength={200} onChange={event=>setRenameValue(event.target.value)} onBlur={()=>{void commitRename(item);}} onKeyDown={event=>{if(event.key==="Enter"){event.preventDefault();event.currentTarget.blur();}else if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setRenaming("");}}}/></div>;
    return <div className={`agent-thread-row${item.id===threadId?" selected":""}${settledRow?" settled":""}${fresh?" unread":""}`} key={item.id} onContextMenu={event=>{const trigger=event.currentTarget.querySelector<HTMLButtonElement>(".agent-thread-actions .ui-menu-trigger");if(trigger){event.preventDefault();trigger.click();}}}><button className="agent-thread-select" aria-current={item.id===threadId?"page":undefined} title={`${name} / ${item.title}`} onClick={()=>{setThreadId(item.id);setProjectId(item.projectId);setFindOpen(false);setView("conversation");}} onDoubleClick={()=>startRename(item)} onKeyDown={event=>{if(event.key==="F2"){event.preventDefault();startRename(item);}}}>{grouping==="project"&&!isPinned?null:<ProjectMark name={name}/>}<span className="sr-only">{name}: </span><span className="agent-thread-title">{item.title}</span>{isPinned&&<Icon name="pin" className="agent-pin-mark"/>}{fresh&&<span className="agent-unread" role="img" aria-label="Unread"/>}</button>
      <span className="agent-thread-meta">{woke(item)?<button className="agent-woke" aria-label={`Dismiss Woke for ${item.title}`} onClick={()=>{void snooze(item,undefined);}}><Icon name="clock"/>Woke</button>:isSnoozed(item)?<span className="agent-snooze-time" title={`Wakes ${new Date(snoozedUntil(item)!).toLocaleString()}`}><Icon name="clock"/>{wakeLabel(snoozedUntil(item)!)}</span>:working?<span className="agent-thread-status"><span className="agent-pulse" aria-hidden="true"/>{item.id===threadId&&run?duration(Date.now()-run.createdAt):"Working"}</span>:<time dateTime={new Date(item.modifiedAt).toISOString()} title={new Date(item.modifiedAt).toLocaleString()}>{ago(item.modifiedAt,now)}</time>}</span>
      <span className="agent-thread-actions">{!working&&<button className="icon-button" aria-label={settledRow?`Restore ${item.title}`:`Settle ${item.title}`} title={settledRow?"Restore thread":"Settle thread"} disabled={settling||!preferencesLoaded} onClick={()=>{void settle(item);}}><Icon name={settledRow?"undo":"check"}/></button>}<ActionMenu label={`Actions for ${item.title}`} actions={actions}><Icon name="more"/></ActionMenu></span></div>;})}</div>;}
  function grouped(items:AgentThread[]){
    if(grouping!=="project")return rows(items);
    return <>{(projects??[]).filter(row=>items.some(item=>item.projectId===row.id)||!query).map(row=>{const own=items.filter(item=>item.projectId===row.id),closed=collapsedGroups.includes(row.id);return <div className="agent-project-group" key={row.id}><div className="agent-project-heading"><button aria-expanded={!closed} onClick={()=>setCollapsedGroups(current=>closed?current.filter(id=>id!==row.id):[...current,row.id])}><Icon name="chevron"/><ProjectMark name={row.name}/><span>{row.name}</span></button><button className="icon-button" aria-label={`New thread in ${row.name}`} title="New thread" onClick={()=>{setThreadId("");setProjectId(row.id);setView("conversation");requestAnimationFrame(()=>document.getElementById("agent-message")?.focus());}}><Icon name="plus"/></button></div>{!closed&&(own.length?rows(own):<p className="agent-group-empty">No threads</p>)}</div>;})}</>;
  }
  const filtered=threads?.filter(row=>`${row.title} ${projects?.find(project=>project.id===row.projectId)?.name??""}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()))??[];
  const visible=filtered.filter(row=>!archived.includes(row.id)&&(!isSnoozed(row)||isWorking(row))),snoozedRows=filtered.filter(row=>!archived.includes(row.id)&&isSnoozed(row)&&!isWorking(row)),pinnedRows=visible.filter(row=>pinned.includes(row.id)&&!isWorking(row)),active=visible.filter(row=>!isWorking(row)&&!isSettled(row)&&!pinned.includes(row.id)),working=visible.filter(isWorking),settled=visible.filter(row=>isSettled(row)&&!pinned.includes(row.id));
  return <div ref={shell} className={`app-shell agent-shell agent-view-${view}${review?"":" agent-review-hidden"}${nav?"":" agent-nav-hidden"}${details?"":" agent-details-hidden"}${maximized&&review?" agent-review-maximized":""}`}>
    <nav ref={tabs} className="agent-mobile-views" aria-label="Agent views">{(["threads","conversation","details","changes","terminal"] as const).map(value=><button key={value} aria-current={view===value?"page":undefined} onClick={()=>setView(value)}>{value[0].toUpperCase()+value.slice(1)}</button>)}</nav>
    <div className="agent-layout" ref={panel} onKeyDown={event=>{if(event.key==="Escape"&&view!=="conversation"&&!editor&&!settings){event.preventDefault();returnToTabs.current=true;setView("conversation");}}}>
      <aside className="agent-sidebar" data-agent-panel="threads" tabIndex={-1} aria-label="Threads"><h1 className="sr-only">Agent</h1><div className="agent-brand"><a href="/" aria-label="Dar home"><Icon name="agent"/>Dar</a><AppSwitcher active="agent" administrator={session.user.administrator}/></div><div className="agent-search-row"><label className="agent-thread-search"><Icon name="search"/><span className="sr-only">Search threads</span><input ref={search} type="search" placeholder="Search" value={query} onChange={event=>setQuery(event.target.value)} onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();setQuery("");}}}/></label><button className="icon-button" aria-label="New thread" title="New thread" disabled={!projects} onClick={chooseNewThread}><Icon name="plus"/></button></div>
        {error&&<div role="alert"><p>{error}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry projects</button></div>}
        {threadError&&<div role="alert"><p>{threadError}</p><button className="text-button" onClick={()=>setReload(n=>n+1)}>Retry threads</button></div>}
        {preferencesError&&<p role="alert" className="error">{preferencesError}</p>}
        <div className="agent-thread-shelves">{!threads?<p role="status">Loading threads…</p>:<>{pinnedRows.length>0&&<div className="agent-pinned" aria-label="Pinned threads" role="group">{rows(pinnedRows)}</div>}{grouped(active)}{!threads.length&&<p className="muted small">No threads yet.</p>}{threads.length>0&&!filtered.length&&<p role="status" className="muted small">No matching threads.</p>}<div className="agent-shelves">{working.length>0&&<details open><summary>Working <span>{working.length}</span></summary>{rows(working)}</details>}{shared.length>0&&<details open><summary>Shared with me <span>{shared.length}</span></summary><div className="agent-thread-list">{shared.filter(item=>`${item.thread.title} ${item.owner}`.toLocaleLowerCase().includes(query.toLocaleLowerCase())).map(item=><div key={item.shareId} className={`agent-thread-row${item.thread.id===sharedId?" selected":""}`}><button className="agent-thread-select" aria-current={item.thread.id===sharedId?"page":undefined} title={`${item.owner} / ${item.thread.title}`} onClick={()=>{setThreadId("");setSharedId(item.thread.id);setView("conversation");}}><ProjectMark name={item.project}/><span className="sr-only">Shared by {item.owner}: </span><span className="agent-thread-title">{item.thread.title}</span></button><span className="agent-thread-meta">{item.owner.split(" ")[0]}</span></div>)}</div></details>}{snoozedRows.length>0&&<details open={query?true:undefined}><summary>Snoozed <span>{snoozedRows.length}</span></summary>{rows(snoozedRows)}</details>}<details open={query?true:undefined}><summary>Settled <span>{settled.length}</span></summary>{rows(settled)}</details></div></>}</div>
        <div className="agent-sidebar-footer"><AppSettingsButton app="agent" onClick={()=>setSettings(true)}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/></div>
        <Resizer label="Resize threads" side="right" value={sidebarWidth} min={208} max={420} onChange={setSidebarWidth}/>
      </aside>
      <main className="agent-center" data-agent-panel="conversation" tabIndex={-1} aria-label="Agent conversation"><header className="agent-thread-heading"><button className="icon-button agent-desktop-toggle" aria-label={nav?"Hide threads":"Show threads"} aria-expanded={nav} onClick={()=>setNav(v=>!v)}><Icon name="menu"/></button><div className="agent-collapsed-controls"><AppSwitcher active="agent" administrator={session.user.administrator}/><WorkspaceAccount session={session} onSignedOut={onSignedOut}/><button className="icon-button" aria-label="Agent settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div>{project&&<span className="agent-heading-project"><ProjectMark name={project.name}/>{project.name}<span>/</span></span>}<h2>{thread?.title??(sharedItem&&!thread?sharedItem.thread.title:"New thread")}</h2>{thread&&<ActionMenu label="Thread actions" actions={[{label:"Find in thread",icon:"search",run:()=>setFindOpen(true)},{label:"Rename thread",icon:"pencil",run:()=>{setNav(true);startRename(thread);}},{label:pinned.includes(thread.id)?"Unpin thread":"Pin thread",icon:"pin",disabled:!preferencesLoaded,run:()=>{void togglePin(thread);}},{label:isSettled(thread)?"Restore thread":"Settle thread",icon:"check",disabled:settling||!preferencesLoaded||isWorking(thread),run:()=>{void settle(thread);}},{label:"Review changes with Codex",icon:"check",disabled:isWorking(thread)||!provider?.connected,run:()=>window.dispatchEvent(new CustomEvent("dar:agent-special",{detail:{thread:thread.id,mode:"review"}}))},{label:"Compact context",icon:"list",disabled:isWorking(thread)||!provider?.connected,run:()=>window.dispatchEvent(new CustomEvent("dar:agent-special",{detail:{thread:thread.id,mode:"compact"}}))},{label:"Share…",icon:"user",run:()=>setSharing(thread)},{label:"Copy as Markdown",icon:"copy",run:()=>{void exportThread(thread,"copy");}},{label:"Export as Markdown",icon:"download",run:()=>{void exportThread(thread,"download");}},{label:"Copy thread link",icon:"share",run:()=>{void copyThread(thread);}},{label:"Archive thread",icon:"archive",disabled:!preferencesLoaded||isWorking(thread),run:()=>{void toggleArchive(thread);}},{label:"Delete thread",icon:"trash",disabled:isWorking(thread),run:()=>{setDeleteError("");setDeleting(thread);}}]}><Icon name="more"/></ActionMenu>}{thread&&<div className="agent-panel-controls"><button className="icon-button agent-desktop-toggle" aria-label="Toggle thread details" aria-expanded={details} onClick={()=>setDetails(value=>!value)}><Icon name="info"/></button><button className="icon-button agent-desktop-toggle" aria-label="Toggle command output" aria-expanded={terminal} onClick={()=>setTerminal(value=>!value)}><Icon name="list"/></button><button className="icon-button agent-desktop-toggle" aria-label={review?"Hide changes":"Show changes"} aria-expanded={review} onClick={()=>review?closeReview():openReview()}><Icon name="file"/></button></div>}</header>
        {runError&&<p role="alert" className="error">{runError}</p>}
        {layoutError&&<p role="alert" className="error">{layoutError}</p>}
        <div className="agent-stage"><div className="agent-chat">{sharedItem&&!thread?<SharedConversation item={sharedItem}/>:thread?<Conversation key={thread.id} thread={thread} project={project} session={session} onChanged={changed} draftCache={draftCache.current} positions={positions.current} provider={provider} checkingProvider={checkingProvider} run={run} onRun={setRunId} onStop={stop} onReview={openReview} historyEpoch={historyEpoch} findOpen={findOpen} onCloseFind={()=>setFindOpen(false)} autoStart={autoStart.current.get(thread.id)} onStarted={()=>{autoStart.current.delete(thread.id);startFiles.current.delete(thread.id);}} sendKey={sendKey} commands={slashCommands} initialFiles={startFiles.current.get(thread.id)} onForked={row=>{setThreads(current=>[row,...(current??[]).filter(item=>item.id!==row.id)]);setThreadId(row.id);setProjectId(row.projectId);setView("conversation");}}/>:project?<DraftComposer key={project.id} session={session} project={project} provider={provider} checkingProvider={checkingProvider} draftCache={draftCache.current} onChooseProject={()=>setEditor("thread")} onCreated={started} sendKey={sendKey} commands={slashCommands}/>:<div className="agent-empty agent-hero">{!projects?<p role="status">Loading projects…</p>:<><h2>Add a project to start</h2><p>Work in a Git repository or a folder in Dar Files. Codex uses an isolated copy.</p><button className="primary-button" onClick={()=>setEditor("project")}><Icon name="plus"/>Add project</button></>}</div>}</div>
        {thread&&<aside className="agent-context" data-agent-panel="details" tabIndex={-1} aria-label="Thread details"><div className="agent-context-card"><div className="agent-context-project"><ProjectMark name={project?.name??"Project"}/><strong>{project?.name??"Project"}</strong><span>{project?.sourceType==="files"?"Dar Files":"Git"}</span></div>{project?.sourceType==="files"?<a href={`/?folder=${project.sourceRef}`}><Icon name="folder"/>Open folder in Files<Icon name="chevron"/></a>:project&&<div className="agent-source-ref"><Icon name="share"/><code>{project.sourceRef}</code></div>}<div className="agent-workspace-label"><Icon name="lock"/>Isolated working copy</div>{thread&&<GitCard threadId={thread.id} session={session} epoch={historyEpoch}/>}<button onClick={openReview}><Icon name="file"/>Changes{run?.document.changes.length?<DiffStat {...run.document.changes.map(change=>diffCounts(change.diff)).reduce((sum,item)=>({added:sum.added+item.added,removed:sum.removed+item.removed}),{added:0,removed:0})}/>:<span>0</span>}<Icon name="chevron"/></button>{run?.document.usage&&<div className="agent-context-usage" title="Tokens reported by Codex for the latest turn"><Icon name="info"/>Latest turn<span>{tokens(run.document.usage.input)} in · {tokens(run.document.usage.output)} out</span></div>}{run?.document.effort&&<div className="agent-context-usage"><Icon name="agent"/>Reasoning<span>{effortLabel(run.document.effort)}</span></div>}{limits?.available&&limits.windows.slice(0,2).map(window=><div key={window.label} className="agent-context-usage agent-limit" title={window.resetsAt?`Resets ${new Date(window.resetsAt).toLocaleString()}`:undefined}><Icon name="clock"/>{window.label}<span>{Math.round(window.usedPercent)}%</span></div>)}<div className="agent-context-provider"><span className={provider?.connected?"agent-connected":""}/>{checkingProvider?"Checking Codex…":provider?.connected?"Codex connected":"Codex unavailable"}<button className="icon-button" aria-label="Connection settings" onClick={()=>setSettings(true)}><Icon name="settings"/></button></div></div></aside>}</div>
        <section className={`agent-terminal${terminal?" is-open":""}`} data-agent-panel="terminal" tabIndex={-1} aria-label="Terminal output"><div className="agent-output-heading"><div role="tablist" aria-label="Output" className="agent-drawer-tabs"><button role="tab" aria-selected={drawerTab==="output"} onClick={()=>setDrawerTab("output")}>Codex output</button><button role="tab" aria-selected={drawerTab==="shell"} disabled={!thread} onClick={()=>setDrawerTab("shell")}>Terminal</button></div>{drawerTab==="output"&&<span className="muted">Read only</span>}<button className="icon-button" aria-label="Close command output" onClick={()=>{setTerminal(false);setView("conversation");}}><Icon name="close"/></button></div>{thread&&<div className="agent-drawer-panel" hidden={drawerTab!=="shell"}><ThreadTerminal threadId={thread.id} session={session} visible={terminal&&drawerTab==="shell"}/></div>}{drawerTab!=="output"?null:run?.document.steps?.some(step=>step.kind==="command")?<ol className="agent-terminal-steps">{run.document.steps.filter(step=>step.kind==="command").map(step=><li key={step.id} data-status={step.status}><div><span className="agent-terminal-prompt">$</span><code>{step.label}</code><span className="agent-terminal-state">{step.status==="running"?"running":step.exitCode!==undefined?`exit ${step.exitCode}`:step.status}</span></div>{step.output&&<pre>{step.output}</pre>}</li>)}</ol>:run?.document.terminal?<pre>{run.document.terminal}</pre>:<p>No command output yet.</p>}</section>
      </main>
      <aside className="agent-review" data-agent-panel="changes" tabIndex={-1} aria-label="Changes"><Resizer label="Resize changes" side="left" value={reviewWidth} min={340} max={1200} onChange={setReviewWidth}/><header className="agent-review-heading"><h2>Changes</h2><button className="icon-button agent-desktop-toggle" aria-label={maximized?"Restore review size":"Maximize review"} onClick={()=>setMaximized(value=>!value)}><Icon name="grid"/></button><button className="icon-button" aria-label="Close changes" onClick={closeReview}><Icon name="close"/></button></header><div className="agent-review-content"><ChangeReview key={run?.id??"empty"} run={run} session={session}/></div></aside>
    </div>
    {editor==="project"&&<ProjectEditor session={session} onClose={()=>setEditor(projects?.length?"thread":undefined)} onCreated={row=>{setProjects(current=>[...current??[],row]);setProjectId(row.id);setThreadId("");setEditor(undefined);setView("conversation");}}/>}
    {editor==="thread"&&projects&&<ThreadChooser projects={projects} busy={false} error={createError} onClose={()=>setEditor(undefined)} onChoose={row=>{setProjectId(row.id);setThreadId("");setEditor(undefined);setView("conversation");}} onAdd={()=>setEditor("project")}/>}
    {settings&&<Dialog title="Agent settings" onClose={()=>setSettings(false)}><div className="ui-settings-dialog"><nav aria-label="Settings sections">{["General","Chat","Sidebar","Archived threads","Keyboard shortcuts"].map(section=><button key={section} className={settingsSection===section?"active":""} aria-current={settingsSection===section?"page":undefined} onClick={()=>setSettingsSection(section)}>{section}</button>)}</nav><section>
      {settingsSection==="General"&&<><h3>Codex connection</h3><p role="status">{checkingProvider?"Checking Codex…":provider?.message??"Connection could not be checked."}</p>{connectionError&&<p className="error" role="alert">{connectionError}</p>}<div className="agent-connection-actions">{session.user.administrator&&!provider?.configured&&<button className="primary-button" disabled={checkingProvider} onClick={()=>{void connectProvider();}}>Connect server Codex</button>}<button className="secondary-button" disabled={checkingProvider} onClick={()=>setProviderEpoch(n=>n+1)}>{checkingProvider?"Checking…":"Refresh connection"}</button></div>{provider?.connected&&<><h3>Usage limits</h3>{!limits?<p role="status" className="muted small">Checking usage…</p>:!limits.available?<p className="muted small">{limits.message??"Usage limits are unavailable."}</p>:<ul className="agent-limits">{limits.windows.map(window=><li key={window.label}><span>{window.label}</span><meter min={0} max={100} value={window.usedPercent} aria-label={`${window.label} used`}/><strong>{Math.round(window.usedPercent)}%</strong><small>{window.resetsAt?`Resets ${new Date(window.resetsAt).toLocaleString([],{weekday:"short",hour:"numeric",minute:"2-digit"})}`:""}</small></li>)}{limits.plan&&<li className="muted small">Plan: {limits.plan}</li>}</ul>}<h3>Models</h3><ul className="agent-settings-models">{provider.models.map(model=><li key={model.id}><strong>{model.name}</strong>{model.isDefault&&<small>Default</small>}<span>{model.efforts?.length?`Reasoning: ${model.efforts.map(effortLabel).join(", ")}`:"No reasoning levels declared"}</span></li>)}</ul></>}<p className="muted small">Runs send the conversation and selected project contents to Codex. Commands have no network access; publishing is disabled.</p><p className="muted small">Uses the Codex ChatGPT login available to the Linux account running Dar. Connect links it to your Dar account. Login tokens stay on the server.</p><p className="muted small">Dar account ID: <code className="agent-settings-account">{session.user.id}</code></p></>}
      {settingsSection==="Chat"&&<><h3>Sending</h3><div className="agent-settings-choice" role="radiogroup" aria-label="Send messages with">{([["enter","Enter sends, Shift Enter adds a line"],["modEnter","Ctrl / Cmd Enter sends, Enter adds a line"]] as const).map(([value,label])=><label key={value}><input type="radio" name="agent-send-key" checked={sendKey===value} disabled={!preferencesLoaded} onChange={()=>{void updatePreferences({agentSendKey:value});}}/>{label}</label>)}</div><h3>Notifications</h3><label className="agent-settings-toggle"><input type="checkbox" checked={!!preferences.agentNotify} disabled={!preferencesLoaded||permission==="unsupported"} onChange={event=>{const enabled=event.target.checked;if(enabled&&typeof Notification!=="undefined"&&Notification.permission==="default")void Notification.requestPermission().then(value=>{setPermission(value);if(value==="granted")void updatePreferences({agentNotify:true});});else void updatePreferences({agentNotify:enabled});}}/>Notify me when Codex finishes in a background thread</label>{permission==="denied"&&<p className="muted small">Notifications are blocked for this site in the browser.</p>}{permission==="unsupported"&&<p className="muted small">This browser does not support notifications.</p>}<p className="muted small">Drafts, queued follow-ups and the selected model and reasoning level are kept in this browser tab.</p></>}
      {settingsSection==="Sidebar"&&<><h3>Thread list</h3><div className="agent-settings-choice" role="radiogroup" aria-label="Thread list">{([["recent","Recent first"],["project","Grouped by project"]] as const).map(([value,label])=><label key={value}><input type="radio" name="agent-grouping" checked={grouping===value} disabled={!preferencesLoaded} onChange={()=>{void updatePreferences({agentGrouping:value});}}/>{label}</label>)}</div><p className="muted small">Pinned threads stay at the top. Panel visibility and widths are remembered in this browser; double-click a resize handle to reset it.</p></>}
      {settingsSection==="Archived threads"&&<><h3>Archived threads</h3>{threads?.filter(row=>archived.includes(row.id)).length?<ul className="agent-archived">{threads.filter(row=>archived.includes(row.id)).map(row=><li key={row.id}><ProjectMark name={projects?.find(item=>item.id===row.projectId)?.name??"Project"}/><span>{row.title}</span><button className="text-button" onClick={()=>{void toggleArchive(row);}}>Unarchive</button><button className="text-button danger" onClick={()=>{setDeleteError("");setDeleting(row);}}>Delete</button></li>)}</ul>:<p className="muted">No archived threads.</p>}</>}
      {settingsSection==="Keyboard shortcuts"&&<><h3>Keyboard shortcuts</h3><dl className="ui-shortcuts"><dt>Ctrl / Cmd K</dt><dd>Command palette: actions, threads and projects</dd><dt>Ctrl / Cmd Shift O</dt><dd>New thread</dd><dt>Ctrl / Cmd B</dt><dd>Show or hide threads</dd><dt>Ctrl / Cmd F</dt><dd>Find in loaded thread messages</dd><dt>{sendKey==="enter"?"Enter":"Ctrl / Cmd Enter"}</dt><dd>Run with Codex when connected; while running, queue a follow-up</dd><dt>{sendKey==="enter"?"Shift Enter":"Enter"}</dt><dd>Insert a new line</dd><dt>/</dt><dd>Commands at the start of a message</dd><dt>@</dt><dd>Mention a workspace file</dd><dt>Up / Down</dt><dd>Previous prompts in an empty composer</dd><dt>F2 / double-click</dt><dd>Rename the focused thread</dd><dt>Escape</dt><dd>Close menus; return from a phone panel</dd></dl></>}
    </section></div></Dialog>}
    {palette&&<CommandPalette onClose={()=>setPalette(false)} items={[
      {key:"new",group:"Actions",label:"New thread",detail:"Ctrl Shift O",icon:"plus",run:chooseNewThread},
      {key:"project",group:"Actions",label:"Add project",icon:"folder",run:()=>setEditor("project")},
      ...thread?[{key:"find",group:"Actions",label:"Find in thread",detail:"Ctrl F",icon:"search" as const,run:()=>setFindOpen(true)},{key:"rename",group:"Actions",label:"Rename thread",icon:"pencil" as const,run:()=>startRename(thread)},{key:"pin",group:"Actions",label:pinned.includes(thread.id)?"Unpin thread":"Pin thread",icon:"pin" as const,run:()=>{void togglePin(thread);}},{key:"archive",group:"Actions",label:"Archive thread",icon:"archive" as const,run:()=>{void toggleArchive(thread);}},{key:"review",group:"Actions",label:review?"Hide changes":"Show changes",icon:"file" as const,run:()=>review?closeReview():openReview()},{key:"terminal",group:"Actions",label:terminal?"Hide command output":"Show command output",icon:"terminal" as const,run:()=>setTerminal(value=>!value)}]:[],
      {key:"nav",group:"Actions",label:nav?"Hide threads":"Show threads",detail:"Ctrl B",icon:"sidebar",run:()=>setNav(value=>!value)},
      {key:"settings",group:"Actions",label:"Agent settings",icon:"settings",run:()=>setSettings(true)},
      ...(threads??[]).filter(row=>!archived.includes(row.id)).map(row=>({key:"thread-"+row.id,group:"Threads",label:row.title,detail:projects?.find(item=>item.id===row.projectId)?.name,mark:projects?.find(item=>item.id===row.projectId)?.name??"Project",run:()=>{setThreadId(row.id);setProjectId(row.projectId);setView("conversation");}})),
      ...(projects??[]).map(row=>({key:"project-"+row.id,group:"New thread in project",label:row.name,detail:row.sourceType==="files"?"Dar Files":"Git",mark:row.name,run:()=>{setThreadId("");setProjectId(row.id);setView("conversation");requestAnimationFrame(()=>document.getElementById("agent-message")?.focus());}})),
    ]}/>}
    {sharing&&<ShareDialog thread={sharing} session={session} onClose={()=>setSharing(undefined)}/>}
    {snoozing&&<Dialog title="Snooze thread" onClose={()=>setSnoozing(undefined)}><form className="agent-git-form" onSubmit={event=>{event.preventDefault();const until=new Date(snoozeAt).getTime();if(until>Date.now()){void snooze(snoozing,until);setSnoozing(undefined);}}}><label>Wake at<input type="datetime-local" required autoFocus value={snoozeAt} min={new Date(Date.now()-new Date().getTimezoneOffset()*60000).toISOString().slice(0,16)} onChange={event=>setSnoozeAt(event.target.value)}/></label><p className="muted small">The thread moves to Snoozed and returns at this time with a Woke badge{preferences.agentNotify?" and a notification":""}.</p><div className="dialog-actions"><button type="button" className="secondary-button" onClick={()=>setSnoozing(undefined)}>Cancel</button><button className="primary-button" disabled={!snoozeAt||new Date(snoozeAt).getTime()<=Date.now()}>Snooze</button></div></form></Dialog>}
    {deleting&&<Dialog title="Delete thread" onClose={()=>{if(!deleteBusy)setDeleting(undefined);}}><div className="agent-delete"><p>Delete <strong>{deleting.title}</strong>? Its messages, run output and private working copy are removed. Edits already applied to Files are kept.</p>{deleteError&&<p role="alert" className="error">{deleteError}</p>}<div className="dialog-actions"><button className="secondary-button" disabled={deleteBusy} onClick={()=>setDeleting(undefined)}>Cancel</button><button className="primary-button danger" disabled={deleteBusy} onClick={()=>{void removeThread(deleting);}}>{deleteBusy?"Deleting…":"Delete thread"}</button></div></div></Dialog>}
  </div>;
}
