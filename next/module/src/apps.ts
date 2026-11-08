import {SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {validAppId,validAppKey} from "../../shared/installed-apps.ts";
import {appEnabled} from "./app-settings.ts";
function fail(code:string):never{throw new SenderError(code);}
const bytes=(s:string)=>encodeURIComponent(s).replace(/%[A-F\d]{2}/g,"x").length;
export function appOperation(ctx:Ctx,userId:string,appId:string,operation:string,key:string,revision:number,schemaVersion:number,value:string){
  const app=ctx.db.installedApps.id.find(appId);if(!app||!appEnabled(ctx,appId))fail("notFound");
  const permissions=JSON.parse(app.document).permissions as string[];
  if(!permissions.includes(operation==="put"||operation==="delete"?"ownUserData:write":"ownUserData:read"))fail("forbidden");
  const ownerKey=JSON.stringify([appId,userId]),id=JSON.stringify([appId,userId,key]);
  const expose=(row:NonNullable<ReturnType<Ctx["db"]["appData"]["id"]["find"]>>)=>({key:row.key,revision:row.revision,schemaVersion:row.schemaVersion,value:JSON.parse(row.value),modifiedAt:row.modifiedAt});
  if(operation==="list"){
    const rows=[...ctx.db.appData.ownerKey.filter(ownerKey)].filter(r=>r.key>key).sort((a,b)=>a.key<b.key?-1:1);
    return {items:rows.slice(0,50).map(expose),cursor:rows.length>50?rows[49].key:null};
  }
  if(!validAppKey(key))fail("invalidInput");const old=ctx.db.appData.id.find(id);
  if(operation==="get")return old?expose(old):null;
  if((old?.revision??0)!==revision)fail("conflict");
  if(operation==="delete"){if(old)ctx.db.appData.id.delete(id);return {ok:true};}
  if(operation!=="put"||schemaVersion<1||schemaVersion>1000000)fail("invalidInput");
  const parsed=JSON.parse(value);if(parsed===undefined||bytes(value)>65536)fail("operationTooLarge");
  const rows=[...ctx.db.appData.ownerKey.filter(ownerKey)];
  if((!old&&rows.length>=128)||rows.reduce((n,r)=>n+bytes(r.value),0)-bytes(old?.value??"")+bytes(value)>1048576)fail("quotaExceeded");
  const row={id,appId,ownerId:userId,ownerKey,key,revision:revision+1,schemaVersion,value,modifiedAt:Number(ctx.timestamp.toMillis())};
  if(old)ctx.db.appData.id.update(row);else ctx.db.appData.insert(row);return expose(row);
}
export function appSnapshot(ctx:Ctx){return {inventory:[...ctx.db.installedApps.iter()],data:[...ctx.db.appData.iter()]};}
export function restoreApps(ctx:Ctx,value?:ReturnType<typeof appSnapshot>){
  if(!value)return;
  // Inventory is informational on restore: only configured packages can activate code.
  for(const row of value.data){
    if(!validAppId(row.appId)||!validAppKey(row.key)||!ctx.db.accounts.id.find(row.ownerId)||row.id!==JSON.stringify([row.appId,row.ownerId,row.key])||row.ownerKey!==JSON.stringify([row.appId,row.ownerId])||!Number.isInteger(row.revision)||row.revision<1||row.schemaVersion<1||bytes(row.value)>65536)fail("invalidInput");
    JSON.parse(row.value);ctx.db.appData.insert(row);
  }
}
