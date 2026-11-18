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
  previewFromMe:boolean;previewSender:string;previewStatus:ChatMessageStatus;unread:number;archived:boolean;pinned:boolean;mutedUntil:number;
  /** The name WhatsApp gives, when a Dar contact names the chat instead. */whatsappName:string;contact:ChatContact|null}
export interface ChatThreadPage {pinned:ChatThread[];threads:ChatThread[];cursor:string|null}
export interface ChatMessage {id:string;messageId:string;sender:string;senderName:string;fromMe:boolean;at:number;kind:ChatMessageKind;text:string;fileName:string;
  quotedId:string;quotedSender:string;quotedText:string;status:ChatMessageStatus;edited:boolean;deleted:boolean;media:ChatMedia|null;reactions:ChatReaction[]}
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
export interface ChatReaction {emoji:string;count:number;mine:boolean;names:string[]}
export const chatQuickReactions=["👍","❤️","😂","😮","😢","🙏"] as const;
export interface ChatMember {jid:string;name:string;admin?:boolean}
export interface ChatInfo {thread:ChatThread;account:{id:string;name:string;state:ChatAccountState};messages:number;complete:boolean;
  media:{messageId:string;kind:ChatMessageKind;mime:string;state:ChatMediaState;fileName:string;size:number;at:number}[];
  group:{name:string;topic:string;members:ChatMember[];created:number}|null;groupError:string}
/** Mute choices as WhatsApp offers them; -1 is always. */
export const chatMuteChoices=[["8 hours",8*3600_000],["1 week",7*86400_000],["Always",-1]] as const;
/** A chat's Dar contact, matched by phone number; Dar never writes contacts to WhatsApp. */
export interface ChatContact {name:string;book:string;href:string}
// ITU country calling codes: one digit for 1 and 7, these two-digit ones, otherwise three digits.
const twoDigitCodes=new Set(["20","27","30","31","32","33","34","36","39","40","41","43","44","45","46","47","48","49","51","52","53","54","55","56","57","58",
  "60","61","62","63","64","65","66","81","82","84","86","90","91","92","93","94","95","98"]);
/** The country calling code at the start of an international number, without "+". */
export function callingCode(digits:string){
  if(digits.startsWith("1")||digits.startsWith("7"))return digits[0];
  return twoDigitCodes.has(digits.slice(0,2))?digits.slice(0,2):digits.slice(0,3);
}
/**
 * The international digits a contact's number stands for. "+…" and "00…" are international;
 * a national number with a trunk 0 takes the country of the account's own number. Other bare
 * numbers are taken as international when they are long enough, otherwise as national.
 */
export function internationalDigits(value:string,ownDigits:string):string{
  const trimmed=value.trim();let digits=trimmed.replace(/\D/g,"");
  if(digits.length<6||digits.length>18)return "";
  if(trimmed.startsWith("+"))return digits;
  if(digits.startsWith("00"))return digits.slice(2);
  const code=ownDigits?callingCode(ownDigits):"";
  if(digits.startsWith("0"))return code?code+digits.replace(/^0+/,""):"";
  if(digits.length>=11)return digits;
  return code&&!digits.startsWith(code)?code+digits:digits;
}
