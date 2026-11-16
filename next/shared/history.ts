/** Folder history (docs/spacetimedb/folder-history.md): limits and the API's shapes. */
export const historyLimits={files:2000,depth:32,changes:2000,changesets:10000,repos:50,messageBytes:2048,page:50,exportBytes:2*1024**3} as const;
export type HistoryChangeKind="added"|"modified"|"deleted"|"renamed";
/** One path in a changeset, a branch or the uncommitted status. Sizes and types describe each side's version. */
export interface HistoryChange {path:string;previousPath?:string;kind:HistoryChangeKind;fileId?:string;versionId?:string;previousVersionId?:string;size?:number;previousSize?:number;mediaType?:string;previousMediaType?:string}
export interface HistoryChangeset {id:string;sequence:number;kind:"commit"|"snapshot"|"restore"|"merge";message:string;author:string;createdAt:number;fileCount:number}
export interface HistoryBranch {name:string;threadId:string;message:string;author:string;createdAt:number;modifiedAt:number;changes:HistoryChange[]}
export interface HistoryOverview {
  folder:{id:string;name:string};tracked:boolean;canWrite:boolean;owner:boolean;
  /** Why this folder cannot be tracked, when it is not tracked. */
  blocked?:"locked"|"nested"|"limit";
  repo?:{revision:number;headId:string;sequence:number;createdAt:number};
  status?:{changes:HistoryChange[];token:string};
  branches:HistoryBranch[];changesets:HistoryChangeset[];nextBefore:number|null;
}
export interface HistoryDetail {changeset:HistoryChangeset;changes:HistoryChange[]}
export const historyBranchName=(threadId:string)=>`agent/${threadId}`;
