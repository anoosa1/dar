import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {appEnabled} from "./app-settings.ts";
import {insertAsset,updateAsset,deleteAsset,insertAlbum,updateAlbum,deleteAlbum,insertAlbumItem,deleteAlbumItem,insertAlbumMember,updateAlbumMember,deleteAlbumMember,insertStack,updateStack,deleteStack,pruneTombstones} from "./photo-sync.ts";
import {lockedFolderType} from "../../shared/locked-folder.ts";
import {photoKind,photoLimits,photoIndexVersion,livePhotoMaxSeconds,rawExtensions,nameParts,type PhotoAsset,type PhotoExif,type PhotoMoved,type PhotoDuplicateGroup,type PhotoAlbum,type PhotoLink,photoLinkLimits} from "../../shared/photos.ts";
type Asset=NonNullable<ReturnType<Ctx["db"]["photoAssets"]["fileId"]["find"]>>;
type Album=NonNullable<ReturnType<Ctx["db"]["photoAlbums"]["id"]["find"]>>;
type Entry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;

/** File changes Photos makes through Files' own checks, acting for the browser session. */
export interface PhotoFiles {
  unlocked:boolean;
  trash(entry:Entry):void;restore(entry:Entry):void;remove(entry:Entry):void;
  /** Moves into a folder, numbering the name if it is taken. */
  move(entry:Entry,parentId:string):void;
  lockedFolder():string;monthFolder(year:number,month:number):string;uploadFolder():string;
}

function fail(code:string):never{throw new SenderError(code);}
function text(value:unknown,max:number,allowEmpty=false){
  if(typeof value!=="string"||!allowEmpty&&!value.trim()||value.includes("\0")||value.length>max)fail("invalidInput");
  if([...value].some(c=>c.codePointAt(0)!>=0xd800&&c.codePointAt(0)!<=0xdfff))fail("invalidInput");
  return value;
}
function num(value:unknown,min:number,max:number){if(typeof value!=="number"||!Number.isFinite(value)||value<min||value>max)fail("invalidInput");return value;}
function int(value:unknown,min=0,max=0xffffffff){if(typeof value!=="number"||!Number.isInteger(value)||value<min||value>max)fail("invalidInput");return value;}
function keys(p:Record<string,unknown>,allowed:string[]){if(Object.keys(p).some(k=>!allowed.includes(k)))fail("invalidInput");}
function opaque(value:unknown){if(typeof value!=="string"||!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value))fail("invalidInput");return value;}
function ids(value:unknown,max=photoLimits.batch){if(!Array.isArray(value)||!value.length||value.length>max)fail("invalidInput");return [...new Set(value.map(opaque))];}
const ownerRange=(ownerId:string,after?:string)=>new Range<string>(after?{tag:"excluded",value:after}:{tag:"included",value:ownerId+"/"},{tag:"excluded",value:ownerId+"0"});
/** Keys starting with prefix; "/" is followed by "0" in key order. */
const prefixRange=(prefix:string)=>new Range<string>({tag:"included",value:prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
const first=<T>(rows:Iterable<T>)=>rows[Symbol.iterator]().next().value as T|undefined;
/** Newest first within an owner; ties keep file ID order. */
export const photoTimeKey=(ownerId:string,takenAt:number,fileId:string)=>`${ownerId}/${String(9e15-Math.max(0,Math.min(Math.round(takenAt),9e15-1))).padStart(16,"0")}/${fileId}`;
const newest=(a:Asset,b:Asset)=>b.takenAt-a.takenAt||a.fileId.localeCompare(b.fileId);
const stillExtensions=["heic","heif","jpg","jpeg"],motionExtensions=["mov","mp4"];
function newId(ctx:Ctx){let id=ctx.newUuidV4().toString();while(ctx.db.photoStacks.id.find(id)||ctx.db.photoAlbums.id.find(id))id=ctx.newUuidV4().toString();return id;}
const exifOf=(asset:Asset)=>{try{return JSON.parse(asset.exif||"{}") as PhotoExif;}catch{return {} as PhotoExif;}};

/** Where a file is: in the folder tree, inside the Locked folder, directly in the trash, or gone (in a trashed folder or deleted). */
type State="library"|"locked"|"trash"|"gone";

/**
 * Per-call reader over the index. Folder placement is cached, so a page of thousands
 * of photos walks each folder chain once.
 */
function reader(ctx:Ctx,viewer=""){
  const folders=new Map<string,State>();
  function folderState(id:string):State{
    const chain:string[]=[];let state:State="gone",current=id;
    for(let depth=0;depth<128;depth++){
      const cached=folders.get(current);if(cached){state=cached;break;}
      const folder=ctx.db.fileEntries.id.find(current);chain.push(current);
      if(!folder||folder.trashed)break;
      if(!folder.parentId){state=folder.mediaType===lockedFolderType?"locked":"library";break;}
      current=folder.parentId;
    }
    for(const id of chain)folders.set(id,state);return state;
  }
  const stateOf=(entry:Entry):State=>entry.trashed?(entry.listKey.startsWith(`${entry.ownerId}/trash/`)?"trash":"gone"):entry.parentId?folderState(entry.parentId):"library";
  /** The owner's live media file for an index row, or undefined if it changed owner, type or was deleted. */
  function fileOf(asset:Asset){const entry=ctx.db.fileEntries.id.find(asset.fileId);return entry&&entry.kind==="file"&&entry.ownerId===asset.ownerId&&photoKind(entry.mediaType,entry.name)?entry:undefined;}
  function sibling(entry:Entry,name:string){
    const lower=name.toLowerCase();
    if(!entry.trashed)return first(ctx.db.fileEntries.listKey.filter(`${entry.ownerId}/${entry.parentId}/${lower}`));
    for(const other of ctx.db.fileEntries.listKey.filter(prefixRange(`${entry.ownerId}/trash/${lower}/`)))if(other.parentId===entry.parentId)return other;
    return undefined;
  }
  const short=(video:Asset)=>video.duration>0&&video.duration<=livePhotoMaxSeconds;
  /**
   * The other half of a Live Photo in the same place: Apple's ContentIdentifier, or a
   * still and a short MOV/MP4 with the same name in the same folder (IMG_1.HEIC + IMG_1.MOV).
   */
  function livePartner(asset:Asset,entry:Entry):Asset|undefined{
    if(asset.kind==="video"&&!short(asset))return undefined;
    const want=asset.kind==="image"?"video":"image",state=stateOf(entry);
    const fits=(other:Asset|null|undefined)=>{if(!other||other.ownerId!==asset.ownerId||other.kind!==want||want==="video"&&!short(other))return false;const e=fileOf(other);return !!e&&stateOf(e)===state;};
    if(asset.liveKey)for(const other of ctx.db.photoAssets.liveKey.filter(asset.liveKey))if(other.fileId!==asset.fileId&&fits(other))return other;
    const {stem,ext}=nameParts(entry.name);
    if(!(asset.kind==="image"?stillExtensions:motionExtensions).includes(ext))return undefined;
    for(const other of asset.kind==="image"?motionExtensions:stillExtensions){const e=sibling(entry,`${stem}.${other}`),row=e&&ctx.db.photoAssets.fileId.find(e.id);if(row&&fits(row))return row;}
    return undefined;
  }
  const stacks=new Map<string,{primaryId:string;count:number}|null>();
  /** A stack's cover and size among members in the given state; stacks of one act like none. */
  function stackOf(asset:Asset,state:State){
    if(!asset.stackId)return undefined;const cacheKey=`${state}/${asset.stackId}`;
    if(!stacks.has(cacheKey)){
      const stack=ctx.db.photoStacks.id.find(asset.stackId);
      const members=stack?[...ctx.db.photoAssets.stackId.filter(stack.id)].filter(a=>{const e=fileOf(a);return !!e&&stateOf(e)===state;}).sort(newest):[];
      stacks.set(cacheKey,stack&&members.length>1?{primaryId:members.some(m=>m.fileId===stack.primaryId)?stack.primaryId:members[0].fileId,count:members.length}:null);
    }
    const value=stacks.get(cacheKey);return value?{id:asset.stackId,...value}:undefined;
  }
  /** Timelines show one tile per Live Photo and per stack. */
  function shown(asset:Asset,entry:Entry,state:State,stacked=true){
    if(asset.kind==="video"&&livePartner(asset,entry))return false;
    const stack=stacked?stackOf(asset,state):undefined;return !stack||stack.primaryId===asset.fileId;
  }
  function expose(asset:Asset,entry:Entry,state=stateOf(entry)):PhotoAsset{
    const value:PhotoAsset={id:asset.fileId,name:entry.name,kind:asset.kind==="video"?"video":"image",mediaType:entry.mediaType,size:entry.size,takenAt:asset.takenAt,width:asset.width,height:asset.height,duration:asset.duration,
      favorite:asset.favorite,archived:asset.archived,thumbnail:asset.blobId!==entry.blobId?"pending":asset.thumbnail==="failed"?"failed":"ready",revision:asset.revision};
    if(asset.kind==="video"){if(asset.playback==="original"||asset.playback==="remux"||asset.playback==="transcode")value.playback=asset.playback;const codec=exifOf(asset).videoCodec;if(codec)value.videoCodec=codec;}
    else{const motion=livePartner(asset,entry);if(motion)value.live=motion.fileId;}
    if(state==="library"||state==="locked"){const stack=stackOf(asset,state);if(stack&&stack.primaryId===asset.fileId)value.stack={id:stack.id,count:stack.count};}
    if(state==="trash")value.trashedAt=entry.modifiedAt;
    // Favorite and archive belong to the owner; others see neither.
    if(viewer&&asset.ownerId!==viewer){value.owner=ctx.db.accounts.id.find(asset.ownerId)?.displayName??"Someone";value.favorite=false;value.archived=false;delete value.stack;}
    return value;
  }
  function folderPath(entry:Entry){const names:string[]=[];let id=entry.parentId;for(let depth=0;id&&depth<32;depth++){const folder=ctx.db.fileEntries.id.find(id);if(!folder)break;names.unshift(folder.name);id=folder.parentId;}return names.join("/");}
  return {stateOf,fileOf,livePartner,stackOf,shown,expose,folderPath};
}
type Reader=ReturnType<typeof reader>;

function inView(view:string,asset:Asset){
  return view==="favorites"?asset.favorite:view==="videos"?asset.kind==="video"&&!asset.archived:view==="archive"?asset.archived:view==="locked"?true:!asset.archived;
}
function albumRows(ctx:Ctx,r:Reader,album:Album){
  const rows:{asset:Asset;entry:Entry}[]=[];
  for(const item of ctx.db.photoAlbumItems.albumId.filter(album.id)){const asset=ctx.db.photoAssets.fileId.find(item.fileId),entry=asset&&r.fileOf(asset);if(asset&&entry&&r.stateOf(entry)==="library"&&r.shown(asset,entry,"library",false))rows.push({asset,entry});}
  return rows;
}
function exposeAlbum(ctx:Ctx,r:Reader,album:Album,userId:string):PhotoAlbum{
  const members=albumRows(ctx,r,album);
  const cover=album.coverId&&members.some(m=>m.asset.fileId===album.coverId)?album.coverId:members[0]?.asset.fileId??"";
  const mine=album.ownerId===userId,role=mine?"owner":ctx.db.photoAlbumMembers.id.find(`${album.id}/${userId}`)?.role==="editor"?"editor":"viewer";
  return {id:album.id,name:album.name,description:album.description,count:members.length,coverId:cover,updatedAt:album.updatedAt,revision:album.revision,role,
    ...mine?{}:{owner:{id:album.ownerId,name:ctx.db.accounts.id.find(album.ownerId)?.displayName??"Someone"}},
    shared:mine&&(first(ctx.db.photoAlbumMembers.albumId.filter(album.id))!==undefined||first(ctx.db.photoLinks.albumId.filter(album.id))!==undefined)};
}
/** A queued indexing job: the runtime reads the blob and reports facts back. */
function job(entry:Entry){return {fileId:entry.id,blobId:entry.blobId,name:entry.name,mediaType:entry.mediaType,modifiedAt:entry.modifiedAt,kind:photoKind(entry.mediaType,entry.name)!};}
function needsIndex(ctx:Ctx,entry:Entry){
  if(entry.kind!=="file"||entry.trashed||!entry.blobId||!photoKind(entry.mediaType,entry.name))return false;
  const asset=ctx.db.photoAssets.fileId.find(entry.id);return !asset||asset.blobId!==entry.blobId||asset.ownerId!==entry.ownerId||asset.indexVersion<photoIndexVersion;
}
/** Removes an index row with its album entries; a stack left empty goes too. */
function forget(ctx:Ctx,fileId:string){
  const row=ctx.db.photoAssets.fileId.find(fileId);
  for(const item of [...ctx.db.photoAlbumItems.fileId.filter(fileId)])deleteAlbumItem(ctx,item.id);
  for(const item of [...ctx.db.photoLinkItems.fileId.filter(fileId)])ctx.db.photoLinkItems.id.delete(item.id);
  if(!row)return;deleteAsset(ctx,fileId);
  if(row.stackId&&first(ctx.db.photoAssets.stackId.filter(row.stackId))===undefined)deleteStack(ctx,row.stackId);
}
function leaveStack(ctx:Ctx,row:Asset){
  if(!row.stackId)return row;const next={...row,stackId:""};updateAsset(ctx,next);
  if(first(ctx.db.photoAssets.stackId.filter(row.stackId))===undefined)deleteStack(ctx,row.stackId);
  return next;
}

/** Duplicate groups: photos with the same perceptual hash, shape and kind, outside the trash and Locked folder. */
function duplicateGroups(ctx:Ctx,r:Reader,userId:string,cursor:string,maxGroups:number){
  const groups:PhotoDuplicateGroup[]=[];let scanned=0,current="",rows:Asset[]=[],last="";
  const flush=()=>{
    if(!current||rows.length<2)return;
    const members=rows.map(asset=>({asset,entry:r.fileOf(asset)})).filter((m):m is {asset:Asset;entry:Entry}=>!!m.entry&&r.stateOf(m.entry)==="library"&&!(m.asset.kind==="video"&&r.livePartner(m.asset,m.entry)));
    // A RAW+JPEG pair or a burst already grouped in a stack is not a duplicate of itself.
    if(new Set(members.map(m=>m.asset.stackId||m.asset.fileId)).size<2)return;
    const digests=new Set(members.map(m=>ctx.db.uploads.id.find(m.entry.blobId)?.digest||m.entry.blobId));
    groups.push({key:current,identical:digests.size===1,items:members.sort((a,b)=>b.entry.size-a.entry.size||newest(a.asset,b.asset)).map(m=>({...r.expose(m.asset,m.entry,"library"),path:r.folderPath(m.entry)}))});
  };
  for(const row of ctx.db.photoAssets.dupKey.filter(ownerRange(userId,cursor||undefined))){
    if(row.dupKey!==current){flush();if(groups.length>=maxGroups)return {groups,cursor:last};current=row.dupKey;rows=[];}
    rows.push(row);last=row.dupKey;
    if(++scanned>50000)return {groups,cursor:""};
  }
  flush();return {groups,cursor:""};
}

export function photosOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>,files:PhotoFiles){
  if(!appEnabled(ctx,"photos"))fail("notFound");
  const now=Number(ctx.timestamp.toMillis()),r=reader(ctx,userId);
  /** Someone else's photo is visible while it is in the library and in an album you own or that is shared with you. */
  const viaAlbum=(fileId:string)=>{for(const item of ctx.db.photoAlbumItems.fileId.filter(fileId)){const a=ctx.db.photoAlbums.id.find(item.albumId);if(a&&(a.ownerId===userId||ctx.db.photoAlbumMembers.id.find(`${a.id}/${userId}`)))return true;}return false;};
  /** An owned asset in one of the allowed places; Locked folder items need an unlocked session. With shared, also others' photos reachable through albums. */
  const find=(value:unknown,allowed:State[]=["library","locked"],shared=false)=>{
    const row=ctx.db.photoAssets.fileId.find(opaque(value));if(!row)fail("notFound");
    const entry=r.fileOf(row);if(!entry)fail("notFound");const state=r.stateOf(entry);
    if(row.ownerId!==userId){if(!shared||state!=="library"||!viaAlbum(row.fileId))fail("notFound");return {row,entry,state,mine:false};}
    if(state==="locked"&&allowed.includes("locked")&&!files.unlocked)fail("lockedFolder");
    if(!allowed.includes(state))fail("notFound");return {row,entry,state,mine:true};
  };
  /** An album you own, or one shared with you; "add" needs editor rights and "own" ownership. */
  const album=(value:unknown,need:"view"|"add"|"own"="own")=>{
    const row=ctx.db.photoAlbums.id.find(opaque(value));if(!row)fail("notFound");
    const role=row.ownerId===userId?"owner":ctx.db.photoAlbumMembers.id.find(`${row.id}/${userId}`)?.role;if(!role)fail("notFound");
    if(need==="own"&&role!=="owner"||need==="add"&&role==="viewer")fail("forbidden");return row;
  };
  const sealedLink=(row:NonNullable<ReturnType<typeof ctx.db.photoLinks.id.find>>)=>{
    const items=row.albumId?albumRows(ctx,r,ctx.db.photoAlbums.id.find(row.albumId)!).length:[...ctx.db.photoLinkItems.linkId.filter(row.id)].length;
    return {id:row.id,albumId:row.albumId,albumName:row.albumId?ctx.db.photoAlbums.id.find(row.albumId)?.name??"":"",count:items,sealed:row.sealed,expiresAt:row.expiresAt,hasPassword:!!row.passwordVerifier,
      allowDownload:row.allowDownload,allowUpload:row.allowUpload,showMetadata:row.showMetadata,createdAt:row.createdAt,revision:row.revision};
  };
  const link=(value:unknown)=>{const row=ctx.db.photoLinks.id.find(opaque(value));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const linkSettings=(q:Record<string,unknown>,albumLink:boolean)=>{
    const flag=(key:string)=>{if(q[key]!==undefined&&typeof q[key]!=="boolean")fail("invalidInput");return q[key] as boolean|undefined;};
    const expiresAt=q.expiresAt===undefined?undefined:num(q.expiresAt,0,now+photoLinkLimits.expiryDays*86400_000);if(expiresAt&&expiresAt<=now)fail("invalidInput");
    const passwordVerifier=q.passwordVerifier===undefined?undefined:text(q.passwordVerifier,300,true);if(passwordVerifier&&!passwordVerifier.startsWith("scrypt$"))fail("invalidInput");
    const allowUpload=flag("allowUpload");if(allowUpload&&!albumLink)fail("invalidInput");
    return {expiresAt,passwordVerifier,allowDownload:flag("allowDownload"),allowUpload,showMetadata:flag("showMetadata")};
  };
  const dropLink=(id:string)=>{for(const item of [...ctx.db.photoLinkItems.linkId.filter(id)])ctx.db.photoLinkItems.id.delete(item.id);for(const pass of [...ctx.db.photoLinkPasses.linkId.filter(id)])ctx.db.photoLinkPasses.digest.delete(pass.digest);ctx.db.photoLinks.id.delete(id);};
  const touch=(row:Album)=>{const next={...row,updatedAt:now,revision:row.revision+1};updateAlbum(ctx,next);return next;};
  /** The selected photos plus the motion halves of selected Live Photos. */
  const withPartners=(rows:{row:Asset;entry:Entry}[])=>{
    const all=new Map(rows.map(x=>[x.row.fileId,x]));
    for(const {row,entry} of rows){const partner=r.livePartner(row,entry),pe=partner&&r.fileOf(partner);if(partner&&pe&&!all.has(partner.fileId))all.set(partner.fileId,{row:partner,entry:pe});}
    return [...all.values()];
  };
  const fresh=(id:string)=>ctx.db.fileEntries.id.find(id)!;
  switch(operation){
    case "overview":{
      keys(p,[]);const counts={library:0,favorites:0,videos:0,archive:0,trash:0,duplicates:0,shared:0};let scanned=0;
      for(const row of ctx.db.photoAssets.ownerId.filter(userId)){if(++scanned>200000)break;const entry=r.fileOf(row);if(!entry||r.stateOf(entry)!=="library"||!r.shown(row,entry,"library"))continue;
        if(!row.archived)counts.library++;if(row.favorite)counts.favorites++;if(row.kind==="video"&&!row.archived)counts.videos++;if(row.archived)counts.archive++;}
      scanned=0;for(const entry of ctx.db.fileEntries.listKey.filter(prefixRange(`${userId}/trash/`))){if(++scanned>20000)break;const row=entry.kind==="file"?ctx.db.photoAssets.fileId.find(entry.id):undefined;if(row&&row.ownerId===userId&&r.fileOf(row)&&!(row.kind==="video"&&r.livePartner(row,entry)))counts.trash++;}
      counts.duplicates=duplicateGroups(ctx,r,userId,"",1000).groups.length;
      const order=(a:Album,b:Album)=>b.updatedAt-a.updatedAt||a.name.localeCompare(b.name);
      const albums=[...ctx.db.photoAlbums.ownerId.filter(userId)].sort(order).map(row=>exposeAlbum(ctx,r,row,userId));
      const sharedAlbums=[...ctx.db.photoAlbumMembers.userId.filter(userId)].map(m=>ctx.db.photoAlbums.id.find(m.albumId)).filter((a):a is Album=>!!a&&!!ctx.db.accounts.id.find(a.ownerId)?.enabled).sort(order).map(row=>exposeAlbum(ctx,r,row,userId));
      counts.shared=sharedAlbums.length;
      return {albums,sharedAlbums,counts,indexing:ctx.db.photoState.name.find("scan")?.value!=="idle",uploadFolder:files.uploadFolder()};
    }
    case "page":{
      keys(p,["view","album","cursor","limit"]);const view=text(p.view,20);if(!["library","favorites","videos","archive","album","trash","locked"].includes(view))fail("invalidInput");
      const limit=int(p.limit??120,1,500),cursor=p.cursor===undefined||p.cursor===""?"":text(p.cursor,300);const items:PhotoAsset[]=[];
      if(view==="album"||view==="trash"){
        const offset=cursor?int(Number(cursor),0,200000):0;let all:{asset:Asset;entry:Entry}[];
        if(view==="album")all=albumRows(ctx,r,album(p.album,"view")).sort((x,y)=>newest(x.asset,y.asset));
        else{
          // Files keeps trashed items until they are deleted; Photos lists the media among them, newest deletion first.
          all=[];let scanned=0;
          for(const entry of ctx.db.fileEntries.listKey.filter(prefixRange(`${userId}/trash/`))){if(++scanned>20000)break;const asset=entry.kind==="file"?ctx.db.photoAssets.fileId.find(entry.id):undefined;if(asset&&asset.ownerId===userId&&r.fileOf(asset)&&!(asset.kind==="video"&&r.livePartner(asset,entry)))all.push({asset,entry});}
          all.sort((x,y)=>y.entry.modifiedAt-x.entry.modifiedAt||newest(x.asset,y.asset));
        }
        for(const {asset,entry} of all.slice(offset,offset+limit))items.push(r.expose(asset,entry));
        return {items,cursor:offset+limit<all.length?String(offset+limit):""};
      }
      const state:State=view==="locked"?"locked":"library";if(state==="locked"&&!files.unlocked)fail("lockedFolder");
      if(cursor&&!cursor.startsWith(userId+"/"))fail("invalidInput");let last="",scanned=0;
      for(const row of ctx.db.photoAssets.timeKey.filter(ownerRange(userId,cursor||undefined))){
        last=row.timeKey;if(++scanned>20000)return {items,cursor:last};
        const entry=r.fileOf(row);if(!entry||r.stateOf(entry)!==state||!inView(view,row)||!r.shown(row,entry,state))continue;
        items.push(r.expose(row,entry,state));if(items.length>=limit)return {items,cursor:last};
      }
      return {items,cursor:""};
    }
    case "detail":{
      keys(p,["id"]);const {row,entry,state,mine}=find(p.id,["library","locked","trash"],true);
      const albums=[...ctx.db.photoAlbumItems.fileId.filter(row.fileId)].map(item=>ctx.db.photoAlbums.id.find(item.albumId)).filter((a):a is Album=>!!a&&(a.ownerId===userId||!!ctx.db.photoAlbumMembers.id.find(`${a.id}/${userId}`))).map(a=>({id:a.id,name:a.name}));
      const exif=exifOf(row);
      return {...r.expose(row,entry,state),description:row.description,exif,latitude:row.hasLocation?row.latitude:null,longitude:row.hasLocation?row.longitude:null,albums,parentId:mine?entry.parentId:"",modifiedAt:entry.modifiedAt,locked:state==="locked",stackId:mine?row.stackId:"",checksum:mine?row.sha1:""};
    }
    case "asset":{keys(p,["id"]);const {row,entry}=find(p.id,["library","locked","trash"],true);return {id:row.fileId,revision:entry.revision,blobId:entry.blobId,indexedBlobId:row.blobId,kind:row.kind,mediaType:entry.mediaType,name:entry.name,thumbnail:row.thumbnail,playback:row.playback,audioCodec:exifOf(row).audioCodec??"",duration:row.duration,size:entry.size};}
    case "indexJob":{
      // The owner's own new upload; anything else waits for the background scan.
      keys(p,["id"]);const entry=ctx.db.fileEntries.id.find(opaque(p.id));if(!entry||entry.ownerId!==userId)fail("notFound");return {job:needsIndex(ctx,entry)?job(entry):null};
    }
    case "update":{
      keys(p,["ids","favorite","archived"]);if(p.favorite===undefined&&p.archived===undefined)fail("invalidInput");
      if(p.favorite!==undefined&&typeof p.favorite!=="boolean"||p.archived!==undefined&&typeof p.archived!=="boolean")fail("invalidInput");
      const rows=ids(p.ids).map(id=>find(id).row);
      for(const row of rows)updateAsset(ctx,{...row,favorite:p.favorite===undefined?row.favorite:p.favorite as boolean,archived:p.archived===undefined?row.archived:p.archived as boolean,revision:row.revision+1});
      return {updated:rows.length};
    }
    case "describe":{
      keys(p,["id","description","revision"]);const {row,entry,state}=find(p.id);if(int(p.revision)!==row.revision)fail("conflict");
      const next={...row,description:text(p.description,photoLimits.description,true).trim(),revision:row.revision+1};updateAsset(ctx,next);return r.expose(next,entry,state);
    }
    case "trash":{
      // Locked items cannot be trashed (Files deletes them permanently instead).
      keys(p,["ids"]);const rows=withPartners(ids(p.ids).map(id=>find(id,["library"])));
      for(const {row} of rows)files.trash(fresh(row.fileId));
      return {trashed:rows.map(x=>x.row.fileId)};
    }
    case "restore":{
      keys(p,["ids"]);const rows=withPartners(ids(p.ids).map(id=>find(id,["trash"])));
      for(const {row} of rows)files.restore(fresh(row.fileId));
      return {restored:rows.map(x=>x.row.fileId)};
    }
    case "delete":{
      keys(p,["ids"]);const rows=withPartners(ids(p.ids).map(id=>find(id,["trash","locked"])));
      for(const {row} of rows){files.remove(fresh(row.fileId));forget(ctx,row.fileId);}
      return {deleted:rows.map(x=>x.row.fileId)};
    }
    case "emptyTrash":{
      // Bounded per call; the browser repeats while more remain.
      keys(p,[]);let deleted=0,scanned=0;
      for(const entry of [...ctx.db.fileEntries.listKey.filter(prefixRange(`${userId}/trash/`))]){
        if(++scanned>20000||deleted>=200)return {deleted,remaining:true};
        const asset=entry.kind==="file"?ctx.db.photoAssets.fileId.find(entry.id):undefined;
        if(!asset||asset.ownerId!==userId||!r.fileOf(asset))continue;
        files.remove(fresh(entry.id));forget(ctx,entry.id);deleted++;
      }
      return {deleted,remaining:false};
    }
    case "lock":{
      if(!files.unlocked)fail("lockedFolder");
      keys(p,["ids"]);const rows=withPartners(ids(p.ids).map(id=>find(id,["library"]))),folder=files.lockedFolder(),moved:PhotoMoved[]=[];
      for(const {row,entry} of rows){moved.push({id:row.fileId,parentId:entry.parentId});files.move(fresh(row.fileId),folder);}
      return {moved};
    }
    case "unlock":{
      // Back to where they came from (Undo) or to the Photos folder by capture month.
      if(!files.unlocked)fail("lockedFolder");
      keys(p,["ids","to"]);const rows=withPartners(ids(p.ids).map(id=>find(id,["locked"])));
      const to=new Map<string,string>();
      if(p.to!==undefined){if(!Array.isArray(p.to)||p.to.length>photoLimits.batch)fail("invalidInput");for(const item of p.to as Record<string,unknown>[]){if(!item||typeof item!=="object")fail("invalidInput");keys(item,["id","parentId"]);to.set(opaque(item.id),item.parentId===""?"":opaque(item.parentId));}}
      for(const {row} of rows){const d=new Date(row.takenAt);files.move(fresh(row.fileId),to.get(row.fileId)??files.monthFolder(d.getUTCFullYear(),d.getUTCMonth()+1));}
      return {moved:rows.map(x=>x.row.fileId)};
    }
    case "stack":{
      keys(p,["ids","primaryId"]);const list=ids(p.ids,photoLimits.stackItems);if(list.length<2)fail("invalidInput");
      const rows=list.map(id=>find(id)),state=rows[0].state;if(rows.some(x=>x.state!==state))fail("invalidInput");
      const primaryId=p.primaryId===undefined?list[0]:opaque(p.primaryId);if(!list.includes(primaryId))fail("invalidInput");
      const id=newId(ctx);insertStack(ctx,{id,ownerId:userId,key:"",primaryId,createdAt:now,updatedAt:now});
      for(const {row} of rows)updateAsset(ctx,{...leaveStack(ctx,ctx.db.photoAssets.fileId.find(row.fileId)!),stackId:id});
      return {id,primaryId,count:rows.length};
    }
    case "stackDetail":{
      keys(p,["id"]);const stack=ctx.db.photoStacks.id.find(opaque(p.id));if(!stack||stack.ownerId!==userId)fail("notFound");
      const cover=ctx.db.photoAssets.fileId.find(stack.primaryId)??first(ctx.db.photoAssets.stackId.filter(stack.id));if(!cover)fail("notFound");
      const {state}=find(cover.fileId);const members=[...ctx.db.photoAssets.stackId.filter(stack.id)].map(asset=>({asset,entry:r.fileOf(asset)})).filter((m):m is {asset:Asset;entry:Entry}=>!!m.entry&&r.stateOf(m.entry)===state).sort((a,b)=>newest(a.asset,b.asset));
      const info=r.stackOf(members[0]?.asset??cover,state);
      return {id:stack.id,primaryId:info?.primaryId??cover.fileId,items:members.map(m=>r.expose(m.asset,m.entry,state))};
    }
    case "stackCover":{
      keys(p,["id","primaryId"]);const stack=ctx.db.photoStacks.id.find(opaque(p.id));if(!stack||stack.ownerId!==userId)fail("notFound");
      const {row}=find(p.primaryId);if(row.stackId!==stack.id)fail("invalidInput");
      updateStack(ctx,{...stack,primaryId:row.fileId,updatedAt:now});return {id:stack.id,primaryId:row.fileId};
    }
    case "unstack":{
      keys(p,["id","ids"]);const stack=ctx.db.photoStacks.id.find(opaque(p.id));if(!stack||stack.ownerId!==userId)fail("notFound");
      const only=p.ids===undefined?undefined:new Set(ids(p.ids));let removed=0;
      for(const row of [...ctx.db.photoAssets.stackId.filter(stack.id)])if(!only||only.has(row.fileId)){leaveStack(ctx,row);removed++;}
      return {removed};
    }
    case "duplicates":{
      keys(p,["cursor"]);const cursor=p.cursor===undefined||p.cursor===""?"":text(p.cursor,400);if(cursor&&!cursor.startsWith(userId+"/"))fail("invalidInput");
      return duplicateGroups(ctx,r,userId,cursor,20);
    }
    case "resolveDuplicates":{
      // Keeps the chosen photos, gives them the others' albums, favorite and description, and trashes the rest.
      keys(p,["key","keep"]);const key=text(p.key,400);if(!key.startsWith(userId+"/"))fail("invalidInput");
      const members=[...ctx.db.photoAssets.dupKey.filter(key)].map(row=>({row,entry:r.fileOf(row)})).filter((m):m is {row:Asset;entry:Entry}=>!!m.entry&&r.stateOf(m.entry)==="library");
      const keep=ids(p.keep);if(keep.some(id=>!members.some(m=>m.row.fileId===id)))fail("conflict");
      const others=members.filter(m=>!keep.includes(m.row.fileId)),target=ctx.db.photoAssets.fileId.find(keep[0])!;
      let next={...target};
      for(const {row} of others){
        next={...next,favorite:next.favorite||row.favorite,description:next.description||row.description};
        for(const item of ctx.db.photoAlbumItems.fileId.filter(row.fileId)){const id=`${item.albumId}/${target.fileId}`;if(!ctx.db.photoAlbumItems.id.find(id))insertAlbumItem(ctx,{id,albumId:item.albumId,fileId:target.fileId,addedAt:now});}
      }
      if(next.favorite!==target.favorite||next.description!==target.description)updateAsset(ctx,{...next,revision:target.revision+1});
      const trashed=withPartners(others);for(const {row} of trashed)files.trash(fresh(row.fileId));
      return {trashed:trashed.map(x=>x.row.fileId),kept:keep};
    }
    case "dismissDuplicates":{
      keys(p,["key"]);const key=text(p.key,400);if(!key.startsWith(userId+"/"))fail("invalidInput");let dismissed=0;
      for(const row of [...ctx.db.photoAssets.dupKey.filter(key)])if(row.ownerId===userId){updateAsset(ctx,{...row,dupKey:""});dismissed++;}
      return {dismissed};
    }
    case "createAlbum":{
      keys(p,["id","name","ids"]);const id=opaque(p.id),name=text(p.name,photoLimits.albumName).trim();
      if(ctx.db.photoAlbums.id.find(id))fail("alreadyExists");if([...ctx.db.photoAlbums.ownerId.filter(userId)].length>=photoLimits.albums)fail("quotaExceeded");
      const row=insertAlbum(ctx,{id,ownerId:userId,name,description:"",coverId:"",createdAt:now,updatedAt:now,revision:1});
      if(p.ids!==undefined)for(const fileId of ids(p.ids)){find(fileId,["library"]);insertAlbumItem(ctx,{id:`${id}/${fileId}`,albumId:id,fileId,addedAt:now});}
      return exposeAlbum(ctx,r,row,userId);
    }
    case "editAlbum":{
      keys(p,["id","name","description","coverId","revision"]);const row=album(p.id);if(int(p.revision)!==row.revision)fail("conflict");
      const coverId=p.coverId===undefined?row.coverId:p.coverId===""?"":find(p.coverId,["library"],true).row.fileId;
      const next={...row,name:p.name===undefined?row.name:text(p.name,photoLimits.albumName).trim(),description:p.description===undefined?row.description:text(p.description,photoLimits.description,true).trim(),coverId,updatedAt:now,revision:row.revision+1};
      updateAlbum(ctx,next);return exposeAlbum(ctx,r,next,userId);
    }
    case "deleteAlbum":{
      // People it was shared with lose it and its links stop working; the photos stay.
      keys(p,["id"]);const row=album(p.id);
      for(const item of [...ctx.db.photoAlbumItems.albumId.filter(row.id)])deleteAlbumItem(ctx,item.id);
      for(const member of [...ctx.db.photoAlbumMembers.albumId.filter(row.id)])deleteAlbumMember(ctx,member.id);
      for(const row2 of [...ctx.db.photoLinks.albumId.filter(row.id)])dropLink(row2.id);
      deleteAlbum(ctx,row.id);return {deleted:true};
    }
    case "addToAlbum":{
      // Editors of a shared album add their own photos.
      keys(p,["id","ids"]);const row=album(p.id,"add"),list=ids(p.ids);let added=0;
      let total=[...ctx.db.photoAlbumItems.albumId.filter(row.id)].length;
      for(const fileId of list){find(fileId,["library"]);if(ctx.db.photoAlbumItems.id.find(`${row.id}/${fileId}`))continue;if(++total>photoLimits.albumItems)fail("quotaExceeded");insertAlbumItem(ctx,{id:`${row.id}/${fileId}`,albumId:row.id,fileId,addedAt:now});added++;}
      return {...exposeAlbum(ctx,r,touch(row),userId),added};
    }
    case "removeFromAlbum":{
      // The owner removes anything; an editor removes only their own photos.
      keys(p,["id","ids"]);const row=album(p.id,"add"),owner=row.ownerId===userId;let removed=0;
      for(const fileId of ids(p.ids)){
        if(!ctx.db.photoAlbumItems.id.find(`${row.id}/${fileId}`))continue;
        if(!owner&&ctx.db.photoAssets.fileId.find(fileId)?.ownerId!==userId)fail("forbidden");
        deleteAlbumItem(ctx,`${row.id}/${fileId}`);removed++;
      }
      const next=touch(row.coverId&&!ctx.db.photoAlbumItems.id.find(`${row.id}/${row.coverId}`)?{...row,coverId:""}:row);return {...exposeAlbum(ctx,r,next,userId),removed};
    }
    case "albumSharing":{
      keys(p,["id"]);const row=album(p.id);
      const members=[...ctx.db.photoAlbumMembers.albumId.filter(row.id)].map(m=>({m,a:ctx.db.accounts.id.find(m.userId)})).filter(x=>!!x.a).map(({m,a})=>({userId:m.userId,username:a!.username,displayName:a!.displayName,role:m.role==="editor"?"editor":"viewer"})).sort((a,b)=>a.displayName.localeCompare(b.displayName));
      return {members,links:[...ctx.db.photoLinks.albumId.filter(row.id)].sort((a,b)=>a.createdAt-b.createdAt).map(sealedLink)};
    }
    case "shareAlbum":{
      keys(p,["id","username","role"]);const row=album(p.id),role=text(p.role,10);if(role!=="viewer"&&role!=="editor")fail("invalidInput");
      const person=ctx.db.accounts.username.find(text(p.username,64).trim().replace(/^@/,"").toLowerCase());
      if(!person?.enabled||person.id===userId||ctx.db.directoryProfiles.userId.find(person.id)?.deleted)fail("notFound");
      const id=`${row.id}/${person.id}`,existing=ctx.db.photoAlbumMembers.id.find(id);
      if(existing)updateAlbumMember(ctx,{...existing,role});
      else{if([...ctx.db.photoAlbumMembers.albumId.filter(row.id)].length>=photoLinkLimits.members)fail("quotaExceeded");insertAlbumMember(ctx,{id,albumId:row.id,userId:person.id,role,addedAt:now});}
      touch(row);return {userId:person.id,username:person.username,displayName:person.displayName,role};
    }
    case "unshareAlbum":{
      // The owner removes anyone; a member may leave.
      keys(p,["id","userId"]);const row=album(p.id,"view"),target=opaque(p.userId);
      if(row.ownerId!==userId&&target!==userId)fail("forbidden");
      if(!ctx.db.photoAlbumMembers.id.find(`${row.id}/${target}`))fail("notFound");
      deleteAlbumMember(ctx,`${row.id}/${target}`);return {removed:true};
    }
    case "links":{keys(p,[]);return {links:[...ctx.db.photoLinks.ownerId.filter(userId)].sort((a,b)=>b.createdAt-a.createdAt).map(sealedLink)};}
    case "createLink":{
      // To one of your albums, or to photos of yours (a fixed set).
      keys(p,["id","digest","sealed","albumId","ids","expiresAt","passwordVerifier","allowDownload","allowUpload","showMetadata"]);
      const id=opaque(p.id),digest=text(p.digest,64);if(!/^[a-f0-9]{64}$/.test(digest)||ctx.db.photoLinks.id.find(id)||ctx.db.photoLinks.digest.find(digest))fail("conflict");
      if([...ctx.db.photoLinks.ownerId.filter(userId)].length>=photoLinkLimits.links)fail("quotaExceeded");
      if((p.albumId===undefined)===(p.ids===undefined))fail("invalidInput");
      const albumId=p.albumId===undefined?"":album(p.albumId).id,files=p.ids===undefined?[]:ids(p.ids).map(f=>find(f,["library"]).row.fileId);
      const s=linkSettings(p,!!albumId);
      const row={id,ownerId:userId,digest,sealed:text(p.sealed,2000),albumId,expiresAt:s.expiresAt??0,passwordVerifier:s.passwordVerifier??"",allowDownload:s.allowDownload??true,allowUpload:s.allowUpload??false,showMetadata:s.showMetadata??true,createdAt:now,revision:1};
      ctx.db.photoLinks.insert(row);for(const fileId of files)ctx.db.photoLinkItems.insert({id:`${id}/${fileId}`,linkId:id,fileId});
      return sealedLink(row);
    }
    case "editLink":{
      keys(p,["id","revision","expiresAt","passwordVerifier","allowDownload","allowUpload","showMetadata"]);const row=link(p.id);if(int(p.revision)!==row.revision)fail("conflict");
      const s=linkSettings(p,!!row.albumId);
      const next={...row,expiresAt:s.expiresAt??row.expiresAt,passwordVerifier:s.passwordVerifier??row.passwordVerifier,allowDownload:s.allowDownload??row.allowDownload,allowUpload:s.allowUpload??row.allowUpload,showMetadata:s.showMetadata??row.showMetadata,revision:row.revision+1};
      ctx.db.photoLinks.id.update(next);
      // A new or removed password signs visitors out.
      if(s.passwordVerifier!==undefined)for(const pass of [...ctx.db.photoLinkPasses.linkId.filter(row.id)])ctx.db.photoLinkPasses.digest.delete(pass.digest);
      return sealedLink(next);
    }
    case "deleteLink":{keys(p,["id"]);dropLink(link(p.id).id);return {deleted:true};}
  }
  fail("notFound");
}

const shape=(width:number,height:number)=>width>height*1.05?"l":height>width*1.05?"p":"s";
/** RAW+JPEG pairs stack with the JPEG/HEIC as cover; burst frames share one stack per BurstUUID. */
function autoStack(ctx:Ctx,row:Omit<Asset,"syncSeq"|"syncKey"|"sha1">,entry:Entry,burstId:string,now:number):string{
  if(row.stackId||row.kind!=="image")return row.stackId;
  if(burstId){
    const key=`${row.ownerId}/burst:${burstId}`,stack=first(ctx.db.photoStacks.key.filter(key));if(stack)return stack.id;
    const id=newId(ctx);insertStack(ctx,{id,ownerId:row.ownerId,key,primaryId:row.fileId,createdAt:now,updatedAt:now});return id;
  }
  const {stem,ext}=nameParts(entry.name),raw=rawExtensions.includes(ext);
  if(!raw&&!stillExtensions.includes(ext))return "";
  for(const other of raw?stillExtensions:rawExtensions){
    const sibling=first(ctx.db.fileEntries.listKey.filter(`${entry.ownerId}/${entry.parentId}/${`${stem}.${other}`.toLowerCase()}`)),pair=sibling&&ctx.db.photoAssets.fileId.find(sibling.id);
    if(!pair||pair.ownerId!==row.ownerId||pair.kind!=="image")continue;
    if(pair.stackId)return pair.stackId;
    const id=newId(ctx);insertStack(ctx,{id,ownerId:row.ownerId,key:"",primaryId:raw?pair.fileId:row.fileId,createdAt:now,updatedAt:now});
    updateAsset(ctx,{...pair,stackId:id});return id;
  }
  return "";
}

/** Service-only indexing: bounded scans of Files for media that needs facts, and storing them. */
export function photosWorker(ctx:Ctx,operation:string,p:Record<string,unknown>){
  const now=Number(ctx.timestamp.toMillis());
  if(operation==="scan"){
    keys(p,["limit","jobs"]);const limit=int(p.limit,1,20000),max=int(p.jobs,1,100);
    if(!appEnabled(ctx,"photos"))return {jobs:[],done:true};
    const state=ctx.db.photoState.name.find("cursor"),cursor=state?.value??"",jobs=[];let last="",scanned=0;
    const range=new Range<string>(cursor?{tag:"excluded",value:cursor}:{tag:"unbounded"},{tag:"unbounded"});
    for(const entry of ctx.db.fileEntries.listKey.filter(range)){last=entry.listKey;if(needsIndex(ctx,entry))jobs.push(job(entry));if(++scanned>=limit||jobs.length>=max)break;}
    const done=scanned<limit&&jobs.length<max;
    // Remove index rows for files that no longer exist; metadata of trashed files is kept for restore.
    const sweep=ctx.db.photoState.name.find("sweep")?.value??"";let swept="",checked=0;
    const batch:Asset[]=[];
    for(const row of ctx.db.photoAssets.timeKey.filter(new Range<string>(sweep?{tag:"excluded",value:sweep}:{tag:"unbounded"},{tag:"unbounded"}))){batch.push(row);if(batch.length>=2000)break;}
    for(const row of batch){
      swept=row.timeKey;checked++;const entry=ctx.db.fileEntries.id.find(row.fileId);
      if(!entry||entry.ownerId!==row.ownerId||entry.kind!=="file")forget(ctx,row.fileId);
    }
    const put=(name:string,value:string)=>{if(ctx.db.photoState.name.find(name))ctx.db.photoState.name.update({name,value});else ctx.db.photoState.insert({name,value});};
    put("cursor",done?"":last);put("sweep",checked>=2000?swept:"");put("scan",done&&!jobs.length?"idle":"busy");
    if(done)pruneTombstones(ctx,now-31*86400_000);
    return {jobs,done};
  }
  if(operation==="store"){
    keys(p,["fileId","blobId","facts","thumbnail"]);const entry=ctx.db.fileEntries.id.find(opaque(p.fileId));
    if(!entry||entry.blobId!==text(p.blobId,200)||!photoKind(entry.mediaType,entry.name))return {stored:false};
    const f=p.facts as Record<string,unknown>;if(!f||typeof f!=="object")fail("invalidInput");
    const takenAt=num(f.takenAt,0,8.64e15),width=int(f.width),height=int(f.height),duration=num(f.duration,0,1e9);
    const latitude=f.latitude===null?null:num(f.latitude,-90,90),longitude=f.longitude===null?null:num(f.longitude,-180,180);
    const exif=JSON.stringify(f.exif??{});if(exif.length>4000)fail("invalidInput");
    const phash=f.phash===""?"":text(f.phash,16);if(phash&&!/^[0-9a-f]{16}$/.test(phash))fail("invalidInput");
    const playback=f.playback===""?"":text(f.playback,20);if(playback&&!["original","remux","transcode"].includes(playback))fail("invalidInput");
    const contentId=f.contentId===""?"":text(f.contentId,200),burstId=f.burstId===""?"":text(f.burstId,200);
    const sha1=f.sha1===undefined||f.sha1===""?"":text(f.sha1,28);if(sha1&&!/^[A-Za-z0-9+/]{27}=$/.test(sha1))fail("invalidInput");
    const thumbnail=p.thumbnail==="failed"?"failed":"ready",old=ctx.db.photoAssets.fileId.find(entry.id);
    if(old&&old.ownerId!==entry.ownerId)forget(ctx,old.fileId);
    const keep=old&&old.ownerId===entry.ownerId?ctx.db.photoAssets.fileId.find(old.fileId):undefined;
    const kind=photoKind(entry.mediaType,entry.name)!;
    // "Not duplicates" stays dismissed while the picture is the same.
    const dupKey=!phash||keep&&keep.dupKey===""&&keep.phash===phash?"":`${entry.ownerId}/${kind}:${phash}:${shape(width,height)}${kind==="video"?":"+Math.round(duration):""}`;
    const row={fileId:entry.id,ownerId:entry.ownerId,blobId:entry.blobId,timeKey:photoTimeKey(entry.ownerId,takenAt,entry.id),kind,takenAt,width,height,duration,exif,
      hasLocation:latitude!==null&&longitude!==null,latitude:latitude??0,longitude:longitude??0,favorite:keep?.favorite??false,archived:keep?.archived??false,description:keep?.description??"",thumbnail,revision:(keep?.revision??0)+1,indexedAt:now,
      indexVersion:photoIndexVersion,phash,dupKey,playback:kind==="video"?playback:"",liveKey:contentId?`${entry.ownerId}/${contentId}`:"",stackId:keep?.stackId??"",sha1};
    if(keep)updateAsset(ctx,row);else insertAsset(ctx,row);
    const stackId=autoStack(ctx,row,entry,burstId,now);if(stackId!==row.stackId)updateAsset(ctx,{...row,stackId});
    return {stored:true};
  }
  fail("notFound");
}

export function photosSearch(ctx:Ctx,userId:string,matches:(value:string)=>boolean,visit:()=>boolean,add:(result:{id:string;title:string;detail:string;href:string;thumbnail:string})=>void){
  const r=reader(ctx);
  const shared=[...ctx.db.photoAlbumMembers.userId.filter(userId)].map(m=>ctx.db.photoAlbums.id.find(m.albumId)).filter((a):a is Album=>!!a);
  for(const album of [...ctx.db.photoAlbums.ownerId.filter(userId),...shared]){if(!visit())return;if(matches(album.name)||matches(album.description))add({id:`album-${album.id}`,title:album.name,detail:album.ownerId===userId?"Photos album":"Shared Photos album",href:`/photos?album=${album.id}`,thumbnail:""});}
  for(const row of ctx.db.photoAssets.ownerId.filter(userId)){
    if(!visit())return;const entry=r.fileOf(row);if(!entry||r.stateOf(entry)!=="library")continue;const exif=exifOf(row);
    if(matches(entry.name)||matches(row.description)||matches(`${exif.make??""} ${exif.model??""}`))
      add({id:row.fileId,title:entry.name,detail:[new Date(row.takenAt).toISOString().slice(0,10),row.description].filter(Boolean).join(" · "),href:`/photos?asset=${row.fileId}`,thumbnail:`/api/v1/photos/assets/${row.fileId}/thumbnail`});
  }
}
export function deletePhotosAccount(ctx:Ctx,ownerId:string){
  for(const album of [...ctx.db.photoAlbums.ownerId.filter(ownerId)]){
    for(const item of [...ctx.db.photoAlbumItems.albumId.filter(album.id)])deleteAlbumItem(ctx,item.id);
    for(const member of [...ctx.db.photoAlbumMembers.albumId.filter(album.id)])deleteAlbumMember(ctx,member.id);
    deleteAlbum(ctx,album.id);
  }
  for(const member of [...ctx.db.photoAlbumMembers.userId.filter(ownerId)])deleteAlbumMember(ctx,member.id);
  for(const link of [...ctx.db.photoLinks.ownerId.filter(ownerId)]){
    for(const item of [...ctx.db.photoLinkItems.linkId.filter(link.id)])ctx.db.photoLinkItems.id.delete(item.id);
    for(const pass of [...ctx.db.photoLinkPasses.linkId.filter(link.id)])ctx.db.photoLinkPasses.digest.delete(pass.digest);
    ctx.db.photoLinks.id.delete(link.id);
  }
  for(const row of [...ctx.db.photoAssets.ownerId.filter(ownerId)])forget(ctx,row.fileId);
  for(const stack of [...ctx.db.photoStacks.ownerId.filter(ownerId)])deleteStack(ctx,stack.id);
  for(const s of [...ctx.db.immichSessions.userId.filter(ownerId)]){for(const c of [...ctx.db.immichCheckpoints.immichId.filter(s.id)])ctx.db.immichCheckpoints.id.delete(c.id);ctx.db.immichSessions.digest.delete(s.digest);}
  ctx.db.immichConnections.userId.delete(ownerId);for(const r of [...ctx.db.immichRemote.userId.filter(ownerId)])ctx.db.immichRemote.id.delete(r.id);
}
export function photosSnapshot(ctx:Ctx){
  return {assets:[...ctx.db.photoAssets.iter()].map(({timeKey,...row})=>row),albums:[...ctx.db.photoAlbums.iter()],items:[...ctx.db.photoAlbumItems.iter()],stacks:[...ctx.db.photoStacks.iter()],
    members:[...ctx.db.photoAlbumMembers.iter()],links:[...ctx.db.photoLinks.iter()],linkItems:[...ctx.db.photoLinkItems.iter()],
    immichConnections:[...ctx.db.immichConnections.iter()],immichRemote:[...ctx.db.immichRemote.iter()]};
}
/** V8 backups carry Photos metadata; derived images are regenerated. Stacks, sharing and phase 2 columns are optional. */
export function restorePhotos(ctx:Ctx,value?:Partial<ReturnType<typeof photosSnapshot>>){
  if(!value)return;
  if(!Array.isArray(value.assets)||!Array.isArray(value.albums)||!Array.isArray(value.items)||[value.stacks,value.members,value.links,value.linkItems,value.immichConnections,value.immichRemote].some(v=>v!==undefined&&!Array.isArray(v)))fail("invalidInput");
  for(const row of value.stacks??[]){if(!ctx.db.accounts.id.find(row.ownerId))fail("invalidInput");opaque(row.id);insertStack(ctx,row);}
  for(const row of value.assets){
    if(!ctx.db.accounts.id.find(row.ownerId))fail("invalidInput");opaque(row.fileId);
    const full=Object.assign({indexVersion:1,phash:"",dupKey:"",playback:"",liveKey:"",stackId:""},row);if(full.stackId&&!ctx.db.photoStacks.id.find(full.stackId))full.stackId="";
    insertAsset(ctx,{...full,timeKey:photoTimeKey(row.ownerId,num(row.takenAt,0,8.64e15),row.fileId)});
  }
  for(const row of value.albums){if(!ctx.db.accounts.id.find(row.ownerId))fail("invalidInput");opaque(row.id);insertAlbum(ctx,row);}
  for(const row of value.items){const album=ctx.db.photoAlbums.id.find(row.albumId);if(!album||row.id!==`${row.albumId}/${row.fileId}`)fail("invalidInput");insertAlbumItem(ctx,row);}
  for(const row of value.members??[]){if(!ctx.db.photoAlbums.id.find(row.albumId)||!ctx.db.accounts.id.find(row.userId)||row.id!==`${row.albumId}/${row.userId}`)fail("invalidInput");insertAlbumMember(ctx,row);}
  for(const row of value.links??[]){if(!ctx.db.accounts.id.find(row.ownerId)||row.albumId&&!ctx.db.photoAlbums.id.find(row.albumId))fail("invalidInput");opaque(row.id);ctx.db.photoLinks.insert(row);}
  for(const row of value.immichConnections??[]){if(!ctx.db.accounts.id.find(row.userId))fail("invalidInput");ctx.db.immichConnections.insert(row);}
  for(const row of value.immichRemote??[]){if(!ctx.db.accounts.id.find(row.userId))fail("invalidInput");ctx.db.immichRemote.insert(row);}
  for(const row of value.linkItems??[]){if(!ctx.db.photoLinks.id.find(row.linkId)||row.id!==`${row.linkId}/${row.fileId}`)fail("invalidInput");ctx.db.photoLinkItems.insert(row);}
}

type Link=NonNullable<ReturnType<Ctx["db"]["photoLinks"]["id"]["find"]>>;
/**
 * A shared link by the digest of its token: unexpired, owner enabled, Photos on.
 * passed is whether the visitor's pass cookie (digest) satisfies a link password.
 */
export function openPhotoLink(ctx:Ctx,digest:string,pass:string){
  const now=Number(ctx.timestamp.toMillis()),link=/^[a-f0-9]{64}$/.test(digest)?ctx.db.photoLinks.digest.find(digest):undefined;
  if(!link||link.expiresAt&&link.expiresAt<=now||!ctx.db.accounts.id.find(link.ownerId)?.enabled||!appEnabled(ctx,"photos"))fail("notFound");
  const row=pass&&/^[a-f0-9]{64}$/.test(pass)?ctx.db.photoLinkPasses.digest.find(pass):undefined;
  return {link,passed:!link.passwordVerifier||!!row&&row.linkId===link.id&&row.expiresAt>now};
}
/** The photos a link shows: the album's (anyone's, as in the album) or the chosen ones, in the library, newest first. */
function linkRows(ctx:Ctx,r:Reader,link:Link){
  if(link.albumId){const album=ctx.db.photoAlbums.id.find(link.albumId);return album?albumRows(ctx,r,album).sort((x,y)=>newest(x.asset,y.asset)):[];}
  const rows:{asset:Asset;entry:Entry}[]=[];
  for(const item of ctx.db.photoLinkItems.linkId.filter(link.id)){const asset=ctx.db.photoAssets.fileId.find(item.fileId),entry=asset&&r.fileOf(asset);if(asset&&entry&&asset.ownerId===link.ownerId&&r.stateOf(entry)==="library")rows.push({asset,entry});}
  return rows.sort((x,y)=>newest(x.asset,y.asset));
}
/** Visitor operations of a shared link (no account). Uploads are handled with Files in index.ts. */
export function photosLinkOperation(ctx:Ctx,digest:string,pass:string,operation:string,p:Record<string,unknown>){
  const now=Number(ctx.timestamp.toMillis()),{link,passed}=openPhotoLink(ctx,digest,pass),r=reader(ctx);
  if(operation==="verifier"){keys(p,[]);return {verifier:link.passwordVerifier};}
  if(operation==="grantPass"){
    // After the runtime checked the password: a pass for half a day, at most 1,000 per link.
    keys(p,["digest"]);const value=text(p.digest,64);if(!/^[a-f0-9]{64}$/.test(value)||!link.passwordVerifier)fail("invalidInput");
    const passes=[...ctx.db.photoLinkPasses.linkId.filter(link.id)];for(const old of passes)if(old.expiresAt<=now)ctx.db.photoLinkPasses.digest.delete(old.digest);
    if(passes.filter(x=>x.expiresAt>now).length>=1000)fail("operationTooLarge");
    ctx.db.photoLinkPasses.insert({digest:value,linkId:link.id,expiresAt:now+photoLinkLimits.passwordDays*86400_000});return {granted:true};
  }
  if(operation==="info"&&!passed)return {needsPassword:true};
  if(!passed)fail("forbidden");
  const rows=linkRows(ctx,r,link),album=link.albumId?ctx.db.photoAlbums.id.find(link.albumId):undefined;
  const item=(value:unknown)=>{const id=opaque(value),row=rows.find(x=>x.asset.fileId===id);if(!row)fail("notFound");return row;};
  const plain=(asset:Asset,entry:Entry)=>{const value=r.expose(asset,entry,"library");value.favorite=false;value.archived=false;delete value.stack;return value;};
  switch(operation){
    case "info":{keys(p,[]);return {needsPassword:false,title:album?.name??(rows.length===1?rows[0].entry.name:`${rows.length} photos`),description:album?.description??"",owner:ctx.db.accounts.id.find(link.ownerId)?.displayName??"",
      count:rows.length,album:!!album,expiresAt:link.expiresAt,allowDownload:link.allowDownload,allowUpload:link.allowUpload&&!!album,showMetadata:link.showMetadata};}
    case "page":{
      keys(p,["cursor","limit"]);const offset=p.cursor===undefined||p.cursor===""?0:int(Number(p.cursor),0,200000),limit=int(p.limit??120,1,500);
      return {items:rows.slice(offset,offset+limit).map(x=>plain(x.asset,x.entry)),cursor:offset+limit<rows.length?String(offset+limit):""};
    }
    case "detail":{
      keys(p,["id"]);const {asset,entry}=item(p.id),base={id:asset.fileId,name:entry.name,kind:asset.kind==="video"?"video":"image",width:asset.width,height:asset.height,duration:asset.duration};
      return link.showMetadata?{...base,takenAt:asset.takenAt,description:asset.description,exif:exifOf(asset),latitude:asset.hasLocation?asset.latitude:null,longitude:asset.hasLocation?asset.longitude:null}:base;
    }
    case "asset":{keys(p,["id"]);const {asset,entry}=item(p.id);return {id:asset.fileId,revision:entry.revision,blobId:entry.blobId,kind:asset.kind,mediaType:entry.mediaType,name:entry.name,playback:asset.playback,audioCodec:exifOf(asset).audioCodec??"",duration:asset.duration,size:entry.size,allowDownload:link.allowDownload};}
  }
  fail("notFound");
}
/** Adds a visitor's finished upload to the link's album. */
export function addLinkUpload(ctx:Ctx,link:Link,fileId:string){
  if(!link.albumId||!link.allowUpload)fail("forbidden");
  const now=Number(ctx.timestamp.toMillis()),id=`${link.albumId}/${fileId}`;
  if(!ctx.db.photoAlbumItems.id.find(id))insertAlbumItem(ctx,{id,albumId:link.albumId,fileId,addedAt:now});
}
/** Operations an app password with Photos read access may use; the rest need write. */
export const photosReadOperations=new Set(["overview","page","detail","asset","duplicates","stackDetail","albumSharing"]);
