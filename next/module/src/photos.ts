import {SenderError,Range} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {appEnabled} from "./app-settings.ts";
import {photoKind,photoLimits,type PhotoAsset,type PhotoExif} from "../../shared/photos.ts";
type Asset=NonNullable<ReturnType<Ctx["db"]["photoAssets"]["fileId"]["find"]>>;
type Album=NonNullable<ReturnType<Ctx["db"]["photoAlbums"]["id"]["find"]>>;
type Entry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;

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
function ids(value:unknown){if(!Array.isArray(value)||!value.length||value.length>photoLimits.batch)fail("invalidInput");return [...new Set(value.map(opaque))];}
const ownerRange=(ownerId:string,after?:string)=>new Range<string>(after?{tag:"excluded",value:after}:{tag:"included",value:ownerId+"/"},{tag:"excluded",value:ownerId+"0"});
/** Newest first within an owner; ties keep file ID order. */
export const photoTimeKey=(ownerId:string,takenAt:number,fileId:string)=>`${ownerId}/${String(9e15-Math.max(0,Math.min(Math.round(takenAt),9e15-1))).padStart(16,"0")}/${fileId}`;

/** Only the owner's live, indexable files appear; trashed or replaced entries drop out. */
function live(ctx:Ctx,asset:Asset):Entry|undefined{
  const entry=ctx.db.fileEntries.id.find(asset.fileId);
  return entry&&entry.kind==="file"&&!entry.trashed&&entry.ownerId===asset.ownerId&&photoKind(entry.mediaType,entry.name)?entry:undefined;
}
function expose(asset:Asset,entry:Entry):PhotoAsset{
  return {id:asset.fileId,name:entry.name,kind:asset.kind==="video"?"video":"image",mediaType:entry.mediaType,size:entry.size,takenAt:asset.takenAt,width:asset.width,height:asset.height,duration:asset.duration,
    favorite:asset.favorite,archived:asset.archived,thumbnail:asset.blobId!==entry.blobId?"pending":asset.thumbnail==="failed"?"failed":"ready",revision:asset.revision};
}
function inView(view:string,asset:Asset){
  return view==="favorites"?asset.favorite:view==="videos"?asset.kind==="video"&&!asset.archived:view==="archive"?asset.archived:!asset.archived;
}
function albumCount(ctx:Ctx,album:Album){let n=0;for(const item of ctx.db.photoAlbumItems.albumId.filter(album.id)){const a=ctx.db.photoAssets.fileId.find(item.fileId);if(a&&live(ctx,a))n++;}return n;}
function exposeAlbum(ctx:Ctx,album:Album){
  let cover=album.coverId;if(!cover||!ctx.db.photoAssets.fileId.find(cover)){cover="";for(const item of ctx.db.photoAlbumItems.albumId.filter(album.id)){const a=ctx.db.photoAssets.fileId.find(item.fileId);if(a&&live(ctx,a)){cover=a.fileId;break;}}}
  return {id:album.id,name:album.name,description:album.description,count:albumCount(ctx,album),coverId:cover,updatedAt:album.updatedAt,revision:album.revision};
}
/** A queued indexing job: the runtime reads the blob and reports facts back. */
function job(entry:Entry){return {fileId:entry.id,blobId:entry.blobId,name:entry.name,mediaType:entry.mediaType,modifiedAt:entry.modifiedAt,kind:photoKind(entry.mediaType,entry.name)!};}
function needsIndex(ctx:Ctx,entry:Entry){
  if(entry.kind!=="file"||entry.trashed||!entry.blobId||!photoKind(entry.mediaType,entry.name))return false;
  const asset=ctx.db.photoAssets.fileId.find(entry.id);return !asset||asset.blobId!==entry.blobId||asset.ownerId!==entry.ownerId;
}



export function photosOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>){
  if(!appEnabled(ctx,"photos"))fail("notFound");
  const now=Number(ctx.timestamp.toMillis());
  const asset=(value:unknown)=>{const row=ctx.db.photoAssets.fileId.find(opaque(value));if(!row||row.ownerId!==userId)fail("notFound");const entry=live(ctx,row);if(!entry)fail("notFound");return {row,entry};};
  const album=(value:unknown)=>{const row=ctx.db.photoAlbums.id.find(opaque(value));if(!row||row.ownerId!==userId)fail("notFound");return row;};
  const touch=(row:Album)=>{const next={...row,updatedAt:now,revision:row.revision+1};ctx.db.photoAlbums.id.update(next);return next;};
  switch(operation){
    case "overview":{
      keys(p,[]);const counts={library:0,favorites:0,videos:0,archive:0};let scanned=0;
      for(const row of ctx.db.photoAssets.ownerId.filter(userId)){if(++scanned>200000)break;if(!live(ctx,row))continue;
        if(!row.archived)counts.library++;if(row.favorite)counts.favorites++;if(row.kind==="video"&&!row.archived)counts.videos++;if(row.archived)counts.archive++;}
      const albums=[...ctx.db.photoAlbums.ownerId.filter(userId)].sort((a,b)=>b.updatedAt-a.updatedAt||a.name.localeCompare(b.name)).map(row=>exposeAlbum(ctx,row));
      return {albums,counts,indexing:ctx.db.photoState.name.find("scan")?.value!=="idle"};
    }
    case "page":{
      keys(p,["view","album","cursor","limit"]);const view=text(p.view,20);if(!["library","favorites","videos","archive","album"].includes(view))fail("invalidInput");
      const limit=int(p.limit??120,1,500),cursor=p.cursor===undefined||p.cursor===""?"":text(p.cursor,300);const items:PhotoAsset[]=[];
      if(view==="album"){
        const row=album(p.album),offset=cursor?int(Number(cursor),0,photoLimits.albumItems):0;
        const all=[...ctx.db.photoAlbumItems.albumId.filter(row.id)].map(item=>ctx.db.photoAssets.fileId.find(item.fileId)).filter((a):a is Asset=>!!a&&a.ownerId===userId).map(a=>({a,e:live(ctx,a)})).filter(x=>!!x.e).sort((x,y)=>y.a.takenAt-x.a.takenAt||x.a.fileId.localeCompare(y.a.fileId));
        for(const {a,e} of all.slice(offset,offset+limit))items.push(expose(a,e!));
        return {items,cursor:offset+limit<all.length?String(offset+limit):""};
      }
      if(cursor&&!cursor.startsWith(userId+"/"))fail("invalidInput");let last="",scanned=0;
      for(const row of ctx.db.photoAssets.timeKey.filter(ownerRange(userId,cursor||undefined))){
        last=row.timeKey;if(++scanned>20000)return {items,cursor:last};
        const entry=live(ctx,row);if(!entry||!inView(view,row))continue;items.push(expose(row,entry));if(items.length>=limit)return {items,cursor:last};
      }
      return {items,cursor:""};
    }
    case "detail":{
      keys(p,["id"]);const {row,entry}=asset(p.id);
      const albums=[...ctx.db.photoAlbumItems.fileId.filter(row.fileId)].map(item=>ctx.db.photoAlbums.id.find(item.albumId)).filter((a):a is Album=>!!a&&a.ownerId===userId).map(a=>({id:a.id,name:a.name}));
      return {...expose(row,entry),description:row.description,exif:JSON.parse(row.exif||"{}") as PhotoExif,latitude:row.hasLocation?row.latitude:null,longitude:row.hasLocation?row.longitude:null,albums,parentId:entry.parentId,modifiedAt:entry.modifiedAt};
    }
    case "asset":{keys(p,["id"]);const {row,entry}=asset(p.id);return {id:row.fileId,blobId:entry.blobId,indexedBlobId:row.blobId,kind:row.kind,mediaType:entry.mediaType,name:entry.name,thumbnail:row.thumbnail};}
    case "indexJob":{
      // The owner's own new upload; anything else waits for the background scan.
      keys(p,["id"]);const entry=ctx.db.fileEntries.id.find(opaque(p.id));if(!entry||entry.ownerId!==userId)fail("notFound");return {job:needsIndex(ctx,entry)?job(entry):null};
    }
    case "update":{
      keys(p,["ids","favorite","archived"]);if(p.favorite===undefined&&p.archived===undefined)fail("invalidInput");
      if(p.favorite!==undefined&&typeof p.favorite!=="boolean"||p.archived!==undefined&&typeof p.archived!=="boolean")fail("invalidInput");
      const rows=ids(p.ids).map(id=>asset(id).row);
      for(const row of rows)ctx.db.photoAssets.fileId.update({...row,favorite:p.favorite===undefined?row.favorite:p.favorite as boolean,archived:p.archived===undefined?row.archived:p.archived as boolean,revision:row.revision+1});
      return {updated:rows.length};
    }
    case "describe":{
      keys(p,["id","description","revision"]);const {row,entry}=asset(p.id);if(int(p.revision)!==row.revision)fail("conflict");
      const next={...row,description:text(p.description,photoLimits.description,true).trim(),revision:row.revision+1};ctx.db.photoAssets.fileId.update(next);return expose(next,entry);
    }
    case "createAlbum":{
      keys(p,["id","name","ids"]);const id=opaque(p.id),name=text(p.name,photoLimits.albumName).trim();
      if(ctx.db.photoAlbums.id.find(id))fail("alreadyExists");if([...ctx.db.photoAlbums.ownerId.filter(userId)].length>=photoLimits.albums)fail("quotaExceeded");
      const row={id,ownerId:userId,name,description:"",coverId:"",createdAt:now,updatedAt:now,revision:1};ctx.db.photoAlbums.insert(row);
      if(p.ids!==undefined)for(const fileId of ids(p.ids)){asset(fileId);ctx.db.photoAlbumItems.insert({id:`${id}/${fileId}`,albumId:id,fileId,addedAt:now});}
      return exposeAlbum(ctx,row);
    }
    case "editAlbum":{
      keys(p,["id","name","description","coverId","revision"]);const row=album(p.id);if(int(p.revision)!==row.revision)fail("conflict");
      const coverId=p.coverId===undefined?row.coverId:p.coverId===""?"":asset(p.coverId).row.fileId;
      const next={...row,name:p.name===undefined?row.name:text(p.name,photoLimits.albumName).trim(),description:p.description===undefined?row.description:text(p.description,photoLimits.description,true).trim(),coverId,updatedAt:now,revision:row.revision+1};
      ctx.db.photoAlbums.id.update(next);return exposeAlbum(ctx,next);
    }
    case "deleteAlbum":{
      keys(p,["id"]);const row=album(p.id);for(const item of [...ctx.db.photoAlbumItems.albumId.filter(row.id)])ctx.db.photoAlbumItems.id.delete(item.id);ctx.db.photoAlbums.id.delete(row.id);return {deleted:true};
    }
    case "addToAlbum":{
      keys(p,["id","ids"]);const row=album(p.id),files=ids(p.ids);let added=0;
      let total=[...ctx.db.photoAlbumItems.albumId.filter(row.id)].length;
      for(const fileId of files){asset(fileId);if(ctx.db.photoAlbumItems.id.find(`${row.id}/${fileId}`))continue;if(++total>photoLimits.albumItems)fail("quotaExceeded");ctx.db.photoAlbumItems.insert({id:`${row.id}/${fileId}`,albumId:row.id,fileId,addedAt:now});added++;}
      return {...exposeAlbum(ctx,touch(row)),added};
    }
    case "removeFromAlbum":{
      keys(p,["id","ids"]);const row=album(p.id);let removed=0;
      for(const fileId of ids(p.ids)){if(ctx.db.photoAlbumItems.id.find(`${row.id}/${fileId}`)){ctx.db.photoAlbumItems.id.delete(`${row.id}/${fileId}`);removed++;}}
      const next=touch(row.coverId&&!ctx.db.photoAlbumItems.id.find(`${row.id}/${row.coverId}`)?{...row,coverId:""}:row);return {...exposeAlbum(ctx,next),removed};
    }
  }
  fail("notFound");
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
    for(const row of ctx.db.photoAssets.timeKey.filter(new Range<string>(sweep?{tag:"excluded",value:sweep}:{tag:"unbounded"},{tag:"unbounded"}))){
      swept=row.timeKey;const entry=ctx.db.fileEntries.id.find(row.fileId);
      if(!entry||entry.ownerId!==row.ownerId||entry.kind!=="file"){for(const item of [...ctx.db.photoAlbumItems.fileId.filter(row.fileId)])ctx.db.photoAlbumItems.id.delete(item.id);ctx.db.photoAssets.fileId.delete(row.fileId);}
      if(++checked>=2000)break;
    }
    const put=(name:string,value:string)=>{if(ctx.db.photoState.name.find(name))ctx.db.photoState.name.update({name,value});else ctx.db.photoState.insert({name,value});};
    put("cursor",done?"":last);put("sweep",checked>=2000?swept:"");put("scan",done&&!jobs.length?"idle":"busy");
    return {jobs,done};
  }
  if(operation==="store"){
    keys(p,["fileId","blobId","facts","thumbnail"]);const entry=ctx.db.fileEntries.id.find(opaque(p.fileId));
    if(!entry||entry.blobId!==text(p.blobId,200)||!photoKind(entry.mediaType,entry.name))return {stored:false};
    const f=p.facts as Record<string,unknown>;if(!f||typeof f!=="object")fail("invalidInput");
    const takenAt=num(f.takenAt,0,8.64e15),width=int(f.width),height=int(f.height),duration=num(f.duration,0,1e9);
    const latitude=f.latitude===null?null:num(f.latitude,-90,90),longitude=f.longitude===null?null:num(f.longitude,-180,180);
    const exif=JSON.stringify(f.exif??{});if(exif.length>4000)fail("invalidInput");
    const thumbnail=p.thumbnail==="failed"?"failed":"ready",old=ctx.db.photoAssets.fileId.find(entry.id);
    if(old&&old.ownerId!==entry.ownerId){for(const item of [...ctx.db.photoAlbumItems.fileId.filter(old.fileId)])ctx.db.photoAlbumItems.id.delete(item.id);ctx.db.photoAssets.fileId.delete(old.fileId);}
    const keep=old&&old.ownerId===entry.ownerId?old:undefined;
    const row={fileId:entry.id,ownerId:entry.ownerId,blobId:entry.blobId,timeKey:photoTimeKey(entry.ownerId,takenAt,entry.id),kind:photoKind(entry.mediaType,entry.name)!,takenAt,width,height,duration,exif,
      hasLocation:latitude!==null&&longitude!==null,latitude:latitude??0,longitude:longitude??0,favorite:keep?.favorite??false,archived:keep?.archived??false,description:keep?.description??"",thumbnail,revision:(keep?.revision??0)+1,indexedAt:now};
    if(keep)ctx.db.photoAssets.fileId.update(row);else ctx.db.photoAssets.insert(row);
    return {stored:true};
  }
  fail("notFound");
}

export function photosSearch(ctx:Ctx,userId:string,matches:(value:string)=>boolean,visit:()=>boolean,add:(result:{id:string;title:string;detail:string;href:string;thumbnail:string})=>void){
  for(const album of ctx.db.photoAlbums.ownerId.filter(userId)){if(!visit())return;if(matches(album.name)||matches(album.description))add({id:`album-${album.id}`,title:album.name,detail:"Photos album",href:`/photos?album=${album.id}`,thumbnail:""});}
  for(const row of ctx.db.photoAssets.ownerId.filter(userId)){
    if(!visit())return;const entry=live(ctx,row);if(!entry)continue;const exif=JSON.parse(row.exif||"{}") as PhotoExif;
    if(matches(entry.name)||matches(row.description)||matches(`${exif.make??""} ${exif.model??""}`))
      add({id:row.fileId,title:entry.name,detail:[new Date(row.takenAt).toISOString().slice(0,10),row.description].filter(Boolean).join(" · "),href:`/photos?asset=${row.fileId}`,thumbnail:`/api/v1/photos/assets/${row.fileId}/thumbnail`});
  }
}
export function deletePhotosAccount(ctx:Ctx,ownerId:string){
  for(const album of [...ctx.db.photoAlbums.ownerId.filter(ownerId)]){for(const item of [...ctx.db.photoAlbumItems.albumId.filter(album.id)])ctx.db.photoAlbumItems.id.delete(item.id);ctx.db.photoAlbums.id.delete(album.id);}
  for(const row of [...ctx.db.photoAssets.ownerId.filter(ownerId)]){for(const item of [...ctx.db.photoAlbumItems.fileId.filter(row.fileId)])ctx.db.photoAlbumItems.id.delete(item.id);ctx.db.photoAssets.fileId.delete(row.fileId);}
}
export function photosSnapshot(ctx:Ctx){
  return {assets:[...ctx.db.photoAssets.iter()].map(({timeKey,...row})=>row),albums:[...ctx.db.photoAlbums.iter()],items:[...ctx.db.photoAlbumItems.iter()]};
}
/** V8 backups carry Photos metadata; derived images are regenerated. */
export function restorePhotos(ctx:Ctx,value?:ReturnType<typeof photosSnapshot>){
  if(!value)return;
  if(!Array.isArray(value.assets)||!Array.isArray(value.albums)||!Array.isArray(value.items))fail("invalidInput");
  for(const row of value.assets){if(!ctx.db.accounts.id.find(row.ownerId))fail("invalidInput");opaque(row.fileId);ctx.db.photoAssets.insert({...row,timeKey:photoTimeKey(row.ownerId,num(row.takenAt,0,8.64e15),row.fileId)});}
  for(const row of value.albums){if(!ctx.db.accounts.id.find(row.ownerId))fail("invalidInput");opaque(row.id);ctx.db.photoAlbums.insert(row);}
  for(const row of value.items){const album=ctx.db.photoAlbums.id.find(row.albumId);if(!album||row.id!==`${row.albumId}/${row.fileId}`)fail("invalidInput");ctx.db.photoAlbumItems.insert(row);}
}
