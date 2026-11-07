import {SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
type Resource=NonNullable<ReturnType<Ctx["db"]["pimResources"]["id"]["find"]>>;
function fail(code:string):never{throw new SenderError(code);}
function changed(ctx:Ctx,userId:string,id:string,action:string){const old=ctx.db.pimChanges.userId.find(userId),row={userId,revision:(old?.revision??0)+1};if(old)ctx.db.pimChanges.userId.update(row);else ctx.db.pimChanges.insert(row);let eventId=ctx.newUuidV4().toString();while(ctx.db.activity.id.find(eventId))eventId=ctx.newUuidV4().toString();ctx.db.activity.insert({id:eventId,ownerId:userId,fileId:id,action,at:Number(ctx.timestamp.toMillis())});}
function notify(ctx:Ctx,collectionId:string){const c=ctx.db.pimCollections.id.find(collectionId)!;for(const id of new Set([c.ownerId,...[...ctx.db.pimGrants.collectionId.filter(c.id)].map(g=>g.userId)])){const old=ctx.db.pimChanges.userId.find(id),row={userId:id,revision:(old?.revision??0)+1};if(old)ctx.db.pimChanges.userId.update(row);else ctx.db.pimChanges.insert(row);}ctx.db.pimCollections.id.update({...c,revision:c.revision+1,modifiedAt:Number(ctx.timestamp.toMillis())});}
export function trashNote(ctx:Ctx,row:Resource){const c=ctx.db.pimCollections.id.find(row.collectionId)!;if(c.kind!=="notes")fail("invalidInput");if(ctx.db.noteTrash.count()>=10000n)fail("operationTooLarge");ctx.db.noteTrash.insert({id:row.id,ownerId:c.ownerId,document:JSON.stringify(row),notebookName:c.name,trashedAt:Number(ctx.timestamp.toMillis())});}
export function noteTrashOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>){
  if(operation==="trash"){
    const rows=[...ctx.db.noteTrash.ownerId.filter(userId)].filter(row=>p.id?row.id===p.id:row.id>String(p.cursor??"")).sort((a,b)=>a.id.localeCompare(b.id));
    return {items:rows.slice(0,p.id?1:200).map(row=>({...row,resource:p.id?JSON.parse(row.document):{...JSON.parse(row.document),content:undefined},document:undefined})),nextCursor:!p.id&&rows.length>200?rows[199].id:""};
  }
  const old=ctx.db.noteTrash.id.find(String(p.id));if(!old||old.ownerId!==userId)fail("notFound");const row=JSON.parse(old.document) as Resource;if(p.revision!==row.revision)fail("conflict");
  if(operation==="purge"){ctx.db.noteTrash.id.delete(old.id);changed(ctx,userId,row.id,"notes.purged");return {ok:true};}
  if(operation!=="restore")fail("invalidInput");const c=ctx.db.pimCollections.id.find(String(p.collectionId||row.collectionId));if(!c||c.ownerId!==userId||c.kind!=="notes")fail("notFound");
  if(ctx.db.pimResources.hrefKey.find(`${c.id}/${row.href}`)||ctx.db.pimResources.uidKey.find(`${c.id}/${row.uid}`))fail("conflict");
  ctx.db.pimResources.insert({...row,collectionId:c.id,hrefKey:`${c.id}/${row.href}`,uidKey:`${c.id}/${row.uid}`,revision:row.revision+1,modifiedAt:Number(ctx.timestamp.toMillis())});ctx.db.noteTrash.id.delete(old.id);notify(ctx,c.id);changed(ctx,userId,row.id,"notes.restored");return ctx.db.pimResources.id.find(row.id);
}
/** Relationship validation runs with the write, so concurrent reparenting cannot create cycles. */
export function validateTaskParent(ctx:Ctx,cid:string,uid:string,projection:string){const data=JSON.parse(projection);if(data.kind!=="task")return;let parent=String(data.parentUid??""),depth=0;const seen=new Set([uid]);while(parent){if(seen.has(parent)||++depth>32)fail("invalidInput");seen.add(parent);const row=ctx.db.pimResources.uidKey.find(`${cid}/${parent}`);if(!row||JSON.parse(row.projection).kind!=="task")fail("invalidInput");parent=String(JSON.parse(row.projection).parentUid??"");}}
export function workspaceSnapshot(ctx:Ctx){return {trash:[...ctx.db.noteTrash.iter()]};}
export function restoreWorkspace(ctx:Ctx,value:ReturnType<typeof workspaceSnapshot>|undefined){for(const row of value?.trash??[]){if(!ctx.db.accounts.id.find(row.ownerId)||JSON.parse(row.document).id!==row.id)fail("invalidInput");ctx.db.noteTrash.insert(row);}for(const row of ctx.db.pimResources.iter())validateTaskParent(ctx,row.collectionId,row.uid,row.projection);}
