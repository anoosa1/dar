/** The Git app (docs/git.md): repositories are folders that keep history. Shapes of /api/v1/git. */
export type GitSource="git"|"dar"|"agent";
export type GitChangeKind="added"|"modified"|"deleted"|"renamed";
export interface GitRef {name:string;kind:"branch"|"tag"}
export interface GitRepository {
  id:string;name:string;
  /** The folder's path from its owner's root, like Projects/dar-site. */
  path:string;parentId:string;createdAt:number;
  /** Changes with every commit, push, merge, restore or deleted ref; views reload when it does. */
  revision:number;
  /** Files that differ from main in Dar; -1 when the folder is too large to compare. */
  dirty:number;agentBranches:number;
  /** Bytes of every version the history holds. */
  size:number;
  /** The README's first paragraph on main. */
  description:string;commits:number;branches:number;tags:number;
  /** The newest commit on any branch, else when history started. */
  updatedAt:number;
  /** Commits per week, oldest first, over the last 12 weeks. */
  weeks:number[];
  /** Clone addresses: the alias follows the folder's path; the ID one never changes. */
  clone:{alias:string;id:string};
  /** Set when Git could not read this repository. */
  error?:string;
}
export interface GitCommit {sha:string;parents:string[];author:string;time:number;subject:string;source:GitSource;refs:GitRef[];added:number;removed:number}
export interface GitFileChange {path:string;previousPath?:string;kind:GitChangeKind;added:number;removed:number;binary:boolean;
  /** Unified hunks; absent for binary files and when the diff is too large to show. */
  patch?:string;truncated?:boolean}
export interface GitCommitDetail extends GitCommit {body:string;committer:string;committedAt:number;files:GitFileChange[];
  /** Set when this commit is on main's history, so the folder can be restored to it. */
  restore?:{changesetId:string;revision:number}}
export interface GitMergeResult {sha:string;fastForward:boolean;deleted:boolean}
export interface GitCommitPage {commits:GitCommit[];total:number;next:number|null;authors:string[]}
export interface GitTreeEntry {name:string;path:string;type:"tree"|"blob"|"link"|"submodule";size:number;last?:{sha:string;subject:string;time:number}}
export interface GitTree {ref:string;sha:string;path:string;entries:GitTreeEntry[];readme?:{path:string;text:string}}
export interface GitBranch {name:string;sha:string;subject:string;author:string;time:number;ahead:number;behind:number}
export interface GitAgentBranch {name:string;threadId:string;message:string;author:string;modifiedAt:number;files:number}
export interface GitBranches {main:GitBranch|null;branches:GitBranch[];agent:GitAgentBranch[]}
export interface GitTag {name:string;sha:string;target:string;message:string;author:string;time:number;annotated:boolean}
export interface GitCompare {base:string;head:string;baseSha:string;headSha:string;ahead:GitCommit[];behind:number;fastForward:boolean;
  /** Git can merge head into base without conflicts. */
  mergeable:boolean;files:GitFileChange[]}
export interface GitEvent {repoId:string;repoName:string;sha:string;subject:string;author:string;time:number;source:GitSource;refs:GitRef[]}
/** Commit times (Unix seconds) over the last year across repositories, and the newest commits. */
export interface GitActivity {times:number[];events:GitEvent[]}
export const gitAlias=(username:string,path:string)=>`/dav/git/${encodeURIComponent(username)}/${path.split("/").map(encodeURIComponent).join("/")}.git`;
/** Git's own name check (git check-ref-format) for a branch or tag name without its refs/ prefix. */
export function validRefName(name:string){
  return name.length>0&&name.length<=200&&!/[\x00-\x20\x7f~^:?*[\\]|\.\.|@\{|\/\/|^\/|\/$|\.$|\.lock(\/|$)|(^|\/)\./.test(name)&&name!=="HEAD"&&name!=="@"&&!name.startsWith("-");
}
