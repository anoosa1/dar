import {Range} from "spacetimedb/server";
import {pimOperation} from "./pim.ts";
import type {Ctx} from "./index.ts";
import type {SearchGroup,SearchResult,SearchResults} from "../../shared/search.ts";
type FileEntry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;
/** Search only account-owned or currently granted scopes. Bound work and response size. */
export function globalSearch(ctx:Ctx,userId:string,query:string,canRead:(entry:FileEntry)=>boolean):SearchResults{
  const q=query.normalize("NFC").toLowerCase().trim(),groups=new Map<SearchGroup,SearchResult[]>();let remaining=20000,truncated=false;
  const visit=()=>{if(remaining--<=0){truncated=true;return false;}return true;};
  const matches=(value:string)=>value.normalize("NFC").toLowerCase().includes(q);
  function add(result:SearchResult){const list=groups.get(result.group)??[];if(list.some(r=>r.id===result.id))return;list.push(result);list.sort((a,b)=>Number(!a.title.toLowerCase().startsWith(q))-Number(!b.title.toLowerCase().startsWith(q))||a.title.localeCompare(b.title)||a.id.localeCompare(b.id));if(list.length>8){list.pop();truncated=true;}groups.set(result.group,list);}
  const preferences=JSON.parse(ctx.db.pimPreferences.userId.find(userId)?.value??"{}");
  const seen=new Set<string>();
  function file(entry:FileEntry){if(seen.has(entry.id))return;seen.add(entry.id);if(entry.trashed||!canRead(entry)||preferences.filesShowHidden!==true&&entry.name.startsWith("."))return;
    if(matches(entry.name))add({id:entry.id,group:"Files",title:entry.name,detail:entry.kind==="folder"?"Folder":"File",href:entry.kind==="folder"?`/?folder=${entry.id}`:entry.ownerId===userId?`/?folder=${entry.parentId}&file=${entry.id}`:`/?section=shared&file=${entry.id}`});
  }
  for(const entry of ctx.db.fileEntries.ownerId.filter(userId)){if(!visit())break;file(entry);}
  const folders:string[]=[];
  for(const grant of ctx.db.grants.userId.filter(userId)){if(!visit())break;const entry=ctx.db.fileEntries.id.find(grant.fileId);if(entry){file(entry);if(entry.kind==="folder"&&canRead(entry))folders.push(entry.id);}}
  const walked=new Set<string>();for(let i=0;i<folders.length;i++){const id=folders[i];if(walked.has(id))continue;walked.add(id);const folder=ctx.db.fileEntries.id.find(id);if(!folder)continue;
    const prefix=`${folder.ownerId}/${id}/`,range=new Range<string>({tag:"included",value:prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
    for(const entry of ctx.db.fileEntries.listKey.filter(range)){if(!visit())break;file(entry);if(entry.kind==="folder"&&!entry.trashed&&canRead(entry))folders.push(entry.id);}if(remaining<=0)break;
  }
  const collections=pimOperation(ctx,userId,"collections",{}) as {id:string;kind:string;name:string}[];
  for(const collection of collections){for(const row of ctx.db.pimResources.collectionId.filter(collection.id)){
    if(!visit())break;const data=JSON.parse(row.projection) as Record<string,unknown>;
    if(!matches(row.projection+(collection.kind==="notes"?"\n"+row.content:"")))continue;
    const group:SearchGroup=collection.kind==="notes"?"Notes":collection.kind==="contacts"?"Contacts":data.kind==="task"?"Tasks":"Calendar";
    const title=String(data.displayName??data.title??"Untitled"),date=String(data.start??"").slice(0,10);
    const href=group==="Notes"||group==="Tasks"?`/${group.toLowerCase()}?item=${row.id}`:group==="Contacts"?`/contacts?book=${collection.id}&contact=${encodeURIComponent(row.href)}`:`/calendar?book=${collection.id}&event=${encodeURIComponent(row.href)}${/^\d{4}-\d\d-\d\d$/.test(date)?`&date=${date}`:""}`;
    add({id:row.id,group,title,detail:collection.name,href});
  }if(remaining<=0)break;}
  for(const app of ctx.db.installedApps.iter()){const data=JSON.parse(app.document) as {name:string;permissions:string[]};
    if(matches(data.name))add({id:app.id,group:"Applications",title:data.name,detail:"Installed app",href:`/apps/${app.id}`});
    if(data.permissions.includes("ownUserData:read"))for(const row of ctx.db.appData.ownerKey.filter(JSON.stringify([app.id,userId]))){if(!visit())break;if(matches(row.value))add({id:app.id,group:"Applications",title:data.name,detail:"Matching saved app data",href:`/apps/${app.id}`});}
  }
  for(const [name,href]of [["Files","/"],["Contacts","/contacts"],["Calendar","/calendar"],["Notes","/notes"],["Tasks","/tasks"],["Settings","/settings"]])if(matches(name))add({id:href,group:"Applications",title:name,detail:"Open app",href});
  return {items:[...groups.values()].flat(),truncated};
}
