import { Identity, ScheduleAt } from "spacetimedb";
import {appTables} from "./app-tables.ts";
import {agentTables} from "./agent-tables.ts";
import {agentOperation,agentSnapshot,restoreAgent} from "./agent.ts";
import {agentWorker} from "./agent-runs.ts";
import {appOperation,appSnapshot,restoreApps} from "./apps.ts";
import {validAppId} from "../../shared/installed-apps.ts";
import {globalSearch} from "./global-search.ts";
import {appSettings} from "./app-settings.ts";
import {essentialApps} from "../../shared/app-settings.ts";
import {passwordAllows, validPasswordScope} from "../../shared/app-scopes.ts";
import { schema, table, t, Range, SenderError, type InferSchema, type ReducerCtx, type ViewCtx } from "spacetimedb/server";
import { pimTables, pimChanges } from "./pim-tables.ts";
import { pimOperation, pimSnapshot, restorePim, reminderWork, commitReminders, deferReminder } from "./pim.ts";
import { identityTables } from "./identity-tables.ts";
import {workspaceTables} from "./workspace-tables.ts";
import {workspaceSnapshot,restoreWorkspace,noteTrashOperation,validateTaskParent} from "./workspace.ts";
import { identityActor, directoryOperation, providerOperation, revokeProvider, revokeUser, identitySnapshot, restoreIdentity, touchProfile,profile as directoryProfile } from "./identity.ts";

const instance = table({}, { id: t.string().primaryKey(), service: t.identity(), issuer: t.string() });
const accounts = table({}, {
  id: t.string().primaryKey(), username: t.string().unique(), displayName: t.string(),
  verifier: t.string(), administrator: t.bool(), enabled: t.bool(), createdAt: t.f64(),
});
const sessions = table({}, {
  id: t.string().primaryKey(), userId: t.string().index("btree"), digest: t.string().unique(),
  csrfDigest: t.string(), expiresAt: t.f64(), active: t.bool(), createdAt: t.f64(),
});
const principals = table({}, { identity: t.identity().primaryKey(), sessionId: t.string() });
const sessionExpiry = table({}, { scheduledId: t.u64().primaryKey().autoInc(), scheduledAt: t.scheduleAt(), sessionId: t.string() });
const fileEntries = table({}, {
  id: t.string().primaryKey(), ownerId: t.string().index("btree"), parentId: t.string(),
  name: t.string(), kind: t.string(), blobId: t.string(), size: t.f64(), mediaType: t.string(),
  revision: t.u32(), modifiedAt: t.f64(), trashed: t.bool(), favorite: t.bool(),
  listKey: t.string().index("btree"),
});
const navigation = table({}, { sessionId: t.string().primaryKey(), parentId: t.string(), section: t.string(), query: t.string(), cursor: t.string() });
const versions = table({}, { id: t.string().primaryKey(), fileId: t.string().index("btree"), blobId: t.string().index("btree"), size: t.f64(), mediaType: t.string(), createdAt: t.f64() });
const grants = table({}, { id: t.string().primaryKey(), fileId: t.string().index("btree"), userId: t.string().index("btree"), role: t.string(), listKey: t.string().index("btree").default("") });
const publicShares = table({}, { id: t.string().primaryKey(), fileId: t.string().index("btree"), digest: t.string().unique(), expiresAt: t.f64() });
const uploads = table({}, { id: t.string().primaryKey(), sessionId: t.string(), ownerId: t.string(), parentId: t.string(), fileId: t.string(),
  name: t.string(), size: t.f64(), mediaType: t.string(), expectedRevision: t.u32(), expiresAt: t.f64(), state: t.string(), digest: t.string(), expiresKey: t.string().index("btree").default("") });
const usage = table({}, { userId: t.string().primaryKey(), used: t.f64(), reserved: t.f64() });
const favorites = table({}, { id: t.string().primaryKey(), fileId: t.string(), listKey: t.string().index("btree") });
const backupPins = table({}, { id: t.string().primaryKey(), backupId: t.string().index("btree"), blobId: t.string().index("btree") });
const appPasswords = table({}, { id: t.string().primaryKey(), userId: t.string().index("btree"), digest: t.string().unique(), label: t.string(), scope: t.string(), sessionId: t.string().unique(), principal: t.identity(), expiresAt: t.f64(), revoked: t.bool() });
const rootRevisions = table({}, { userId: t.string().primaryKey(), revision: t.u32(), modifiedAt: t.f64() });
const activity = table({}, { id: t.string().primaryKey(), ownerId: t.string().index("btree"), fileId: t.string(), action: t.string(), at: t.f64() });
const db = schema({ instance, accounts, sessions, principals, sessionExpiry, fileEntries, navigation, versions, grants, publicShares, uploads, usage, activity, favorites, backupPins, appPasswords, rootRevisions, ...pimTables, ...identityTables, ...workspaceTables,...appTables,...agentTables }, { env: { DAR_ISSUER: t.string() } });
export default db;
export type Ctx = ReducerCtx<InferSchema<typeof db>>;
type VCtx = ViewCtx<InferSchema<typeof db>>;
type FileEntry = NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
type ReadCtx = Pick<VCtx, "sender" | "db">;
const safeUser = t.row("SafeUser", {
  id: t.string().primaryKey(), username: t.string(), displayName: t.string(), administrator: t.bool(),
});

function fail(code: string): never { throw new SenderError(code); }
function readResult(work: () => string): string {
  try { return work(); }
  catch (error) { if (error instanceof SenderError) return JSON.stringify({ error: { code: error.message } }); throw error; }
}
function service(ctx: Ctx): void {
  if (!ctx.db.instance.id.find("instance")?.service.isEqual(ctx.sender)) fail("forbidden");
}
function now(ctx: Ctx): number { return Number(ctx.timestamp.toMillis()); }
function userFor(ctx: VCtx) {
  const principal = ctx.db.principals.identity.find(ctx.sender);
  if (!principal) return undefined;
  const session = ctx.db.sessions.id.find(principal.sessionId);
  if (!session?.active) return undefined;
  const user = ctx.db.accounts.id.find(session.userId);
  return user?.enabled ? user : undefined;
}
function activeUser(ctx: Ctx) {
  const principal = ctx.db.principals.identity.find(ctx.sender);
  const session = principal && ctx.db.sessions.id.find(principal.sessionId);
  if (!session?.active || session.expiresAt <= now(ctx)) fail("unauthenticated");
  const user = ctx.db.accounts.id.find(session.userId);
  if (!user?.enabled) fail("unauthenticated");
  return user;
}
function fromSession(ctx: Ctx, id: string) {
  const session = ctx.db.sessions.id.find(id);
  if (!session?.active || session.expiresAt <= now(ctx)) fail("unauthenticated");
  const user = ctx.db.accounts.id.find(session.userId);
  if (!user?.enabled) fail("unauthenticated");
  return user;
}
function nameOf(name: string): string {
  name = name.normalize("NFC").trim();
  if (!name || name.length > 255 || /[\x00-\x1f/\\]/.test(name) || name === "." || name === ".." || [...name].some(c => c.codePointAt(0)! >= 0xd800 && c.codePointAt(0)! <= 0xdfff)) fail("invalidName");
  return name;
}
function opaqueId(value: string): void { if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(value)) fail("invalidInput"); }
function access(ctx: ReadCtx, entry: FileEntry, userId: string): "owner" | "write" | "read" | undefined {
  let node: FileEntry | null = entry;
  let role: "owner" | "write" | "read" | undefined = entry.ownerId === userId ? "owner" : undefined;
  for (let depth = 0; node && depth < 128; depth++) {
    if (node.trashed) return undefined;
    if (!role || role === "read") for (const grant of ctx.db.grants.fileId.filter(node.id)) {
      if (grant.userId === userId && (grant.role === "write" || !role)) role = grant.role as "write" | "read";
    }
    if (!node.parentId) return role;
    node = ctx.db.fileEntries.id.find(node.parentId);
  }
  return undefined;
}
function writable(ctx: Ctx, id: string, userId: string) {
  const entry = ctx.db.fileEntries.id.find(id);
  if (!entry) fail("notFound");
  const role = access(ctx, entry, userId);
  if (role !== "write" && role !== "owner") fail("forbidden");
  return entry;
}
function destination(ctx: Ctx, parentId: string, userId: string): string {
  if (!parentId) return userId;
  const parent = writable(ctx, parentId, userId);
  if (parent.kind !== "folder") fail("invalidInput");
  return parent.ownerId;
}
function key(ownerId: string, parentId: string, name: string) { return `${ownerId}/${parentId}/${name.toLowerCase()}`; }
function windowOf(prefix: string, cursor?: string) {
  // Increment the prefix itself: appending U+FFFF would exclude emoji and
  // other supplementary Unicode filenames from a UTF-8-ordered index.
  const points = [...prefix];
  let last = points.pop()!.codePointAt(0)!;
  while (last === 0x10ffff) last = points.pop()!.codePointAt(0)!;
  const upper = points.join("") + String.fromCodePoint(last === 0xd7ff ? 0xe000 : last + 1);
  return new Range<string>({ tag: cursor ? "excluded" : "included", value: cursor ?? prefix }, { tag: "excluded", value: upper });
}
function findKey(ctx: ReadCtx, value: string) { return ctx.db.fileEntries.listKey.filter(value)[Symbol.iterator]().next().value as FileEntry | undefined; }
function touchFolder(ctx: Ctx, ownerId: string, parentId: string) {
  if (parentId) {
    const parent = ctx.db.fileEntries.id.find(parentId);
    if (parent) ctx.db.fileEntries.id.update({ ...parent, revision: parent.revision + 1, modifiedAt: now(ctx) });
  } else {
    const root = ctx.db.rootRevisions.userId.find(ownerId);
    const value = { userId: ownerId, revision: (root?.revision ?? 1) + 1, modifiedAt: now(ctx) };
    if (root) ctx.db.rootRevisions.userId.update(value); else ctx.db.rootRevisions.insert(value);
  }
}
function unusedUuid(ctx: Ctx): string {
  // The SDK seeds reducer randomness from its timestamp; simultaneous calls can
  // produce the same first UUID. Advance the generator past retained IDs in
  // every namespace where the module generates identifiers.
  let id = ctx.newUuidV4().toString();
  while (ctx.db.activity.id.find(id) || ctx.db.fileEntries.id.find(id) || ctx.db.versions.id.find(id) || ctx.db.sessions.id.find(id)
    || ctx.db.principals.identity.find(new Identity(BigInt(`0x${id.replaceAll("-", "")}`)))) id = ctx.newUuidV4().toString();
  return id;
}
function event(ctx: Ctx, ownerId: string, fileId: string, action: string, parentId?: string) {
  const id = unusedUuid(ctx);
  ctx.db.activity.insert({ id, ownerId, fileId, action, at: now(ctx) });
  touchFolder(ctx, ownerId, parentId ?? ctx.db.fileEntries.id.find(fileId)?.parentId ?? "");
}
function revision(entry: FileEntry, expected: number) { if (entry.revision !== expected) fail("conflict"); }
function renameGrants(ctx: Ctx, fileId: string, name: string) {
  for (const grant of ctx.db.grants.fileId.filter(fileId)) ctx.db.grants.id.update({ ...grant, listKey: `${grant.userId}/${name.toLowerCase()}/${grant.id}` });
}

export const init = db.init(ctx => {
  ctx.db.instance.insert({ id: "instance", service: ctx.sender, issuer: ctx.env.DAR_ISSUER });
});
export const connected = db.clientConnected(ctx => {
  if (ctx.db.instance.id.find("instance")?.service.isEqual(ctx.sender)) return;
  const jwt = ctx.senderAuth.jwt;
  const config = ctx.db.instance.id.find("instance");
  if (!jwt || jwt.issuer !== config?.issuer || !jwt.audience.includes("dar-browser")) fail("unauthenticated");
  const sessionId = jwt.fullPayload.sid;
  if (typeof sessionId !== "string" || !jwt.subject.startsWith(`${sessionId}:`)) fail("unauthenticated");
  const session = ctx.db.sessions.id.find(sessionId);
  const user = session && ctx.db.accounts.id.find(session.userId);
  if (!session?.active || session.expiresAt <= now(ctx) || !user?.enabled) fail("unauthenticated");
  const existing = ctx.db.principals.identity.find(ctx.sender);
  if (!existing) ctx.db.principals.insert({ identity: ctx.sender, sessionId: session.id });
});

export const expireSession = db.reducer({ onSchedule: sessionExpiry }, { expiry: sessionExpiry.rowType }, (ctx, { expiry }) => {
  if (!ctx.senderAuth.isInternal) fail("forbidden");
  const session = ctx.db.sessions.id.find(expiry.sessionId);
  if (session && session.expiresAt <= now(ctx)) {ctx.db.sessions.id.update({ ...session, active: false });revokeProvider(ctx,undefined,session.id);}
});
export const serviceStatus = db.procedure(t.string(), ctx => ctx.withTx(tx => {
  service(tx);
  return JSON.stringify({ initialized: tx.db.accounts.count() > 0n });
}));
// Instance branding shares the existing revisioned settings store and backup path.
export const serviceAppearance = db.procedure({sessionId:t.string(), revision:t.u32(), value:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);
  const userId = "instance:appearance", old = tx.db.pimPreferences.userId.find(userId);
  if(p.value){
    const user=fromSession(tx,p.sessionId);
    if(!user.administrator || tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");
    if((old?.revision??0)!==p.revision)fail("conflict");
    const value=JSON.parse(p.value);
    if(typeof value.logo!=="string" || value.logo.length>350000 || !["","image/png","image/webp"].includes(value.mediaType))fail("invalidInput");
    const row={userId,revision:(old?.revision??0)+1,value:JSON.stringify({logo:value.logo,mediaType:value.mediaType})};
    if(old)tx.db.pimPreferences.userId.update(row);else tx.db.pimPreferences.insert(row);
    return JSON.stringify(row);
  }
  return JSON.stringify(old??{userId,revision:0,value:'{"logo":"","mediaType":""}'});
}));
export const serviceStartup = db.reducer({ issuer: t.string() }, (ctx, { issuer }) => {
  service(ctx);
  if (!ctx.db.identityMigrations.id.find(1)) ctx.db.identityMigrations.insert({ id:1, appliedAt:now(ctx) });
  if(!ctx.db.workspaceMigrations.id.find(1))ctx.db.workspaceMigrations.insert({id:1,appliedAt:now(ctx)});
  if (issuer !== ctx.env.DAR_ISSUER) fail("invalidInput");
  const config = ctx.db.instance.id.find("instance")!;
  if (config.issuer === issuer) return;
  ctx.db.instance.id.update({ ...config, issuer });
  revokeProvider(ctx);
  for (const session of ctx.db.sessions.iter()) if (session.active) ctx.db.sessions.id.update({ ...session, active: false });
  for (const password of ctx.db.appPasswords.iter()) if (!password.revoked) ctx.db.appPasswords.id.update({ ...password, revoked: true });
});
export const serviceAccount = db.procedure({ username: t.string() }, t.string(), (ctx, { username }) => ctx.withTx(tx => {
  service(tx);
  return JSON.stringify(tx.db.accounts.username.find(username) ?? null);
}));
export const serviceCreateAccount = db.reducer({
  id: t.string(), username: t.string(), displayName: t.string(), verifier: t.string(), first: t.bool(),
}, (ctx, p) => {
  service(ctx);
  if (p.first !== (ctx.db.accounts.count() === 0n)) fail("conflict");
  opaqueId(p.id);
  if (!/^[a-z0-9][a-z0-9_.-]{2,63}$/.test(p.username) || p.displayName.length < 1 || p.displayName.length > 100) fail("invalidInput");
  if (ctx.db.accounts.username.find(p.username)) fail("conflict");
  if (ctx.db.accounts.count()>=10000n)fail("operationTooLarge");
  ctx.db.accounts.insert({ id: p.id, username: p.username, displayName: p.displayName, verifier: p.verifier,
    administrator: p.first, enabled: true, createdAt: now(ctx) });
  ctx.db.usage.insert({ userId: p.id, used: 0, reserved: 0 });
});
export const serviceCreateSession = db.reducer({
  id: t.string(), userId: t.string(), digest: t.string(), csrfDigest: t.string(), expiresAt: t.f64(), expectedVerifier: t.string(),
}, (ctx, p) => {
  service(ctx);
  const account = ctx.db.accounts.id.find(p.userId);
  if (!account?.enabled || account.verifier !== p.expectedVerifier) fail("unauthenticated");
  if (!Number.isFinite(p.expiresAt) || p.expiresAt <= now(ctx) || p.expiresAt > now(ctx) + 7 * 86400_000) fail("invalidInput");
  const { expectedVerifier: _expectedVerifier, ...session } = p;
  ctx.db.sessions.insert({ ...session, active: true, createdAt: now(ctx) });
  ctx.db.sessionExpiry.insert({ scheduledId: 0n, scheduledAt: ScheduleAt.time(BigInt(Math.ceil(p.expiresAt)) * 1000n), sessionId: p.id });
});
export const serviceSession = db.procedure({ digest: t.string() }, t.string(), (ctx, { digest }) => ctx.withTx(tx => {
  service(tx);
  const session = tx.db.sessions.digest.find(digest);
  const user = session && tx.db.accounts.id.find(session.userId);
  return JSON.stringify(session?.active && session.expiresAt > now(tx) && user?.enabled ? { session,
    user: { id: user.id, username: user.username, displayName: user.displayName, administrator: user.administrator } } : null);
}));
export const serviceRevokeSession = db.reducer({ id: t.string() }, (ctx, { id }) => {
  service(ctx);
  const session = ctx.db.sessions.id.find(id);
  if (session) ctx.db.sessions.id.update({ ...session, active: false });
  revokeProvider(ctx, undefined, id);
});
export const serviceChangePassword = db.reducer({ userId: t.string(), verifier: t.string(), expectedVerifier: t.string() }, (ctx, { userId, verifier, expectedVerifier }) => {
  service(ctx);
  const user = ctx.db.accounts.id.find(userId);
  if (!user) fail("notFound");
  if (user.verifier !== expectedVerifier) fail("conflict");
  ctx.db.accounts.id.update({ ...user, verifier });
  touchProfile(ctx,userId);
  revokeUser(ctx,userId);
});
export const serviceSetEnabled = db.reducer({ userId: t.string(), enabled: t.bool() }, (ctx, { userId, enabled }) => {
  service(ctx);
  const user = ctx.db.accounts.id.find(userId);
  if (!user || user.administrator) fail("forbidden");
  if(enabled&&ctx.db.directoryProfiles.userId.find(userId)?.deleted)fail("notFound");
  ctx.db.accounts.id.update({ ...user, enabled });
  if(user.enabled!==enabled)touchProfile(ctx,userId);
  if (!enabled) revokeUser(ctx,userId);
});
export const me = db.view({ public: true }, t.option(safeUser), ctx => {
  const user = userFor(ctx);
  return user ? { id: user.id, username: user.username, displayName: user.displayName, administrator: user.administrator } : undefined;
});

export const myFiles = db.view({ public: true }, t.array(fileEntries.rowType), ctx => {
  const user = userFor(ctx);
  if (!user) return [];
  const principal = ctx.db.principals.identity.find(ctx.sender)!;
  const preferences=JSON.parse(ctx.db.pimPreferences.userId.find(user.id)?.value??"{}");
  const visible=(entry:FileEntry)=>preferences.filesShowHidden===true||!entry.name.startsWith(".");
  const nav = ctx.db.navigation.sessionId.find(ctx.sender.toHexString());
  const parentId = nav?.parentId ?? "";
  const section = nav?.section ?? "files";
  if (section === "favorites") {
    const result: FileEntry[] = [];
    const prefix = `${user.id}/${nav?.query ?? ""}`;
    for (const saved of ctx.db.favorites.listKey.filter(windowOf(prefix, nav?.cursor?.startsWith(prefix) ? nav.cursor : undefined))) {
      const entry = ctx.db.fileEntries.id.find(saved.fileId);
      if (entry && visible(entry) && access(ctx, entry, user.id)) result.push({ ...entry, listKey: saved.listKey });
      if (result.length >= 201) break;
    }
    return result;
  }
  if (section === "shared") {
    const result: FileEntry[] = [];
    const prefix = `${user.id}/${nav?.query ?? ""}`;
    for (const grant of ctx.db.grants.listKey.filter(windowOf(prefix, nav?.cursor?.startsWith(prefix) ? nav.cursor : undefined))) {
      const entry = ctx.db.fileEntries.id.find(grant.fileId);
      if (entry && visible(entry) && access(ctx, entry, user.id)) result.push({ ...entry, favorite: false, listKey: grant.listKey });
      if (result.length >= 201) break;
    }
    return result;
  }
  const ownerId = parentId ? ctx.db.fileEntries.id.find(parentId)?.ownerId : user.id;
  if (!ownerId) return [];
  const prefix = section === "trash" ? `${user.id}/trash/${nav?.query ?? ""}` : `${ownerId}/${parentId}/${nav?.query ?? ""}`;
  const start = windowOf(prefix, nav?.cursor && nav.cursor.startsWith(prefix) ? nav.cursor : undefined);
  const result: FileEntry[] = [];
  for (const entry of ctx.db.fileEntries.listKey.filter(start)) {
    if(!visible(entry))continue;
    if (section !== "trash" && !access(ctx, entry, user.id)) continue;
    if (nav?.query && !entry.name.toLowerCase().includes(nav.query)) continue;
    result.push(entry);
    if (result.length >= 201) break;
  }
  return result;
});
export const createFolder = db.reducer({ id: t.string(), parentId: t.string(), name: t.string() }, (ctx, p) => {
  const user = activeUser(ctx);
  opaqueId(p.id);
  const name = nameOf(p.name);
  const ownerId = destination(ctx, p.parentId, user.id);
  const listKey = key(ownerId, p.parentId, name);
  if (findKey(ctx, listKey)) fail("conflict");
  ctx.db.fileEntries.insert({ id: p.id, ownerId, parentId: p.parentId, name, kind: "folder", blobId: "", size: 0,
    mediaType: "inode/directory", revision: 1, modifiedAt: now(ctx), trashed: false, favorite: false, listKey });
  event(ctx, ownerId, p.id, "created");
});

export const browse = db.reducer({ parentId: t.string(), section: t.string(), query: t.string(), cursor: t.string() }, (ctx, p) => {
  const user = activeUser(ctx);
  if (!["files", "favorites", "shared", "trash"].includes(p.section) || p.query.length > 100 || p.cursor.length > 1024) fail("invalidInput");
  if (p.parentId) {
    const parent = ctx.db.fileEntries.id.find(p.parentId);
    if (!parent || parent.kind !== "folder" || !access(ctx, parent, user.id)) fail("notFound");
  }
  const sessionId = ctx.sender.toHexString();
  const row = { sessionId, ...p, query: p.query.normalize("NFC").toLowerCase() };
  if (ctx.db.navigation.sessionId.find(sessionId)) ctx.db.navigation.sessionId.update(row);
  else ctx.db.navigation.insert(row);
});
export const renameEntry = db.reducer({ id: t.string(), name: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = writable(ctx, p.id, user.id);
  revision(entry, p.expectedRevision);
  const name = nameOf(p.name), listKey = key(entry.ownerId, entry.parentId, name);
  const collision = findKey(ctx, listKey);
  if (collision && collision.id !== entry.id) fail("conflict");
  ctx.db.fileEntries.id.update({ ...entry, name, listKey, revision: entry.revision + 1, modifiedAt: now(ctx) });
  if (entry.favorite) ctx.db.favorites.id.update({ id: entry.id, fileId: entry.id, listKey: `${entry.ownerId}/${name.toLowerCase()}/${entry.id}` });
  renameGrants(ctx, entry.id, name);
  event(ctx, entry.ownerId, entry.id, "renamed");
});
export const favoriteEntry = db.reducer({ id: t.string(), favorite: t.bool(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = writable(ctx, p.id, user.id);
  if (entry.ownerId !== user.id) fail("forbidden");
  revision(entry, p.expectedRevision);
  ctx.db.fileEntries.id.update({ ...entry, favorite: p.favorite, revision: entry.revision + 1 });
  if (p.favorite && !ctx.db.favorites.id.find(entry.id)) ctx.db.favorites.insert({ id: entry.id, fileId: entry.id, listKey: `${entry.ownerId}/${entry.name.toLowerCase()}/${entry.id}` });
  if (!p.favorite) ctx.db.favorites.id.delete(entry.id);
  touchFolder(ctx, entry.ownerId, entry.parentId);
});
export const moveEntry = db.reducer({ id: t.string(), parentId: t.string(), expectedRevision: t.u32(), name: t.option(t.string()) }, (ctx, p) => {
  const user = activeUser(ctx), entry = writable(ctx, p.id, user.id);
  if (entry.ownerId !== user.id || destination(ctx, p.parentId, user.id) !== entry.ownerId) fail("forbidden");
  revision(entry, p.expectedRevision);
  let parentId = p.parentId;
  for (let depth = 0; parentId; depth++) {
    if (parentId === entry.id || depth >= 128) fail("invalidInput");
    parentId = ctx.db.fileEntries.id.find(parentId)?.parentId ?? "";
  }
  const name = p.name === undefined ? entry.name : nameOf(p.name);
  const listKey = key(entry.ownerId, p.parentId, name);
  const existing = findKey(ctx, listKey);
  if (existing && existing.id !== entry.id) fail("conflict");
  ctx.db.fileEntries.id.update({ ...entry, name, parentId: p.parentId, listKey, revision: entry.revision + 1, modifiedAt: now(ctx) });
  if (entry.favorite) ctx.db.favorites.id.update({ id: entry.id, fileId: entry.id, listKey: `${entry.ownerId}/${name.toLowerCase()}/${entry.id}` });
  renameGrants(ctx, entry.id, name);
  event(ctx, entry.ownerId, entry.id, "moved");
  if (entry.parentId !== p.parentId) touchFolder(ctx, entry.ownerId, entry.parentId);
});
export const copyEntry = db.reducer({ id: t.string(), newId: t.string(), parentId: t.string(), name: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  opaqueId(p.newId);
  const user = activeUser(ctx), source = ctx.db.fileEntries.id.find(p.id);
  if (!source || !access(ctx, source, user.id)) fail("notFound");
  revision(source, p.expectedRevision);
  const ownerId = destination(ctx, p.parentId, user.id), name = nameOf(p.name);
  if (findKey(ctx, key(ownerId, p.parentId, name))) fail("conflict");
  let ancestor = p.parentId;
  for (let depth = 0; ancestor; depth++) {
    if (ancestor === source.id || depth >= 128) fail("invalidInput");
    ancestor = ctx.db.fileEntries.id.find(ancestor)?.parentId ?? "";
  }
  const pending = [{ source, id: p.newId, parentId: p.parentId, name }];
  let count = 0, bytes = 0;
  while (pending.length) {
    const item = pending.pop()!;
    if (++count > 500) fail("operationTooLarge");
    if (!access(ctx, item.source, user.id)) fail("forbidden");
    const entry = { ...item.source, id: item.id, name: item.name, parentId: item.parentId, ownerId, favorite: false, revision: 1, modifiedAt: now(ctx), listKey: key(ownerId, item.parentId, item.name) };
    ctx.db.fileEntries.insert(entry);
    if (entry.kind === "file") {
      bytes += entry.size;
      ctx.db.versions.insert({ id: unusedUuid(ctx), fileId: entry.id, blobId: entry.blobId, size: entry.size, mediaType: entry.mediaType, createdAt: now(ctx) });
    } else for (const child of ctx.db.fileEntries.listKey.filter(windowOf(`${source.ownerId}/${item.source.id}/`))) {
      pending.push({ source: child, id: unusedUuid(ctx), parentId: entry.id, name: child.name });
    }
  }
  const meter = ctx.db.usage.userId.find(ownerId) ?? ctx.db.usage.insert({ userId: ownerId, used: 0, reserved: 0 });
  if (meter.used + meter.reserved + bytes > 10 * 1024 ** 3) fail("quotaExceeded");
  ctx.db.usage.userId.update({ ...meter, used: meter.used + bytes });
  event(ctx, ownerId, p.newId, "copied");
});
export const trashEntry = db.reducer({ id: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = writable(ctx, p.id, user.id);
  if (entry.ownerId !== user.id) fail("forbidden");
  revision(entry, p.expectedRevision);
  ctx.db.fileEntries.id.update({ ...entry, trashed: true, listKey: `${user.id}/trash/${entry.name.toLowerCase()}/${entry.id}`, revision: entry.revision + 1, modifiedAt: now(ctx) });
  event(ctx, entry.ownerId, entry.id, "trashed");
});
export const restoreEntry = db.reducer({ id: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = ctx.db.fileEntries.id.find(p.id);
  if (!entry || entry.ownerId !== user.id || !entry.trashed) fail("notFound");
  revision(entry, p.expectedRevision);
  const parent = entry.parentId && ctx.db.fileEntries.id.find(entry.parentId);
  const parentId = parent && access(ctx, parent, user.id) ? parent.id : "";
  const listKey = key(user.id, parentId, entry.name);
  if (findKey(ctx, listKey)) fail("conflict");
  ctx.db.fileEntries.id.update({ ...entry, parentId, trashed: false, listKey, revision: entry.revision + 1, modifiedAt: now(ctx) });
  event(ctx, user.id, entry.id, "restored");
});
export const deleteEntry = db.reducer({ id: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = ctx.db.fileEntries.id.find(p.id);
  if (!entry || entry.ownerId !== user.id || !entry.trashed) fail("notFound");
  revision(entry, p.expectedRevision);
  const pending = [entry]; let count = 0, bytes = 0;
  while (pending.length) {
    const item = pending.pop()!;
    if (++count > 2000) fail("operationTooLarge");
    if (item.kind === "folder") for (const child of ctx.db.fileEntries.listKey.filter(windowOf(`${user.id}/${item.id}/`))) pending.push(child);
    for (const version of ctx.db.versions.fileId.filter(item.id)) { bytes += version.size; ctx.db.versions.id.delete(version.id); }
    for (const grant of ctx.db.grants.fileId.filter(item.id)) ctx.db.grants.id.delete(grant.id);
    for (const share of ctx.db.publicShares.fileId.filter(item.id)) ctx.db.publicShares.id.delete(share.id);
    ctx.db.fileEntries.id.delete(item.id);
    ctx.db.favorites.id.delete(item.id);
  }
  const meter = ctx.db.usage.userId.find(user.id);
  if (meter) ctx.db.usage.userId.update({ ...meter, used: Math.max(0, meter.used - bytes) });
  event(ctx, user.id, entry.id, "deleted", entry.parentId);
});
export const grantAccess = db.reducer({ id: t.string(), fileId: t.string(), username: t.string(), role: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  opaqueId(p.id);
  const user = activeUser(ctx), entry = writable(ctx, p.fileId, user.id);
  if (entry.ownerId !== user.id) fail("forbidden");
  revision(entry, p.expectedRevision);
  const recipient = ctx.db.accounts.username.find(p.username.toLowerCase());
  if (!recipient?.enabled || recipient.id === user.id || !["read", "write"].includes(p.role)) fail("invalidInput");
  for (const grant of ctx.db.grants.fileId.filter(entry.id)) if (grant.userId === recipient.id) ctx.db.grants.id.delete(grant.id);
  ctx.db.grants.insert({ id: p.id, fileId: entry.id, userId: recipient.id, role: p.role, listKey: `${recipient.id}/${entry.name.toLowerCase()}/${p.id}` });
  ctx.db.fileEntries.id.update({ ...entry, revision: entry.revision + 1 });
});
export const revokeGrant = db.reducer({ id: t.string() }, (ctx, { id }) => {
  const user = activeUser(ctx), grant = ctx.db.grants.id.find(id);
  if (!grant) fail("notFound");
  const entry = ctx.db.fileEntries.id.find(grant.fileId);
  if (entry?.ownerId !== user.id) fail("forbidden");
  ctx.db.grants.id.delete(id);
});
export const fileDetails = db.procedure({ id: t.string() }, t.string(), (ctx, { id }) => readResult(() => ctx.withTx(tx => {
  const user = activeUser(tx), entry = tx.db.fileEntries.id.find(id);
  if (!entry || !(entry.ownerId === user.id && entry.trashed) && !access(tx, entry, user.id)) fail("notFound");
  const role = entry.ownerId === user.id ? "owner" : access(tx, entry, user.id);
  const history = [...tx.db.versions.fileId.filter(id)].map(v => ({ ...v })).sort((a,b) => b.createdAt-a.createdAt);
  const sharing = role === "owner" ? [...tx.db.grants.fileId.filter(id)].map(g => ({ id: g.id, username: tx.db.accounts.id.find(g.userId)?.username, role: g.role })) : [];
  const ancestors: { id: string; name: string }[] = [];
  let parentId = entry.parentId;
  for (let depth = 0; parentId && depth < 128; depth++) {
    const parent = tx.db.fileEntries.id.find(parentId);
    if (!parent || !access(tx, parent, user.id)) break;
    ancestors.unshift({ id: parent.id, name: parent.name }); parentId = parent.parentId;
  }
  return JSON.stringify({ entry, role, ancestors, versions: history, grants: sharing, publicShares: role === "owner" ? [...tx.db.publicShares.fileId.filter(id)].map(s => ({ id: s.id, expiresAt: s.expiresAt })) : [] });
})));
export const folderChoices = db.procedure({ parentId: t.string(), query: t.option(t.string()), cursor: t.option(t.string()) }, t.string(), (ctx, { parentId, query, cursor }) => readResult(() => ctx.withTx(tx => {
  const user = activeUser(tx), ownerId = destination(tx, parentId, user.id);
  if (ownerId !== user.id) fail("forbidden");
  if ((query?.length ?? 0) > 100 || (cursor?.length ?? 0) > 1024) fail("invalidInput");
  const prefix = `${ownerId}/${parentId}/${query?.normalize("NFC").toLowerCase() ?? ""}`;
  const children: { id: string; name: string; listKey: string }[] = [];
  for (const entry of tx.db.fileEntries.listKey.filter(windowOf(prefix, cursor?.startsWith(prefix) ? cursor : undefined))) {
    if (entry.kind === "folder" && !entry.trashed) children.push({ id: entry.id, name: entry.name, listKey: entry.listKey });
    if (children.length >= 201) break;
  }
  return JSON.stringify({ items: children.slice(0, 200), nextCursor: children.length > 200 ? children[199].listKey : "" });
})));
export const myUsage = db.view({ public: true }, t.option(usage.rowType), ctx => {
  const user = userFor(ctx); return user ? ctx.db.usage.userId.find(user.id) ?? undefined : undefined;
});

export const serviceBeginUpload = db.procedure({ sessionId: t.string(), id: t.string(), fileId: t.string(), parentId: t.string(), name: t.string(), size: t.f64(), mediaType: t.string(), expectedRevision: t.u32() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, p.sessionId);
  opaqueId(p.id); opaqueId(p.fileId);
  if (!Number.isSafeInteger(p.size) || p.size < 0 || p.size > 1024 ** 3 || p.mediaType.length > 150 || !/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/i.test(p.mediaType)) fail("invalidInput");
  let ownerId: string, name: string, parentId: string;
  const existing = tx.db.fileEntries.id.find(p.fileId);
  if (p.expectedRevision) {
    if (!existing || existing.kind !== "file") fail("notFound");
    writable(tx, existing.id, user.id); revision(existing, p.expectedRevision);
    ownerId = existing.ownerId; name = existing.name; parentId = existing.parentId;
  } else {
    ownerId = destination(tx, p.parentId, user.id); name = nameOf(p.name); parentId = p.parentId;
    if (existing || findKey(tx, key(ownerId, parentId, name))) fail("conflict");
  }
  let meter = tx.db.usage.userId.find(ownerId);
  if (!meter) meter = tx.db.usage.insert({ userId: ownerId, used: 0, reserved: 0 });
  if (meter.used + meter.reserved + p.size > 10 * 1024 ** 3) fail("quotaExceeded");
  tx.db.usage.userId.update({ ...meter, reserved: meter.reserved + p.size });
  const expiresAt = now(tx) + 3600_000;
  const upload = { ...p, ownerId, name, parentId, expiresAt, state: "uploading", digest: "", expiresKey: String(expiresAt).padStart(16, "0") };
  tx.db.uploads.insert(upload);
  return JSON.stringify(upload);
}));
export const serviceUpload = db.procedure({ sessionId: t.string(), id: t.string() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); fromSession(tx, p.sessionId);
  const upload = tx.db.uploads.id.find(p.id);
  if (!upload || upload.sessionId !== p.sessionId || upload.expiresAt <= now(tx)) fail("notFound");
  return JSON.stringify(upload);
}));
export const serviceFinishUpload = db.reducer({ sessionId: t.string(), id: t.string(), digest: t.string() }, (ctx, p) => {
  service(ctx); const user = fromSession(ctx, p.sessionId), upload = ctx.db.uploads.id.find(p.id);
  if (!upload || upload.sessionId !== p.sessionId || upload.expiresAt <= now(ctx) || !/^[a-f0-9]{64}$/.test(p.digest)) fail("notFound");
  if (upload.state === "complete") { if (upload.digest !== p.digest) fail("conflict"); return; }
  if (upload.state !== "uploading") fail("conflict");
  const existing = ctx.db.fileEntries.id.find(upload.fileId);
  if (upload.expectedRevision) {
    if (!existing) fail("notFound");
    writable(ctx, existing.id, user.id); revision(existing, upload.expectedRevision);
  } else if (existing || findKey(ctx, key(upload.ownerId, upload.parentId, upload.name))) fail("conflict");
  // Replacements require current access to the file; creating a child requires
  // access to its destination. A direct file grant need not grant its parent.
  if (!upload.expectedRevision && destination(ctx, upload.parentId, user.id) !== upload.ownerId) fail("forbidden");
  const entry = { id: upload.fileId, ownerId: upload.ownerId, parentId: upload.parentId, name: upload.name, kind: "file", blobId: upload.id,
    size: upload.size, mediaType: upload.mediaType, revision: (existing?.revision ?? 0) + 1, modifiedAt: now(ctx), trashed: false, favorite: existing?.favorite ?? false,
    listKey: key(upload.ownerId, upload.parentId, upload.name) };
  if (existing) ctx.db.fileEntries.id.update(entry); else ctx.db.fileEntries.insert(entry);
  ctx.db.versions.insert({ id: upload.id, fileId: entry.id, blobId: upload.id, size: upload.size, mediaType: upload.mediaType, createdAt: now(ctx) });
  ctx.db.uploads.id.update({ ...upload, state: "complete", digest: p.digest });
  const meter = ctx.db.usage.userId.find(upload.ownerId)!;
  ctx.db.usage.userId.update({ ...meter, reserved: meter.reserved - upload.size, used: meter.used + upload.size });
  event(ctx, upload.ownerId, entry.id, existing ? "updated" : "uploaded");
});
export const serviceCancelUpload = db.reducer({ sessionId: t.string(), id: t.string() }, (ctx, p) => {
  service(ctx); fromSession(ctx, p.sessionId); const upload = ctx.db.uploads.id.find(p.id);
  if (!upload || upload.sessionId !== p.sessionId) fail("notFound");
  if (upload.state !== "uploading") return;
  const meter = ctx.db.usage.userId.find(upload.ownerId)!;
  ctx.db.usage.userId.update({ ...meter, reserved: Math.max(0, meter.reserved - upload.size) });
  ctx.db.uploads.id.update({ ...upload, state: "cancelled" });
});
export const serviceDownload = db.procedure({ sessionId: t.string(), id: t.string(), versionId: t.string() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, p.sessionId), entry = tx.db.fileEntries.id.find(p.id);
  const password=tx.db.appPasswords.sessionId.find(p.sessionId);
  if(password && !passwordAllows(password.scope,"files","read"))fail("forbidden");
  if (!entry || entry.kind !== "file" || !access(tx, entry, user.id)) fail("notFound");
  const version = p.versionId ? tx.db.versions.id.find(p.versionId) : undefined;
  if (p.versionId && version?.fileId !== entry.id) fail("notFound");
  return JSON.stringify({ ...entry, blobId: version?.blobId ?? entry.blobId, size: version?.size ?? entry.size, mediaType: version?.mediaType ?? entry.mediaType });
}));
export const restoreVersion = db.reducer({ id: t.string(), versionId: t.string(), expectedRevision: t.u32() }, (ctx, p) => {
  const user = activeUser(ctx), entry = writable(ctx, p.id, user.id), version = ctx.db.versions.id.find(p.versionId);
  revision(entry, p.expectedRevision);
  if (!version || version.fileId !== entry.id) fail("notFound");
  const meter = ctx.db.usage.userId.find(entry.ownerId)!;
  if (meter.used + meter.reserved + version.size > 10 * 1024 ** 3) fail("quotaExceeded");
  ctx.db.usage.userId.update({ ...meter, used: meter.used + version.size });
  ctx.db.versions.insert({ ...version, id: unusedUuid(ctx), createdAt: now(ctx) });
  ctx.db.fileEntries.id.update({ ...entry, blobId: version.blobId, size: version.size, mediaType: version.mediaType, revision: entry.revision + 1, modifiedAt: now(ctx) });
  event(ctx, entry.ownerId, entry.id, "versionRestored");
});
export const createPublicShare = db.reducer({ id: t.string(), fileId: t.string(), digest: t.string(), expiresAt: t.f64(), expectedRevision: t.u32() }, (ctx, p) => {
  opaqueId(p.id);
  const user = activeUser(ctx), entry = writable(ctx, p.fileId, user.id);
  if (entry.ownerId !== user.id || entry.kind !== "file") fail("forbidden");
  revision(entry, p.expectedRevision);
  if (!/^[a-f0-9]{64}$/.test(p.digest) || !Number.isFinite(p.expiresAt) || p.expiresAt <= now(ctx) || p.expiresAt > now(ctx) + 30 * 86400_000) fail("invalidInput");
  ctx.db.publicShares.insert({ id: p.id, fileId: p.fileId, digest: p.digest, expiresAt: p.expiresAt });
});
export const revokePublicShare = db.reducer({ id: t.string() }, (ctx, { id }) => {
  const user = activeUser(ctx), share = ctx.db.publicShares.id.find(id);
  if (!share || ctx.db.fileEntries.id.find(share.fileId)?.ownerId !== user.id) fail("forbidden");
  ctx.db.publicShares.id.delete(id);
});
export const servicePublicDownload = db.procedure({ digest: t.string() }, t.string(), (ctx, { digest }) => ctx.withTx(tx => {
  service(tx); const share = tx.db.publicShares.digest.find(digest);
  if (!share || share.expiresAt <= now(tx)) fail("notFound");
  const entry = tx.db.fileEntries.id.find(share.fileId);
  if (!entry || !access(tx, entry, entry.ownerId) || !tx.db.accounts.id.find(entry.ownerId)?.enabled) fail("notFound");
  return JSON.stringify(entry);
}));

export const serviceMaintenance = db.procedure(t.string(), ctx => ctx.withTx(tx => {
  service(tx); const candidates: string[] = [];
  for (const upload of tx.db.uploads.expiresKey.filter(new Range({ tag: "unbounded" }, { tag: "excluded", value: String(now(tx)).padStart(16, "0") }))) {
    if (upload.expiresAt > now(tx)) continue;
    if (candidates.length >= 100) break;
    if (upload.state === "uploading") {
      const meter = tx.db.usage.userId.find(upload.ownerId);
      if (meter) tx.db.usage.userId.update({ ...meter, reserved: Math.max(0, meter.reserved - upload.size) });
      tx.db.uploads.id.update({ ...upload, state: "cancelled" });
    }
    if (tx.db.versions.blobId.filter(upload.id)[Symbol.iterator]().next().done && tx.db.backupPins.blobId.filter(upload.id)[Symbol.iterator]().next().done) {
      tx.db.uploads.id.update({ ...upload, state: "deleting" }); candidates.push(upload.id);
    }
  }
  return JSON.stringify(candidates);
}));
export const serviceAcknowledgeGc = db.reducer({ id: t.string() }, (ctx, { id }) => {
  service(ctx);
  const upload = ctx.db.uploads.id.find(id);
  if (upload?.state === "deleting") ctx.db.uploads.id.delete(id);
});
export const serviceBackup = db.procedure({ id: t.string() }, t.string(), (ctx, { id }) => ctx.withTx(tx => {
  service(tx);
  const history = [...tx.db.versions.iter()], blobIds = [...new Set(history.map(version => version.blobId))];
  const blobs = blobIds.map(blobId => {
    const upload = tx.db.uploads.id.find(blobId);
    if (!upload || upload.state === "deleting" || !upload.digest) fail("contentUnavailable");
    tx.db.backupPins.insert({ id: `${id}/${blobId}`, backupId: id, blobId });
    return { id: blobId, size: upload.size, digest: upload.digest };
  });
  return JSON.stringify({ format: "dar-backup-v6", agent:agentSnapshot(tx), apps:appSnapshot(tx), workspace:workspaceSnapshot(tx), identity: identitySnapshot(tx), pim: pimSnapshot(tx), id, createdAt: now(tx), blobs,
    accounts: [...tx.db.accounts.iter()], files: [...tx.db.fileEntries.iter()], versions: history,
    grants: [...tx.db.grants.iter()], publicShares: [...tx.db.publicShares.iter()], favorites: [...tx.db.favorites.iter()],
    uploads: blobIds.map(blobId => tx.db.uploads.id.find(blobId)),
    activity: [...tx.db.activity.iter()].map(row => ({ ...row, id: row.id.toString() })),
  });
}));
export const serviceReleaseBackup = db.reducer({ id: t.string() }, (ctx, { id }) => {
  service(ctx); for (const pin of ctx.db.backupPins.backupId.filter(id)) ctx.db.backupPins.id.delete(pin.id);
});
export const serviceExport = db.procedure({ id: t.string(), username: t.string() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = tx.db.accounts.username.find(p.username.toLowerCase());
  if (!user) fail("notFound");
  const files = [...tx.db.fileEntries.ownerId.filter(user.id)].filter(entry => access(tx, entry, user.id));
  const blobs = [...new Set(files.filter(entry => entry.kind === "file").map(entry => entry.blobId))].map(id => {
    const upload = tx.db.uploads.id.find(id);
    if (!upload || !upload.digest || upload.state === "deleting") fail("contentUnavailable");
    tx.db.backupPins.insert({ id: `${p.id}/${id}`, backupId: p.id, blobId: id });
    return { id, size: upload.size, digest: upload.digest };
  });
  return JSON.stringify({ username: user.username, files, blobs });
}));
type Row<K extends "accounts" | "versions" | "grants" | "publicShares" | "favorites" | "uploads"> = NonNullable<ReturnType<Ctx["db"][K]["id"]["find"]>>;
export const serviceRestore = db.reducer({ snapshot: t.string() }, (ctx, { snapshot }) => {
  service(ctx);
  if (ctx.db.accounts.count() !== 0n || ctx.db.fileEntries.count() !== 0n) fail("conflict");
  const value = JSON.parse(snapshot) as { format: string; accounts: Row<"accounts">[]; files: FileEntry[]; versions: Row<"versions">[];
    grants: Row<"grants">[]; publicShares: Row<"publicShares">[]; favorites: Row<"favorites">[]; uploads: Row<"uploads">[];
    activity: { id: string; ownerId: string; fileId: string; action: string; at: number }[]; pim?: ReturnType<typeof pimSnapshot>; identity?: ReturnType<typeof identitySnapshot>;workspace?:ReturnType<typeof workspaceSnapshot>;apps?:ReturnType<typeof appSnapshot>;agent?:ReturnType<typeof agentSnapshot> };
  const formats=["dar-backup-v1", "dar-backup-v2", "dar-backup-v3","dar-backup-v4","dar-backup-v5","dar-backup-v6"],version=formats.indexOf(value.format)+1;
  if (!version||version>=2&&!value.pim||version>=3&&!value.identity||version>=4&&!value.workspace||version>=5&&!value.apps||version>=6&&!value.agent) fail("invalidInput");
  for (const row of value.accounts) {
    ctx.db.accounts.insert(row); ctx.db.usage.insert({ userId: row.id, used: 0, reserved: 0 });
  }
  for (const row of value.files) {
    if (!ctx.db.accounts.id.find(row.ownerId) || nameOf(row.name) !== row.name || findKey(ctx, row.listKey)) fail("invalidInput");
    ctx.db.fileEntries.insert(row);
  }
  for (const row of value.files) {
    let parentId = row.parentId;
    for (let depth = 0; parentId; depth++) {
      const parent = ctx.db.fileEntries.id.find(parentId);
      if (!parent || parent.ownerId !== row.ownerId || parent.kind !== "folder" || parent.id === row.id || depth >= 128) fail("invalidInput");
      parentId = parent.parentId;
    }
  }
  for (const row of value.versions) {
    const entry = ctx.db.fileEntries.id.find(row.fileId);
    if (!entry || entry.kind !== "file") fail("invalidInput");
    ctx.db.versions.insert(row);
    const meter = ctx.db.usage.userId.find(entry.ownerId)!;
    ctx.db.usage.userId.update({ ...meter, used: meter.used + row.size });
  }
  for (const row of value.grants) {
    if (!ctx.db.fileEntries.id.find(row.fileId) || !ctx.db.accounts.id.find(row.userId)) fail("invalidInput");
    ctx.db.grants.insert(row);
  }
  for (const row of value.publicShares) ctx.db.publicShares.insert(row);
  for (const row of value.favorites) ctx.db.favorites.insert(row);
  for (const row of value.uploads) ctx.db.uploads.insert({ ...row, state: "complete" });
  for (const row of value.activity) ctx.db.activity.insert(row);
  restorePim(ctx, value.pim);
  restoreIdentity(ctx,value.identity);
  restoreWorkspace(ctx,value.workspace);
  restoreApps(ctx,value.apps);
  restoreAgent(ctx,value.agent);
});

export const serviceInstalledApps=db.reducer({document:t.string()},(ctx,p)=>{
  service(ctx);const apps=JSON.parse(p.document) as {id:string}[];
  if(!Array.isArray(apps)||apps.length>100||apps.some(a=>!validAppId(a.id)))fail("invalidInput");
  for(const old of ctx.db.installedApps.iter())ctx.db.installedApps.id.delete(old.id);
  for(const app of apps)ctx.db.installedApps.insert({id:app.id,document:JSON.stringify(app)});
});
export const serviceAppSettings=db.procedure({sessionId:t.string(),appId:t.string(),enabled:t.bool(),revision:t.u32()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);
  const settings=appSettings(tx);
  if(!p.appId)return JSON.stringify(settings);
  const user=fromSession(tx,p.sessionId);
  if(!user.administrator||tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");
  if((essentialApps as readonly string[]).includes(p.appId))fail("forbidden");
  if(!["notes","tasks","agent"].includes(p.appId)&&!validAppId(p.appId))fail("invalidInput");
  if(settings.revision!==p.revision)fail("conflict");
  const disabled=settings.disabled.filter(id=>id!==p.appId);if(!p.enabled)disabled.push(p.appId);
  if(disabled.length>200)fail("operationTooLarge");
  const old=tx.db.pimPreferences.userId.find("instance:apps"),row={userId:"instance:apps",revision:settings.revision+1,value:JSON.stringify({disabled})};
  if(old)tx.db.pimPreferences.userId.update(row);else tx.db.pimPreferences.insert(row);
  return JSON.stringify({revision:row.revision,disabled});
}));
export const serviceAppData=db.procedure({sessionId:t.string(),appId:t.string(),operation:t.string(),key:t.string(),revision:t.u32(),schemaVersion:t.u32(),value:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);if(tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");
  return JSON.stringify(appOperation(tx,user.id,p.appId,p.operation,p.key,p.revision,p.schemaVersion,p.value));
}));
export const serviceAgent=db.procedure({sessionId:t.string(),operation:t.string(),document:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);if(tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");
  if(p.document.length>128*1024)fail("operationTooLarge");
  const value:unknown=JSON.parse(p.document);if(!value||typeof value!=="object"||Array.isArray(value))fail("invalidInput");
  return JSON.stringify(agentOperation(tx,user.id,p.operation,value as Record<string,unknown>,entry=>!!access(tx,entry,user.id),p.sessionId));
}));
export const serviceAgentWorker=db.procedure({id:t.string(),lease:t.string(),operation:t.string(),document:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);return JSON.stringify(agentWorker(tx,p.id,p.lease,p.operation,p.document,sid=>fromSession(tx,sid).id,(entry,userId)=>!!access(tx,entry,userId)));
}));
export const serviceGlobalSearch=db.procedure({sessionId:t.string(),query:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);if(tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");
  if(p.query.length>200)fail("invalidInput");if(!p.query.trim())return JSON.stringify({items:[],truncated:false});
  return JSON.stringify(globalSearch(tx,user.id,p.query,entry=>!!access(tx,entry,user.id)));
}));

export const serviceSessions = db.procedure({ sessionId: t.string() }, t.string(), (ctx, { sessionId }) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, sessionId);
  return JSON.stringify([...tx.db.sessions.userId.filter(user.id)].filter(s => s.active && !s.digest.startsWith("protocol:") && s.expiresAt > now(tx)).map(s => ({ id: s.id, createdAt: s.createdAt, expiresAt: s.expiresAt, current: s.id === sessionId })));
}));

export const myPimChanges = db.view({ public: true }, t.option(pimChanges.rowType), ctx => {
  const user = userFor(ctx);
  return user ? ctx.db.pimChanges.userId.find(user.id) ?? { userId: user.id, revision: 0 } : undefined;
});
export const servicePimReminderWork = db.procedure(t.string(), ctx => ctx.withTx(tx => { service(tx); return JSON.stringify(reminderWork(tx)); }));
export const servicePimCommitReminders = db.reducer({ resourceId: t.string(), revision: t.u32(), from: t.f64(), to: t.f64(), reminders: t.string() }, (ctx, p) => {
  service(ctx); commitReminders(ctx, p.resourceId, p.revision, p.from, p.to, JSON.parse(p.reminders));
});
export const servicePimDeferReminder = db.reducer({ resourceId: t.string(), revision: t.u32(), from: t.f64() }, (ctx, p) => { service(ctx); deferReminder(ctx, p.resourceId, p.revision, p.from); });
export const servicePim = db.procedure({ sessionId: t.string(), operation: t.string(), payload: t.string() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, p.sessionId);
  const password = tx.db.appPasswords.sessionId.find(p.sessionId);
  if (password && (password.revoked || password.expiresAt <= now(tx))) fail("unauthenticated");
  return JSON.stringify(pimOperation(tx, user.id, p.operation, JSON.parse(p.payload), password?.scope));
}));
export const serviceWorkspace=db.procedure({sessionId:t.string(),operation:t.string(),payload:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);if(tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");const data=JSON.parse(p.payload);
  if(p.operation!=="batch")return JSON.stringify(noteTrashOperation(tx,user.id,p.operation,data));
  if(!Array.isArray(data.operations)||data.operations.length>500)fail("operationTooLarge");
  for(const [id,revision]of Object.entries(data.collections??{})){const c=tx.db.pimCollections.id.find(id);if(!c||c.revision!==revision)fail("conflict");}
  const result=[];const affected=new Set<string>();
  for(const op of data.operations){if(!["put","delete","move"].includes(op.operation))fail("invalidInput");for(const id of [op.collectionId,op.targetCollectionId])if(id)affected.add(id);result.push(pimOperation(tx,user.id,op.operation,op,undefined,true));}
  for(const id of affected)for(const row of tx.db.pimResources.collectionId.filter(id))validateTaskParent(tx,id,row.uid,row.projection);
  return JSON.stringify({items:result});
}));
/** Personal profile writes cannot rename an account, grant roles or alter security. */
export const serviceProfile=db.procedure({sessionId:t.string(),operation:t.string(),payload:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);if(tx.db.appPasswords.sessionId.find(p.sessionId))fail("forbidden");const old=directoryProfile(tx,user.id)!;
  if(p.operation==="read")return JSON.stringify(old);if(p.operation!=="save")fail("invalidInput");const input=JSON.parse(p.payload);
  if(input.revision!==old.revision)fail("conflict");
  for(const key of ["displayName","givenName","familyName"])if(typeof input[key]!=="string"||input[key].length>100||/[\u0000-\u001f]/.test(input[key]))fail("invalidInput");
  if(!input.displayName.trim()||typeof input.email!=="string"||input.email.length>254||input.email&&!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(input.email))fail("invalidInput");
  const attributes=JSON.parse(tx.db.directoryProfiles.userId.find(user.id)?.document??"{}"),emails=[...(attributes.emails??[])];let position=emails.findIndex((e:{primary?:boolean})=>e.primary);if(position<0)position=0;
  if(input.email)emails.splice(position,emails.length?1:0,{...emails[position],value:input.email,primary:true});else if(emails.length)emails.splice(position,1);
  if(emails.length&&!emails.some(e=>e.primary))emails[0]={...emails[0],primary:true};
  const document={...attributes,userName:user.username,displayName:input.displayName.trim(),active:user.enabled,name:{...attributes.name,givenName:input.givenName,familyName:input.familyName},emails};
  return JSON.stringify(directoryOperation(tx,"saveUser",{id:user.id,revision:old.revision,document:JSON.stringify(document)},{id:user.id,scim:false}));
}));
export const servicePimAuthenticate = db.procedure({ username: t.string(), digest: t.string(), app: t.string(), write: t.bool() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const password = tx.db.appPasswords.digest.find(p.digest);
  const user = password && tx.db.accounts.id.find(password.userId);
  const session = password && tx.db.sessions.id.find(password.sessionId);
  if (!password || password.revoked || password.expiresAt <= now(tx) || !user?.enabled || user.username !== p.username.toLowerCase() || !session?.active) fail("unauthenticated");
  if (!["contacts", "calendar"].includes(p.app) || !passwordAllows(password.scope,p.app,p.write?"write":"read")) fail("forbidden");
  return JSON.stringify({ sessionId: session.id, username: user.username });
}));
export const serviceRevokeOwnedSession = db.reducer({ sessionId: t.string(), id: t.string() }, (ctx, p) => {
  service(ctx); const user = fromSession(ctx, p.sessionId), session = ctx.db.sessions.id.find(p.id);
  if (!session || session.userId !== user.id) fail("notFound");
  ctx.db.sessions.id.update({ ...session, active: false });
  revokeProvider(ctx,undefined,p.id);
});

export const serviceIdentity = db.procedure({ sessionId:t.string(), bearer:t.string(), proof:t.string(), operation:t.string(), payload:t.string() },t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const actor=identityActor(tx,p.sessionId,p.bearer,p.proof,!["authorize","users","user","groups","group","clients","tokens"].includes(p.operation));
  return JSON.stringify(directoryOperation(tx,p.operation,JSON.parse(p.payload),actor));
}));
export const serviceIdentityProof = db.reducer({ sessionId:t.string(), digest:t.string(), expectedVerifier:t.string() },(ctx,p)=>{
  service(ctx);const user=fromSession(ctx,p.sessionId);if(!user.administrator||user.verifier!==p.expectedVerifier)fail("forbidden");
  for(const proof of ctx.db.identityProofs.iter())if(proof.expiresAt<=now(ctx))ctx.db.identityProofs.digest.delete(proof.digest);
  if(ctx.db.identityProofs.count()>=2000n)fail("operationTooLarge");
  ctx.db.identityProofs.insert({digest:p.digest,sessionId:p.sessionId,verifier:user.verifier,expiresAt:now(ctx)+300000});
});
export const serviceProvider = db.procedure({ operation:t.string(), payload:t.string() },t.string(),(ctx,p)=>ctx.withTx(tx=>{service(tx);return JSON.stringify(providerOperation(tx,p.operation,JSON.parse(p.payload)));}));
export const serviceConnectedApplications = db.procedure({sessionId:t.string(),revoke:t.string()},t.string(),(ctx,p)=>ctx.withTx(tx=>{
  service(tx);const user=fromSession(tx,p.sessionId);
  const grants=[...tx.db.providerModels.accountId.filter(user.id)].filter(m=>m.model==="Grant"&&m.expiresAt>now(tx));
  if(p.revoke){const matching=grants.filter(g=>g.clientId===p.revoke);if(!matching.length)fail("notFound");for(const grant of matching)providerOperation(tx,"revokeGrant",{id:grant.id});}
  return JSON.stringify([...new Set(grants.filter(g=>g.clientId!==p.revoke).map(g=>g.clientId))].map(id=>({id,clientId:id,name:JSON.parse(tx.db.identityClients.id.find(id)?.metadata??"{}").client_name})));
}));
export const serviceCreateAppPassword = db.reducer({ sessionId: t.string(), id: t.string(), digest: t.string(), label: t.string(), scope: t.string() }, (ctx, p) => {
  service(ctx); const user = fromSession(ctx, p.sessionId);
  if (!p.label.trim() || p.label.length > 100 || !validPasswordScope(p.scope) || !/^[a-f0-9]{64}$/.test(p.digest)) fail("invalidInput");
  // The internal principal cannot be authenticated by a browser token. Only the
  // service-owned protocol adapter can invoke domain functions on its behalf.
  const protocolSession = unusedUuid(ctx);
  const identity = new Identity(BigInt(`0x${unusedUuid(ctx).replaceAll("-", "")}`));
  const expiresAt = now(ctx) + 365 * 86400_000;
  ctx.db.sessions.insert({ id: protocolSession, userId: user.id, digest: `protocol:${p.id}`, csrfDigest: "", expiresAt, active: true, createdAt: now(ctx) });
  ctx.db.principals.insert({ identity, sessionId: protocolSession });
  ctx.db.appPasswords.insert({ id: p.id, userId: user.id, digest: p.digest, label: p.label.trim(), scope: p.scope, sessionId: protocolSession, principal: identity, expiresAt, revoked: false });
});
export const serviceAppPasswords = db.procedure({ sessionId: t.string() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, p.sessionId);
  return JSON.stringify([...tx.db.appPasswords.userId.filter(user.id)].filter(row => !row.revoked).map(row => ({ id: row.id, label: row.label, scope: row.scope, expiresAt: row.expiresAt })));
}));
export const serviceRevokeAppPassword = db.reducer({ sessionId: t.string(), id: t.string() }, (ctx, p) => {
  service(ctx); const user = fromSession(ctx, p.sessionId), password = ctx.db.appPasswords.id.find(p.id);
  if (!password || password.userId !== user.id) fail("notFound");
  ctx.db.appPasswords.id.update({ ...password, revoked: true });
  const session = ctx.db.sessions.id.find(password.sessionId);
  if (session) ctx.db.sessions.id.update({ ...session, active: false });
});
export const serviceProtocolAuthenticate = db.procedure({ username: t.string(), digest: t.string(), write: t.bool() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const password = tx.db.appPasswords.digest.find(p.digest);
  const user = password && tx.db.accounts.id.find(password.userId);
  const session = password && tx.db.sessions.id.find(password.sessionId);
  if (!password || password.revoked || password.expiresAt <= now(tx) || !user?.enabled || user.username !== p.username.toLowerCase() || !session?.active) fail("unauthenticated");
  if (!passwordAllows(password.scope,"files",p.write?"write":"read")) fail("forbidden");
  return JSON.stringify({ sessionId: password.sessionId, username: user.username });
}));

export const serviceDav = db.procedure({ sessionId: t.string(), method: t.string(), path: t.string(), destination: t.string(), expectedRevision: t.u32(), overwrite: t.bool(), depth: t.u32() }, t.string(), (ctx, p) => ctx.withTx(tx => {
  service(tx); const user = fromSession(tx, p.sessionId), password = tx.db.appPasswords.sessionId.find(p.sessionId);
  if (!password || password.revoked || password.expiresAt <= now(tx)) fail("unauthenticated");
  const permission = ["STAT", "PROPFIND"].includes(p.method) ? "read" : "write";
  // Writes need metadata for revision checks, without granting content reads.
  if (!passwordAllows(password.scope,"files",permission) && !(p.method === "STAT" && passwordAllows(password.scope,"files","write"))) fail("forbidden");
  const actor = new Proxy(tx, { get(target, property) {
    if (property === "sender") return password.principal;
    const value = Reflect.get(target, property, target);
    return typeof value === "function" ? value.bind(target) : value;
  } });
  function resolve(value: string): FileEntry | undefined {
    let parentId = "", result: FileEntry | undefined;
    const parts = value.split("/").filter(Boolean);
    if (parts.length > 128 || value.length > 4096) fail("invalidInput");
    for (let i = 0; i < parts.length; i++) {
      result = findKey(tx, key(user.id, parentId, nameOf(parts[i])));
      if (!result || !access(tx, result, user.id)) return undefined;
      if (i !== parts.length - 1 && result.kind !== "folder") return undefined;
      parentId = result.id;
    }
    return result;
  }
  function parent(value: string) {
    const parts = value.split("/").filter(Boolean), name = nameOf(parts.pop() ?? "");
    const directory = parts.length ? resolve(parts.join("/")) : undefined;
    if (parts.length && (!directory || directory.kind !== "folder")) fail("notFound");
    return { parentId: directory?.id ?? "", name };
  }
  const entry = resolve(p.path);
  if (p.method === "PREPARE") {
    const target = parent(p.path);
    return JSON.stringify({ entry, ...target });
  }
  if (p.method !== "MKCOL" && p.path.split("/").some(Boolean) && !entry) fail("notFound");
  const rootState = tx.db.rootRevisions.userId.find(user.id);
  const rootEntry = { id: "", ownerId: user.id, name: user.username, kind: "folder", size: 0, mediaType: "inode/directory", revision: rootState?.revision ?? 1, blobId: "", modifiedAt: rootState?.modifiedAt ?? user.createdAt };
  if (p.method === "STAT") return JSON.stringify(entry ?? rootEntry);
  if (p.method === "PROPFIND") {
    const root = entry ?? rootEntry;
    const children = [];
    if (p.depth && root.kind === "folder") for (const child of tx.db.fileEntries.listKey.filter(windowOf(`${user.id}/${root.id}/`))) {
      if (children.length >= 10_000) fail("operationTooLarge");
      children.push(child);
    }
    return JSON.stringify({ root, children });
  }
  if (p.method === "MKCOL") {
    if (entry) fail("conflict");
    const target = parent(p.path); createFolder(actor, { id: unusedUuid(tx), ...target });
    return JSON.stringify({ created: true });
  }
  if (!entry) fail("forbidden");
  revision(entry, p.expectedRevision);
  if (p.method === "DELETE") trashEntry(actor, { id: entry.id, expectedRevision: entry.revision });
  else if (p.method === "MOVE" || p.method === "COPY") {
    const target = parent(p.destination), existing = resolve(p.destination);
    if (existing?.id === entry.id) fail("invalidInput");
    if (existing) {
      if (!p.overwrite) fail("conflict");
      trashEntry(actor, { id: existing.id, expectedRevision: existing.revision });
    }
    if (p.method === "MOVE") moveEntry(actor, { id: entry.id, ...target, expectedRevision: entry.revision });
    else copyEntry(actor, { id: entry.id, newId: unusedUuid(tx), ...target, expectedRevision: entry.revision });
  } else fail("invalidInput");
  return JSON.stringify({ ok: true });
}));
