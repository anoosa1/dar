/** Chat: a client for other platforms (WhatsApp first). Shared by the module, runtime and browser. */
export type ChatPlatform="whatsapp";
/** linking: waiting for a QR scan or pairing code; connected/connecting/disconnected: a linked device;
 * loggedOut: unlinked on the phone (history stays, read-only); failed: the bridge reported a lasting problem. */
export type ChatAccountState="linking"|"connecting"|"connected"|"disconnected"|"loggedOut"|"failed";
export type ChatView="chats"|"unread"|"groups"|"archived";
export type ChatMessageKind="text"|"image"|"video"|"audio"|"voice"|"document"|"sticker"|"location"|"contact"|"poll"|"unsupported";
export type ChatMessageStatus=""|"pending"|"sent"|"delivered"|"read"|"failed";

export interface ChatAccount {id:string;platform:ChatPlatform;name:string;jid:string;state:ChatAccountState;detail:string;
  createdAt:number;linkedAt:number;syncedAt:number;historyProgress:number;importing:boolean;unread:number;counts:Record<ChatView,number>}
export interface ChatOverview {accounts:ChatAccount[];bridge:{available:boolean;detail:string}}
export interface ChatThread {id:string;accountId:string;jid:string;kind:"direct"|"group";name:string;at:number;preview:string;previewKind:ChatMessageKind|"deleted"|"";
  previewFromMe:boolean;previewSender:string;previewStatus:ChatMessageStatus;unread:number;archived:boolean;pinned:boolean;mutedUntil:number}
export interface ChatThreadPage {pinned:ChatThread[];threads:ChatThread[];cursor:string|null}
export interface ChatMessage {id:string;messageId:string;sender:string;senderName:string;fromMe:boolean;at:number;kind:ChatMessageKind;text:string;fileName:string;
  quotedId:string;quotedSender:string;quotedText:string;status:ChatMessageStatus;edited:boolean;deleted:boolean;media:ChatMedia|null}
export interface ChatMessagePage {thread:ChatThread;messages:ChatMessage[];before:string|null}
/** What the browser sees while linking: a QR code to draw, a pairing code to type, or why it stopped. */
export interface ChatLinkState {accountId:string;state:"starting"|"qr"|"code"|"paired"|"failed";qr?:string;expiresAt?:number;code?:string;reason?:string}

export const chatLimits={accounts:10,name:64,text:65536,page:50,messagePage:60,messagesPerAccount:500_000} as const;
export const chatKindLabels:Record<ChatMessageKind,string>={text:"",image:"Photo",video:"Video",audio:"Audio",voice:"Voice message",document:"Document",
  sticker:"Sticker",location:"Location",contact:"Contact",poll:"Poll",unsupported:"Message not supported here yet"};
/** Fixed-width keys that sort newest first, so lists are range scans. */
export const chatTimeKey=(at:number)=>String(Math.max(0,9_999_999_999_999-Math.floor(Math.max(0,at)))).padStart(13,"0");
/** Incoming photos, stickers and voice notes up to this size download as they arrive; the rest when opened. */
export const chatAutoDownloadBytes=16*1024*1024;
export type ChatMediaState="remote"|"stored"|"retrying"|"expired"|"failed";
export interface ChatMedia {kind:ChatMessageKind;mime:string;size:number;seconds:number;width:number;height:number;state:ChatMediaState}
const extensions:Record<string,string>={"image/jpeg":"jpg","image/png":"png","image/webp":"webp","image/gif":"gif","video/mp4":"mp4","video/3gpp":"3gp","audio/ogg":"ogg","audio/mpeg":"mp3","audio/mp4":"m4a","audio/aac":"aac","application/pdf":"pdf"};
const labels:Record<string,string>={image:"Photo",sticker:"Sticker",video:"Video",audio:"Audio",voice:"Voice message",document:"Document"};
/** The name an attachment downloads and saves under: the sender's file name, or one from its kind and time. */
export function localName(m:{kind:string;mime:string;fileName:string;at:number}){
  const given=m.fileName.replace(/[/\\\0]/g,"_").trim();
  if(given&&given!=="."&&given!=="..")return given.slice(0,200);
  const d=new Date(m.at),pad=(n:number)=>String(n).padStart(2,"0"),extension=extensions[m.mime];
  return `WhatsApp ${labels[m.kind]??"Attachment"} ${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())} at ${pad(d.getHours())}.${pad(d.getMinutes())}.${pad(d.getSeconds())}${extension?"."+extension:""}`;
}
