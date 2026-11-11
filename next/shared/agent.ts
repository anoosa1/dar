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
/** Typed work entries recorded from Codex JSON events. Older runs only have activity strings. */
export interface AgentStep {id:string;kind:"command"|"edit"|"reasoning"|"plan"|"tool"|"search"|"question";label:string;status:"running"|"completed"|"failed";exitCode?:number;output?:string;items?:{text:string;done:boolean}[]}
export interface AgentUsage {input:number;cached:number;output:number}
export interface AgentAttachment {id:string;name:string;mediaType:string;size:number}
export interface AgentQuestion {id:string;header:string;question:string;options:{label:string;description:string}[];other:boolean;secret:boolean}
/** A blocking question from the running turn. Approval requests are never exposed: Dar declines escalation. */
export interface AgentPending {id:string;questions:AgentQuestion[]}
export interface AgentRunDocument {output:string;terminal:string;activity:string[];changes:AgentChange[];error:string;truncated:boolean;steps?:AgentStep[];usage?:AgentUsage;effort?:string;
  mode?:"plan"|"default"|"review"|"compact";attachments?:AgentAttachment[];steered?:string[];pending?:AgentPending|null;codexThread?:string;contextRevision?:number;contextWindow?:number;contextUsed?:number}
export interface AgentLimitWindow {label:string;usedPercent:number;windowMinutes:number|null;resetsAt:number|null}
export interface AgentLimits {available:boolean;plan:string|null;windows:AgentLimitWindow[];message?:string}
export const agentRunModes=["plan","review","compact"] as const;
export const agentImageTypes=["image/png","image/jpeg","image/gif","image/webp"] as const;
export const agentAttachmentLimits={count:4,bytes:8*1024*1024} as const;
export interface AgentRunSummary {id:string;threadId:string;messageId:string;model:string;status:AgentRunStatus;createdAt:number;modifiedAt:number}
export interface AgentRun extends AgentRunSummary {document:AgentRunDocument}
export interface AgentModel {id:string;name:string;isDefault:boolean;efforts?:string[];defaultEffort?:string}
export interface AgentProvider {configured:boolean;connected:boolean;models:AgentModel[];message:string}
export const agentEffort=/^[a-z]{1,20}$/;
export const activeAgentRun=(status:string)=>["queued","running","cancelling"].includes(status);
