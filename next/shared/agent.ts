export type AgentProject={id:string;ownerId:string;name:string;createdAt:number}&({sourceType:"git";sourceRef:string}|{sourceType:"files";sourceRef:string});
/** Credential-free repository locations only. Cloning still requires runner network policy. */
export const validRepositoryUrl=(value:string)=>/^https:\/\/[A-Za-z0-9][A-Za-z0-9.-]*(?::[0-9]{1,5})?\/[A-Za-z0-9._~/-]+$/.test(value)||/^git@[A-Za-z0-9][A-Za-z0-9.-]*:[A-Za-z0-9._/-]+$/.test(value);
export interface AgentFolders {folder:{id:string;name:string;parentId:string}|null;items:{id:string;name:string}[];nextCursor:string|null}
export interface AgentThread {id:string;ownerId:string;projectId:string;title:string;revision:number;createdAt:number;modifiedAt:number;runStatus?:AgentRunStatus}
export interface AgentMessage {id:string;ownerId:string;threadId:string;sequence:number;role:"user"|"assistant";content:string;createdAt:number}
export interface AgentHistory {thread:AgentThread;items:AgentMessage[];nextBefore:number|null;runs:AgentRunSummary[]}
export const agentLimits={projects:32,threads:128,messages:1000,messageBytes:16384,page:50} as const;
export type AgentRunStatus="queued"|"running"|"cancelling"|"completed"|"failed"|"cancelled"|"interrupted";
export interface AgentChange {path:string;kind:"added"|"modified"|"deleted";diff:string;fileId?:string;expectedRevision?:number;content?:string;mediaType?:string}
export interface AgentRunDocument {output:string;terminal:string;activity:string[];changes:AgentChange[];error:string;truncated:boolean}
export interface AgentRunSummary {id:string;threadId:string;messageId:string;model:string;status:AgentRunStatus;createdAt:number;modifiedAt:number}
export interface AgentRun extends AgentRunSummary {document:AgentRunDocument}
export interface AgentProvider {configured:boolean;connected:boolean;models:{id:string;name:string;isDefault:boolean}[];message:string}
export const activeAgentRun=(status:string)=>["queued","running","cancelling"].includes(status);
