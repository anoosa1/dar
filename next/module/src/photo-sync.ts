import type {Ctx} from "./index.ts";

/**
 * Change tracking for Immich-compatible sync. Every Photos write takes the next value of
 * one sequence; deletions leave tombstones naming who may hear of them. A change that
 * cannot be expressed row by row (a folder of photos trashed or moved) raises a user's
 * epoch, and their mobile sessions start over with a full sync.
 */
type Asset=NonNullable<ReturnType<Ctx["db"]["photoAssets"]["fileId"]["find"]>>;
type Album=NonNullable<ReturnType<Ctx["db"]["photoAlbums"]["id"]["find"]>>;
type Item=NonNullable<ReturnType<Ctx["db"]["photoAlbumItems"]["id"]["find"]>>;
type Member=NonNullable<ReturnType<Ctx["db"]["photoAlbumMembers"]["id"]["find"]>>;
type Stack=NonNullable<ReturnType<Ctx["db"]["photoStacks"]["id"]["find"]>>;

function put(ctx:Ctx,name:string,value:string){if(ctx.db.photoState.name.find(name))ctx.db.photoState.name.update({name,value});else ctx.db.photoState.insert({name,value});}
export function currentSeq(ctx:Ctx){return Number(ctx.db.photoState.name.find("seq")?.value??0);}
export function nextSeq(ctx:Ctx){const seq=currentSeq(ctx)+1;put(ctx,"seq",String(seq));return seq;}
export const syncKey=(ownerId:string,seq:number)=>`${ownerId}/${String(seq).padStart(16,"0")}`;
export function epochOf(ctx:Ctx,userId:string){return Number(ctx.db.photoState.name.find(`epoch:${userId}`)?.value??0);}
export function raiseEpoch(ctx:Ctx,userIds:Iterable<string>){const seq=nextSeq(ctx);for(const id of new Set(userIds))put(ctx,`epoch:${id}`,String(seq));}
/** The owner and members of an album. */
export function albumAudience(ctx:Ctx,albumId:string){const album=ctx.db.photoAlbums.id.find(albumId);return album?[album.ownerId,...[...ctx.db.photoAlbumMembers.albumId.filter(albumId)].map(m=>m.userId)]:[];}
function tombstone(ctx:Ctx,kind:string,entityId:string,extraId:string,audience:string[]){
  const seq=nextSeq(ctx),key=String(seq).padStart(16,"0");ctx.db.photoSyncDeletes.insert({id:key,key,kind,entityId,extraId,audience:[...new Set(audience)].join(","),seq,at:Number(ctx.timestamp.toMillis())});
}

export function insertAsset(ctx:Ctx,row:Omit<Asset,"syncSeq"|"syncKey"|"sha1">&{sha1?:string}){const seq=nextSeq(ctx);return ctx.db.photoAssets.insert({sha1:"",...row,syncSeq:seq,syncKey:syncKey(row.ownerId,seq)});}
export function updateAsset(ctx:Ctx,row:Omit<Asset,"syncSeq"|"syncKey"|"sha1">&{sha1?:string}){const seq=nextSeq(ctx);return ctx.db.photoAssets.fileId.update({sha1:ctx.db.photoAssets.fileId.find(row.fileId)?.sha1??"",...row,syncSeq:seq,syncKey:syncKey(row.ownerId,seq)});}
export function deleteAsset(ctx:Ctx,fileId:string){const row=ctx.db.photoAssets.fileId.find(fileId);if(row)tombstone(ctx,"asset",fileId,"",[row.ownerId]);return ctx.db.photoAssets.fileId.delete(fileId);}
/** A file event in Files: the photo's visibility may have changed. */
export function touchAsset(ctx:Ctx,fileId:string){const row=ctx.db.photoAssets.fileId.find(fileId);if(row)updateAsset(ctx,row);}

export function insertAlbum(ctx:Ctx,row:Omit<Album,"syncSeq">){return ctx.db.photoAlbums.insert({...row,syncSeq:nextSeq(ctx)});}
export function updateAlbum(ctx:Ctx,row:Omit<Album,"syncSeq">){return ctx.db.photoAlbums.id.update({...row,syncSeq:nextSeq(ctx)});}
export function deleteAlbum(ctx:Ctx,id:string){tombstone(ctx,"album",id,"",albumAudience(ctx,id));return ctx.db.photoAlbums.id.delete(id);}
export function insertAlbumItem(ctx:Ctx,row:Omit<Item,"syncSeq">){return ctx.db.photoAlbumItems.insert({...row,syncSeq:nextSeq(ctx)});}
export function deleteAlbumItem(ctx:Ctx,id:string){const row=ctx.db.photoAlbumItems.id.find(id);if(row)tombstone(ctx,"albumToAsset",row.albumId,row.fileId,albumAudience(ctx,row.albumId));return ctx.db.photoAlbumItems.id.delete(id);}
/** A new member, or one who leaves, gets a full sync; the others hear of the membership row. */
export function insertAlbumMember(ctx:Ctx,row:Omit<Member,"syncSeq">){const result=ctx.db.photoAlbumMembers.insert({...row,syncSeq:nextSeq(ctx)});raiseEpoch(ctx,[row.userId]);return result;}
export function updateAlbumMember(ctx:Ctx,row:Omit<Member,"syncSeq">){return ctx.db.photoAlbumMembers.id.update({...row,syncSeq:nextSeq(ctx)});}
export function deleteAlbumMember(ctx:Ctx,id:string){
  const row=ctx.db.photoAlbumMembers.id.find(id);
  if(row){tombstone(ctx,"albumUser",row.albumId,row.userId,albumAudience(ctx,row.albumId));raiseEpoch(ctx,[row.userId]);}
  return ctx.db.photoAlbumMembers.id.delete(id);
}
export function insertStack(ctx:Ctx,row:Omit<Stack,"syncSeq">){return ctx.db.photoStacks.insert({...row,syncSeq:nextSeq(ctx)});}
export function updateStack(ctx:Ctx,row:Omit<Stack,"syncSeq">){return ctx.db.photoStacks.id.update({...row,syncSeq:nextSeq(ctx)});}
export function deleteStack(ctx:Ctx,id:string){const row=ctx.db.photoStacks.id.find(id);if(row)tombstone(ctx,"stack",id,"",[row.ownerId]);return ctx.db.photoStacks.id.delete(id);}

/**
 * Files calls this for every file event. A photo's own event touches its row; a folder
 * that is trashed, restored, moved or deleted may hide or show many photos at once, so its
 * owner and the members of their albums resync.
 */
export function photoFileEvent(ctx:Ctx,ownerId:string,fileId:string,action:string){
  const entry=ctx.db.fileEntries.id.find(fileId);
  if(entry?.kind==="folder"){
    if(!["trashed","restored","moved","deleted"].includes(action))return;
    const audience=[ownerId];for(const album of ctx.db.photoAlbums.ownerId.filter(ownerId))for(const member of ctx.db.photoAlbumMembers.albumId.filter(album.id))audience.push(member.userId);
    raiseEpoch(ctx,audience);return;
  }
  touchAsset(ctx,fileId);
}
/** Tombstones older than 31 days are dropped; sessions that old start over (see immich.ts). */
export function pruneTombstones(ctx:Ctx,before:number){
  let removed=0;for(const row of [...ctx.db.photoSyncDeletes.iter()]){if(row.at<before){ctx.db.photoSyncDeletes.id.delete(row.id);if(++removed>=5000)break;}}
}
