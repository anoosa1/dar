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
  /** Clone addresses: the alias follows the folder's path (the owner's own; others get the ID one); the ID one never changes. */
  clone:{alias:string;id:string};
  /** The caller's access: repositories shared with them are read or write. */
  role:"owner"|"write"|"read";
  /** The owner's display name, for repositories shared with the caller. */
  owner?:string;
  openIssues:number;openPulls:number;
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

// ---- Issues and pull requests (docs/git.md#issues-and-pull-requests) ---------------------------

export type GitIssueKind="issue"|"pull";
export type GitIssueState="open"|"closed"|"merged";
/** Label colours are a fixed palette of theme classes (CSP has no inline styles). */
export const gitLabelColors=["plum","violet","blue","teal","green","amber","orange","rose","gray"] as const;
export type GitLabelColor=typeof gitLabelColors[number];
export interface GitLabel {id:string;name:string;color:GitLabelColor;description:string}
export const trackerLimits={title:256,body:65_536,labelName:50,labelDescription:200,labels:100,labelsPerItem:10,items:10_000,comments:2_500,page:500} as const;
/** An issue or pull request in a list. */
export interface GitIssueSummary {
  number:number;kind:GitIssueKind;title:string;state:GitIssueState;author:string;createdAt:number;updatedAt:number;closedAt?:number;
  comments:number;labels:string[];
  /** Pull requests: head is merged into base. An Agent branch (agent/…) merges into main through Agent's merge. */
  base?:string;head?:string;agent?:boolean;
}
export interface GitIssueList {items:GitIssueSummary[];open:number;closed:number;labels:GitLabel[];
  /** Matching items beyond the newest trackerLimits.page that were left out. */
  truncated:boolean}
export type GitTimelineEvent="closed"|"reopened"|"merged"|"renamed"|"labeled"|"unlabeled"|"referenced"|"pushed"|"branchDeleted";
export type GitTimelineItem=
  {id:string;kind:"comment";author:string;body:string;createdAt:number;editedAt?:number;canEdit:boolean;canDelete:boolean}|
  {id:string;kind:"event";event:GitTimelineEvent;author:string;createdAt:number;
    /** closed/merged by a pull request, referenced from another item, renamed from, labelled with, pushed from/to. */
    number?:number;title?:string;from?:string;to?:string;label?:string;sha?:string};
export interface GitIssue extends GitIssueSummary {
  body:string;revision:number;closedBy?:string;timeline:GitTimelineItem[];
  /** Pull requests once merged or closed: the tips they had, so their commits stay readable. */
  baseSha?:string;headSha?:string;mergedSha?:string;
  permissions:{edit:boolean;close:boolean;label:boolean;merge:boolean;comment:boolean};
}
/** A pull request's commits and files: live while it is open, as merged or closed afterwards. */
export interface GitPullChanges {
  /** Git branches: the comparison; absent when its branch is gone. */
  compare?:GitCompare;
  /** Agent branches: the changes waiting to be merged, and the folder revision a merge expects. */
  agent?:{changes:import("./history.ts").HistoryChange[];revision:number;threadId:string};
  /** Why there is nothing to show, such as a deleted branch. */
  note?:string;
}
/** #12 in text; "Closes #12" (also close/fixes/resolves…) in a pull request closes the issue when it is merged. */
export const issueReference=/(^|[^\w&/#])#(\d{1,6})\b/g;
export const closingReference=/\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s*:?\s+#(\d{1,6})\b/gi;
