/** Folder history (docs/spacetimedb/folder-history.md): limits and the API's shapes. */
export const historyLimits={files:2000,depth:32,changes:2000,changesets:10000,repos:50,messageBytes:2048,page:50,exportBytes:2*1024**3} as const;
export type HistoryChangeKind="added"|"modified"|"deleted"|"renamed";
/** One path in a changeset, a branch or the uncommitted status. Sizes and types describe each side's version. */
export interface HistoryChange {path:string;previousPath?:string;kind:HistoryChangeKind;fileId?:string;versionId?:string;previousVersionId?:string;size?:number;previousSize?:number;mediaType?:string;previousMediaType?:string}
/** sha is the Git commit ID, once Git has seen the commit (a clone, fetch, bundle or push). */
export interface HistoryChangeset {id:string;sequence:number;kind:"commit"|"snapshot"|"restore"|"merge";message:string;author:string;createdAt:number;fileCount:number;sha?:string}
export interface HistoryBranch {name:string;threadId:string;message:string;author:string;createdAt:number;modifiedAt:number;changes:HistoryChange[]}
export interface HistoryOverview {
  folder:{id:string;name:string};tracked:boolean;canWrite:boolean;owner:boolean;
  /** Why this folder cannot be tracked, when it is not tracked. */
  blocked?:"locked"|"nested"|"limit";
  repo?:{revision:number;headId:string;sequence:number;createdAt:number};
  status?:{changes:HistoryChange[];token:string};
  branches:HistoryBranch[];
  /** Git branches and tags pushed besides main, by full ref name. */
  gitRefs:{name:string;sha:string}[];
  changesets:HistoryChangeset[];nextBefore:number|null;
}
export interface HistoryDetail {changeset:HistoryChangeset;changes:HistoryChange[]}
export const historyBranchName=(threadId:string)=>`agent/${threadId}`;
/** Git over HTTPS (docs/spacetimedb/folder-history.md#git). */
export const gitLimits={
  /** A push's pack data. */
  packBytes:4*1024**3,
  /** A commit or tag object's own bytes (headers, signature and message). */
  objectBytes:256*1024,
  /** Commits and changes one push may add; larger histories are pushed in parts. */
  commits:5000,changes:50_000,
} as const;
export const gitPath=(folderId:string)=>`/dav/git/${folderId}.git`;
