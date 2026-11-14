import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {photoKind} from "../../shared/photos.ts";
import {lockedFolderType} from "../../shared/locked-folder.ts";
import {currentSeq,nextSeq,epochOf,syncKey,updateAsset,updateAlbum,updateAlbumMember,updateStack,insertAlbum,insertAlbumItem} from "./photo-sync.ts";

/**
 * Immich mobile-app compatibility (server v3.4.1 contract). Sessions bind an Immich access
 * token to a Photos app password; the sync stream is served in channels, each a sequence of
 * JSON lines ordered by Dar's change sequence. Update IDs are UUID-shaped: the time in
 * milliseconds, then the sequence in the last group, so acks round-trip to a sequence.
 */
type Asset=NonNullable<ReturnType<Ctx["db"]["photoAssets"]["fileId"]["find"]>>;
type Entry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
type Session=NonNullable<ReturnType<Ctx["db"]["immichSessions"]["digest"]["find"]>>;
type Account=NonNullable<ReturnType<Ctx["db"]["accounts"]["id"]["find"]>>;
type Line={type:string;data:Record<string,unknown>;ack:string};

function fail(code:string):never{throw new SenderError(code);}
const uuid=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
export const immichVersion={major:3,minor:4,patch:1};
/** A UUIDv7-shaped update ID: 48 bits of time, then the change sequence in the last group. */
export function updateId(ms:number,seq:number){const t=Math.max(0,Math.floor(ms)).toString(16).padStart(12,"0").slice(-12);return `${t.slice(0,8)}-${t.slice(8,12)}-7000-8000-${Math.max(0,seq).toString(16).padStart(12,"0")}`;}
export function ackSeq(ack:string|undefined){const id=ack?.split("|")[1]??"";return uuid.test(id)?parseInt(id.slice(24),16):-1;}
export function ackTime(ack:string|undefined){const id=ack?.split("|")[1]??"";return uuid.test(id)?parseInt(id.replaceAll("-","").slice(0,12),16):0;}
const iso=(ms:number)=>new Date(Math.max(0,ms)).toISOString();

/** Request types and the channels that serve them, in Immich's order. Unlisted types (partners, memories, people, faces, OCR, edits, metadata) have nothing to send. */
export const immichChannels:Record<string,string[]>={
  AuthUsersV1:["authUserV1"],AuthUsersV2:["authUser"],UsersV1:["users"],AssetsV2:["assetDeletes","assets"],StacksV1:["stackDeletes","stacks"],
  AlbumAssetsV2:["albumAssets"],AlbumsV2:["albumDeletes","albums"],AlbumUsersV1:["albumUserDeletes","albumUsers"],AlbumToAssetsV1:["albumToAssetDeletes","albumToAssets"],
  AssetExifsV1:["exifs"],AlbumAssetExifsV1:["albumAssetExifs"],
};
export const immichOrder=["AuthUsersV1","AuthUsersV2","UsersV1","PartnersV1","AssetsV2","StacksV1","PartnerAssetsV2","PartnerStacksV1","AlbumAssetsV2","AlbumsV2","AlbumUsersV1","AlbumToAssetsV1","AssetExifsV1","AlbumAssetExifsV1"];
/** The entity type whose checkpoint a channel advances. */
export const channelAck:Record<string,string>={authUserV1:"AuthUserV1",authUser:"AuthUserV2",users:"UserV1",assetDeletes:"AssetDeleteV1",assets:"AssetV2",stackDeletes:"StackDeleteV1",stacks:"StackV1",
  albumAssets:"AlbumAssetCreateV2",albumDeletes:"AlbumDeleteV1",albums:"AlbumV2",albumUserDeletes:"AlbumUserDeleteV1",albumUsers:"AlbumUserV1",albumToAssetDeletes:"AlbumToAssetDeleteV1",
  albumToAssets:"AlbumToAssetV1",exifs:"AssetExifV1",albumAssetExifs:"AlbumAssetExifCreateV1"};
const entityTypes=new Set([...Object.values(channelAck),"SyncAckV1","SyncResetV1","SyncCompleteV1","AssetV1","AlbumV1","PartnerV1","PartnerDeleteV1","PartnerAssetV1","PartnerAssetV2","PartnerAssetBackfillV1","PartnerAssetBackfillV2","PartnerAssetDeleteV1",
  "PartnerAssetExifV1","PartnerAssetExifBackfillV1","PartnerStackV1","PartnerStackBackfillV1","PartnerStackDeleteV1","UserDeleteV1","AssetEditV1","AssetEditDeleteV1","AssetMetadataV1","AssetMetadataDeleteV1","AssetOcrV1","AssetOcrDeleteV1",
  "AlbumUserBackfillV1","AlbumAssetCreateV1","AlbumAssetUpdateV1","AlbumAssetUpdateV2","AlbumAssetBackfillV1","AlbumAssetBackfillV2","AlbumAssetExifUpdateV1","AlbumAssetExifBackfillV1","AlbumToAssetBackfillV1",
  "MemoryV1","MemoryV2","MemoryDeleteV1","MemoryToAssetV1","MemoryToAssetV2","MemoryToAssetDeleteV1","PersonV1","PersonDeleteV1","AssetFaceV1","AssetFaceV2","AssetFaceV3","AssetFaceDeleteV1","UserMetadataV1","UserMetadataDeleteV1"]);

/** The account an Immich access token speaks for, while its app password is valid and allows Photos. */
export function immichSession(ctx:Ctx,digest:string,allows:(scope:string,permission:"read"|"write")=>boolean,write:boolean):{session:Session;user:Account}{
  const now=Number(ctx.timestamp.toMillis()),session=/^[a-f0-9]{64}$/.test(digest)?ctx.db.immichSessions.digest.find(digest):undefined;
  const protocol=session&&ctx.db.sessions.id.find(session.sessionId),password=session&&ctx.db.appPasswords.sessionId.find(session.sessionId),user=session&&ctx.db.accounts.id.find(session.userId);
  if(!session||!protocol?.active||protocol.expiresAt<=now||!password||password.revoked||password.expiresAt<=now||!user?.enabled)fail("unauthenticated");
  if(!allows(password.scope,write?"write":"read"))fail("forbidden");
  if(now-session.usedAt>60_000)ctx.db.immichSessions.digest.update({...session,usedAt:now});
  return {session,user};
}
export function immichUser(ctx:Ctx,user:Account){
  const email=`${user.username}@dar`;
  return {id:user.id,email,name:user.displayName,isAdmin:user.administrator,avatarColor:"primary",profileImagePath:"",profileChangedAt:iso(user.createdAt),createdAt:iso(user.createdAt),updatedAt:iso(user.createdAt),
    deletedAt:null,oauthId:"",shouldChangePassword:false,storageLabel:null,quotaSizeInBytes:10*1024**3,quotaUsageInBytes:ctx.db.usage.userId.find(user.id)?.used??0,status:"active",license:null,clusterGroupId:user.id};
}

/** Where a file is (as in photos.ts): library, locked, trash or gone. */
function placeOf(ctx:Ctx){
  const cache=new Map<string,string>();
  const folder=(id:string):string=>{const hit=cache.get(id);if(hit)return hit;const f=ctx.db.fileEntries.id.find(id);const state=!f||f.trashed?"gone":!f.parentId?(f.mediaType===lockedFolderType?"locked":"library"):folder(f.parentId);cache.set(id,state);return state;};
  return (entry:Entry)=>entry.trashed?(entry.listKey.startsWith(`${entry.ownerId}/trash/`)?"trash":"gone"):entry.parentId?folder(entry.parentId):"library";
}
function fileOf(ctx:Ctx,asset:Asset){const e=ctx.db.fileEntries.id.find(asset.fileId);return e&&e.kind==="file"&&e.ownerId===asset.ownerId&&photoKind(e.mediaType,e.name)?e:undefined;}
/** Motion halves of Live Photos are hidden, and their stills point at them. Same rule as photos.ts, by ContentIdentifier or name. */
function liveLinks(ctx:Ctx,asset:Asset,entry:Entry){
  const sibling=(name:string)=>{for(const row of ctx.db.fileEntries.listKey.filter(`${entry.ownerId}/${entry.parentId}/${name.toLowerCase()}`))return row;return undefined;};
  const short=(a:Asset)=>a.kind==="video"&&a.duration>0&&a.duration<=4;
  const dot=entry.name.lastIndexOf("."),stem=dot>0?entry.name.slice(0,dot):entry.name,ext=dot>0?entry.name.slice(dot+1).toLowerCase():"";
  if(asset.kind==="image"){
    if(asset.liveKey)for(const other of ctx.db.photoAssets.liveKey.filter(asset.liveKey))if(other.fileId!==asset.fileId&&short(other))return {motion:other.fileId};
    if(["heic","heif","jpg","jpeg"].includes(ext))for(const e of ["mov","mp4"]){const s=sibling(`${stem}.${e}`),a=s&&ctx.db.photoAssets.fileId.find(s.id);if(a&&short(a))return {motion:a.fileId};}
    return {};
  }
  if(!short(asset))return {};
  if(asset.liveKey)for(const other of ctx.db.photoAssets.liveKey.filter(asset.liveKey))if(other.kind==="image"&&other.fileId!==asset.fileId)return {hidden:true};
  if(["mov","mp4"].includes(ext))for(const e of ["heic","heif","jpg","jpeg"]){const s=sibling(`${stem}.${e}`),a=s&&ctx.db.photoAssets.fileId.find(s.id);if(a?.kind==="image")return {hidden:true};}
  return {};
}
function assetData(ctx:Ctx,asset:Asset,entry:Entry,state:string,viewer:string){
  const live=liveLinks(ctx,asset,entry),mine=asset.ownerId===viewer;
  return {id:asset.fileId,ownerId:asset.ownerId,originalFileName:entry.name,checksum:asset.sha1,thumbhash:null,fileCreatedAt:iso(asset.takenAt),fileModifiedAt:iso(entry.modifiedAt),localDateTime:iso(asset.takenAt),
    createdAt:iso(asset.indexedAt),deletedAt:state==="trash"?iso(entry.modifiedAt):null,type:asset.kind==="video"?"VIDEO":"IMAGE",isFavorite:mine&&asset.favorite,isEdited:false,libraryId:null,
    visibility:live.hidden?"hidden":mine&&asset.archived?"archive":"timeline",duration:asset.kind==="video"?Math.round(asset.duration*1000):null,width:asset.width||null,height:asset.height||null,
    livePhotoVideoId:live.motion??null,stackId:mine&&asset.stackId?asset.stackId:null};
}
function exifData(asset:Asset,entry:Entry){
  let e:Record<string,unknown>={};try{e=JSON.parse(asset.exif||"{}");}catch{e={};}
  const exposure=typeof e.exposure==="number"?(e.exposure<1?`1/${Math.round(1/e.exposure)}`:String(e.exposure)):null;
  return {assetId:asset.fileId,description:asset.description,exifImageWidth:asset.width||null,exifImageHeight:asset.height||null,fileSizeInByte:entry.size,orientation:null,dateTimeOriginal:iso(asset.takenAt),modifyDate:iso(entry.modifiedAt),
    timeZone:null,latitude:asset.hasLocation?asset.latitude:null,longitude:asset.hasLocation?asset.longitude:null,projectionType:null,city:null,state:null,country:null,make:e.make??null,model:e.model??null,lensModel:e.lens??null,
    fNumber:e.fNumber??null,focalLength:e.focalLength??null,iso:e.iso??null,exposureTime:exposure,profileDescription:null,rating:null,fps:null};
}

/** Albums the user owns or is a member of. */
function visibleAlbums(ctx:Ctx,userId:string){
  const albums=[...ctx.db.photoAlbums.ownerId.filter(userId)];
  for(const m of ctx.db.photoAlbumMembers.userId.filter(userId)){const a=ctx.db.photoAlbums.id.find(m.albumId);if(a)albums.push(a);}
  return albums;
}

/**
 * The start of a sync stream: whether the device must start over (its epoch is behind, or
 * its last complete sync is over 30 days old), and its checkpoints. A fresh session first
 * gives every row it can see a sequence, so partial streams resume exactly.
 */
export function immichSyncStart(ctx:Ctx,session:Session,reset:boolean){
  const now=Number(ctx.timestamp.toMillis());
  const checkpoints=[...ctx.db.immichCheckpoints.immichId.filter(session.id)];
  if(reset){for(const row of checkpoints)ctx.db.immichCheckpoints.id.delete(row.id);checkpoints.length=0;}
  const complete=checkpoints.find(c=>c.type==="SyncCompleteV1");
  if(checkpoints.length&&(epochOf(ctx,session.userId)>session.epochSeen||complete&&now-ackTime(complete.ack)>30*86400_000))return {reset:true,checkpoints:{},nowId:"",backfilling:false};
  // Rows from before the upgrade have no sequence yet: number them, in bounded batches.
  let numbered=0;const budget=2000;
  for(const row of ctx.db.photoAssets.ownerId.filter(session.userId)){if(numbered>=budget)break;if(!row.syncKey){updateAsset(ctx,row);numbered++;}}
  for(const album of visibleAlbums(ctx,session.userId)){
    if(numbered>=budget)break;
    if(!album.syncSeq){updateAlbum(ctx,album);numbered++;}
    for(const member of ctx.db.photoAlbumMembers.albumId.filter(album.id))if(!member.syncSeq&&numbered<budget){updateAlbumMember(ctx,member);numbered++;}
    for(const item of ctx.db.photoAlbumItems.albumId.filter(album.id)){
      if(numbered>=budget)break;
      const asset=ctx.db.photoAssets.fileId.find(item.fileId);if(asset&&!asset.syncKey){updateAsset(ctx,asset);numbered++;}
      if(!item.syncSeq){ctx.db.photoAlbumItems.id.update({...item,syncSeq:nextSeq(ctx)});numbered++;}
    }
  }
  for(const stack of ctx.db.photoStacks.ownerId.filter(session.userId))if(!stack.syncSeq&&numbered<budget){updateStack(ctx,stack);numbered++;}
  if(numbered>=budget)return {reset:false,checkpoints:{},nowId:"",backfilling:true};
  if(!checkpoints.length)ctx.db.immichSessions.digest.update({...ctx.db.immichSessions.digest.find(session.digest)!,epochSeen:currentSeq(ctx)});
  return {reset:false,checkpoints:Object.fromEntries(checkpoints.map(c=>[c.type,c.ack])),nowId:updateId(now,currentSeq(ctx)),backfilling:false};
}

/** One page of a sync channel: lines after the given sequence, at most limit. */
export function immichSyncPage(ctx:Ctx,session:Session,user:Account,channel:string,after:number,limit:number):{lines:Line[];last:number;more:boolean}{
  const now=Number(ctx.timestamp.toMillis()),userId=user.id,ackType=channelAck[channel];if(!ackType)fail("invalidInput");
  const lines:Line[]=[];let last=after;
  const emit=(type:string,data:Record<string,unknown>,seq:number,ack=ackType)=>{lines.push({type,data,ack:`${ack}|${updateId(now,seq)}`});last=seq;};
  const place=placeOf(ctx);
  // Users: the account itself and the people it shares albums with; small, sent every stream.
  if(channel==="authUser"||channel==="authUserV1"){
    const u=immichUser(ctx,user);
    emit(ackType,{id:u.id,name:u.name,email:u.email,isAdmin:u.isAdmin,avatarColor:"primary",deletedAt:null,hasProfileImage:false,profileChangedAt:u.profileChangedAt,oauthId:channel==="authUserV1"?"":null,pinCode:null,storageLabel:null,quotaSizeInBytes:u.quotaSizeInBytes,quotaUsageInBytes:u.quotaUsageInBytes},Math.max(1,currentSeq(ctx)));
    return {lines,last,more:false};
  }
  if(channel==="users"){

    const ids=new Set([userId]);for(const a of visibleAlbums(ctx,userId)){ids.add(a.ownerId);for(const m of ctx.db.photoAlbumMembers.albumId.filter(a.id))ids.add(m.userId);}
    for(const id of ids){const a=ctx.db.accounts.id.find(id);if(a)emit("UserV1",{id:a.id,name:a.displayName,email:`${a.username}@dar`,avatarColor:"primary",deletedAt:null,hasProfileImage:false,profileChangedAt:iso(a.createdAt)},Math.max(1,currentSeq(ctx)));}
    return {lines,last,more:false};
  }
  // Tombstones for the user, ordered by sequence.
  const deletes:Record<string,[string,(row:{entityId:string;extraId:string})=>Record<string,unknown>]>={
    assetDeletes:["asset",r=>({assetId:r.entityId})],stackDeletes:["stack",r=>({stackId:r.entityId})],albumDeletes:["album",r=>({albumId:r.entityId})],
    albumUserDeletes:["albumUser",r=>({albumId:r.entityId,userId:r.extraId})],albumToAssetDeletes:["albumToAsset",r=>({albumId:r.entityId,assetId:r.extraId})],
  };
  if(deletes[channel]){
    const [kind,shape]=deletes[channel];
    for(const row of ctx.db.photoSyncDeletes.key.filter(new Range<string>({tag:"excluded",value:String(Math.max(0,after)).padStart(16,"0")},{tag:"unbounded"}))){
      if(lines.length>=limit)return {lines,last,more:true};
      if(row.kind!==kind||!row.audience.split(",").includes(userId))continue;
      emit(ackType,shape(row),row.seq);
    }
    return {lines,last,more:false};
  }
  // The user's own assets (and their EXIF), by syncKey.
  if(channel==="assets"||channel==="exifs"){
    const range=new Range<string>({tag:"excluded",value:syncKey(userId,Math.max(0,after))},{tag:"excluded",value:userId+"0"});
    for(const asset of ctx.db.photoAssets.syncKey.filter(range)){
      if(lines.length>=limit)return {lines,last,more:true};
      const entry=fileOf(ctx,asset),state=entry?place(entry):"gone";
      if(!entry||state==="gone"||state==="locked"){if(channel==="assets")emit("AssetDeleteV1",{assetId:asset.fileId},asset.syncSeq);else last=asset.syncSeq;continue;}
      if(channel==="assets")emit("AssetV2",assetData(ctx,asset,entry,state,userId),asset.syncSeq);else emit("AssetExifV1",exifData(asset,entry),asset.syncSeq);
    }
    return {lines,last,more:false};
  }
  // Rows without an index by sequence are gathered, sorted and paged.
  type Pending={seq:number;type:string;data:Record<string,unknown>};const pending:Pending[]=[];
  const albums=visibleAlbums(ctx,userId);
  if(channel==="albums")for(const a of albums)if(a.syncSeq>after)pending.push({seq:a.syncSeq,type:"AlbumV2",data:{id:a.id,name:a.name,description:a.description,createdAt:iso(a.createdAt),updatedAt:iso(a.updatedAt),thumbnailAssetId:a.coverId||null,isActivityEnabled:false,order:"desc"}});
  if(channel==="albumUsers")for(const a of albums){
    if(a.syncSeq>after)pending.push({seq:a.syncSeq,type:"AlbumUserV1",data:{albumId:a.id,userId:a.ownerId,role:"owner"}});
    for(const m of ctx.db.photoAlbumMembers.albumId.filter(a.id))if(m.syncSeq>after)pending.push({seq:m.syncSeq,type:"AlbumUserV1",data:{albumId:a.id,userId:m.userId,role:m.role==="editor"?"editor":"viewer"}});
  }
  if(channel==="albumToAssets")for(const a of albums)for(const item of ctx.db.photoAlbumItems.albumId.filter(a.id))if(item.syncSeq>after)pending.push({seq:item.syncSeq,type:"AlbumToAssetV1",data:{albumId:a.id,assetId:item.fileId}});
  if(channel==="albumAssets"||channel==="albumAssetExifs"){
    const seen=new Set<string>();
    for(const a of albums)for(const item of ctx.db.photoAlbumItems.albumId.filter(a.id)){
      const asset=ctx.db.photoAssets.fileId.find(item.fileId);if(!asset||asset.ownerId===userId||asset.syncSeq<=after||seen.has(asset.fileId))continue;seen.add(asset.fileId);
      const entry=fileOf(ctx,asset);if(!entry||place(entry)!=="library")continue;
      pending.push(channel==="albumAssets"?{seq:asset.syncSeq,type:"AlbumAssetCreateV2",data:assetData(ctx,asset,entry,"library",userId)}:{seq:asset.syncSeq,type:"AlbumAssetExifCreateV1",data:exifData(asset,entry)});
    }
  }
  if(channel==="stacks")for(const s of ctx.db.photoStacks.ownerId.filter(userId))if(s.syncSeq>after)pending.push({seq:s.syncSeq,type:"StackV1",data:{id:s.id,ownerId:s.ownerId,primaryAssetId:s.primaryId,createdAt:iso(s.createdAt),updatedAt:iso(s.updatedAt)}});
  pending.sort((x,y)=>x.seq-y.seq);
  for(const row of pending.slice(0,limit))emit(row.type,row.data,row.seq);
  return {lines,last,more:pending.length>limit};
}

export function immichSetAcks(ctx:Ctx,session:Session,acks:unknown){
  if(!Array.isArray(acks)||acks.length>1000)fail("invalidInput");
  const latest=new Map<string,string>();
  for(const ack of acks){
    if(typeof ack!=="string"||ack.length>200)fail("invalidInput");const type=ack.split("|")[0];
    // Acknowledging a reset starts the device over.
    if(type==="SyncResetV1"){for(const row of [...ctx.db.immichCheckpoints.immichId.filter(session.id)])ctx.db.immichCheckpoints.id.delete(row.id);ctx.db.immichSessions.digest.update({...session,epochSeen:currentSeq(ctx)});return;}
    if(!entityTypes.has(type))fail("invalidInput");
    const previous=latest.get(type);if(!previous||ackSeq(ack)>=ackSeq(previous))latest.set(type,ack);
  }
  for(const [type,ack] of latest){const id=`${session.id}/${type}`,row={id,immichId:session.id,type,ack};if(ctx.db.immichCheckpoints.id.find(id))ctx.db.immichCheckpoints.id.update(row);else ctx.db.immichCheckpoints.insert(row);}
}
export function immichAcks(ctx:Ctx,session:Session){return [...ctx.db.immichCheckpoints.immichId.filter(session.id)].map(c=>({type:c.type,ack:c.ack}));}
export function immichDeleteAcks(ctx:Ctx,session:Session,types:unknown){
  if(types!==undefined&&(!Array.isArray(types)||types.some(t=>typeof t!=="string")))fail("invalidInput");
  for(const row of [...ctx.db.immichCheckpoints.immichId.filter(session.id)])if(!types||(types as string[]).includes(row.type))ctx.db.immichCheckpoints.id.delete(row.id);
}

/** Bulk upload check: an own photo with the same SHA-1 (also in the trash) is a duplicate. */
export function immichBulkCheck(ctx:Ctx,userId:string,assets:unknown){
  if(!Array.isArray(assets)||assets.length>5000)fail("invalidInput");const place=placeOf(ctx);
  return {results:assets.map(item=>{
    const {id,checksum}=item as {id:unknown;checksum:unknown};if(typeof id!=="string"||typeof checksum!=="string"||id.length>200||checksum.length>100)fail("invalidInput");
    const sha1=checksumToBase64(checksum);
    for(const asset of sha1?ctx.db.photoAssets.sha1.filter(sha1):[]){
      if(asset.ownerId!==userId)continue;const entry=fileOf(ctx,asset),state=entry?place(entry):"gone";
      if(state!=="gone")return {id,action:"reject",reason:"duplicate",assetId:asset.fileId,isTrashed:state==="trash"};
    }
    return {id,action:"accept"};
  })};
}
/** Immich clients send SHA-1 checksums as base64 (and older ones as hex). */
export function checksumToBase64(value:string){
  if(/^[a-f0-9]{40}$/i.test(value))return base64(value.match(/../g)!.map(h=>parseInt(h,16)));
  return /^[A-Za-z0-9+/]{27}=$/.test(value)?value:"";
}
/** An own asset with this SHA-1, if any. */
export function immichDuplicate(ctx:Ctx,userId:string,sha1:string){
  const place=placeOf(ctx);
  for(const asset of sha1?ctx.db.photoAssets.sha1.filter(sha1):[]){if(asset.ownerId!==userId)continue;const entry=fileOf(ctx,asset);if(entry&&place(entry)!=="gone")return asset.fileId;}
  return "";
}
export function immichSessions(ctx:Ctx,userId:string){
  return [...ctx.db.immichSessions.userId.filter(userId)].map(s=>({id:s.id,deviceType:s.deviceType,deviceOS:s.deviceOS,createdAt:s.createdAt,usedAt:s.usedAt})).sort((a,b)=>b.usedAt-a.usedAt);
}
export function deleteImmichSession(ctx:Ctx,session:Session){
  for(const row of [...ctx.db.immichCheckpoints.immichId.filter(session.id)])ctx.db.immichCheckpoints.id.delete(row.id);
  ctx.db.immichSessions.digest.delete(session.digest);
}
function base64(bytes:number[]){
  const chars="ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";let out="";
  for(let i=0;i<bytes.length;i+=3){const [a,b=0,c=0]=bytes.slice(i,i+3),n=a<<16|b<<8|c;out+=chars[n>>18&63]+chars[n>>12&63]+(i+1<bytes.length?chars[n>>6&63]:"=")+(i+2<bytes.length?chars[n&63]:"=");}
  return out;
}

/** Dar's connection to an Immich server, as the owner sees it (the API key never leaves the runtime). */
export function immichConnectionView(ctx:Ctx,userId:string){
  const row=ctx.db.immichConnections.userId.find(userId);
  if(!row)return {connected:false};
  let summary:unknown={};try{summary=JSON.parse(row.summary||"{}");}catch{summary={};}
  return {connected:true,url:row.url,direction:row.direction,intervalMinutes:row.intervalMinutes,enabled:row.enabled,lastRunAt:row.lastRunAt,lastError:row.lastError,summary,revision:row.revision};
}
export function saveImmichConnection(ctx:Ctx,userId:string,p:Record<string,unknown>){
  if(Object.keys(p).some(k=>!["url","sealedKey","direction","intervalMinutes","enabled","revision"].includes(k)))fail("invalidInput");
  const old=ctx.db.immichConnections.userId.find(userId);if((old?.revision??0)!==p.revision)fail("conflict");
  const url=typeof p.url==="string"?p.url.trim().replace(/\/+$/,""):old?.url??"";
  if(!/^https?:\/\/[^\s/?#]+(\/[^\s?#]*)?$/.test(url)||url.length>500)fail("invalidInput");
  const direction=p.direction===undefined?old?.direction??"import":p.direction;if(direction!=="import"&&direction!=="twoWay")fail("invalidInput");
  const interval=p.intervalMinutes===undefined?old?.intervalMinutes??60:p.intervalMinutes;if(typeof interval!=="number"||![15,60,360,1440].includes(interval))fail("invalidInput");
  const sealedKey=p.sealedKey===undefined?old?.sealedKey??"":p.sealedKey;if(typeof sealedKey!=="string"||!sealedKey||sealedKey.length>2000)fail("invalidInput");
  const enabled=p.enabled===undefined?old?.enabled??true:p.enabled;if(typeof enabled!=="boolean")fail("invalidInput");
  const row={userId,url,sealedKey,direction,intervalMinutes:interval,enabled,lastRunAt:old&&old.url===url?old.lastRunAt:0,lastError:old&&old.url===url?old.lastError:"",summary:old&&old.url===url?old.summary:"{}",revision:(old?.revision??0)+1};
  if(old)ctx.db.immichConnections.userId.update(row);else ctx.db.immichConnections.insert(row);
  return immichConnectionView(ctx,userId);
}
export function deleteImmichConnection(ctx:Ctx,userId:string){
  ctx.db.immichConnections.userId.delete(userId);
  for(const row of [...ctx.db.immichRemote.userId.filter(userId)])ctx.db.immichRemote.id.delete(row.id);
  return {deleted:true};
}
/** Connections whose interval has passed (or one account's, when asked to sync now). */
export function immichDue(ctx:Ctx,userId:string){
  const now=Number(ctx.timestamp.toMillis()),rows=userId?[ctx.db.immichConnections.userId.find(userId)].filter(r=>!!r):[...ctx.db.immichConnections.iter()].filter(r=>r.enabled&&r.lastRunAt+r.intervalMinutes*60_000<=now);
  return {connections:rows.filter(r=>ctx.db.accounts.id.find(r.userId)?.enabled).map(r=>({userId:r.userId,url:r.url,sealedKey:r.sealedKey,direction:r.direction}))};
}
export function immichReport(ctx:Ctx,userId:string,error:string,summary:unknown){
  const row=ctx.db.immichConnections.userId.find(userId);if(!row)return {ok:false};
  ctx.db.immichConnections.userId.update({...row,lastRunAt:Number(ctx.timestamp.toMillis()),lastError:error.slice(0,500),summary:JSON.stringify(summary).slice(0,2000)});return {ok:true};
}
/** What is already known locally: mapped remote IDs and the SHA-1 of the owner's photos (paged by file ID). */
export function immichKnown(ctx:Ctx,userId:string,after:string,limit:number){
  const remote=after?[]:[...ctx.db.immichRemote.userId.filter(userId)].map(r=>({remoteId:r.remoteId,fileId:r.fileId,checksum:r.checksum}));
  const place=placeOf(ctx),local:{fileId:string;sha1:string;name:string;mediaType:string;takenAt:number;modifiedAt:number;blobId:string;size:number;favorite:boolean;archived:boolean}[]=[];
  const rows=[...ctx.db.photoAssets.ownerId.filter(userId)].sort((a,b)=>a.fileId.localeCompare(b.fileId));
  for(const asset of rows){
    if(after&&asset.fileId<=after)continue;if(local.length>=limit)break;
    const entry=fileOf(ctx,asset);if(!entry||place(entry)!=="library"||!asset.sha1)continue;
    local.push({fileId:asset.fileId,sha1:asset.sha1,name:entry.name,mediaType:entry.mediaType,takenAt:asset.takenAt,modifiedAt:entry.modifiedAt,blobId:entry.blobId,size:entry.size,favorite:asset.favorite,archived:asset.archived});
  }
  return {remote,local,next:local.length>=limit?local.at(-1)!.fileId:""};
}
export function immichMap(ctx:Ctx,userId:string,remoteId:string,fileId:string,checksum:string){
  if(remoteId.length>100||fileId.length>100||checksum.length>100)fail("invalidInput");
  const id=`${userId}/${remoteId}`,row={id,userId,remoteId,fileId,checksum};
  if(ctx.db.immichRemote.id.find(id))ctx.db.immichRemote.id.update(row);else ctx.db.immichRemote.insert(row);return {ok:true};
}
/** After an imported file is indexed: Immich's favorite, archive and description. */
export function immichApply(ctx:Ctx,userId:string,fileId:string,favorite:boolean,archived:boolean,description:string){
  const asset=ctx.db.photoAssets.fileId.find(fileId);if(!asset||asset.ownerId!==userId)return {applied:false};
  updateAsset(ctx,{...asset,favorite,archived,description:description.slice(0,2000),revision:asset.revision+1});return {applied:true};
}
/** An Immich album, imported once by remote ID and kept in step with the imported photos. */
export function immichAlbum(ctx:Ctx,userId:string,remoteId:string,name:string,description:string,fileIds:string[]){
  const now=Number(ctx.timestamp.toMillis()),mapId=`${userId}/album:${remoteId}`;let albumId=ctx.db.immichRemote.id.find(mapId)?.fileId??"";
  if(!albumId||!ctx.db.photoAlbums.id.find(albumId)){
    if([...ctx.db.photoAlbums.ownerId.filter(userId)].length>=500)return {albumId:"",added:0};
    albumId=ctx.newUuidV4().toString();while(ctx.db.photoAlbums.id.find(albumId))albumId=ctx.newUuidV4().toString();
    insertAlbum(ctx,{id:albumId,ownerId:userId,name:name.slice(0,200)||"Immich album",description:description.slice(0,2000),coverId:"",createdAt:now,updatedAt:now,revision:1});
    immichMap(ctx,userId,`album:${remoteId}`,albumId,"");
  }
  let added=0;
  for(const fileId of fileIds.slice(0,50000)){const asset=ctx.db.photoAssets.fileId.find(fileId);if(!asset||asset.ownerId!==userId||ctx.db.photoAlbumItems.id.find(`${albumId}/${fileId}`))continue;insertAlbumItem(ctx,{id:`${albumId}/${fileId}`,albumId,fileId,addedAt:now});added++;}
  return {albumId,added};
}
