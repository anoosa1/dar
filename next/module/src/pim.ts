import { Range, SenderError } from "spacetimedb/server";
import {passwordAllows} from "../../shared/app-scopes.ts";
import type { Ctx } from "./index.ts";
import {trashNote,validateTaskParent} from "./workspace.ts";
import {appEnabled} from "./app-settings.ts";
import type {ContactData} from "../../shared/pim.ts";
import type {NoteData} from "../../shared/workspace.ts";

function fail(code: string): never { throw new SenderError(code); }
function text(value: unknown, max = 255): string { if (typeof value !== "string" || !value || value.length > max || /[\u0000]/.test(value)) fail("invalidInput"); return value; }
function id(value: unknown) { const result = text(value, 36); if (!/^[a-f0-9-]{36}$/.test(result)) fail("invalidInput"); return result; }
function expected(actual: number, value: unknown) { if (value !== actual) fail("conflict"); }
function dueKey(at: number) { return String(Math.floor(at)).padStart(16,"0"); }
function bump(ctx: Ctx, userId: string) {
  const old = ctx.db.pimChanges.userId.find(userId), row = { userId, revision: (old?.revision ?? 0) + 1 };
  if (old) ctx.db.pimChanges.userId.update(row); else ctx.db.pimChanges.insert(row);
}
function notify(ctx: Ctx, collection: { id: string; ownerId: string }) {
  bump(ctx, collection.ownerId);
  for (const grant of ctx.db.pimGrants.collectionId.filter(collection.id)) bump(ctx, grant.userId);
}
function collection(ctx: Ctx, userId: string, value: unknown, write = false) {
  const row = ctx.db.pimCollections.id.find(id(value));
  if (!row) fail("notFound");
  if(row.kind==="notes"&&!appEnabled(ctx,"notes"))fail("notFound");
  const role = row.ownerId === userId ? "owner" : ctx.db.pimGrants.id.find(`${row.id}/${userId}`)?.role;
  if (!role) fail("notFound");
  if (write && role === "read") fail("forbidden");
  return { ...row, role };
}
function invalidateReminders(ctx: Ctx, resourceId: string) {
  ctx.db.pimReminderCursors.resourceId.delete(resourceId);
  for (const reminder of ctx.db.pimReminders.resourceId.filter(resourceId)) ctx.db.pimReminders.id.delete(reminder.id);
}
function log(ctx: Ctx, ownerId: string, resourceId: string, action: string) {
  let eventId = ctx.newUuidV4().toString();
  while (ctx.db.activity.id.find(eventId)) eventId = ctx.newUuidV4().toString();
  ctx.db.activity.insert({ id: eventId, ownerId, fileId: resourceId, action, at: Number(ctx.timestamp.toMillis()) });
}

/** Runs only behind the owner-credential boundary, for a freshly checked session. */
export function pimOperation(ctx: Ctx, userId: string, operation: string, p: Record<string, unknown>, protocolScope?: string, batch=false) {
  const at = Number(ctx.timestamp.toMillis());
  const allowsKind = (kind: string) => !protocolScope || passwordAllows(protocolScope,kind,"read") || passwordAllows(protocolScope,kind,"write");
  if (protocolScope && !["collections", "list", "read", "put", "delete"].includes(operation)) fail("forbidden");
  if(operation === "resource") {
    const row=ctx.db.pimResources.id.find(String(p.id));if(!row)fail("notFound");
    collection(ctx,userId,row.collectionId);return row;
  }
  if(operation === "search") {
    if(p.kind!=="notes"&&p.kind!=="contacts")fail("invalidInput");
    const collections=pimOperation(ctx,userId,"collections",{}) as ReturnType<typeof collection>[];
    const groups=new Set<string>(),q=String(p.q??"").normalize("NFC").toLowerCase(),cursor=String(p.cursor??"");
    const results:{key:string;row:object}[]=[];
    for(const c of collections){
      if(c.kind!==p.kind||p.collection&&c.id!==p.collection)continue;
      for(const row of ctx.db.pimResources.collectionId.filter(c.id)){
        const data=JSON.parse(row.projection) as ContactData|NoteData,contact="displayName" in data;
        if(contact)for(const group of data.groups)groups.add(group);
        if(q&&!`${row.projection}\n${contact?"":row.content}`.normalize("NFC").toLowerCase().includes(q))continue;
        if(p.favorite&&!data.favorite||p.tag&&!(contact?data.groups:data.tags).includes(String(p.tag)))continue;
        const name=contact?(p.sort==="lastName"?data.lastName:data.firstName)||data.displayName:data.title;
        const key=!contact&&p.sort!=="title"?`${String(9999999999999-row.modifiedAt).padStart(13,"0")}\u0000${row.id}`:`${name.normalize("NFC").toLowerCase()}\u0000${row.id}`;
        if(key<=cursor)continue;
        let lo=0,hi=results.length;while(lo<hi){const mid=(lo+hi)>>>1;if(results[mid].key<key)lo=mid+1;else hi=mid;}
        if(lo>200)continue;
        const {content,...metadata}=row;
        results.splice(lo,0,{key,row:{...metadata,data,role:c.role,...contact?{bookName:c.name}:{collectionName:c.name,snippet:content.slice(0,180)}}});
        if(results.length>201)results.pop();
      }
    }
    return {items:results.slice(0,200).map(r=>r.row),groups:[...groups].sort(),nextCursor:results.length>200?results[199].key:""};
  }
  if (operation === "collections") {
    const rows = [...ctx.db.pimCollections.ownerId.filter(userId)].map(row => ({ ...row, role: "owner" }));
    for (const grant of ctx.db.pimGrants.userId.filter(userId)) {
      const row = ctx.db.pimCollections.id.find(grant.collectionId);
      if (row) rows.push({ ...row, role: grant.role });
    }
    if (rows.length > 200) fail("operationTooLarge");
    return rows.filter(row => allowsKind(row.kind)&&(row.kind!=="notes"||appEnabled(ctx,"notes")));
  }
  if (operation === "createCollection") {
    const kind = text(p.kind);
    if(kind==="notes"&&!appEnabled(ctx,"notes"))fail("notFound");
    if (!["contacts", "calendar", "notes"].includes(kind)) fail("invalidInput");
    if ([...ctx.db.pimCollections.ownerId.filter(userId)].length >= 100) fail("operationTooLarge");
    const color = text(p.color, 7); if (!/^#[0-9a-f]{6}$/i.test(color)) fail("invalidInput");
    const row = { id: id(p.id), ownerId: userId, kind, name: text(text(p.name).trim()), color, revision: 1, modifiedAt: at };
    ctx.db.pimCollections.insert(row); notify(ctx, row); log(ctx, userId, row.id, `${kind}.collectionCreated`); return row;
  }
  if (operation === "preferences") {
    return ctx.db.pimPreferences.userId.find(userId) ?? { userId, value: "{}", revision: 0 };
  }
  if (operation === "savePreferences") {
    const old = ctx.db.pimPreferences.userId.find(userId); expected(old?.revision ?? 0, p.revision);
    const row = { userId, value: text(p.value, 16384), revision: (old?.revision ?? 0) + 1 };
    if (old) ctx.db.pimPreferences.userId.update(row); else ctx.db.pimPreferences.insert(row);
    bump(ctx, userId); return row;
  }
  if (operation === "reminders") return [...ctx.db.pimReminders.userId.filter(userId)].filter(row => !row.acknowledged && row.dueAt <= at).slice(0, 200);
  if (operation === "acknowledge") {
    const reminder = ctx.db.pimReminders.id.find(text(p.id));
    if (!reminder || reminder.userId !== userId) fail("notFound");
    ctx.db.pimReminders.id.update({ ...reminder, acknowledged: true }); bump(ctx, userId); return { ok: true };
  }
  const c = collection(ctx, userId, p.collectionId, !["list", "read", "grants", "page"].includes(operation));
  if (!allowsKind(c.kind)) fail("forbidden");
  if(operation === "page") {
    const cursor=String(p.cursor??"");
    const items=[];
    for(const row of ctx.db.pimResources.byCollectionHref.filter([c.id,new Range(cursor?{tag:"excluded",value:cursor}:{tag:"unbounded"},{tag:"unbounded"})])){items.push(row);if(items.length===201)break;}
    return {items:items.slice(0,200),nextCursor:items.length>200?items[199].href:""};
  }
  const metadataOnly=operation==="list" && p.metadataOnly===true;
  if (protocolScope && !passwordAllows(protocolScope,c.kind,["list","read"].includes(operation)&&!metadataOnly?"read":"write")) fail("forbidden");
  if (operation === "list") {
    const rows = [];
    for (const row of ctx.db.pimResources.collectionId.filter(c.id)) { if (rows.length >= 10000) fail("operationTooLarge"); rows.push(row); }
    return { collection: c, items: metadataOnly ? rows.map(({content,projection,...row})=>({...row,content:"",projection:""})) : rows };
  }
  if (["renameCollection", "deleteCollection", "share", "revoke", "grants"].includes(operation)) {
    if (c.role !== "owner") fail("forbidden");
    if (operation === "grants") return [...ctx.db.pimGrants.collectionId.filter(c.id)].map(row => ({ ...row, username: ctx.db.accounts.id.find(row.userId)?.username }));
    expected(c.revision, p.revision);
    if (operation === "share") {
      const user = ctx.db.accounts.username.find(text(p.username).toLowerCase()), role = text(p.role);
      if (!user?.enabled || user.id === userId || !["read", "write"].includes(role)) fail("invalidInput");
      const grant = { id: `${c.id}/${user.id}`, collectionId: c.id, userId: user.id, role };
      if (ctx.db.pimGrants.id.find(grant.id)) ctx.db.pimGrants.id.update(grant); else ctx.db.pimGrants.insert(grant);
      bump(ctx, user.id);
    } else if (operation === "revoke") {
      const grant = ctx.db.pimGrants.id.find(text(p.id));
      if (!grant || grant.collectionId !== c.id) fail("notFound");
      ctx.db.pimGrants.id.delete(grant.id); bump(ctx, grant.userId);
    } else if (operation === "deleteCollection") {
      notify(ctx, c);
      let count = 0;
      for (const row of ctx.db.pimResources.collectionId.filter(c.id)) { if (++count > 10000) fail("operationTooLarge"); if(c.kind==="notes")trashNote(ctx,row);invalidateReminders(ctx, row.id); ctx.db.pimResources.id.delete(row.id); }
      for (const grant of ctx.db.pimGrants.collectionId.filter(c.id)) ctx.db.pimGrants.id.delete(grant.id);
      ctx.db.pimCollections.id.delete(c.id); log(ctx, userId, c.id, `${c.kind}.collectionDeleted`); return { ok: true };
    }
    const { role: _role, ...base } = c;
    const color = p.color === undefined ? c.color : text(p.color, 7); if (!/^#[0-9a-f]{6}$/i.test(color)) fail("invalidInput");
    const updated = { ...base, name: operation === "renameCollection" ? text(text(p.name).trim()) : c.name, color, revision: c.revision + 1, modifiedAt: at };
    ctx.db.pimCollections.id.update(updated); notify(ctx, c); log(ctx, userId, c.id, `${c.kind}.${operation}`); return updated;
  }
  const href = text(p.href, 255);
  if (!/^[a-zA-Z0-9_.-]+\.(vcf|ics|md)$/.test(href)||!href.endsWith(c.kind==="contacts"?".vcf":c.kind==="notes"?".md":".ics")) fail("invalidInput");
  const old = ctx.db.pimResources.hrefKey.find(`${c.id}/${href}`);
  if (operation === "read") { if (!old) fail("notFound"); return old; }
  if(operation==="move"){
    if(!batch||!old)fail("invalidInput");expected(old.revision,p.revision);const target=collection(ctx,userId,p.targetCollectionId,true);if(target.kind!==c.kind||target.ownerId!==c.ownerId)fail("forbidden");if(target.id===c.id)return old;
    if(ctx.db.pimResources.uidKey.find(`${target.id}/${old.uid}`)||ctx.db.pimResources.hrefKey.find(`${target.id}/${href}`))fail("conflict");
    const moved={...old,collectionId:target.id,hrefKey:`${target.id}/${href}`,uidKey:`${target.id}/${old.uid}`,content:p.content===undefined?old.content:text(p.content,262144),projection:p.projection===undefined?old.projection:text(p.projection,65536),revision:old.revision+1,modifiedAt:at};
    invalidateReminders(ctx,old.id);ctx.db.pimResources.id.delete(old.id);ctx.db.pimResources.insert(moved);validateTaskParent(ctx,target.id,old.uid,moved.projection);if(target.kind==="calendar")ctx.db.pimReminderCursors.insert({resourceId:old.id,revision:moved.revision,at,runAt:dueKey(at+1000)});
    for(const item of [c,target]){const {role:_role,...base}=item;ctx.db.pimCollections.id.update({...base,revision:item.revision+1,modifiedAt:at});notify(ctx,item);}log(ctx,c.ownerId,old.id,`${c.kind}.moved`);return moved;
  }
  if (operation !== "put" && operation !== "delete") fail("invalidInput");
  expected(old?.revision ?? 0, p.revision);
  if (operation === "delete") {
    if (!old) fail("notFound");
    if(c.kind==="notes")trashNote(ctx,old);
    if(JSON.parse(old.projection).kind==="task")for(const child of ctx.db.pimResources.collectionId.filter(c.id))if(JSON.parse(child.projection).parentUid===old.uid)fail("conflict");
    invalidateReminders(ctx, old.id); ctx.db.pimResources.id.delete(old.id);
  } else {
    const uid = text(p.uid, 1024), uidKey = `${c.id}/${uid}`, duplicate = ctx.db.pimResources.uidKey.find(uidKey);
    if(old && old.uid!==uid)fail("invalidInput");
    if(old && c.kind==="calendar" && (JSON.parse(old.projection).kind==="task")!==(JSON.parse(text(p.projection,65536)).kind==="task"))fail("invalidInput");
    if (duplicate && duplicate.id !== old?.id) fail("conflict");
    if (!old && [...ctx.db.pimResources.collectionId.filter(c.id)].length >= 10000) fail("operationTooLarge");
    if(typeof p.content!=="string"||p.content.length>262144||p.content.includes("\u0000"))fail("invalidInput");
    const row = { id: old?.id ?? id(p.id), collectionId: c.id, href, hrefKey: `${c.id}/${href}`, uid, uidKey, content: c.kind==="notes"?p.content:text(p.content, 262144), projection: text(p.projection, 65536), revision: (old?.revision ?? 0) + 1, modifiedAt: at };
    validateTaskParent(ctx,c.id,uid,row.projection);
    const task=JSON.parse(row.projection);if(task.kind==="task"&&task.status==="COMPLETED")for(const child of ctx.db.pimResources.collectionId.filter(c.id)){const data=JSON.parse(child.projection);if(data.parentUid===uid&&!["COMPLETED","CANCELLED"].includes(data.status))fail("conflict");}
    if (old) { invalidateReminders(ctx, old.id); ctx.db.pimResources.id.update(row); } else ctx.db.pimResources.insert(row);
    if (c.kind === "calendar") ctx.db.pimReminderCursors.insert({ resourceId: row.id, revision: row.revision, at, runAt: dueKey(at + 1000) });
  }
  const { role: _role, ...base } = c;
  ctx.db.pimCollections.id.update({ ...base, revision: c.revision + 1, modifiedAt: at });
  notify(ctx, c); log(ctx, c.ownerId, old?.id ?? String(p.id), `${c.kind}.${operation}`);
  return operation === "delete" ? { ok: true } : ctx.db.pimResources.hrefKey.find(`${c.id}/${href}`);
}

export function pimSnapshot(ctx: Ctx) {
  return { collections: [...ctx.db.pimCollections.iter()], resources: [...ctx.db.pimResources.iter()], grants: [...ctx.db.pimGrants.iter()], preferences: [...ctx.db.pimPreferences.iter()], reminders: [...ctx.db.pimReminders.iter()], cursors: [...ctx.db.pimReminderCursors.iter()] };
}
type Snapshot = ReturnType<typeof pimSnapshot>;
export function restorePim(ctx: Ctx, data: Snapshot | undefined) {
  if (!data) return;
  for (const row of data.collections) {
    if (!ctx.db.accounts.id.find(row.ownerId) || !["contacts", "calendar", "notes"].includes(row.kind)) fail("invalidInput");
    ctx.db.pimCollections.insert(row);
  }
  for (const row of data.resources) {
    if (!ctx.db.pimCollections.id.find(row.collectionId) || row.hrefKey !== `${row.collectionId}/${row.href}` || row.uidKey !== `${row.collectionId}/${row.uid}`) fail("invalidInput");
    ctx.db.pimResources.insert(row);
  }
  for (const row of data.grants) {
    if (!ctx.db.pimCollections.id.find(row.collectionId) || !ctx.db.accounts.id.find(row.userId)) fail("invalidInput");
    ctx.db.pimGrants.insert(row);
  }
  for (const row of data.preferences) { if (!["instance:appearance","instance:apps"].includes(row.userId) && !ctx.db.accounts.id.find(row.userId)) fail("invalidInput"); ctx.db.pimPreferences.insert(row); }
  for (const row of data.reminders) {
    if (!ctx.db.accounts.id.find(row.userId) || !ctx.db.pimResources.id.find(row.resourceId)) fail("invalidInput");
    ctx.db.pimReminders.insert(row);
  }
  for (const row of data.cursors ?? []) { if (!ctx.db.pimResources.id.find(row.resourceId)) fail("invalidInput"); ctx.db.pimReminderCursors.insert({ ...row, runAt: row.runAt || dueKey(row.at) }); }
  for (const row of ctx.db.pimResources.iter()) if (ctx.db.pimCollections.id.find(row.collectionId)?.kind === "calendar" && !ctx.db.pimReminderCursors.resourceId.find(row.id)) ctx.db.pimReminderCursors.insert({ resourceId: row.id, revision: row.revision, at: row.modifiedAt, runAt: dueKey(row.modifiedAt) });
}

export function reminderWork(ctx: Ctx) {
  const at = Number(ctx.timestamp.toMillis()), result = [];
  for (const cursor of ctx.db.pimReminderCursors.runAt.filter(new Range({ tag: "unbounded" }, { tag: "included", value: dueKey(at) }))) {
    const row = ctx.db.pimResources.id.find(cursor.resourceId);
    if (!row) continue;
    const c = ctx.db.pimCollections.id.find(row.collectionId);
    if (c?.kind !== "calendar" || !ctx.db.accounts.id.find(c.ownerId)?.enabled) continue;
    const from = cursor.revision === row.revision ? cursor.at : row.modifiedAt;
    if (from >= at - 1000) continue;
    const preferences = JSON.parse(ctx.db.pimPreferences.userId.find(c.ownerId)?.value ?? "{}");
    result.push({ resource: row, from, to: Math.min(at, from + 7 * 86400000), userId: c.ownerId, timezone: preferences.timezone ?? "UTC" });
    if (result.length >= 200) break;
  }
  return result;
}
export function commitReminders(ctx: Ctx, resourceId: string, revision: number, from: number, to: number, reminders: { id: string; dueAt: number; title: string }[]) {
  const resource = ctx.db.pimResources.id.find(resourceId), cursor = ctx.db.pimReminderCursors.resourceId.find(resourceId);
  if (!resource || resource.revision !== revision || (cursor?.revision === revision ? cursor.at : resource.modifiedAt) !== from) fail("conflict");
  if (!Number.isFinite(to) || to <= from || to > Number(ctx.timestamp.toMillis()) || to - from > 7 * 86400000 || reminders.length > 2000) fail("invalidInput");
  const c = ctx.db.pimCollections.id.find(resource.collectionId)!;
  for (const row of reminders) {
    if (!Number.isFinite(row.dueAt) || row.dueAt < from || row.dueAt > to || row.id.length > 200) fail("invalidInput");
    const id = `${resourceId}/${revision}/${row.id}`;
    if (!ctx.db.pimReminders.id.find(id)) ctx.db.pimReminders.insert({ ...row, id, userId: c.ownerId, resourceId, resourceRevision: revision, acknowledged: false });
  }
  const next = { resourceId, revision, at: to, runAt: dueKey(Number(ctx.timestamp.toMillis()) + 60000) };
  if (cursor) ctx.db.pimReminderCursors.resourceId.update(next); else ctx.db.pimReminderCursors.insert(next);
  if (reminders.length) bump(ctx, c.ownerId);
}
export function deferReminder(ctx: Ctx, resourceId: string, revision: number, from: number) {
  const cursor = ctx.db.pimReminderCursors.resourceId.find(resourceId);
  if (cursor?.revision === revision && cursor.at === from) ctx.db.pimReminderCursors.resourceId.update({ ...cursor, runAt: dueKey(Number(ctx.timestamp.toMillis()) + 300000) });
}
