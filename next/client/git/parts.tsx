import {useEffect,useId,useRef,useState,type ReactNode} from "react";
import {request} from "../api.ts";
import {Avatar} from "../shell.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {Segmented} from "../settings-sheet.tsx";
import {Diff} from "../diff.tsx";
import type {BrowserSession} from "../api.ts";
import type {GitCompare,GitFileChange,GitMergeResult,GitRepository,GitSource} from "../../shared/git.ts";

/** Pieces the Git app's views share (workspace.tsx, tracker.tsx). */
export const api="/api/v1/git";
export const repoApi=(id:string)=>`${api}/repositories/${id}`;
export const message=(cause:unknown)=>cause instanceof Error?cause.message:"The request could not be completed.";
export const plural=(count:number,one:string,many=one+"s")=>`${count.toLocaleString()} ${count===1?one:many}`;
export const short=(sha:string)=>sha.slice(0,7);
export const sourceLabel:Record<GitSource,string>={git:"Git push",dar:"Dar",agent:"Agent"};
export const sourceIcon:Record<GitSource,IconName>={git:"upload",dar:"folder",agent:"agent"};
export const kindLabel:Record<GitFileChange["kind"],string>={added:"Added",modified:"Modified",deleted:"Deleted",renamed:"Renamed"};
export function SourceChip({source}:{source:GitSource}){return <span className={`git-source is-${source}`}><Icon name={sourceIcon[source]}/>{sourceLabel[source]}</span>;}
export function Sha({sha}:{sha:string}){return <code className="git-sha" title={sha}>{short(sha)}</code>;}
export function Person({name,source}:{name:string;source?:GitSource}){return source==="agent"?<span className="avatar git-agent-avatar" aria-hidden="true"><Icon name="agent"/></span>:<Avatar name={name}/>;}
export function copy(text:string,notify:(text:string)=>void,what:string){void navigator.clipboard.writeText(text).then(()=>notify(`Copied ${what}`),()=>notify("Select the text and copy it"));}
export function useLoad<T>(url:string|undefined,deps:unknown[]=[]){
  const [value,setValue]=useState<T>(),[error,setError]=useState(""),[attempt,setAttempt]=useState(0);
  useEffect(()=>{if(!url)return;const abort=new AbortController();setError("");
    void request<T>(url,{signal:abort.signal}).then(setValue).catch(cause=>{if(!abort.signal.aborted)setError(message(cause));});return()=>abort.abort();},[url,attempt,...deps]);
  return {value,error,retry:()=>setAttempt(n=>n+1),setValue};
}
export function Failure({error,retry,what}:{error:string;retry:()=>void;what:string}){return <div className="git-failure error" role="alert"><Icon name="alert"/><div><strong>{what} could not be loaded</strong><span>{error}</span></div><button className="secondary-button" onClick={retry}>Retry</button></div>;}
export function Loading({label}:{label:string}){return <div className="git-skeleton" role="status" aria-label={label}><span/><span/><span/></div>;}
export function SheetHead({title,children,onClose}:{title:ReactNode;children?:ReactNode;onClose:()=>void}){
  return <div className="git-sheet-head"><button className="icon-button git-sheet-back" aria-label="Close" onClick={onClose}><Icon name="back"/></button><h2>{title}</h2>{children}<button className="icon-button git-sheet-close" aria-label="Close" title="Close (Escape)" onClick={onClose}><Icon name="close"/></button></div>;
}
export function useSheetFocus(onClose:()=>void){
  const root=useRef<HTMLDivElement>(null);
  useEffect(()=>{root.current?.focus({preventScroll:true});},[]);
  return {ref:root,tabIndex:-1,onKeyDown:(event:React.KeyboardEvent)=>{if(event.key==="Escape"&&!(event.target as HTMLElement).closest("[role=menu],[role=listbox],[role=dialog]")){event.preventDefault();onClose();}}};
}
export function FileChanges({files,wrap}:{files:GitFileChange[];wrap:boolean}){
  const [open,setOpen]=useState<Set<string>>(()=>new Set(files.slice(0,5).map(file=>file.path)));
  return <div className="git-diffs">{files.map(file=><section key={file.path} className="git-diff">
    <button type="button" className="git-diff-head" aria-expanded={open.has(file.path)} onClick={()=>setOpen(current=>{const next=new Set(current);if(next.has(file.path))next.delete(file.path);else next.add(file.path);return next;})}>
      <Icon name="chevron" className={open.has(file.path)?"is-open":""}/><span className="git-diff-path">{file.previousPath?`${file.previousPath} → ${file.path}`:file.path}</span><span className={`git-kind is-${file.kind}`}>{kindLabel[file.kind]}</span>
      <span className="git-stat">{file.binary?"Binary":<><span className="git-added">+{file.added}</span> <span className="git-removed">−{file.removed}</span></>}</span></button>
    {open.has(file.path)&&(file.patch?<Diff source={file.patch} wrap={wrap} label={`Changes to ${file.path}`}/>:<p className="git-hint">{file.binary?"Binary file; no line changes to show.":file.truncated?"This diff is too large to show here. Clone the repository to see it.":file.kind==="renamed"?"Renamed without changes.":"No line changes."}</p>)}
  </section>)}</div>;
}
/**
 * Merging from the web: fast-forward, or a merge commit when the branches have diverged. A pull
 * request passes its own default message; the merge marks it merged in the same transaction.
 */
export function MergeBox({repo,compare,session,onMerged,defaultMessage}:{repo:GitRepository;compare:GitCompare;session:BrowserSession;onMerged:(result:GitMergeResult)=>void;defaultMessage?:string}){
  const [mode,setMode]=useState<"fast-forward"|"merge">(compare.fastForward?"fast-forward":"merge"),[text,setText]=useState(""),[remove,setRemove]=useState(compare.head!=="main");
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),messageId=useId();
  const blocked=mode==="merge"&&!compare.mergeable;
  async function merge(){
    setBusy(true);setError("");
    try{onMerged(await request<GitMergeResult>(`${repoApi(repo.id)}/merge`,{method:"POST",body:JSON.stringify({base:compare.base,head:compare.head,baseSha:compare.baseSha,headSha:compare.headSha,mode,...mode==="merge"&&(text.trim()||defaultMessage)?{message:text.trim()||defaultMessage}:{},deleteBranch:remove})},session.csrfToken));}
    catch(cause){setError(message(cause));setBusy(false);}
  }
  return <section className="git-merge" aria-label={`Merge ${compare.head} into ${compare.base}`}>
    <h3 className="git-sheet-label">Merge into {compare.base}</h3>
    <Segmented label="How to merge" value={mode} options={[["fast-forward","Fast-forward"],["merge","Merge commit"]] as const} disabled={busy} onChange={setMode}/>
    {mode==="fast-forward"?compare.fastForward?<p className="git-hint">{compare.base} moves forward to {compare.head}; no new commit is made.</p>
      :<p className="git-hint">{compare.base} has {plural(compare.behind,"commit")} that {compare.head} doesn’t, so it can’t fast-forward. Choose Merge commit.</p>
      :compare.mergeable?<label className="git-field" htmlFor={messageId}>Message<input id={messageId} value={text} maxLength={2000} placeholder={defaultMessage??`Merge branch '${compare.head}'${compare.base==="main"?"":` into ${compare.base}`}`} onChange={event=>setText(event.target.value)}/></label>
      :<div className="git-banner"><Icon name="alert"/><div><strong>These branches conflict</strong><span>Merge {compare.base} into {compare.head} with Git, resolve the conflicts and push; then merge here.</span></div></div>}
    {compare.base==="main"&&<p className="git-hint">The folder in Files changes with main. Files with uncommitted changes in Dar stop the merge until they are committed.</p>}
    {compare.head!=="main"&&<label className="git-check"><input type="checkbox" checked={remove} disabled={busy} onChange={event=>setRemove(event.target.checked)}/>Delete {compare.head} after merging</label>}
    {error&&<p className="error" role="alert">{error}</p>}
    <button className="primary-button git-merge-button" disabled={busy||blocked||(mode==="fast-forward"&&!compare.fastForward)} onClick={()=>void merge()}><Icon name="merge"/>{busy?"Merging…":mode==="fast-forward"?`Fast-forward ${compare.base}`:`Merge into ${compare.base}`}</button>
  </section>;
}
