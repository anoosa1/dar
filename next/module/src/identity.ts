import { SenderError, Range } from "spacetimedb/server";
import {deleteNewsAccount} from "./news.ts";
import type { Ctx } from "./index.ts";

function fail(code: string): never { throw new SenderError(code); }
const now = (ctx: Ctx) => Number(ctx.timestamp.toMillis());
export function revokeProvider(ctx: Ctx, userId?: string, sessionId?: string, clientId?: string) {
  const rows = userId ? ctx.db.providerModels.accountId.filter(userId) : clientId ? ctx.db.providerModels.clientId.filter(clientId) : sessionId ? ctx.db.providerModels.nativeSessionId.filter(sessionId) : ctx.db.providerModels.iter();
  for (const row of rows) if (!sessionId || row.nativeSessionId === sessionId) ctx.db.providerModels.id.delete(row.id);
}
export function revokeUser(ctx: Ctx, userId: string) {
  for (const session of ctx.db.sessions.userId.filter(userId)) if (session.active) ctx.db.sessions.id.update({ ...session, active:false });
  for (const password of ctx.db.appPasswords.userId.filter(userId)) if (!password.revoked) ctx.db.appPasswords.id.update({ ...password, revoked:true });
  for (const token of ctx.db.provisioningTokens.ownerId.filter(userId)) if (!token.revoked) ctx.db.provisioningTokens.id.update({ ...token, revoked:true });
  revokeProvider(ctx,userId);
}
export function identityActor(ctx: Ctx, sessionId: string, bearer: string, proof: string, write: boolean) {
  if (bearer) {
    const token=ctx.db.provisioningTokens.digest.find(bearer), owner=token&&ctx.db.accounts.id.find(token.ownerId);
    if(!token||token.revoked||token.expiresAt<=now(ctx)||!owner?.enabled||!owner.administrator)fail("unauthenticated");
    if(write&&token.scope!=="write")fail("forbidden");return {id:owner.id,scim:true};
  }
  const session=ctx.db.sessions.id.find(sessionId),account=session&&ctx.db.accounts.id.find(session.userId);
  if(!session?.active||session.expiresAt<=now(ctx)||!account?.enabled)fail("unauthenticated");
  if(!account.administrator)fail("forbidden");
  if(write){const p=ctx.db.identityProofs.digest.find(proof);if(!p||p.sessionId!==sessionId||p.expiresAt<=now(ctx)||p.verifier!==account.verifier)fail("forbidden");}
  return {id:account.id,scim:false};
}
export function profile(ctx: Ctx,userId: string) {
  const a=ctx.db.accounts.id.find(userId);if(!a)return undefined;
  const p=ctx.db.directoryProfiles.userId.find(userId);if(p?.deleted)return undefined;
  const data=JSON.parse(p?.document??"{}");
  return {...data,id:a.id,userName:a.username,displayName:a.displayName,active:a.enabled,administrator:a.administrator,revision:p?.revision??1,createdAt:a.createdAt,modifiedAt:p?.modifiedAt??a.createdAt,
    groups:[...ctx.db.directoryMembers.userId.filter(a.id)].map(m=>({value:m.groupId,display:JSON.parse(ctx.db.directoryGroups.id.find(m.groupId)?.document??"{}").displayName}))};
}
function expected(actual:number,value:unknown){if(actual!==value)fail("conflict");}
export function touchProfile(ctx:Ctx,id:string){const old=ctx.db.directoryProfiles.userId.find(id),row={userId:id,document:old?.document??"{}",deleted:old?.deleted??false,revision:(old?.revision??1)+1,modifiedAt:now(ctx)};if(old)ctx.db.directoryProfiles.userId.update(row);else ctx.db.directoryProfiles.insert(row);}
export function directoryOperation(ctx: Ctx, operation: string,p:Record<string,unknown>, actor:{id:string;scim:boolean}) {
  if(operation==="authorize"||operation==="authorizeWrite")return {ok:true};
  if(operation==="users")return [...ctx.db.accounts.iter()].map(a=>profile(ctx,a.id)).filter(Boolean);
  if(operation==="user") {const result=profile(ctx,String(p.id));if(!result)fail("notFound");return result;}
  if(operation==="groups")return [...ctx.db.directoryGroups.iter()].map(g=>group(ctx,g.id));
  if(operation==="group")return group(ctx,String(p.id));
  if(operation==="saveUser") {
    const id=String(p.id), old=ctx.db.accounts.id.find(id), previous=ctx.db.directoryProfiles.userId.find(id);
    if(previous?.deleted)fail("notFound");expected(old?(previous?.revision??1):0,p.revision);
    const document=JSON.parse(String(p.document)),username=String(document.userName).toLowerCase();
    if(old?.administrator&&(username!==old.username||document.active!==old.enabled||p.verifier!==undefined))fail("forbidden");
    if(!/^[a-z0-9][a-z0-9_.-]{2,63}$/.test(username)||!String(document.displayName).trim()||String(document.displayName).length>100)fail("invalidInput");
    const duplicate=ctx.db.accounts.username.find(username);if(duplicate&&duplicate.id!==id)fail("conflict");
    if(!old&&ctx.db.accounts.count()>=10000n)fail("operationTooLarge");
    const row={id,username,displayName:String(document.displayName),verifier:typeof p.verifier==="string"?p.verifier:old?.verifier??"",administrator:old?.administrator??false,enabled:document.active!==false,createdAt:old?.createdAt??now(ctx)};
    const {userName:_u,displayName:_d,active:_a,...attributes}=document;
    const next={userId:id,document:JSON.stringify(attributes),revision:(previous?.revision??(old?1:0))+1,modifiedAt:now(ctx),deleted:false};
    if(old)ctx.db.accounts.id.update(row);else {ctx.db.accounts.insert(row);ctx.db.usage.insert({userId:id,used:0,reserved:0});}
    if(previous)ctx.db.directoryProfiles.userId.update(next);else ctx.db.directoryProfiles.insert(next);
    if(old&&old.displayName!==row.displayName)for(const m of ctx.db.directoryMembers.userId.filter(id)){const g=ctx.db.directoryGroups.id.find(m.groupId)!;ctx.db.directoryGroups.id.update({...g,revision:g.revision+1,modifiedAt:now(ctx)});}
    if(!row.enabled||old&&row.verifier!==old.verifier)revokeUser(ctx,id);
    return profile(ctx,id);
  }
  if(operation==="deleteUser") {
    const a=ctx.db.accounts.id.find(String(p.id)),old=a&&ctx.db.directoryProfiles.userId.find(a.id);
    if(!a||old?.deleted)fail("notFound");if(a.administrator)fail("forbidden");expected(old?.revision??1,p.revision);
    revokeUser(ctx,a.id);ctx.db.accounts.id.update({...a,enabled:false});
    for(const record of ctx.db.appData.ownerId.filter(a.id))ctx.db.appData.id.delete(record.id);
    for(const row of ctx.db.agentMessages.ownerId.filter(a.id))ctx.db.agentMessages.id.delete(row.id);
    for(const row of ctx.db.agentRuns.ownerId.filter(a.id))ctx.db.agentRuns.id.delete(row.id);
    for(const row of ctx.db.agentThreads.ownerId.filter(a.id))ctx.db.agentThreads.id.delete(row.id);
    for(const row of ctx.db.agentProjects.ownerId.filter(a.id))ctx.db.agentProjects.id.delete(row.id);
    deleteNewsAccount(ctx,a.id);
    const row={userId:a.id,document:old?.document??"{}",revision:(old?.revision??1)+1,modifiedAt:now(ctx),deleted:true};
    if(old)ctx.db.directoryProfiles.userId.update(row);else ctx.db.directoryProfiles.insert(row);
    for(const member of ctx.db.directoryMembers.userId.filter(a.id)){ctx.db.directoryMembers.id.delete(member.id);const g=ctx.db.directoryGroups.id.find(member.groupId)!;ctx.db.directoryGroups.id.update({...g,revision:g.revision+1,modifiedAt:now(ctx)});}
    return {ok:true};
  }
  if(operation==="saveGroup") {
    const id=String(p.id),old=ctx.db.directoryGroups.id.find(id);expected(old?.revision??0,p.revision);
    const data=JSON.parse(String(p.document)),members=data.members??[];
    if(!old&&ctx.db.directoryGroups.count()>=2000n)fail("operationTooLarge");
    if(!String(data.displayName??"").trim()||members.length>2000)fail("invalidInput");
    for(const member of members)if(!profile(ctx,String(member.value)))fail("invalidInput");
    const {members:_m,...attributes}=data,row={id,document:JSON.stringify(attributes),revision:(old?.revision??0)+1,createdAt:old?.createdAt??now(ctx),modifiedAt:now(ctx)};
    if(old)ctx.db.directoryGroups.id.update(row);else ctx.db.directoryGroups.insert(row);
    const affected=new Set<string>(members.map((m:{value:string})=>m.value));
    for(const m of ctx.db.directoryMembers.groupId.filter(id)){affected.add(m.userId);ctx.db.directoryMembers.id.delete(m.id);}
    for(const userId of new Set<string>(members.map((m:{value:string})=>m.value)))ctx.db.directoryMembers.insert({id:`${id}/${userId}`,groupId:id,userId});
    for(const userId of affected)touchProfile(ctx,userId);
    return group(ctx,id);
  }
  if(operation==="deleteGroup") {
    const g=ctx.db.directoryGroups.id.find(String(p.id));if(!g)fail("notFound");expected(g.revision,p.revision);
    for(const member of ctx.db.directoryMembers.groupId.filter(g.id)){touchProfile(ctx,member.userId);ctx.db.directoryMembers.id.delete(member.id);}ctx.db.directoryGroups.id.delete(g.id);return {ok:true};
  }
  if(actor.scim)fail("forbidden");
  if(operation==="adminWrite")return {ok:true};
  if(operation==="clients")return [...ctx.db.identityClients.iter()].map(({encrypted:_e,...row})=>row);
  if(operation==="saveClient") {
    const old=ctx.db.identityClients.id.find(String(p.id));expected(old?.revision??0,p.revision);
    if(!old&&ctx.db.identityClients.count()>=200n)fail("operationTooLarge");
    const row={id:String(p.id),metadata:String(p.metadata),encrypted:String(p.encrypted),enabled:p.enabled!==false,revision:(old?.revision??0)+1,modifiedAt:now(ctx)};
    if(old){ctx.db.identityClients.id.update(row);revokeProvider(ctx,undefined,undefined,row.id);}else ctx.db.identityClients.insert(row);return {...row,encrypted:undefined};
  }
  if(operation==="deleteClient") {const c=ctx.db.identityClients.id.find(String(p.id));if(!c)fail("notFound");expected(c.revision,p.revision);revokeProvider(ctx,undefined,undefined,c.id);ctx.db.identityClients.id.delete(c.id);return {ok:true};}
  if(operation==="tokens")return [...ctx.db.provisioningTokens.iter()].map(({digest:_d,...row})=>row);
  if(operation==="createToken") {
    if(ctx.db.provisioningTokens.count()>=1000n)for(const row of ctx.db.provisioningTokens.iter())if(row.revoked||row.expiresAt<=now(ctx)){ctx.db.provisioningTokens.id.delete(row.id);if(ctx.db.provisioningTokens.count()<1000n)break;}
    if(!["read","write"].includes(String(p.scope))||Number(p.expiresAt)<=now(ctx)||Number(p.expiresAt)>now(ctx)+366*86400000||ctx.db.provisioningTokens.count()>=1000n)fail("invalidInput");ctx.db.provisioningTokens.insert({id:String(p.id),digest:String(p.digest),ownerId:actor.id,label:String(p.label),scope:String(p.scope),expiresAt:Number(p.expiresAt),revoked:false});return {ok:true};
  }
  if(operation==="revokeToken") {const token=ctx.db.provisioningTokens.id.find(String(p.id));if(!token)fail("notFound");ctx.db.provisioningTokens.id.update({...token,revoked:true});return {ok:true};}
  fail("invalidInput");
}
function group(ctx:Ctx,id:string) {
  const row=ctx.db.directoryGroups.id.find(id);if(!row)fail("notFound");
  return {...JSON.parse(row.document),id,revision:row.revision,createdAt:row.createdAt,modifiedAt:row.modifiedAt,members:[...ctx.db.directoryMembers.groupId.filter(id)].map(m=>({value:m.userId,display:ctx.db.accounts.id.find(m.userId)?.displayName}))};
}
export function providerOperation(ctx:Ctx,operation:string,p:Record<string,unknown>) {
  if(operation==="saveClient")return directoryOperation(ctx,operation,p,{id:"",scim:false});
  const id=String(p.id??"");
  if(operation==="client")return ctx.db.identityClients.id.find(id);
  if(operation==="account")return profile(ctx,id);
  if(operation==="session") {const s=ctx.db.sessions.id.find(id);return s?.active&&s.expiresAt>now(ctx)&&ctx.db.accounts.id.find(s.userId)?.enabled?s:null;}
  if(operation==="find"||operation==="findUid"||operation==="findUserCode") {
    const row=operation==="find"?ctx.db.providerModels.id.find(id):[...ctx.db.providerModels[operation==="findUid"?"uidKey":"userCodeKey"].filter(id)][0];
    if(!row||row.expiresAt<=now(ctx))return null;
    if(row.accountId&&!profile(ctx,row.accountId)?.active)return null;
    if(row.clientId&&!ctx.db.identityClients.id.find(row.clientId)?.enabled)return null;
    if(row.grantKey&&(ctx.db.providerModels.id.find(row.grantKey)?.expiresAt??0)<=now(ctx))return null;
    if(row.nativeSessionId){const s=ctx.db.sessions.id.find(row.nativeSessionId);if(!s?.active||s.expiresAt<=now(ctx))return null;}
    return row;
  }
  if(operation==="upsert") {
    const row=p as unknown as NonNullable<ReturnType<Ctx["db"]["providerModels"]["id"]["find"]>>;
    if(row.grantKey&&(ctx.db.providerModels.id.find(row.grantKey)?.expiresAt??0)<=now(ctx))fail("conflict");
    if(row.accountId&&!profile(ctx,row.accountId)?.active)fail("unauthenticated");
    if(row.nativeSessionId){const s=ctx.db.sessions.id.find(row.nativeSessionId);if(!s?.active||s.expiresAt<=now(ctx)||row.accountId&&row.accountId!==s.userId)fail("unauthenticated");}
    if(row.clientId&&!ctx.db.identityClients.id.find(row.clientId)?.enabled)fail("forbidden");
    const old=ctx.db.providerModels.id.find(id);if(!old&&ctx.db.providerModels.count()>=50000n)fail("operationTooLarge");
    if(old)ctx.db.providerModels.id.update({...row,consumed:Math.max(row.consumed,old.consumed)});else ctx.db.providerModels.insert(row);return {ok:true};
  }
  if(operation==="consume") {const row=ctx.db.providerModels.id.find(id);if(!row||row.consumed||row.expiresAt<=now(ctx))fail("conflict");ctx.db.providerModels.id.update({...row,consumed:Math.floor(now(ctx)/1000)});return {ok:true};}
  if(operation==="destroy") {ctx.db.providerModels.id.delete(id);return {ok:true};}
  if(operation==="revokeGrant") {for(const row of ctx.db.providerModels.grantKey.filter(id))ctx.db.providerModels.id.delete(row.id);ctx.db.providerModels.id.delete(id);return {ok:true};}
  if(operation==="cleanup") {let count=0;for(const row of ctx.db.providerModels.expiryKey.filter(new Range({tag:"unbounded"},{tag:"included",value:String(now(ctx)).padStart(16,"0")}))){if(++count>1000)break;ctx.db.providerModels.id.delete(row.id);}for(const row of ctx.db.identityProofs.iter())if(row.expiresAt<=now(ctx))ctx.db.identityProofs.digest.delete(row.digest);return {ok:true};}
  fail("invalidInput");
}
export function identitySnapshot(ctx:Ctx) {return {profiles:[...ctx.db.directoryProfiles.iter()],groups:[...ctx.db.directoryGroups.iter()],members:[...ctx.db.directoryMembers.iter()],clients:[...ctx.db.identityClients.iter()]};}
export function restoreIdentity(ctx:Ctx,data:ReturnType<typeof identitySnapshot>|undefined) {
  if(!data)return;
  for(const row of data.profiles){if(!ctx.db.accounts.id.find(row.userId))fail("invalidInput");ctx.db.directoryProfiles.insert(row);}
  for(const row of data.groups)ctx.db.directoryGroups.insert(row);
  for(const row of data.members){if(!ctx.db.accounts.id.find(row.userId)||!ctx.db.directoryGroups.id.find(row.groupId))fail("invalidInput");ctx.db.directoryMembers.insert(row);}
  for(const row of data.clients)ctx.db.identityClients.insert(row);
}
