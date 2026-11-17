import {useEffect,useId,useMemo,useState,type ReactNode} from "react";
import {request,type BrowserSession} from "../api.ts";
import {Icon,type IconName} from "../icons.tsx";
import {ActionMenu,Select} from "../controls.tsx";
import {Segmented} from "../settings-sheet.tsx";
import {Dialog} from "../dialog.tsx";
import {EditorPane} from "../pane.tsx";
import {Markdown,type MarkdownReference} from "../pim/markdown.tsx";
import {ChangeRow} from "../folder-history.tsx";
import {edited} from "../pim/presentation.ts";
import {localDateTime} from "../dates.ts";
import {repoApi,message,plural,short,Sha,Person,useLoad,Failure,Loading,SheetHead,useSheetFocus,FileChanges,MergeBox} from "./parts.tsx";
import {gitLabelColors,trackerLimits,type GitBranches,type GitCompare,type GitIssue,type GitIssueKind,type GitIssueList,type GitIssueState,type GitIssueSummary,type GitLabel,type GitLabelColor,type GitPullChanges,type GitRepository,type GitTimelineItem} from "../../shared/git.ts";

/**
 * Issues and pull requests (docs/git.md#issues-and-pull-requests), in the Git app's frame: a tab
 * lists them, one opens in the right sheet (conversation; a pull request also has its commits,
 * files and merge), and creating or editing uses the right editing pane, as everywhere else.
 */
export type IssueEditing={type:"issue";kind:GitIssueKind;issue?:GitIssue;head?:string;base?:string}|{type:"labels"};
const noun=(kind:GitIssueKind)=>kind==="pull"?"pull request":"issue";
const writer=(repo:GitRepository)=>repo.role!=="read";

/** Database refusals in words; codes arrive as the error message. */
export function trackerFailure(cause:unknown,kind:GitIssueKind|"label"="issue"){
  const text=message(cause);
  if(/alreadyExists/.test(text))return kind==="label"?"A label with this name already exists.":"A pull request between these branches is already open.";
  if(/conflict/.test(text))return kind==="pull"?"It changed meanwhile, or its branch is gone. Reload and try again.":"It changed since you opened it. Reload and try again.";
  if(/forbidden/.test(text))return "You can read this repository but not change this. Ask its owner for write access.";
  if(/notFound/.test(text))return kind==="pull"?"That branch no longer exists.":"It no longer exists.";
  if(/operationTooLarge/.test(text))return "This repository has reached its limit.";
  if(/invalidInput/.test(text))return "Check the fields and try again.";
  return text;
}

function StateIcon({kind,state}:{kind:GitIssueKind;state:GitIssueState}){
  const icon:IconName=kind==="pull"?state==="closed"?"pullClosed":state==="merged"?"merge":"pull":state==="open"?"issue":"issueDone";
  return <Icon name={icon} className={`git-state-icon is-${state}`}/>;
}
function StateChip({kind,state}:{kind:GitIssueKind;state:GitIssueState}){
  return <span className={`git-state is-${state}`}><StateIcon kind={kind} state={state}/>{state==="open"?"Open":state==="merged"?"Merged":"Closed"}</span>;
}
function LabelChip({label}:{label:GitLabel}){return <span className={`git-label is-${label.color}`} title={label.description||undefined}>{label.name}</span>;}
function Labels({ids,labels}:{ids:string[];labels:GitLabel[]}){
  const known=ids.map(id=>labels.find(label=>label.id===id)).filter((label):label is GitLabel=>!!label);
  return known.length?<span className="git-labels">{known.map(label=><LabelChip key={label.id} label={label}/>)}</span>:null;
}
function BranchPair({head,base}:{head:string;base:string}){
  return <span className="git-branch-pair"><code>{head}</code><Icon name="back" className="git-pair-arrow"/><code>{base}</code><span className="sr-only">{`${head} into ${base}`}</span></span>;
}
/** #12 opens that issue or pull request in the sheet. */
function useReference(onOpen:(number:number)=>void):MarkdownReference{
  return useMemo(()=>(number,key)=><button key={key} type="button" className="git-link git-ref-link" onClick={()=>onOpen(number)}>#{number}</button>,[onOpen]);
}

// ---- Lists --------------------------------------------------------------------------------------

/** The Issues and Pull requests tabs: open/closed, a label filter, and the newest first. */
export function IssuesTab({repo,kind,version,selected,onOpen,onEdit}:{repo:GitRepository;kind:GitIssueKind;version:number;selected?:number;onOpen:(number:number)=>void;onEdit:(editing:IssueEditing)=>void}){
  const [state,setState]=useState<"open"|"closed">("open"),[label,setLabel]=useState("");
  const query=new URLSearchParams({kind,state,...label?{label}:{}});
  const {value:list,error,retry}=useLoad<GitIssueList>(`${repoApi(repo.id)}/issues?${query}`,[version]);
  const canCreate=kind==="issue"||writer(repo);
  const title=kind==="pull"?"Pull requests":"Issues";
  return <div className="git-scroll git-issues">
    <div className="git-toolbar">
      <Segmented label={`${title} to show`} value={state} options={[["open",`Open${list?` ${list.open}`:""}`],["closed",`Closed${list?` ${list.closed}`:""}`]] as const} onChange={setState}/>
      {!!list?.labels.length&&<Select aria-label="Label" value={label} onChange={event=>setLabel(event.target.value)}><option value="">All labels</option>{list.labels.map(item=><option key={item.id} value={item.id}>{item.name}</option>)}</Select>}
      <span className="git-toolbar-end">
        {writer(repo)&&<button type="button" className="secondary-button git-tool-button" onClick={()=>onEdit({type:"labels"})}><Icon name="label"/>Labels</button>}
        {canCreate&&<button type="button" className="primary-button git-tool-button" onClick={()=>onEdit({type:"issue",kind})}><Icon name="plus"/>New {noun(kind)}</button>}
      </span>
    </div>
    {error?<Failure error={error} retry={retry} what={title}/>:!list?<Loading label={`Loading ${title.toLowerCase()}`}/>
      :!list.items.length?<div className="git-empty git-empty-inline"><span className="empty-icon"><Icon name={kind==="pull"?"pull":"issue"}/></span>
        <h2>{label?`No ${state} ${noun(kind)}s with this label`:`No ${state} ${noun(kind)}s`}</h2>
        <p>{kind==="pull"?state==="open"?"Open a pull request to discuss a branch before it is merged. Merging it, here or with Git, closes it.":"Merged and closed pull requests appear here."
          :state==="open"?"Track bugs, ideas and tasks for this repository. Anyone it is shared with can open issues and comment.":"Closed issues appear here."}</p></div>
      :<ul className="git-issue-list" aria-label={title}>{list.items.map(item=><li key={item.number}><IssueRow item={item} labels={list.labels} selected={selected===item.number} onOpen={()=>onOpen(item.number)}/></li>)}</ul>}
    {list?.truncated&&<p className="git-hint">Only the newest {trackerLimits.page} are listed.</p>}
  </div>;
}
function IssueRow({item,labels,selected,onOpen}:{item:GitIssueSummary;labels:GitLabel[];selected:boolean;onOpen:()=>void}){
  return <button type="button" className={`git-issue-row${selected?" is-selected":""}`} aria-pressed={selected} onClick={onOpen}>
    <StateIcon kind={item.kind} state={item.state}/>
    <span className="git-issue-main"><span className="git-issue-title"><span>{item.title}</span><Labels ids={item.labels} labels={labels}/></span>
      <span className="git-issue-sub">#{item.number} · {item.state==="open"?`opened ${edited(item.createdAt)}`:`${item.state==="merged"?"merged":"closed"} ${edited(item.closedAt??item.updatedAt)}`} by {item.author}
        {item.kind==="pull"&&item.head&&item.base&&<> · <BranchPair head={item.head} base={item.base}/></>}{item.agent&&<span className="git-source is-agent"><Icon name="agent"/>Agent</span>}</span></span>
    <span className="git-issue-end">{item.comments>0&&<span className="git-muted" title={plural(item.comments,"comment")}><Icon name="comment"/>{item.comments}<span className="sr-only"> comments</span></span>}</span>
  </button>;
}

// ---- One issue or pull request -------------------------------------------------------------------

type PullView="conversation"|"commits"|"files";
export function IssueSheet({repo,number,session,version,notify,onClose,onOpen,onCommit,onEdit,onChanged}:{repo:GitRepository;number:number;session:BrowserSession;version:number;notify:(text:string)=>void;onClose:()=>void;onOpen:(number:number)=>void;onCommit:(sha:string)=>void;onEdit:(editing:IssueEditing)=>void;onChanged:()=>void}){
  const url=`${repoApi(repo.id)}/issues/${number}`,focus=useSheetFocus(onClose);
  const {value:issue,error,retry,setValue}=useLoad<GitIssue>(url,[version,repo.revision]);
  const {value:list}=useLoad<GitIssueList>(`${repoApi(repo.id)}/issues?kind=issue&state=open`,[version]);
  const pull=issue?.kind==="pull";
  const {value:changes,error:changesError,retry:retryChanges}=useLoad<GitPullChanges>(pull?`${url}/changes`:undefined,[version,repo.revision,issue?.state]);
  const [view,setView]=useState<PullView>("conversation"),[wrap,setWrap]=useState(false),[busy,setBusy]=useState(false),[failure,setFailure]=useState("");
  const reference=useReference(onOpen),labels=list?.labels??[];
  const kind=issue?.kind??"issue";
  async function change(update:Record<string,unknown>,done?:string){
    if(!issue)return;
    setBusy(true);setFailure("");
    try{await request(url,{method:"PATCH",body:JSON.stringify({revision:issue.revision,...update})},session.csrfToken);setValue(await request<GitIssue>(url));if(done)notify(done);onChanged();}
    catch(cause){setFailure(trackerFailure(cause,kind));}finally{setBusy(false);}
  }
  const reload=async()=>{setValue(await request<GitIssue>(url));onChanged();};
  const compare=changes?.compare;
  return <div className="git-sheet" {...focus}>
    <SheetHead title={<>{pull?"Pull request":"Issue"} #{number}</>} onClose={onClose}>
      {issue?.permissions.edit&&<button type="button" className="icon-button" aria-label={`Edit ${noun(kind)}`} title="Edit" onClick={()=>onEdit({type:"issue",kind,issue})}><Icon name="pencil"/></button>}
    </SheetHead>
    <div className="git-sheet-body">
      {error?<Failure error={trackerFailure(error,kind)} retry={retry} what={`This ${noun(kind)}`}/>:!issue?<Loading label={`Loading ${noun(kind)}`}/>:<>
        <div className="git-issue-head"><h3 className="git-commit-title">{issue.title}</h3>
          <div className="git-issue-meta"><StateChip kind={issue.kind} state={issue.state}/><span><strong>{issue.author}</strong> opened {edited(issue.createdAt)}</span>
            {issue.head&&issue.base&&<BranchPair head={issue.head} base={issue.base}/>}{issue.agent&&<span className="git-source is-agent"><Icon name="agent"/>Agent</span>}</div></div>
        {(issue.labels.length>0||issue.permissions.label)&&<div className="git-issue-labels"><Labels ids={issue.labels} labels={labels}/>{!issue.labels.length&&<span className="git-muted">No labels</span>}
          {issue.permissions.label&&<ActionMenu label="Labels" className="git-label-menu" align="start" actions={()=>[...labels.map(item=>({label:item.name,icon:"label" as const,current:issue.labels.includes(item.id),
            disabled:busy||!issue.labels.includes(item.id)&&issue.labels.length>=trackerLimits.labelsPerItem,run:()=>void change({labels:issue.labels.includes(item.id)?issue.labels.filter(id=>id!==item.id):[...issue.labels,item.id]})})),
            {label:"Manage labels",icon:"settings" as const,separator:labels.length>0,run:()=>onEdit({type:"labels"})}]}><Icon name="label"/><span>Edit labels</span></ActionMenu>}</div>}
        {pull&&<Segmented label="Pull request view" value={view} onChange={setView} options={[["conversation","Conversation"],["commits",`Commits${compare?` ${compare.ahead.length}`:""}`],["files",`Files${compare?` ${compare.files.length}`:changes?.agent?` ${changes.agent.changes.length}`:""}`]] as const}/>}
        {failure&&<p className="error" role="alert">{failure}</p>}
        {view==="conversation"||!pull?<>
          <Timeline issue={issue} repo={repo} session={session} reference={reference} onChanged={()=>void reload().catch(cause=>setFailure(trackerFailure(cause,kind)))}/>
          {pull&&issue.state==="open"&&issue.permissions.merge&&(changesError?<Failure error={trackerFailure(changesError,"pull")} retry={retryChanges} what="The branches"/>:!changes?<Loading label="Checking the branches"/>
            :changes.note?<div className="git-banner"><Icon name="alert"/><div><strong>This pull request can’t be merged</strong><span>{changes.note}</span></div></div>
            :compare?compare.ahead.length?<MergeBox key={`${compare.baseSha}:${compare.headSha}`} repo={repo} compare={compare} session={session} defaultMessage={`Merge pull request #${issue.number} from ${issue.head}\n\n${issue.title}`}
              onMerged={result=>{notify(result.fastForward?`${issue.base} now points at ${issue.head}`:`Merged #${issue.number}`);void reload();}}/>
              :<div className="git-banner is-info"><Icon name="info"/><div><strong>Nothing to merge</strong><span>{issue.base} already has every commit on {issue.head}. Push commits to the branch, or close this pull request.</span></div></div>
            :changes.agent?<AgentMerge repo={repo} issue={issue} agent={changes.agent} session={session} onMerged={()=>{notify(`Merged #${issue.number} into the folder`);void reload();}}/>:null)}
          <Composer issue={issue} repo={repo} session={session} busy={busy} reference={reference} onClose={state=>void change({state},state==="closed"?`Closed #${issue.number}`:`Reopened #${issue.number}`)} onCommented={()=>void reload()} />
        </>:changesError?<Failure error={trackerFailure(changesError,"pull")} retry={retryChanges} what="The changes"/>:!changes?<Loading label="Loading changes"/>
          :changes.note&&!compare?<p className="git-hint">{changes.note}</p>
          :view==="commits"?compare?compare.ahead.length?<ol className="git-mini-commits">{compare.ahead.map(commit=><li key={commit.sha}><button type="button" onClick={()=>onCommit(commit.sha)}><Person name={commit.author} source={commit.source}/><span>{commit.subject}</span><Sha sha={commit.sha}/><small>{edited(commit.time)}</small></button></li>)}</ol>
              :<p className="git-hint">{issue.head} has no commits that {issue.base} doesn’t.</p>
            :<p className="git-hint">Agent changes are not commits yet: merging them commits them to main. {changes.agent&&<a href={`/agent?thread=${encodeURIComponent(changes.agent.threadId)}`}>Open the thread in Agent</a>}</p>
          :compare?<><div className="git-files-summary"><strong>{plural(compare.files.length,"file")} changed</strong><span className="git-stat"><span className="git-added">+{compare.files.reduce((sum,file)=>sum+file.added,0)}</span> <span className="git-removed">−{compare.files.reduce((sum,file)=>sum+file.removed,0)}</span></span>
              <label className="git-wrap"><input type="checkbox" checked={wrap} onChange={event=>setWrap(event.target.checked)}/>Wrap lines</label></div><FileChanges files={compare.files} wrap={wrap}/></>
            :changes.agent?<div className="history-changes">{changes.agent.changes.map(item=><ChangeRow key={item.path} folderId={repo.id} change={item} live={false}/>)}</div>:null}
      </>}
    </div>
  </div>;
}

/** The description, then comments and events in order. */
function Timeline({issue,repo,session,reference,onChanged}:{issue:GitIssue;repo:GitRepository;session:BrowserSession;reference:MarkdownReference;onChanged:()=>void}){
  return <ol className="git-timeline" aria-label="Conversation">
    <li><article className="git-comment" aria-label={`Description by ${issue.author}`}>
      <header><Person name={issue.author}/><strong>{issue.author}</strong><span className="git-muted" title={localDateTime(issue.createdAt,{dateStyle:"medium",timeStyle:"short"})}>{edited(issue.createdAt)}</span></header>
      <div className="git-comment-body">{issue.body?<Markdown source={issue.body} reference={reference}/>:<p className="git-muted">No description.</p>}</div>
    </article></li>
    {issue.timeline.map(item=><li key={item.id}>{item.kind==="comment"?<Comment item={item} repo={repo} session={session} reference={reference} onChanged={onChanged}/>:<TimelineEvent item={item} reference={reference}/>}</li>)}
  </ol>;
}
function TimelineEvent({item,reference}:{item:Extract<GitTimelineItem,{kind:"event"}>;reference:MarkdownReference}){
  const ref=(number:number)=>reference(number,0);
  const [icon,text]:[IconName,ReactNode]=item.event==="closed"?["issueDone",item.number?<>closed this with {ref(item.number)}</>:"closed this"]
    :item.event==="reopened"?["issue","reopened this"]
    :item.event==="merged"?["merge",item.sha?<>merged this as <Sha sha={item.sha}/></>:"merged this into the folder"]
    :item.event==="renamed"?["pencil",<>changed the title from <del>{item.from}</del> to {item.to}</>]
    :item.event==="labeled"?["label",<>added the label <strong>{item.label}</strong></>]
    :item.event==="unlabeled"?["label",<>removed the label <strong>{item.label}</strong></>]
    :item.event==="referenced"?["link",<>mentioned this in {ref(item.number!)}{item.title?<span className="git-muted"> {item.title}</span>:null}</>]
    :item.event==="pushed"?["upload",<>pushed to the branch: <Sha sha={item.from!}/> → <Sha sha={item.to!}/></>]
    :["branch",<>deleted the branch <code>{item.from}</code>, which closed this</>];
  return <div className={`git-event is-${item.event}`}><Icon name={icon}/><span><strong>{item.author}</strong> {text} <span className="git-muted" title={localDateTime(item.createdAt,{dateStyle:"medium",timeStyle:"short"})}>{edited(item.createdAt)}</span></span></div>;
}
function Comment({item,repo,session,reference,onChanged}:{item:Extract<GitTimelineItem,{kind:"comment"}>;repo:GitRepository;session:BrowserSession;reference:MarkdownReference;onChanged:()=>void}){
  const [editing,setEditing]=useState(false),[text,setText]=useState(item.body),[busy,setBusy]=useState(false),[error,setError]=useState(""),[confirm,setConfirm]=useState(false);
  const url=`${repoApi(repo.id)}/comments/${item.id}`;
  async function save(){setBusy(true);setError("");try{await request(url,{method:"PATCH",body:JSON.stringify({body:text})},session.csrfToken);setEditing(false);onChanged();}catch(cause){setError(trackerFailure(cause));}finally{setBusy(false);}}
  async function remove(){setBusy(true);setError("");try{await request(url,{method:"DELETE"},session.csrfToken);setConfirm(false);onChanged();}catch(cause){setError(trackerFailure(cause));setBusy(false);}}
  const actions=[...item.canEdit?[{label:"Edit",icon:"pencil" as const,run:()=>{setText(item.body);setEditing(true);}}]:[],...item.canDelete?[{label:"Delete",icon:"trash" as const,danger:true,separator:item.canEdit,run:()=>setConfirm(true)}]:[]];
  return <article className="git-comment" aria-label={`Comment by ${item.author}`}>
    <header><Person name={item.author}/><strong>{item.author}</strong><span className="git-muted" title={localDateTime(item.createdAt,{dateStyle:"medium",timeStyle:"short"})}>{edited(item.createdAt)}{item.editedAt?" · edited":""}</span>
      {actions.length>0&&!editing&&<ActionMenu label={`Actions for the comment by ${item.author}`} actions={actions}><Icon name="more"/></ActionMenu>}</header>
    {editing?<div className="git-comment-body"><MarkdownField label="Edit comment" value={text} onChange={setText} reference={reference} autoFocus/>
      {error&&<p className="error" role="alert">{error}</p>}
      <div className="git-composer-actions"><button type="button" className="text-button" onClick={()=>setEditing(false)}>Cancel</button><button type="button" className="primary-button" disabled={busy||!text.trim()} onClick={()=>void save()}>Save</button></div></div>
      :<div className="git-comment-body"><Markdown source={item.body} reference={reference}/>{error&&<p className="error" role="alert">{error}</p>}</div>}
    {confirm&&<Dialog title="Delete this comment?" onClose={()=>setConfirm(false)}><p>The comment by {item.author} is removed from the conversation for everyone.</p>{error&&<p className="error" role="alert">{error}</p>}
      <div className="dialog-actions"><button className="text-button" onClick={()=>setConfirm(false)}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>void remove()}>Delete comment</button></div></Dialog>}
  </article>;
}
/** Markdown with Write and Preview. */
function MarkdownField({label,value,onChange,reference,placeholder,autoFocus,rows=5}:{label:string;value:string;onChange:(value:string)=>void;reference?:MarkdownReference;placeholder?:string;autoFocus?:boolean;rows?:number}){
  const [mode,setMode]=useState<"write"|"preview">("write"),id=useId();
  return <div className="git-markdown-field">
    <div className="git-markdown-bar"><label htmlFor={id}>{label}</label><Segmented label={`${label} view`} value={mode} options={[["write","Write"],["preview","Preview"]] as const} onChange={setMode}/></div>
    {mode==="write"?<textarea id={id} value={value} rows={rows} maxLength={trackerLimits.body} placeholder={placeholder} autoFocus={autoFocus} onChange={event=>onChange(event.target.value)}/>
      :<div className="git-markdown-preview" id={id} tabIndex={0} aria-label={`${label} preview`}>{value.trim()?<Markdown source={value} reference={reference}/>:<p className="git-muted">Nothing to preview.</p>}</div>}
    <small className="git-muted">Markdown. #12 links an issue or pull request.</small>
  </div>;
}
function Composer({issue,repo,session,busy,reference,onClose,onCommented}:{issue:GitIssue;repo:GitRepository;session:BrowserSession;busy:boolean;reference:MarkdownReference;onClose:(state:"open"|"closed")=>void;onCommented:()=>void}){
  const [text,setText]=useState(""),[sending,setSending]=useState(false),[error,setError]=useState("");
  async function send(close?:boolean){
    setSending(true);setError("");
    try{if(text.trim())await request(`${repoApi(repo.id)}/issues/${issue.number}/comments`,{method:"POST",body:JSON.stringify({body:text})},session.csrfToken);setText("");onCommented();if(close)onClose("closed");}
    catch(cause){setError(trackerFailure(cause,issue.kind));}finally{setSending(false);}
  }
  const canClose=issue.permissions.close,open=issue.state==="open";
  return <section className="git-composer" aria-label="New comment">
    <MarkdownField label="Add a comment" value={text} onChange={setText} reference={reference} placeholder="Leave a comment" rows={4}/>
    {error&&<p className="error" role="alert">{error}</p>}
    <div className="git-composer-actions">
      {canClose&&(open?<button type="button" className="secondary-button" disabled={busy||sending} onClick={()=>void send(true)}><Icon name={issue.kind==="pull"?"pullClosed":"issueDone"}/>{text.trim()?"Close with comment":`Close ${noun(issue.kind)}`}</button>
        :issue.state==="closed"&&<button type="button" className="secondary-button" disabled={busy||sending} onClick={()=>onClose("open")}><Icon name={issue.kind==="pull"?"pull":"issue"}/>Reopen</button>)}
      <button type="button" className="primary-button" disabled={sending||!text.trim()} onClick={()=>void send()}><Icon name="comment"/>Comment</button>
    </div>
  </section>;
}
/** An Agent branch merges into the folder through Agent's own merge, which checks each file. */
function AgentMerge({repo,issue,agent,session,onMerged}:{repo:GitRepository;issue:GitIssue;agent:NonNullable<GitPullChanges["agent"]>;session:BrowserSession;onMerged:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  async function merge(){
    setBusy(true);setError("");
    try{
      const result=await request<{merged:boolean;conflicts?:string[]}>(`/api/v1/files/history/${repo.id}/branches/merge`,{method:"POST",body:JSON.stringify({name:issue.head,revision:agent.revision})},session.csrfToken);
      if(result.merged)onMerged();else{setError(`${result.conflicts?.length===1?"This file changed":"These files changed"} in the folder since Agent started: ${result.conflicts?.slice(0,6).join(", ")}. Ask Agent to update its changes.`);setBusy(false);}
    }catch(cause){setError(/conflict/.test(message(cause))?"The folder changed meanwhile. Reload and try again.":message(cause));setBusy(false);}
  }
  return <section className="git-merge" aria-label={`Merge ${issue.head} into the folder`}>
    <h3 className="git-sheet-label">Merge into main</h3>
    <p className="git-hint">{plural(agent.changes.length,"file")} from Agent change in the folder and are committed to main. Files changed since Agent started stop the merge.</p>
    {error&&<p className="error" role="alert">{error}</p>}
    <button type="button" className="primary-button git-merge-button" disabled={busy||!agent.changes.length} onClick={()=>void merge()}><Icon name="merge"/>{busy?"Merging…":"Merge into main"}</button>
  </section>;
}

// ---- Editing panes ------------------------------------------------------------------------------

/** New issue, new pull request (from a branch or an Agent branch) or editing one, in the right pane. */
export function IssueEditor({repo,editing,session,onClose,onSaved}:{repo:GitRepository;editing:Extract<IssueEditing,{type:"issue"}>;session:BrowserSession;onClose:()=>void;onSaved:(number:number)=>void}){
  const existing=editing.issue,kind=editing.kind,pull=kind==="pull";
  const [title,setTitle]=useState(existing?.title??""),[body,setBody]=useState(existing?.body??""),[chosen,setChosen]=useState<string[]>(existing?.labels??[]);
  const [base,setBase]=useState(editing.base??"main"),[head,setHead]=useState(editing.head??""),[busy,setBusy]=useState(false),[error,setError]=useState(""),titleId=useId();
  const {value:labels}=useLoad<GitLabel[]>(`${repoApi(repo.id)}/labels`);
  const {value:branches}=useLoad<GitBranches>(pull&&!existing?`${repoApi(repo.id)}/branches`:undefined);
  const gitBranches=branches?[...branches.main?["main"]:[],...branches.branches.map(branch=>branch.name)]:[];
  const agentBranches=branches?.agent??[],agent=head.startsWith("agent/");
  useEffect(()=>{if(!pull||existing||head||!branches)return;const first=branches.branches[0]?.name??agentBranches[0]?.name;if(first)setHead(first);},[branches]);
  // A pull request from a branch starts with its newest commit's subject, or the Agent branch's message.
  useEffect(()=>{if(existing||!pull||title)return;const agentBranch=agentBranches.find(branch=>branch.name===head),branch=branches?.branches.find(item=>item.name===head);
    if(agentBranch?.message)setTitle(agentBranch.message);else if(branch)setTitle(branch.subject);},[head,branches]);
  const {value:compare,error:compareError}=useLoad<GitCompare>(pull&&!existing&&head&&!agent&&head!==base?`${repoApi(repo.id)}/compare?${new URLSearchParams({base,head})}`:undefined);
  const valid=!!title.trim()&&title.length<=trackerLimits.title&&(!pull||!!existing||!!head&&head!==base);
  const dirty=title!==(existing?.title??"")||body!==(existing?.body??"");
  async function save(){
    setBusy(true);setError("");
    try{
      if(existing){
        const labelsChanged=repo.role!=="read"&&(chosen.length!==existing.labels.length||chosen.some(id=>!existing.labels.includes(id)));
        await request(`${repoApi(repo.id)}/issues/${existing.number}`,{method:"PATCH",body:JSON.stringify({revision:existing.revision,title,body,...labelsChanged?{labels:chosen}:{}})},session.csrfToken);
        onSaved(existing.number);
      }else{
        const created=await request<{number:number}>(`${repoApi(repo.id)}/issues`,{method:"POST",body:JSON.stringify({kind,title,body,...chosen.length?{labels:chosen}:{},...pull?{base:agent?"main":base,head}:{}})},session.csrfToken);
        onSaved(created.number);
      }
    }catch(cause){setError(trackerFailure(cause,kind));setBusy(false);}
  }
  const heading=existing?`Edit ${noun(kind)} #${existing.number}`:`New ${noun(kind)}`;
  return <EditorPane title={heading} onClose={onClose} dirty={dirty&&!busy}>
    <form className="git-new git-issue-form" onSubmit={event=>{event.preventDefault();if(valid&&!busy)void save();}}>
      {pull&&!existing&&<section className="git-form-section" aria-label="Branches">
        <h3 className="git-sheet-label"><Icon name="pull"/>Branches</h3>
        {!branches?<Loading label="Loading branches"/>:<>
          <div className="git-compare-pick">
            <Select aria-label="Merge into" value={agent?"main":base} disabled={agent} onChange={event=>setBase(event.target.value)}>{gitBranches.map(name=><option key={name} value={name}>{name}</option>)}</Select>
            <Icon name="back"/>
            <Select aria-label="Branch to merge" value={head} onChange={event=>setHead(event.target.value)}>
              {!head&&<option value="">Choose a branch</option>}
              {gitBranches.filter(name=>name!=="main").map(name=><option key={name} value={name}>{name}</option>)}
              {agentBranches.map(branch=><option key={branch.name} value={branch.name}>{`${branch.name} (Agent)`}</option>)}
            </Select>
          </div>
          {!head?<p className="git-hint">Push a branch with Git, or let Agent work on this folder, to open a pull request.</p>
            :agent?<p className="git-hint">Agent’s changes merge into main through Agent’s merge, which checks every file first.</p>
            :head===base?<p className="git-hint">Choose two different branches.</p>
            :compareError?<p className="error" role="alert">{trackerFailure(compareError,"pull")}</p>
            :!compare?<Loading label="Comparing"/>
            :!compare.ahead.length?<div className="git-banner is-info"><Icon name="info"/><div><strong>Nothing to merge yet</strong><span>{base} already has every commit on {head}. You can still open the pull request and push to the branch.</span></div></div>
            :<p className={`git-pull-check ${compare.mergeable?"is-ok":"is-conflict"}`}><Icon name={compare.mergeable?"check":"alert"}/>{plural(compare.ahead.length,"commit")} · {plural(compare.files.length,"file")} · {compare.mergeable?compare.fastForward?"Can fast-forward":"Can merge":"Conflicts with "+base}</p>}
        </>}
      </section>}
      <label className="git-field" htmlFor={titleId}>Title<input id={titleId} value={title} maxLength={trackerLimits.title} required autoFocus onChange={event=>setTitle(event.target.value)}/></label>
      <MarkdownField label="Description" value={body} onChange={setBody} rows={8} placeholder={pull?"What this changes and why. “Closes #12” closes that issue when this is merged.":"What happens, and what should happen instead."}/>
      {repo.role!=="read"&&<fieldset className="git-form-section git-label-choices"><legend className="git-sheet-label"><Icon name="label"/>Labels</legend>
        {!labels?<Loading label="Loading labels"/>:!labels.length?<p className="git-hint">No labels yet. Create them from Labels in the Issues tab.</p>
          :labels.map(label=><label key={label.id} className="git-check"><input type="checkbox" checked={chosen.includes(label.id)} disabled={!chosen.includes(label.id)&&chosen.length>=trackerLimits.labelsPerItem}
            onChange={event=>setChosen(current=>event.target.checked?[...current,label.id]:current.filter(id=>id!==label.id))}/><LabelChip label={label}/>{label.description&&<small className="git-muted">{label.description}</small>}</label>)}</fieldset>}
      {error&&<p className="error" role="alert">{error}</p>}
      <div className="git-new-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button>
        <button type="submit" className="primary-button" disabled={!valid||busy}>{busy?"Saving…":existing?"Save":pull?"Open pull request":"Open issue"}</button></div>
    </form>
  </EditorPane>;
}

/** The repository's labels: name, colour (always named, never colour alone) and description. */
export function LabelsEditor({repo,session,onClose,onChanged}:{repo:GitRepository;session:BrowserSession;onClose:()=>void;onChanged:()=>void}){
  const {value:labels,error,retry,setValue}=useLoad<GitLabel[]>(`${repoApi(repo.id)}/labels`);
  const [form,setForm]=useState<{id?:string;name:string;color:GitLabelColor;description:string}>({name:"",color:"plum",description:""});
  const [busy,setBusy]=useState(false),[failure,setFailure]=useState(""),[remove,setRemove]=useState<GitLabel>(),nameId=useId(),descriptionId=useId();
  const reload=async()=>{setValue(await request<GitLabel[]>(`${repoApi(repo.id)}/labels`));onChanged();};
  async function save(){
    setBusy(true);setFailure("");
    try{
      const {id,...fields}=form;
      await request(id?`${repoApi(repo.id)}/labels/${id}`:`${repoApi(repo.id)}/labels`,{method:id?"PATCH":"POST",body:JSON.stringify(fields)},session.csrfToken);
      setForm({name:"",color:form.color,description:""});await reload();
    }catch(cause){setFailure(trackerFailure(cause,"label"));}finally{setBusy(false);}
  }
  async function confirmRemove(label:GitLabel){
    setBusy(true);setFailure("");
    try{await request(`${repoApi(repo.id)}/labels/${label.id}`,{method:"DELETE"},session.csrfToken);setRemove(undefined);if(form.id===label.id)setForm({name:"",color:"plum",description:""});await reload();}
    catch(cause){setFailure(trackerFailure(cause,"label"));}finally{setBusy(false);}
  }
  const valid=!!form.name.trim()&&form.name.length<=trackerLimits.labelName;
  return <EditorPane title="Labels" onClose={onClose} dirty={!!form.name&&!busy}>
    <form className="git-new" onSubmit={event=>{event.preventDefault();if(valid&&!busy)void save();}}>
      <section className="git-form-section" aria-label={form.id?"Edit label":"New label"}>
        <h3 className="git-sheet-label">{form.id?"Edit label":"New label"}</h3>
        <label className="git-field" htmlFor={nameId}>Name<input id={nameId} value={form.name} maxLength={trackerLimits.labelName} onChange={event=>setForm({...form,name:event.target.value})}/></label>
        <label className="git-field" htmlFor={descriptionId}>Description<input id={descriptionId} value={form.description} maxLength={trackerLimits.labelDescription} onChange={event=>setForm({...form,description:event.target.value})}/></label>
        <fieldset className="git-colors"><legend>Colour</legend>{gitLabelColors.map(color=><label key={color} className={`git-color is-${color}`}><input type="radio" name="label-color" value={color} checked={form.color===color} onChange={()=>setForm({...form,color})}/><span className="git-color-dot" aria-hidden="true"/>{color[0].toUpperCase()+color.slice(1)}</label>)}</fieldset>
        <p className="git-label-preview">Preview <LabelChip label={{id:"",name:form.name.trim()||"label",color:form.color,description:form.description}}/></p>
        {failure&&<p className="error" role="alert">{failure}</p>}
        <div className="git-new-actions">{form.id&&<button type="button" className="text-button" onClick={()=>setForm({name:"",color:"plum",description:""})}>Cancel editing</button>}<button type="submit" className="primary-button" disabled={!valid||busy}>{form.id?"Save label":"Create label"}</button></div>
      </section>
      <section className="git-form-section" aria-label="Labels in this repository">
        <h3 className="git-sheet-label">{labels?plural(labels.length,"label"):"Labels"}</h3>
        {error?<Failure error={error} retry={retry} what="Labels"/>:!labels?<Loading label="Loading labels"/>:!labels.length?<p className="git-hint">Labels sort issues and pull requests, such as bug or docs.</p>
          :<ul className="git-label-list">{labels.map(label=><li key={label.id}><LabelChip label={label}/><span className="git-muted">{label.description}</span>
            <ActionMenu label={`Actions for ${label.name}`} actions={[{label:"Edit",icon:"pencil",run:()=>setForm({id:label.id,name:label.name,color:label.color,description:label.description})},{label:"Delete",icon:"trash",danger:true,separator:true,run:()=>setRemove(label)}]}><Icon name="more"/></ActionMenu></li>)}</ul>}
      </section>
    </form>
    {remove&&<Dialog title={`Delete ${remove.name}?`} onClose={()=>setRemove(undefined)}><p>The label is removed from every issue and pull request in {repo.name}. Their history keeps a note that it was there.</p>{failure&&<p className="error" role="alert">{failure}</p>}
      <div className="dialog-actions"><button className="text-button" onClick={()=>setRemove(undefined)}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>void confirmRemove(remove)}>Delete label</button></div></Dialog>}
  </EditorPane>;
}
