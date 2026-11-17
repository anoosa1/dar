import {useCallback,useEffect,useId,useMemo,useRef,useState,type ReactNode} from "react";
import {request,formatBytes,type BrowserSession} from "../api.ts";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {ActionMenu,NewItemButton,Select,useFloating} from "../controls.tsx";
import {Segmented} from "../settings-sheet.tsx";
import {Dialog} from "../dialog.tsx";
import {EditorPane} from "../pane.tsx";
import {DarFilePicker,type DarFileSelection} from "../file-picker.tsx";
import {Markdown} from "../pim/markdown.tsx";
import {useLiveConnection} from "../live.ts";
import {edited} from "../pim/presentation.ts";
import {localDate,localDateTime} from "../dates.ts";
import {graphRows,type GraphRow} from "./graph.ts";
import {IssuesTab,IssueSheet,IssueEditor,LabelsEditor,type IssueEditing} from "./tracker.tsx";
import {api,repoApi,message,plural,short,kindLabel,SourceChip,Sha,Person,copy,useLoad,Failure,Loading,SheetHead,useSheetFocus,FileChanges,MergeBox} from "./parts.tsx";
import type {HistoryOverview} from "../../shared/history.ts";
import type {GitActivity,GitAgentBranch,GitBranch,GitBranches,GitCommit,GitCommitDetail,GitCommitPage,GitCompare,GitRef,GitRepository,GitTag,GitTree} from "../../shared/git.ts";

type View="overview"|"repositories"|"agent";
type Tab="code"|"issues"|"pulls"|"commits"|"branches"|"tags";
type Sheet={type:"commit";sha:string}|{type:"compare";base:string;head:string}|{type:"file";ref:string;path:string}|{type:"issue";number:number};
interface Notice {message:string}
const views:[View,string,IconName][]=[["overview","Overview","activity"],["repositories","Repositories","repo"],["agent","Agent branches","agent"]];
const tabs:[Tab,string,IconName][]=[["code","Code","code"],["issues","Issues","issue"],["pulls","Pull requests","pull"],["commits","Commits","commit"],["branches","Branches","branch"],["tags","Tags","tag"]];
/** Repository marks take one of a fixed set of hues from the name (colour is data, not theme). */
const hue=(text:string)=>{let h=0;for(const c of text)h=(h*31+c.codePointAt(0)!)%9973;return h%6;};
const zipUrl=(id:string,ref:string)=>`${repoApi(id)}/archive?ref=${encodeURIComponent(ref)}`;

function readState(){
  const p=new URLSearchParams(location.search),view=p.get("view"),tab=p.get("tab");
  return {view:(views.some(([id])=>id===view)?view:"overview") as View,repo:p.get("repo")??undefined,tab:(tabs.some(([id])=>id===tab)?tab:"code") as Tab,
    ref:p.get("ref")??"",path:p.get("path")??"",commit:p.get("commit")??undefined,issue:/^[1-9]\d{0,5}$/.test(p.get("issue")??"")?Number(p.get("issue")):undefined};
}

/**
 * Git (selected mock B, docs/spacetimedb/git-mocks.html): every Files folder that keeps history is a
 * repository. Overview shows a year of activity, the repositories and recent commits; a repository
 * has Code, Commits (graph or list), Branches and Tags, with commits, comparisons and files in the
 * right sheet. Changes happen through Git, Files and Agent; here only branches and tags are deleted.
 */
export function GitWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readState,[]);
  const [repos,setRepos]=useState<GitRepository[]>(),[error,setError]=useState(""),[reload,setReload]=useState(0);
  const [view,setView]=useState<View>(start.view),[repoId,setRepoId]=useState(start.repo),[tab,setTab]=useState<Tab>(start.tab);
  const [ref,setRef]=useState(start.ref),[path,setPath]=useState(start.path),[sheet,setSheet]=useState<Sheet|undefined>(start.issue?{type:"issue",number:start.issue}:start.commit?{type:"commit",sha:start.commit}:undefined);
  const [creating,setCreating]=useState(false),[notice,setNotice]=useState<Notice|null>(null);
  // Issues and pull requests change without a new repository revision; this reloads their views.
  const [editing,setEditing]=useState<IssueEditing>(),[trackerVersion,setTrackerVersion]=useState(0);
  const trackerChanged=()=>{setTrackerVersion(n=>n+1);setReload(n=>n+1);};
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const load=useCallback(async(signal?:AbortSignal)=>{try{setRepos(await request<GitRepository[]>(`${api}/repositories`,{signal}));setError("");}catch(cause){if(!signal?.aborted)setError(message(cause));}},[]);
  useEffect(()=>{const abort=new AbortController();void load(abort.signal);return()=>abort.abort();},[reload]);
  useEffect(()=>{const focus=()=>setReload(n=>n+1);addEventListener("focus",focus);return()=>removeEventListener("focus",focus);},[]);
  useEffect(()=>{const url=new URL(location.href);for(const key of ["view","repo","tab","ref","path","commit","issue"])url.searchParams.delete(key);
    if(repoId){url.searchParams.set("repo",repoId);if(tab!=="code")url.searchParams.set("tab",tab);if(ref)url.searchParams.set("ref",ref);if(path)url.searchParams.set("path",path);if(sheet?.type==="commit")url.searchParams.set("commit",sheet.sha);if(sheet?.type==="issue")url.searchParams.set("issue",String(sheet.number));}
    else if(view!=="overview")url.searchParams.set("view",view);
    history.replaceState(null,"",url);},[view,repoId,tab,ref,path,sheet]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(null),5000);return()=>clearTimeout(timer);},[notice]);

  const repo=repoId?repos?.find(item=>item.id===repoId):undefined;
  const notify=(text:string)=>setNotice({message:text});
  const openRepo=(id:string,nextTab:Tab="code")=>{setRepoId(id);setTab(nextTab);setRef("");setPath("");setSheet(undefined);setEditing(undefined);setNavigation(false);};
  const choose=(next:View)=>{setView(next);setRepoId(undefined);setSheet(undefined);setNavigation(false);};
  const agentCount=repos?.reduce((sum,item)=>sum+item.agentBranches,0)??0;
  const title=repo?repo.name:views.find(([id])=>id===view)![1];

  const sidebar=<>
    <NewItemButton onClick={()=>setCreating(true)}>New repository</NewItemButton>
    <nav aria-label="Git views">{views.map(([id,label,icon])=><button key={id} className={`nav-item${!repoId&&view===id?" active":""}`} aria-current={!repoId&&view===id?"page":undefined} onClick={()=>choose(id)}><Icon name={icon}/><span className="git-row-title">{label}</span>
      {id==="repositories"&&!!repos?.length&&<span className="git-count">{repos.length}</span>}{id==="agent"&&agentCount>0&&<span className="git-count">{agentCount}</span>}</button>)}</nav>
    <div className="sidebar-heading git-heading"><h2>Repositories</h2></div>
    <nav aria-label="Repositories" className="git-repo-nav">{repos?.filter(item=>item.role==="owner").map(item=><button key={item.id} className={`nav-item${repoId===item.id?" active":""}`} aria-current={repoId===item.id?"page":undefined} onClick={()=>openRepo(item.id)}>
      <RepoMark name={item.name}/><span className="git-row-title">{item.name}</span>{item.dirty>0&&<span className="git-dirty-dot" title={`${plural(item.dirty,"uncommitted change")} in Dar`}><span className="sr-only">{plural(item.dirty,"uncommitted change")}</span></span>}</button>)}</nav>
    {!!repos?.some(item=>item.role!=="owner")&&<><div className="sidebar-heading git-heading"><h2>Shared with you</h2></div>
      <nav aria-label="Shared repositories" className="git-repo-nav">{repos.filter(item=>item.role!=="owner").map(item=><button key={item.id} className={`nav-item${repoId===item.id?" active":""}`} aria-current={repoId===item.id?"page":undefined} onClick={()=>openRepo(item.id)}>
        <RepoMark name={item.name}/><span className="git-row-title">{item.name}</span><span className="git-count" title={`Shared by ${item.owner}`}>{item.role==="read"?"Read":"Write"}</span></button>)}</nav></>}
    <div className="git-key-card"><Icon name="key"/><div><strong>Clone with an app password</strong><span>Sign in as {session.user.username} with an app password that has Files access.</span><a href="/settings?section=security">Create app password</a></div></div>
  </>;

  let content:ReactNode;
  if(error&&!repos)content=<div className="git-empty" role="alert"><h2>Git could not be loaded</h2><p>{error}</p><button className="secondary-button" onClick={()=>setReload(n=>n+1)}>Try again</button></div>;
  else if(!repos)content=<p className="git-empty" role="status">Loading repositories…</p>;
  else if(repoId&&!repo)content=<div className="git-empty" role="alert"><h2>This repository is not available</h2><p>The folder may have stopped keeping history, or you no longer have access.</p><button className="secondary-button" onClick={()=>choose("repositories")}>Show repositories</button></div>;
  else if(repo)content=<Repository key={repo.id} repo={repo} session={session} tab={tab} onTab={next=>{setTab(next);setSheet(undefined);}} trackerVersion={trackerVersion} onEdit={setEditing} gitRef={ref} onRef={setRef} path={path} onPath={setPath} sheet={sheet} onSheet={setSheet} notify={notify} onChanged={()=>setReload(n=>n+1)}/>;
  else if(!repos.length)content=<div className="git-empty"><span className="empty-icon"><Icon name="repo"/></span><h2>No repositories yet</h2><p>A folder becomes a repository when it keeps history. Start a new one, or keep history on a folder you already have.</p><div className="git-empty-actions"><button className="primary-button" onClick={()=>setCreating(true)}><Icon name="plus"/>New repository</button><a className="secondary-button" href="/">Open Files</a></div></div>;
  else if(view==="overview")content=<Overview repos={repos} onOpen={openRepo}/>;
  else if(view==="repositories")content=<RepositoryList repos={repos} onOpen={openRepo}/>;
  else content=<AgentBranches repos={repos} onOpen={id=>openRepo(id,"branches")}/>;

  const details=repo&&sheet?<aside className="pim-details git-details" aria-label={sheet.type==="commit"?"Commit":sheet.type==="compare"?"Compare":sheet.type==="issue"?"Conversation":"File"}>
    {sheet.type==="issue"?<IssueSheet key={sheet.number} repo={repo} number={sheet.number} session={session} version={trackerVersion} notify={notify} onClose={()=>setSheet(undefined)} onOpen={number=>setSheet({type:"issue",number})}
        onCommit={sha=>setSheet({type:"commit",sha})} onEdit={setEditing} onChanged={trackerChanged}/>
      :sheet.type==="commit"?<CommitSheet key={sheet.sha} repo={repo} sha={sheet.sha} session={session} onChanged={()=>setReload(n=>n+1)} onClose={()=>setSheet(undefined)} onBrowse={sha=>{setTab("code");setRef(sha);setPath("");setSheet(undefined);}} onCommit={sha=>setSheet({type:"commit",sha})} notify={notify}/>
      :sheet.type==="compare"?<CompareSheet key={`${sheet.base}...${sheet.head}`} repo={repo} session={session} onPull={(base,head)=>setEditing({type:"issue",kind:"pull",base,head})} notify={notify} onMerged={()=>{setSheet(undefined);setReload(n=>n+1);}} base={sheet.base} head={sheet.head} onChange={(base,head)=>setSheet({type:"compare",base,head})} onCommit={sha=>setSheet({type:"commit",sha})} onClose={()=>setSheet(undefined)}/>
      :<FileSheet key={`${sheet.ref}:${sheet.path}`} repo={repo} gitRef={sheet.ref} path={sheet.path} onClose={()=>setSheet(undefined)}/>}
  </aside>:null;
  return <div className={`app-shell pim-shell git-shell${details?" has-pim-details":""}`}>
    <WorkspaceHeader app="git" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId}/>
    <div className="pim-workspace git-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar git-sidebar${navigation?" is-open":""}`} aria-label="Git navigation">{sidebar}</aside>
      <main className="pim-main git-main" tabIndex={-1} aria-label={title}>
        {!repo&&<div className="git-heading-bar"><h1>{title}</h1>{view==="repositories"&&repos&&<span className="git-heading-count">{repos.length}</span>}</div>}
        {error&&repos&&<div role="alert" className="error git-error">{error}<button className="text-button" onClick={()=>{setError("");setReload(n=>n+1);}}>Reload</button></div>}
        {content}
      </main>
      {details}
    </div>
    {repo&&editing?.type==="issue"&&<IssueEditor key={editing.issue?.number??`new-${editing.kind}`} repo={repo} editing={editing} session={session} onClose={()=>setEditing(undefined)}
      onSaved={number=>{const kind=editing.kind;setEditing(undefined);trackerChanged();setTab(kind==="pull"?"pulls":"issues");setSheet({type:"issue",number});notify(editing.issue?"Saved":`Opened #${number}`);}}/>}
    {repo&&editing?.type==="labels"&&<LabelsEditor repo={repo} session={session} onClose={()=>setEditing(undefined)} onChanged={trackerChanged}/>}
    {creating&&<NewRepository session={session} onClose={()=>setCreating(false)} onCreated={id=>{setCreating(false);void load().then(()=>openRepo(id));notify("Repository ready");}}/>}
    <div className="git-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast git-toast"><span>{notice.message}</span></div>}</div>
  </div>;
}

function RepoMark({name,size=""}:{name:string;size?:""|"is-m"|"is-l"}){return <span className={`git-mark hue-${hue(name)} ${size}`} aria-hidden="true">{[...name][0]?.toUpperCase()??"?"}</span>;}
/** Commits per week as bars; heights are fixed classes scaled to the busiest week. */
function Spark({weeks,label}:{weeks:number[];label:string}){
  const max=Math.max(1,...weeks);
  return <span className="git-spark" role="img" aria-label={label}>{weeks.map((value,index)=><i key={index} className={`h${value?Math.max(1,Math.round(value/max*8)):0}`}/>)}</span>;
}
function Refs({refs}:{refs:GitRef[]}){return <>{refs.map(item=><span key={`${item.kind}:${item.name}`} className={`git-ref is-${item.kind}`}><Icon name={item.kind==="tag"?"tag":"branch"}/>{item.name}</span>)}</>;}

// ---- Overview, list and Agent branches -----------------------------------------------------

function Overview({repos,onOpen}:{repos:GitRepository[];onOpen:(id:string)=>void}){
  const {value:activity,error,retry}=useLoad<GitActivity>(`${api}/activity`);
  return <div className="git-scroll git-overview">
    {error?<Failure error={error} retry={retry} what="Activity"/>:!activity?<Loading label="Loading activity"/>:<Heatmap times={activity.times} repos={repos.length}/>}
    <div className="git-overview-grid">
      <section aria-label="Repositories"><h2 className="git-section-title">Repositories</h2><div className="git-cards">{repos.map(repo=><button key={repo.id} type="button" className="git-card" onClick={()=>onOpen(repo.id)}>
        <span className="git-card-head"><RepoMark name={repo.name} size="is-m"/><strong>{repo.name}</strong>{repo.dirty>0&&<span className="git-dirty-dot"><span className="sr-only">Uncommitted changes in Dar</span></span>}<small>{edited(repo.updatedAt)}</small></span>
        <span className={`git-description${repo.description?"":" is-empty"}`}>{repo.error?repo.error:repo.description||"No description"}</span>
        <span className="git-card-meta"><span><Icon name="commit"/>{repo.commits}</span><span><Icon name="branch"/>{repo.branches}</span><span><Icon name="tag"/>{repo.tags}</span><Spark weeks={repo.weeks} label={`Commits per week for ${repo.name}`}/></span>
      </button>)}</div></section>
      <section aria-label="Recent activity"><h2 className="git-section-title">Recent activity</h2>
        {error?null:!activity?<Loading label="Loading recent activity"/>:!activity.events.length?<p className="git-hint">Commits from Git, Dar and Agent appear here.</p>
          :<ol className="git-feed">{activity.events.map(event=><li key={`${event.repoId}:${event.sha}`}><Person name={event.author} source={event.source}/><div>
            <span className="git-feed-what"><strong>{event.author}</strong> committed to <button type="button" className="git-link" onClick={()=>onOpen(event.repoId)}>{event.repoName}</button><Refs refs={event.refs}/></span>
            <span className="git-feed-message">{event.subject}</span><span className="git-feed-meta"><SourceChip source={event.source}/>{edited(event.time)}</span></div></li>)}</ol>}
      </section>
    </div>
  </div>;
}
/** A year of commits per local day, Sunday-first columns ending this week. */
function Heatmap({times,repos}:{times:number[];repos:number}){
  const {cells,months,total}=useMemo(()=>{
    const today=new Date();today.setHours(0,0,0,0);
    const first=new Date(today);first.setDate(first.getDate()-364-first.getDay());
    const counts=new Map<number,number>();
    for(const time of times){const day=new Date(time*1000);day.setHours(0,0,0,0);if(day>=first&&day<=today)counts.set(day.getTime(),(counts.get(day.getTime())??0)+1);}
    const values=[...counts.values()].sort((a,b)=>a-b),at=(q:number)=>values[Math.min(values.length-1,Math.floor(values.length*q))]??1;
    const level=(count:number)=>!count?0:count<=at(.25)?1:count<=at(.5)?2:count<=at(.8)?3:4;
    const cells:{key:number;count:number;level:number;future:boolean}[]=[],months:string[]=[];
    for(let day=new Date(first);cells.length<53*7;day.setDate(day.getDate()+1)){
      const count=counts.get(day.getTime())??0;cells.push({key:day.getTime(),count,level:level(count),future:day>today});
      if(day.getDay()===0)months.push(day.getDate()<=7?localDate(day,{month:"short"}):"");
    }
    return {cells,months,total:[...counts.values()].reduce((a,b)=>a+b,0)};
  },[times]);
  // Recent weeks matter most: a narrow screen starts scrolled to today.
  const scroller=useRef<HTMLDivElement>(null);
  useEffect(()=>{const node=scroller.current;if(!node)return;const end=()=>{node.scrollLeft=node.scrollWidth;};end();
    const resized=new ResizeObserver(end);resized.observe(node);return()=>resized.disconnect();},[cells]);
  return <section className="git-heat-card" aria-label="Activity over the last year">
    <div className="git-heat-head"><strong>{plural(total,"commit")} in the last year</strong><span>across {plural(repos,"repository","repositories")}</span>
      <span className="git-heat-legend" aria-hidden="true">Less<i className="l0"/><i className="l1"/><i className="l2"/><i className="l3"/><i className="l4"/>More</span></div>
    <div ref={scroller} className="git-heat-scroll"><div className="git-heat-months" aria-hidden="true">{months.map((month,index)=><span key={index}>{month}</span>)}</div>
      <div className="git-heat-body"><div className="git-heat-days" aria-hidden="true"><span/><span>Mon</span><span/><span>Wed</span><span/><span>Fri</span><span/></div>
        <div className="git-heat" role="img" aria-label={`${plural(total,"commit")} over the last year`}>{cells.map(cell=><i key={cell.key} className={cell.future?"is-future":`l${cell.level}`} title={cell.future?undefined:`${plural(cell.count,"commit")} on ${localDate(cell.key,{dateStyle:"medium"})}`}/>)}</div></div></div>
  </section>;
}
function RepositoryList({repos,onOpen}:{repos:GitRepository[];onOpen:(id:string)=>void}){
  return <ul className="git-scroll git-repo-list" aria-label="Repositories">{repos.map(repo=><li key={repo.id}><button type="button" className="git-repo-row" onClick={()=>onOpen(repo.id)}>
    <RepoMark name={repo.name} size="is-l"/>
    <span className="git-repo-body"><span className="git-repo-name"><strong>{repo.name}</strong><small>{repo.path}</small>{repo.dirty>0&&<span className="git-warn-chip"><Icon name="pencil"/>{repo.dirty} uncommitted</span>}{repo.agentBranches>0&&<span className="git-source is-agent"><Icon name="agent"/>{plural(repo.agentBranches,"Agent branch","Agent branches")}</span>}</span>
      <span className={`git-description${repo.description?"":" is-empty"}`}>{repo.error??(repo.description||"No description. Add a README.md to describe this repository.")}</span>
      <span className="git-repo-meta"><span><Icon name="commit"/>{plural(repo.commits,"commit")}</span><span><Icon name="branch"/>{plural(repo.branches,"branch","branches")}</span><span><Icon name="tag"/>{plural(repo.tags,"tag")}</span><span><Icon name="clock"/>Updated {edited(repo.updatedAt)}</span><span>{formatBytes(repo.size)}</span></span></span>
    <Spark weeks={repo.weeks} label={`Commits per week over 12 weeks for ${repo.name}`}/>
  </button></li>)}</ul>;
}
function AgentBranches({repos,onOpen}:{repos:GitRepository[];onOpen:(id:string)=>void}){
  const withBranches=repos.filter(repo=>repo.agentBranches>0),[rows,setRows]=useState<{repo:GitRepository;branch:GitAgentBranch}[]>(),[error,setError]=useState(""),[attempt,setAttempt]=useState(0);
  useEffect(()=>{const abort=new AbortController();setError("");
    void Promise.all(withBranches.map(repo=>request<GitBranches>(`${repoApi(repo.id)}/branches`,{signal:abort.signal}).then(value=>value.agent.map(branch=>({repo,branch})))))
      .then(list=>setRows(list.flat().sort((a,b)=>b.branch.modifiedAt-a.branch.modifiedAt))).catch(cause=>{if(!abort.signal.aborted)setError(message(cause));});
    return()=>abort.abort();},[withBranches.map(repo=>repo.id).join(),attempt]);
  if(error)return <div className="git-scroll"><Failure error={error} retry={()=>setAttempt(n=>n+1)} what="Agent branches"/></div>;
  if(!rows)return <div className="git-scroll"><Loading label="Loading Agent branches"/></div>;
  if(!rows.length)return <div className="git-empty"><span className="empty-icon"><Icon name="agent"/></span><h2>No Agent branches</h2><p>When Agent works on a folder that keeps history, its changes wait on a branch here until you merge them in Agent.</p></div>;
  return <ul className="git-scroll git-branch-list" aria-label="Agent branches">{rows.map(({repo,branch})=><li key={`${repo.id}:${branch.name}`} className="git-branch-row">
    <Icon name="branch" className="is-agent"/><div className="git-branch-body"><span className="git-branch-name">{branch.name}</span><span>{branch.message||"No message"} · <button type="button" className="git-link" onClick={()=>onOpen(repo.id)}>{repo.name}</button> · {plural(branch.files,"file")} · {edited(branch.modifiedAt)}</span></div>
    <a className="secondary-button" href={`/agent?thread=${encodeURIComponent(branch.threadId)}`}><Icon name="external"/>Open in Agent</a></li>)}</ul>;
}

// ---- A repository ------------------------------------------------------------------------------

interface RefLists {branches:GitBranches;tags:GitTag[]}
function Repository({repo,session,tab,onTab,gitRef,onRef,path,onPath,sheet,onSheet,notify,onChanged,trackerVersion,onEdit}:{repo:GitRepository;session:BrowserSession;tab:Tab;onTab:(tab:Tab)=>void;gitRef:string;onRef:(ref:string)=>void;path:string;onPath:(path:string)=>void;sheet?:Sheet;onSheet:(sheet:Sheet|undefined)=>void;notify:(text:string)=>void;onChanged:()=>void;trackerVersion:number;onEdit:(editing:IssueEditing)=>void}){
  const [version,setVersion]=useState(0);
  const [refs,setRefs]=useState<RefLists>(),[refsError,setRefsError]=useState("");
  useEffect(()=>{const abort=new AbortController();
    void Promise.all([request<GitBranches>(`${repoApi(repo.id)}/branches`,{signal:abort.signal}),request<GitTag[]>(`${repoApi(repo.id)}/tags`,{signal:abort.signal})])
      .then(([branches,tags])=>{setRefs({branches,tags});setRefsError("");}).catch(cause=>{if(!abort.signal.aborted)setRefsError(message(cause));});return()=>abort.abort();},[repo.id,repo.revision,version]);
  const changed=()=>{setVersion(n=>n+1);onChanged();};
  const [confirm,setConfirm]=useState<{name:string;old:string;kind:"branch"|"tag";commits?:number}>();
  const counts:Record<Tab,number|undefined>={code:undefined,issues:repo.openIssues,pulls:repo.openPulls,commits:repo.commits,branches:repo.branches,tags:repo.tags};
  const write=repo.role!=="read",pullFrom=(head:string)=>onEdit({type:"issue",kind:"pull",base:"main",head});
  const tabIds=useId();
  return <div className="git-repo">
    <div className="git-repo-head"><RepoMark name={repo.name} size="is-m"/><div className="git-repo-title"><h1>{repo.name}</h1><a className="git-crumb" href={repo.role==="owner"?`/?folder=${repo.parentId}&file=${repo.id}`:`/?folder=${repo.id}`}><Icon name={repo.role==="owner"?"folder":"share"}/>{repo.role==="owner"?repo.path:`Shared by ${repo.owner} · ${repo.role==="read"?"read only":"can write"}`}</a></div>
      <div className="git-tools"><CloneMenu repo={repo} gitRef={gitRef||"main"} username={session.user.username} notify={notify}/>
        <ActionMenu label={`More actions for ${repo.name}`} actions={[{label:"Show in Files",icon:"folder",href:repo.role==="owner"?`/?folder=${repo.parentId}&file=${repo.id}`:`/?folder=${repo.id}`},{label:"Copy clone address",icon:"copy",run:()=>copy(repo.clone.alias,notify,"clone address")},{label:"Download Git bundle",icon:"archive",href:`/api/v1/files/history/${repo.id}/bundle`,download:true,separator:true}]}><Icon name="more"/></ActionMenu></div></div>
    <div className="git-tabs" role="tablist" aria-label={`${repo.name} sections`}>{tabs.map(([id,label,icon])=><button key={id} id={`${tabIds}-${id}`} role="tab" aria-selected={tab===id} aria-controls={`${tabIds}-panel`} className={tab===id?"is-active":""} onClick={()=>onTab(id)}><Icon name={icon}/>{label}{counts[id]!==undefined&&<span className="git-tab-count">{counts[id]}</span>}</button>)}</div>
    <div className="git-panel" role="tabpanel" id={`${tabIds}-panel`} aria-labelledby={`${tabIds}-${tab}`}>
      {refsError&&<Failure error={refsError} retry={()=>setVersion(n=>n+1)} what="Branches and tags"/>}
      {tab==="issues"||tab==="pulls"?<IssuesTab key={tab} repo={repo} kind={tab==="pulls"?"pull":"issue"} version={trackerVersion} selected={sheet?.type==="issue"?sheet.number:undefined} onOpen={number=>onSheet({type:"issue",number})} onEdit={onEdit}/>
        :tab==="code"?<Code repo={repo} refs={refs} gitRef={gitRef} onRef={onRef} path={path} onPath={onPath} onFile={file=>onSheet({type:"file",ref:gitRef||"main",path:file})} onCommits={()=>onTab("commits")} onCommit={sha=>onSheet({type:"commit",sha})}/>
        :tab==="commits"?<Commits repo={repo} refs={refs} selected={sheet?.type==="commit"?sheet.sha:undefined} onCommit={sha=>onSheet({type:"commit",sha})}/>
        :tab==="branches"?<BranchesTab repo={repo} refs={refs} write={write} onPull={pullFrom} onCompare={head=>onSheet({type:"compare",base:"main",head})} onCommit={sha=>onSheet({type:"commit",sha})} onDelete={branch=>setConfirm({name:`refs/heads/${branch.name}`,old:branch.sha,kind:"branch",commits:branch.ahead})} onCode={name=>{onRef(name);onPath("");onTab("code");}} notify={notify}/>
        :<TagsTab repo={repo} refs={refs} write={write} onCommit={sha=>onSheet({type:"commit",sha})} onDelete={tag=>setConfirm({name:`refs/tags/${tag.name}`,old:tag.sha,kind:"tag"})} notify={notify}/>}
    </div>
    {confirm&&<Dialog title={`Delete ${confirm.name.replace(/^refs\/(heads|tags)\//,"")}?`} onClose={()=>setConfirm(undefined)}><DeleteRef repo={repo} target={confirm} session={session} onCancel={()=>setConfirm(undefined)} onDeleted={()=>{setConfirm(undefined);notify(confirm.kind==="tag"?"Tag deleted":"Branch deleted");if(gitRef===confirm.name.replace(/^refs\/(heads|tags)\//,""))onRef("");if(sheet?.type==="compare")onSheet(undefined);changed();}}/></Dialog>}
  </div>;
}
function DeleteRef({repo,target,session,onCancel,onDeleted}:{repo:GitRepository;target:{name:string;old:string;kind:"branch"|"tag";commits?:number};session:BrowserSession;onCancel:()=>void;onDeleted:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  const name=target.name.replace(/^refs\/(heads|tags)\//,"");
  return <><p>{target.kind==="tag"?`The tag ${name} is removed from ${repo.name}. Commits it reached stay while a branch reaches them.`
    :target.commits?`${plural(target.commits,"commit is","commits are")} only on ${name}. They are removed from Dar and their storage is released. Clones that already have them keep them.`:`${name} is removed from ${repo.name}. Its commits are also on main, so nothing else changes.`}</p>
    {error&&<p className="error" role="alert">{error}</p>}
    <div className="dialog-actions"><button className="text-button" onClick={onCancel}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setBusy(true);setError("");
      void request(`${repoApi(repo.id)}/refs`,{method:"DELETE",body:JSON.stringify({name:target.name,old:target.old})},session.csrfToken).then(onDeleted).catch(cause=>{setError(message(cause));setBusy(false);});}}>Delete {target.kind}</button></div></>;
}

/** Clone: the alias and the permanent address, downloads and a way to an app password. */
function CloneMenu({repo,gitRef,username,notify}:{repo:GitRepository;gitRef:string;username:string;notify:(text:string)=>void}){
  const [open,setOpen]=useState(false),trigger=useRef<HTMLButtonElement>(null),panel=useRef<HTMLDivElement>(null),field=useId(),permanent=useId();
  useFloating(open,trigger,panel,"end");
  useEffect(()=>{if(!open)return;const dismiss=(event:PointerEvent)=>{if(!panel.current?.contains(event.target as Node)&&!trigger.current?.contains(event.target as Node))setOpen(false);};
    document.addEventListener("pointerdown",dismiss);panel.current?.querySelector<HTMLInputElement>("input")?.focus();return()=>document.removeEventListener("pointerdown",dismiss);},[open]);
  const close=()=>{setOpen(false);trigger.current?.focus();};
  return <>
    <button ref={trigger} type="button" className="primary-button git-clone-button" aria-expanded={open} aria-haspopup="dialog" onClick={()=>setOpen(!open)}><Icon name="download"/>Clone<Icon name="chevron"/></button>
    {open&&<div ref={panel} className="git-clone-panel" role="dialog" aria-label="Clone" onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();event.stopPropagation();close();}}}>
      <label htmlFor={field}>Clone with HTTPS</label>
      <span className="git-clone-row"><input id={field} readOnly value={repo.clone.alias} onFocus={event=>event.target.select()}/><button type="button" className="icon-button" aria-label="Copy clone address" title="Copy" onClick={()=>copy(repo.clone.alias,notify,"clone address")}><Icon name="copy"/></button></span>
      <label htmlFor={permanent}>Permanent address <small>keeps working after a rename or move</small></label>
      <span className="git-clone-row"><input id={permanent} readOnly value={repo.clone.id} onFocus={event=>event.target.select()}/><button type="button" className="icon-button" aria-label="Copy permanent address" title="Copy" onClick={()=>copy(repo.clone.id,notify,"permanent address")}><Icon name="copy"/></button></span>
      <p>Sign in as {username} with an app password that has Files access. Write access lets you push.</p>
      <div className="git-clone-actions">
        <a className="git-menu-row" href={zipUrl(repo.id,gitRef)} download><Icon name="download"/>Download ZIP of {gitRef.length===40?short(gitRef):gitRef}</a>
        <a className="git-menu-row" href={`/api/v1/files/history/${repo.id}/bundle`} download><Icon name="archive"/>Download Git bundle<small>All branches and tags</small></a>
        <a className="git-menu-row" href="/settings?section=security"><Icon name="key"/>Create app password</a>
      </div>
    </div>}
  </>;
}
function RefPicker({refs,value,onChange,all=false,label}:{refs?:RefLists;value:string;onChange:(value:string)=>void;all?:boolean;label:string}){
  const branches=refs?[...refs.branches.main?["main"]:[],...refs.branches.branches.map(branch=>branch.name)]:["main"],tags=refs?.tags.map(tag=>tag.name)??[];
  const known=!value||value==="all"||branches.includes(value)||tags.includes(value);
  return <Select aria-label={label} className="git-ref-picker" value={value||(all?"all":"main")} onChange={event=>onChange(event.target.value==="all"||event.target.value==="main"&&!all?"":event.target.value)}>
    {all&&<option value="all">All branches</option>}{branches.map(name=><option key={`b:${name}`} value={name}>{name}</option>)}{tags.map(name=><option key={`t:${name}`} value={name}>{`${name} (tag)`}</option>)}{!known&&<option value={value}>{short(value)}</option>}
  </Select>;
}

function Code({repo,refs,gitRef,onRef,path,onPath,onFile,onCommits,onCommit}:{repo:GitRepository;refs?:RefLists;gitRef:string;onRef:(ref:string)=>void;path:string;onPath:(path:string)=>void;onFile:(path:string)=>void;onCommits:()=>void;onCommit:(sha:string)=>void}){
  const query=new URLSearchParams({...gitRef?{ref:gitRef}:{},...path?{path}:{}});
  const {value:tree,error,retry}=useLoad<GitTree>(`${repoApi(repo.id)}/tree?${query}`,[repo.revision]);
  const {value:latest}=useLoad<GitCommitPage>(`${repoApi(repo.id)}/commits?${new URLSearchParams({ref:gitRef||"main",limit:"1",...path?{}:{}})}`,[repo.revision]);
  if(!repo.commits&&!repo.branches)return <div className="git-empty"><span className="empty-icon"><Icon name="commit"/></span><h2>No commits yet</h2><p>Commit the folder in Files, or push to it with Git:</p><pre className="git-command">{`git clone ${repo.clone.alias}`}</pre></div>;
  const parts=path?path.split("/"):[];
  return <div className="git-scroll git-code">
    <div className="git-toolbar"><RefPicker refs={refs} value={gitRef} onChange={value=>{onRef(value);onPath("");}} label="Branch or tag"/>
      <nav className="git-path" aria-label="Folder"><button type="button" className="git-link" onClick={()=>onPath("")}>{repo.name}</button>{parts.map((part,index)=><span key={index}><Icon name="chevron"/>{index===parts.length-1?<strong aria-current="page">{part}</strong>:<button type="button" className="git-link" onClick={()=>onPath(parts.slice(0,index+1).join("/"))}>{part}</button>}</span>)}</nav></div>
    {repo.dirty>0&&!gitRef&&<div className="git-banner"><Icon name="pencil"/><div><strong>{plural(repo.dirty,"file")} changed in Dar since the last commit</strong><span>Pushes that change {repo.dirty===1?"it":"them"} are refused until {repo.dirty===1?"it is":"they are"} committed.</span></div><a className="text-button" href={`/?folder=${repo.parentId}&file=${repo.id}`}>Review and commit</a></div>}
    {error?<Failure error={error} retry={retry} what="Files"/>:!tree?<Loading label="Loading files"/>:<>
      {latest?.commits[0]&&<div className="git-latest"><Person name={latest.commits[0].author} source={latest.commits[0].source}/><strong>{latest.commits[0].author}</strong><button type="button" className="git-link git-latest-message" onClick={()=>onCommit(latest.commits[0].sha)}>{latest.commits[0].subject}</button><Sha sha={latest.commits[0].sha}/><span className="git-muted">{edited(latest.commits[0].time)}</span><button type="button" className="text-button" onClick={onCommits}><Icon name="history"/>{plural(latest.total,"commit")}</button></div>}
      <table className="git-files"><caption className="sr-only">Files in {path||repo.name}</caption><thead className="sr-only"><tr><th>Name</th><th>Last commit</th><th>Changed</th></tr></thead><tbody>
        {path&&<tr><td colSpan={3}><button type="button" className="git-file-name" onClick={()=>onPath(parts.slice(0,-1).join("/"))}><Icon name="back"/>..</button></td></tr>}
        {tree.entries.map(entry=><tr key={entry.path}><td><button type="button" className={`git-file-name is-${entry.type}`} disabled={entry.type==="submodule"} onClick={()=>entry.type==="tree"?onPath(entry.path):onFile(entry.path)}><Icon name={entry.type==="tree"?"folder":entry.type==="link"?"link":"file"}/>{entry.name}</button></td>
          <td className="git-file-commit">{entry.last&&<button type="button" className="git-link" onClick={()=>onCommit(entry.last!.sha)}>{entry.last.subject}</button>}</td><td className="git-file-time">{entry.last?edited(entry.last.time):""}</td></tr>)}
      </tbody></table>
      {!tree.entries.length&&<p className="git-hint">This folder is empty.</p>}
      {tree.readme&&<section className="git-readme" aria-label={tree.readme.path}><h2><Icon name="file"/>{tree.readme.path.split("/").at(-1)}</h2><div className="git-readme-body">{/\.(md|markdown)$/i.test(tree.readme.path)?<Markdown source={tree.readme.text}/>:<pre>{tree.readme.text}</pre>}</div></section>}
    </>}
  </div>;
}

function Commits({repo,refs,selected,onCommit}:{repo:GitRepository;refs?:RefLists;selected?:string;onCommit:(sha:string)=>void}){
  const [ref,setRef]=useState("all"),[author,setAuthor]=useState(""),[source,setSource]=useState(""),[mode,setMode]=useState<"graph"|"list">(()=>{try{return localStorage.getItem("dar.git.commits")==="list"?"list":"graph";}catch{return "graph";}});
  const [pages,setPages]=useState<GitCommitPage[]>([]),[error,setError]=useState(""),[loading,setLoading]=useState(false),[attempt,setAttempt]=useState(0);
  const params=(skip:number)=>new URLSearchParams({ref,skip:String(skip),...author?{author}:{},...source?{source}:{}});
  useEffect(()=>{const abort=new AbortController();setPages([]);setError("");setLoading(true);
    void request<GitCommitPage>(`${repoApi(repo.id)}/commits?${params(0)}`,{signal:abort.signal}).then(page=>setPages([page])).catch(cause=>{if(!abort.signal.aborted)setError(message(cause));}).finally(()=>{if(!abort.signal.aborted)setLoading(false);});
    return()=>abort.abort();},[repo.id,repo.revision,ref,author,source,attempt]);
  useEffect(()=>{try{localStorage.setItem("dar.git.commits",mode);}catch{/* per-browser convenience only */}},[mode]);
  async function more(){const next=pages.at(-1)?.next;if(next===null||next===undefined)return;setLoading(true);try{const page=await request<GitCommitPage>(`${repoApi(repo.id)}/commits?${params(next)}`);setPages(list=>[...list,page]);}catch(cause){setError(message(cause));}finally{setLoading(false);}}
  const commits=pages.flatMap(page=>page.commits),first=pages[0],graph=mode==="graph"&&!author&&!source;
  const rows=useMemo(()=>graph?graphRows(commits):[],[commits,graph]);
  return <div className="git-scroll git-commits">
    <div className="git-toolbar">
      <RefPicker refs={refs} value={ref==="all"?"all":ref} all onChange={value=>setRef(value||"main")} label="Branch"/>
      <Select aria-label="Author" value={author} onChange={event=>setAuthor(event.target.value)}><option value="">All authors</option>{first?.authors.map(name=><option key={name} value={name}>{name}</option>)}</Select>
      <Select aria-label="Source" value={source} onChange={event=>setSource(event.target.value)}><option value="">All sources</option><option value="git">Git push</option><option value="dar">Dar</option><option value="agent">Agent</option></Select>
      <span className="git-toolbar-end">{first&&<span className="git-muted">{plural(first.total,"commit")}</span>}<Segmented label="Commit view" value={mode} options={[["graph","Graph"],["list","List"]] as const} onChange={setMode}/></span>
    </div>
    {mode==="graph"&&(author||source)&&<p className="git-hint">The graph shows every commit; filtered commits are listed instead.</p>}
    {error?<Failure error={error} retry={()=>setAttempt(n=>n+1)} what="Commits"/>:!first?<Loading label="Loading commits"/>:!commits.length?<p className="git-hint">No commits match.</p>
      :graph?<ol className="git-graph" aria-label="Commits">{commits.map((commit,index)=><li key={commit.sha}><CommitRow commit={commit} selected={selected===commit.sha} onOpen={()=>onCommit(commit.sha)} graph={rows[index]}/></li>)}</ol>
      :<CommitList commits={commits} selected={selected} onCommit={onCommit}/>}
    {pages.at(-1)?.next!=null&&<button className="secondary-button git-more" disabled={loading} onClick={()=>void more()}>{loading?"Loading…":"Load more commits"}</button>}
  </div>;
}
function CommitList({commits,selected,onCommit}:{commits:GitCommit[];selected?:string;onCommit:(sha:string)=>void}){
  const groups:{day:string;commits:GitCommit[]}[]=[];
  for(const commit of commits){const day=localDate(commit.time,{weekday:"long",month:"long",day:"numeric",year:"numeric"});if(groups.at(-1)?.day!==day)groups.push({day,commits:[]});groups.at(-1)!.commits.push(commit);}
  return <div className="git-commit-list">{groups.map(group=><section key={group.day} aria-label={group.day}><h3 className="git-day"><Icon name="commit"/>{group.day}</h3><ol>{group.commits.map(commit=><li key={commit.sha}><CommitRow commit={commit} selected={selected===commit.sha} onOpen={()=>onCommit(commit.sha)}/></li>)}</ol></section>)}</div>;
}
const laneX=(lane:number)=>9+lane*14,rowHeight=46;
/** One row of the graph: lines through the row, into the commit, and down to its parents. */
function Lanes({row}:{row:GraphRow}){
  const x=laneX(row.lane),mid=rowHeight/2,width=laneX(Math.max(row.width,1)-1)+9;
  return <svg className="git-lanes" width={width} height={rowHeight} viewBox={`0 0 ${width} ${rowHeight}`} aria-hidden="true">
    {row.through.map(lane=><path key={`t${lane}`} className={`lane-${lane%5}`} d={`M${laneX(lane)} 0V${rowHeight}`}/>)}
    {row.top&&<path className={`lane-${row.lane%5}`} d={`M${x} 0V${mid}`}/>}
    {row.into.map(lane=><path key={`i${lane}`} className={`lane-${lane%5}`} d={`M${laneX(lane)} 0C${laneX(lane)} ${mid} ${x} ${mid*0.4} ${x} ${mid}`}/>)}
    {row.out.map(lane=><path key={`o${lane}`} className={`lane-${(lane===row.lane?row.lane:lane)%5}`} d={lane===row.lane?`M${x} ${mid}V${rowHeight}`:`M${x} ${mid}C${x} ${rowHeight-4} ${laneX(lane)} ${mid+4} ${laneX(lane)} ${rowHeight}`}/>)}
    <circle className={`lane-${row.lane%5}${row.out.length>1?" is-merge":""}`} cx={x} cy={mid} r={row.out.length>1?5:4.5}/>
  </svg>;
}
function CommitRow({commit,selected,onOpen,graph}:{commit:GitCommit;selected:boolean;onOpen:()=>void;graph?:GraphRow}){
  return <button type="button" className={`git-commit${selected?" is-selected":""}${graph?" is-graph":""}`} aria-pressed={selected} onClick={onOpen}>
    {graph?<Lanes row={graph}/>:<Person name={commit.author} source={commit.source}/>}
    <span className="git-commit-main"><span className="git-commit-subject">{commit.parents.length>1&&<Icon name="merge" className="git-merge-icon"/>}<span>{commit.subject||"(no message)"}</span><Refs refs={commit.refs}/></span>
      <span className="git-commit-sub">{graph&&<Person name={commit.author} source={commit.source}/>}<strong>{commit.author}</strong><SourceChip source={commit.source}/><span>{edited(commit.time)}</span></span></span>
    <span className="git-commit-end"><span className="git-stat"><span className="git-added">+{commit.added}</span> <span className="git-removed">−{commit.removed}</span></span><Sha sha={commit.sha}/></span>
  </button>;
}

function BranchesTab({repo,refs,write,onPull,onCompare,onCommit,onDelete,onCode,notify}:{repo:GitRepository;refs?:RefLists;write:boolean;onPull:(head:string)=>void;onCompare:(head:string)=>void;onCommit:(sha:string)=>void;onDelete:(branch:GitBranch)=>void;onCode:(name:string)=>void;notify:(text:string)=>void}){
  if(!refs)return <div className="git-scroll"><Loading label="Loading branches"/></div>;
  const {main,branches,agent}=refs.branches,widest=Math.max(1,...branches.flatMap(branch=>[branch.ahead,branch.behind]));
  const bar=(value:number)=>`w${value?Math.max(1,Math.round(value/widest*10)):0}`;
  const row=(branch:GitBranch,isMain=false)=><li key={branch.name} className="git-branch-row">
    <Icon name="branch"/><div className="git-branch-body"><span className="git-branch-name">{branch.name}{isMain&&<span className="git-tag">Default</span>}</span><span><button type="button" className="git-link" onClick={()=>onCommit(branch.sha)}>{branch.subject}</button> · {branch.author} · {edited(branch.time)}</span></div>
    {isMain?<span className="git-muted git-branch-state">The folder matches this branch</span>:<span className="git-divergence" role="img" aria-label={`${branch.behind} behind and ${branch.ahead} ahead of main`}><span className="is-behind"><small>{branch.behind} behind</small><i className={bar(branch.behind)}/></span><span className="is-ahead"><small>{branch.ahead} ahead</small><i className={bar(branch.ahead)}/></span></span>}
    <span className="git-branch-actions">{!isMain&&<button type="button" className="secondary-button" onClick={()=>onCompare(branch.name)}><Icon name="compare"/>Compare</button>}
      <ActionMenu label={`Actions for ${branch.name}`} actions={[{label:"Browse files",icon:"code",run:()=>onCode(isMain?"":branch.name)},...isMain?[]:[{label:"Compare with main",icon:"compare" as const,run:()=>onCompare(branch.name)}],...isMain||!write?[]:[{label:"Open pull request",icon:"pull" as const,run:()=>onPull(branch.name)}],{label:"Download ZIP",icon:"download",href:zipUrl(repo.id,branch.name),download:true},{label:"Copy name",icon:"copy",run:()=>copy(branch.name,notify,"branch name")},...isMain||!write?[]:[{label:"Delete branch",icon:"trash" as const,danger:true,separator:true,run:()=>onDelete(branch)}]]}><Icon name="more"/></ActionMenu></span>
  </li>;
  return <div className="git-scroll git-branches">
    <section className="git-group" aria-label="Default branch"><h2>Default branch</h2>{main?<ul>{row(main,true)}</ul>:<p className="git-hint">Nothing is committed on main yet.</p>}</section>
    <section className="git-group" aria-label="Branches"><h2>Branches</h2>{branches.length?<ul>{branches.map(branch=>row(branch))}</ul>:<p className="git-hint">Push a branch with Git to keep work beside main: <code>git push origin my-branch</code></p>}</section>
    {agent.length>0&&<section className="git-group" aria-label="Agent branches"><h2>Agent branches</h2><ul>{agent.map(branch=><li key={branch.name} className="git-branch-row"><Icon name="branch" className="is-agent"/><div className="git-branch-body"><span className="git-branch-name">{branch.name}</span><span>{branch.message||"No message"} · {plural(branch.files,"file")} · {edited(branch.modifiedAt)}</span></div><span/><span className="git-branch-actions">{write&&<button type="button" className="secondary-button" onClick={()=>onPull(branch.name)}><Icon name="pull"/>Open pull request</button>}<a className="secondary-button" href={`/agent?thread=${encodeURIComponent(branch.threadId)}`}><Icon name="external"/>Open in Agent</a></span></li>)}</ul>
      <p className="git-group-note"><Icon name="info"/>Agent branches are reviewed in Agent, or in a pull request here, and merged into the folder. Pushes to agent/ branches are refused.</p></section>}
  </div>;
}
function TagsTab({repo,refs,write,onCommit,onDelete,notify}:{repo:GitRepository;refs?:RefLists;write:boolean;onCommit:(sha:string)=>void;onDelete:(tag:GitTag)=>void;notify:(text:string)=>void}){
  if(!refs)return <div className="git-scroll"><Loading label="Loading tags"/></div>;
  if(!refs.tags.length)return <div className="git-empty"><span className="empty-icon"><Icon name="tag"/></span><h2>No tags yet</h2><p>Tag a version with Git and push it:</p><pre className="git-command">{"git tag -a v1.0 -m \"Version 1.0\"\ngit push origin v1.0"}</pre></div>;
  return <ul className="git-scroll git-tag-list" aria-label="Tags">{refs.tags.map(tag=><li key={tag.name} className="git-tag-row">
    <Icon name="tag"/><div className="git-branch-body"><span className="git-branch-name">{tag.name}</span><span>{tag.message||"Lightweight tag"} · {tag.author} · {edited(tag.time)}</span></div>
    <button type="button" className="git-link" onClick={()=>onCommit(tag.target)}><Sha sha={tag.target}/></button>
    <span className="git-branch-actions"><a className="secondary-button" href={zipUrl(repo.id,tag.name)} download><Icon name="download"/>ZIP</a>
      <ActionMenu label={`Actions for ${tag.name}`} actions={[{label:"Show commit",icon:"commit",run:()=>onCommit(tag.target)},{label:"Copy name",icon:"copy",run:()=>copy(tag.name,notify,"tag name")},...write?[{label:"Delete tag",icon:"trash" as const,danger:true,separator:true,run:()=>onDelete(tag)}]:[]]}><Icon name="more"/></ActionMenu></span>
  </li>)}</ul>;
}

// ---- Right sheet ------------------------------------------------------------------------------

function CommitSheet({repo,sha,session,onChanged,onClose,onBrowse,onCommit,notify}:{repo:GitRepository;sha:string;session:BrowserSession;onChanged:()=>void;onClose:()=>void;onBrowse:(sha:string)=>void;onCommit:(sha:string)=>void;notify:(text:string)=>void}){
  const {value:commit,error,retry}=useLoad<GitCommitDetail>(`${repoApi(repo.id)}/commits/${sha}`),[wrap,setWrap]=useState(false),focus=useSheetFocus(onClose),[restoring,setRestoring]=useState(false);
  return <div className="git-sheet" {...focus}>
    <SheetHead title={<>Commit <Sha sha={sha}/></>} onClose={onClose}><button className="icon-button" aria-label="Copy commit ID" title="Copy commit ID" onClick={()=>copy(sha,notify,"commit ID")}><Icon name="copy"/></button></SheetHead>
    <div className="git-sheet-body">
      {error?<Failure error={error} retry={retry} what="This commit"/>:!commit?<Loading label="Loading commit"/>:<>
        <div><h3 className="git-commit-title">{commit.subject||"(no message)"}</h3>{commit.body&&<p className="git-commit-body">{commit.body}</p>}</div>
        <div className="git-who"><Person name={commit.author} source={commit.source}/><div><span><strong>{commit.author}</strong> committed {edited(commit.time)}</span><small>{localDateTime(commit.time,{dateStyle:"medium",timeStyle:"short"})} · <SourceChip source={commit.source}/></small></div></div>
        <div className="property-card git-facts">
          <div className="property"><Icon name="commit"/><div><small>Git ID</small><code className="git-full-sha">{commit.sha}</code></div></div>
          <div className="property"><Icon name={commit.parents.length>1?"merge":"history"}/><div><small>{commit.parents.length>1?"Parents":"Parent"}</small><span>{commit.parents.length?commit.parents.map((parent,index)=><button key={parent} type="button" className="git-link" onClick={()=>onCommit(parent)}>{index?", ":""}<Sha sha={parent}/></button>):"None, this is the first commit"}</span></div></div>
          {commit.refs.length>0&&<div className="property"><Icon name="branch"/><div><small>On</small><span className="git-refs"><Refs refs={commit.refs}/></span></div></div>}
          {commit.committer!==commit.author&&<div className="property"><Icon name="user"/><div><small>Committed by</small><span>{commit.committer}, {edited(commit.committedAt)}</span></div></div>}
        </div>
        <div className="git-files-summary"><strong>{plural(commit.files.length,"file")} changed</strong>{commit.parents.length>1&&<span className="git-muted">against the first parent</span>}<span className="git-stat"><span className="git-added">+{commit.added}</span> <span className="git-removed">−{commit.removed}</span></span>
          <label className="git-wrap"><input type="checkbox" checked={wrap} onChange={event=>setWrap(event.target.checked)}/>Wrap lines</label></div>
        <FileChanges files={commit.files} wrap={wrap}/>
      </>}
    </div>
    <div className="git-sheet-foot"><button className="secondary-button" onClick={()=>onBrowse(sha)}><Icon name="code"/>Browse files</button><a className="secondary-button" href={zipUrl(repo.id,sha)} download><Icon name="download"/>ZIP</a>
      {commit?.restore&&repo.role!=="read"&&<button className="secondary-button" onClick={()=>setRestoring(true)}><Icon name="history"/>Restore folder to this point</button>}</div>
    {restoring&&commit?.restore&&<Dialog title="Restore this folder?" onClose={()=>setRestoring(false)}><RestoreFolder repo={repo} commit={commit} session={session} onCancel={()=>setRestoring(false)} onDone={()=>{setRestoring(false);notify(`Folder restored to “${commit.subject}”`);onChanged();}}/></Dialog>}
  </div>;
}
/** Restores the live folder to a main commit; it is recorded as a new commit, so no history is lost. */
function RestoreFolder({repo,commit,session,onCancel,onDone}:{repo:GitRepository;commit:GitCommitDetail;session:BrowserSession;onCancel:()=>void;onDone:()=>void}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  return <><p>The files in {repo.path} change back to how they were at “{commit.subject}” ({short(commit.sha)}). This is recorded as a new commit on main, so nothing in the history is lost.</p>
    {error&&<p className="error" role="alert">{error}</p>}
    <div className="dialog-actions"><button className="text-button" onClick={onCancel}>Cancel</button><button className="primary-button" disabled={busy} onClick={()=>{setBusy(true);setError("");
      void request(`/api/v1/files/history/${repo.id}/restore`,{method:"POST",body:JSON.stringify({id:commit.restore!.changesetId,revision:commit.restore!.revision})},session.csrfToken).then(onDone).catch(cause=>{
        const text=message(cause);setError(/uncommittedChanges/.test(text)?"The folder has uncommitted changes. Commit them in Files first, then restore.":/conflict/.test(text)?"The history changed meanwhile. Close this and try again.":text);setBusy(false);});}}>Restore folder</button></div></>;
}
function CompareSheet({repo,session,notify,onMerged,onPull,base,head,onChange,onCommit,onClose}:{repo:GitRepository;session:BrowserSession;notify:(text:string)=>void;onMerged:()=>void;onPull:(base:string,head:string)=>void;base:string;head:string;onChange:(base:string,head:string)=>void;onCommit:(sha:string)=>void;onClose:()=>void}){
  const {value,error,retry}=useLoad<GitCompare>(`${repoApi(repo.id)}/compare?${new URLSearchParams({base,head})}`,[repo.revision]),focus=useSheetFocus(onClose);
  const {value:lists}=useLoad<GitBranches>(`${repoApi(repo.id)}/branches`);
  const names=lists?[...lists.main?["main"]:[],...lists.branches.map(branch=>branch.name)]:[base,head];
  return <div className="git-sheet" {...focus}>
    <SheetHead title="Compare" onClose={onClose}/>
    <div className="git-sheet-body">
      <div className="git-compare-pick"><Select aria-label="Base branch" value={base} onChange={event=>onChange(event.target.value,head)}>{[...new Set(names)].map(name=><option key={name} value={name}>{name}</option>)}</Select><Icon name="back"/><Select aria-label="Compared branch" value={head} onChange={event=>onChange(base,event.target.value)}>{[...new Set(names)].map(name=><option key={name} value={name}>{name}</option>)}</Select></div>
      {error?<Failure error={error} retry={retry} what="The comparison"/>:!value?<Loading label="Comparing"/>:<>
        {!value.ahead.length?<p className="git-hint">{head} has no commits that {base} doesn’t.{value.behind?` It is ${plural(value.behind,"commit")} behind.`:""}</p>:<>
          <h3 className="git-sheet-label">{plural(value.ahead.length,"commit")} on {head}</h3>
          <ol className="git-mini-commits">{value.ahead.map(commit=><li key={commit.sha}><button type="button" onClick={()=>onCommit(commit.sha)}><Person name={commit.author} source={commit.source}/><span>{commit.subject}</span><Sha sha={commit.sha}/><small>{edited(commit.time)}</small></button></li>)}</ol>
          <div className="git-files-summary"><strong>{plural(value.files.length,"file")} changed</strong><span className="git-stat"><span className="git-added">+{value.files.reduce((sum,file)=>sum+file.added,0)}</span> <span className="git-removed">−{value.files.reduce((sum,file)=>sum+file.removed,0)}</span></span></div>
          <ul className="git-file-summary">{value.files.map(file=><li key={file.path}><span className={`git-kind-letter is-${file.kind}`} title={kindLabel[file.kind]}>{kindLabel[file.kind][0]}<span className="sr-only">{kindLabel[file.kind]}</span></span><span className="git-diff-path">{file.path}</span><span className="git-stat">{file.binary?"Binary":<><span className="git-added">+{file.added}</span> <span className="git-removed">−{file.removed}</span></>}</span></li>)}</ul>
        </>}
        {value.ahead.length>0&&repo.role!=="read"&&<button type="button" className="secondary-button git-open-pull" onClick={()=>onPull(base,head)}><Icon name="pull"/>Open pull request</button>}
        {value.ahead.length>0&&repo.role!=="read"&&<MergeBox key={`${value.baseSha}:${value.headSha}`} repo={repo} compare={value} session={session} onMerged={result=>{notify(result.fastForward?`${base} now points at ${head}`:`Merged ${head} into ${base}`);onMerged();}}/>}
      </>}
    </div>
  </div>;
}
function FileSheet({repo,gitRef,path,onClose}:{repo:GitRepository;gitRef:string;path:string;onClose:()=>void}){
  const url=`${repoApi(repo.id)}/blob?${new URLSearchParams({ref:gitRef,path})}`,focus=useSheetFocus(onClose);
  const [text,setText]=useState<string|null>(),[error,setError]=useState("");
  useEffect(()=>{const abort=new AbortController();setText(undefined);setError("");
    void fetch(`${url}&view`,{signal:abort.signal}).then(async response=>{if(!response.ok)throw new Error("This file could not be loaded.");
      setText((response.headers.get("content-type")??"").startsWith("text/plain")?await response.text():null);}).catch(cause=>{if(!abort.signal.aborted)setError(message(cause));});return()=>abort.abort();},[url]);
  const name=path.split("/").at(-1)!;
  return <div className="git-sheet" {...focus}>
    <SheetHead title={name} onClose={onClose}><a className="icon-button" href={url} download aria-label={`Download ${name}`} title="Download"><Icon name="download"/></a></SheetHead>
    <div className="git-sheet-body"><p className="git-muted git-file-path">{path} · {gitRef.length===40?short(gitRef):gitRef}</p>
      {error?<p className="error" role="alert">{error}</p>:text===undefined?<Loading label="Loading file"/>:text===null?<p className="git-hint">This file can’t be shown here. Download it to open it.</p>
        :/\.(md|markdown)$/i.test(name)?<div className="git-readme-body"><Markdown source={text}/></div>:<pre className="git-source-view" tabIndex={0} aria-label={`Contents of ${name}`}>{text}</pre>}</div>
  </div>;
}

// ---- New repository ---------------------------------------------------------------------------

/** A new folder that keeps history (optionally starting with a README), or history on an existing folder. */
function NewRepository({session,onClose,onCreated}:{session:BrowserSession;onClose:()=>void;onCreated:(id:string)=>void}){
  const live=useLiveConnection(session),[mode,setMode]=useState<"new"|"existing">("new"),[name,setName]=useState(""),[readme,setReadme]=useState(true),[place,setPlace]=useState<DarFileSelection>();
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),nameId=useId();
  const valid=mode==="existing"?!!place?.id:!!name.trim()&&!/[/\\]/.test(name)&&name.trim()!=="."&&name.trim()!==".."&&name.length<=255;
  async function track(folderId:string){
    const response=await fetch(`/api/v1/files/history/${folderId}`,{method:"POST",headers:{"content-type":"application/json","x-csrf-token":session.csrfToken},body:JSON.stringify({action:"track"})});
    if(!response.ok){const text=await response.text();throw new Error(/quotaExceeded/.test(text)?"You keep history on as many folders as Dar allows.":/forbidden/.test(text)?"History can’t be kept here: the folder is in the Locked folder, inside another repository, or not yours.":"History could not be turned on for this folder.");}
  }
  async function create(){
    setBusy(true);setError("");
    try{
      if(mode==="existing"){await track(place!.id);onCreated(place!.id);return;}
      const connection=live.connection;if(!connection)throw new Error("Git is reconnecting. Try again in a moment.");
      const id=crypto.randomUUID(),title=name.trim();
      await connection.reducers.createFolder({id,parentId:place?.id??"",name:title});
      await track(id);
      if(readme){
        const text=`# ${title}\n`,reservation=await request<{id:string}>("/api/v1/files/uploads",{method:"POST",body:JSON.stringify({name:"README.md",parentId:id,size:new TextEncoder().encode(text).length,mediaType:"text/markdown"})},session.csrfToken);
        const put=await fetch(`/api/v1/files/uploads/${reservation.id}`,{method:"PUT",headers:{"x-csrf-token":session.csrfToken},body:text});if(!put.ok)throw new Error("The README could not be added.");
        const status=await request<HistoryOverview>(`/api/v1/files/history/${id}`);
        await request(`/api/v1/files/history/${id}/commits`,{method:"POST",body:JSON.stringify({message:"Add README",token:status.status!.token})},session.csrfToken);
      }
      onCreated(id);
    }catch(cause){setError(message(cause));setBusy(false);}
  }
  return <EditorPane title="New repository" onClose={onClose} dirty={!!name&&!busy}>
    <form className="git-new" onSubmit={event=>{event.preventDefault();if(valid&&!busy)void create();}}>
      <Segmented label="Start from" value={mode} options={[["new","New folder"],["existing","Existing folder"]] as const} onChange={setMode}/>
      {mode==="new"?<>
        <label className="git-field" htmlFor={nameId}>Name<input id={nameId} value={name} maxLength={255} autoFocus required onChange={event=>setName(event.target.value)} aria-invalid={!!name&&!valid}/></label>
        <div className="git-field"><span>Location</span><DarFilePicker mode="folder" allowRoot onChange={setPlace}/><small>Creates {place?.id?place.name:"All files"} › {name.trim()||"name"} in Files and keeps its history.</small></div>
        <label className="git-check"><input type="checkbox" checked={readme} onChange={event=>setReadme(event.target.checked)}/>Add a README.md and make the first commit</label>
      </>:<div className="git-field"><span>Folder</span><DarFilePicker mode="folder" onChange={setPlace}/><small>{place?.id?`${place.name} keeps its history from now on; its files are not changed.`:"Choose a folder you own."}</small></div>}
      {error&&<p className="error" role="alert">{error}</p>}
      <div className="git-new-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button type="submit" className="primary-button" disabled={!valid||busy}>{busy?"Creating…":mode==="new"?"Create":"Keep history"}</button></div>
    </form>
  </EditorPane>;
}
