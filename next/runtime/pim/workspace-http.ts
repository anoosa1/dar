import {randomUUID} from "node:crypto";
import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {pimService,pimEtag} from "./service.ts";
import {requireMatch} from "./http.ts";
import type {PimResource,PimCollection} from "../../shared/pim.ts";
import {timezone} from "./calendar.ts";
import {field,invalid} from "./standards.ts";
import {isTask,readTask,writeTask,taskOccurrences,nextTaskOccurrence,taskDue} from "./tasks.ts";
import {readNote} from "./notes.ts";
import type {NoteData,TaskData} from "../../shared/workspace.ts";

export async function handleWorkspaceHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sid:string,body:()=>Promise<Record<string,unknown>>){
  const viewerZone=timezone(url.searchParams.get("timezone")??"UTC");
  const notes=url.pathname.startsWith("/api/v1/notes"),base=notes?"/api/v1/notes":"/api/v1/tasks",kind=notes?"notes":"calendar",path=url.pathname.slice(base.length),method=req.method??"GET",service=pimService(db,sid);
  const workspace=<T>(operation:string,p:object={})=>db.call<T>("service_workspace",[sid,operation,JSON.stringify(p)]);
  const json=(value:unknown,status=200,row?:{id:string;revision:number})=>{if(row)res.setHeader("etag",pimEtag(row));res.writeHead(status,{"content-type":"application/json","cache-control":"no-store"});res.end(JSON.stringify(value,(key,v)=>["modifiedAt","trashedAt"].includes(key)&&typeof v==="number"?new Date(v).toISOString():v));};
  const send=(content:string,name:string,media:string)=>{res.writeHead(200,{"content-type":media,"content-disposition":`attachment; filename="${name}"`,"cache-control":"no-store"});res.end(content);};
  if(notes&&path==="/trash"&&method==="GET")return json(await workspace("trash",{cursor:url.searchParams.get("cursor")??""}));
  const trash=/^\/trash\/([^/]+)(?:\/(restore))?$/.exec(path);
  if(notes&&trash){const rows=await workspace<{items:{id:string;resource:PimResource}[]}>("trash",{id:trash[1]}),row=rows.items.find(r=>r.id===trash[1]);if(!row)throw new AppError("notFound",404);if(method==="GET")return json(row,200,row.resource);if(method==="DELETE"||method==="POST"&&trash[2])return json(await workspace(trash[2]?"restore":"purge",{...trash[2]?await body():{},id:row.id,revision:requireMatch(req,row.resource)}));}
  const collections=(await service.collections()).filter(c=>c.kind===kind),collectionName=notes?"notebooks":"lists";
  const group=new RegExp(`^/${collectionName}(?:/([^/]+))?(?:/(grants|export|import))?$`).exec(path);
  if(group){const [,id,sub]=group;if(!id){if(method==="GET")return json({items:collections,nextCursor:""});if(method==="POST"){const p=await body(),row=await service.call<PimCollection>("createCollection",{id:randomUUID(),kind,name:field(p.name,255),color:field(p.color??"#2563eb",7)});return json(row,201,row);}}
    const c=collections.find(c=>c.id===id);if(!c)throw new AppError("notFound",404);
    if(sub==="grants"){if(method==="GET")return json({items:await service.call("grants",{collectionId:id}),nextCursor:""});if(["POST","DELETE"].includes(method))return json(await service.call(method==="POST"?"share":"revoke",{...await body(),collectionId:id,revision:requireMatch(req,c)}));}
    if(sub==="export"&&method==="GET"){if(!notes)return send(await service.exportText(id),"tasks.ics","text/calendar; charset=utf-8");const rows=(await service.list(id)).items;const archive=JSON.stringify({format:"dar-notebook-v1",name:c.name,items:rows.map(r=>({...JSON.parse(r.projection),content:r.content}))});if(rows.length>500||Buffer.byteLength(archive)>2*1024*1024)invalid("Notebook export exceeds 500 notes or 2 MiB; export individual notes.");return send(archive,"notebook.json","application/json; charset=utf-8");}
    if(sub==="import"&&method==="POST"){const p=await body();if(!notes)return json(await service.importText(c,field(p.content,2*1024*1024)));if(!Array.isArray(p.items)||p.items.length>500)invalid("Import at most 500 notes.");const result=[];for(const input of p.items){try{const value=readNote(input),row=await service.put(c,`${randomUUID()}.md`,{...value.data,content:value.content},0);result.push({id:row.id});}catch(e){result.push({error:e instanceof Error?e.message:"Import failed"});}}return json({items:result});}
    if(!sub){if(method==="GET")return json(c,200,c);if(["PUT","DELETE"].includes(method))return json(await service.call(method==="PUT"?"renameCollection":"deleteCollection",{...method==="PUT"?await body():{},collectionId:id,revision:requireMatch(req,c)}));}
  }
  const item=/^\/items(?:\/([^/]+))?(?:\/(export|move|copy|complete|occurrences))?$/.exec(path);if(!item)throw new AppError("notFound",404);
  const [,id,action]=item;
  if(!id&&method==="GET"){
    const cursor=Buffer.from(url.searchParams.get("cursor")??"","base64url").toString("utf8"),cid=url.searchParams.get("collection"),view=url.searchParams.get("view"),sort=url.searchParams.get("sort")??"modified",tag=url.searchParams.get("tag")??"",q=(url.searchParams.get("q")??"").normalize("NFC").toLowerCase();
    if(notes){
      const result=await service.call<{items:unknown[];nextCursor:string}>("search",{kind:"notes",collection:cid,q,tag,favorite:view==="favorites",sort,cursor});
      return json({items:result.items,nextCursor:result.nextCursor?Buffer.from(result.nextCursor).toString("base64url"):""});
    }
    const today=new Intl.DateTimeFormat("en-CA",{timeZone:viewerZone,year:"numeric",month:"2-digit",day:"2-digit"}).format(new Date());
    type Candidate={key:string;row:Omit<PimResource,"content">&{data:TaskData;role:string;collectionName:string;overdue:boolean;nextOccurrence:ReturnType<typeof nextTaskOccurrence>|undefined}};
    const results:Candidate[]=[];
    // Recurrence needs the standards adapter. Stream bounded database pages and
    // retain only this response's best 201 candidates, computing each key once.
    for(const c of collections){
      if(cid&&c.id!==cid)continue;
      let scanCursor="";
      do{
        const page=await service.call<{items:PimResource[];nextCursor:string}>("page",{collectionId:c.id,cursor:scanCursor});scanCursor=page.nextCursor;
        for(const row of page.items){
          const data=JSON.parse(row.projection) as TaskData;if(data.kind!=="task")continue;
          if(q&&!row.projection.normalize("NFC").toLowerCase().includes(q)||tag&&!data.categories.includes(tag))continue;
          const nextOccurrence=data.recurring?nextTaskOccurrence(row,viewerZone):undefined,d=nextOccurrence??data,due=taskDue(row,viewerZone,nextOccurrence),done=d.status==="COMPLETED";
          if(!(view==="completed"?done:view==="all"?true:!done&&d.status!=="CANCELLED"&&(view==="today"?!!d.due&&due.day<=today:view==="upcoming"?!!d.due&&due.day>today:view==="important"?d.priority>0&&d.priority<=4:view==="inbox"?!d.parentUid:true)))continue;
          const key=`${sort==="title"?data.title.toLowerCase():sort==="due"?due.sort:String(9999999999999-Number(row.modifiedAt)).padStart(13,"0")}\u0000${row.id}`;
          if(key<=cursor)continue;
          let lo=0,hi=results.length;while(lo<hi){const mid=(lo+hi)>>>1;if(results[mid].key<key)lo=mid+1;else hi=mid;}
          if(lo>200)continue;
          const {content,...metadata}=row;
          results.splice(lo,0,{key,row:{...metadata,data,role:c.role,collectionName:c.name,overdue:!!due.day&&due.day<today&&!["COMPLETED","CANCELLED"].includes(data.status),nextOccurrence}});if(results.length>201)results.pop();
        }
      }while(scanCursor);
    }
    return json({items:results.slice(0,200).map(r=>r.row),nextCursor:results.length>200?Buffer.from(results[199].key).toString("base64url"):""});
  }
  if(!id&&method==="POST"){const p=await body(),c=collections.find(c=>c.id===p.collectionId);if(!c)throw new AppError("notFound",404);const row=await service.put(c,`${randomUUID()}.${notes?"md":"ics"}`,notes?p:{component:"task",data:p.data??p},0);return json({...row,data:JSON.parse(row.projection)},201,row);}
  const row=await service.call<PimResource>("resource",{id}),c=collections.find(c=>c.id===row.collectionId);
  if(!c||!notes&&!isTask(row.content))throw new AppError("notFound",404);
  const found={row,c},rows:{row:PimResource;c:PimCollection}[]=!notes&&(!action&&method==="GET"||method==="DELETE"||["move","copy","complete"].includes(action))?(await service.list(c.id)).items.filter(r=>isTask(r.content)).map(row=>({row,c})):[];
  if(!action&&method==="GET")return json({...row,data:JSON.parse(row.projection),role:c.role,...!notes?{nextOccurrence:JSON.parse(row.projection).recurring?nextTaskOccurrence(row,"UTC"):undefined,children:rows.filter(r=>r.c.id===c.id&&(JSON.parse(r.row.projection) as TaskData).parentUid===row.uid).map(r=>({id:r.row.id,uid:r.row.uid,title:JSON.parse(r.row.projection).title,status:JSON.parse(r.row.projection).status})),parentId:rows.find(r=>r.c.id===c.id&&r.row.uid===JSON.parse(row.projection).parentUid)?.row.id}:{}},200,row);
  if(!notes&&action==="complete"&&method==="POST"){
    requireMatch(req,row);const p=await body(),selected=[found],seen=new Set([row.uid]);for(let i=0;i<selected.length;i++)for(const child of rows)if(child.c.id===c.id&&!seen.has(child.row.uid)&&JSON.parse(child.row.projection).parentUid===selected[i].row.uid){seen.add(child.row.uid);selected.push(child);if(selected.length>500)invalid("Task subtree exceeds 500 items.");}
    if(selected.length>1&&p.children!=="complete")throw new AppError("conflict",409,"Choose whether to complete all subtasks.");
    const operations=selected.reverse().map(entry=>{const next=writeTask({status:"COMPLETED",progress:100},entry.row.content);return {operation:"put",collectionId:c.id,href:entry.row.href,id:entry.row.id,uid:entry.row.uid,revision:entry.row.revision,content:next.content,projection:JSON.stringify(next.data)};});return json(await workspace("batch",{collections:{[c.id]:c.revision},operations}));
  }
  if(!notes&&action==="occurrences"&&method==="GET")return json({items:taskOccurrences(row,url.searchParams.get("timezone")??"UTC",new Date(url.searchParams.get("from")??""),new Date(url.searchParams.get("to")??"")),nextCursor:""});
  if(action==="export"&&method==="GET")return send(row.content,notes?"note.md":"task.ics",notes?"text/markdown; charset=utf-8":"text/calendar; charset=utf-8");
  if(!action&&method==="PUT"){const p=await body(),updated=await service.put(c,row.href,notes?p:{component:"task",...(p.data||p.content?p:{data:p})},requireMatch(req,row),row.content);return json({...updated,data:JSON.parse(updated.projection)},200,updated);}
  if(!action&&method==="DELETE"){
    requireMatch(req,row);if(notes)return json(await service.call("delete",{collectionId:c.id,href:row.href,revision:row.revision}));
    const direct=rows.filter(x=>x.c.id===c.id&&(JSON.parse(x.row.projection) as TaskData).parentUid===row.uid);
    if(!direct.length)return json(await service.call("delete",{collectionId:c.id,href:row.href,revision:row.revision}));
    if(!req.headers["content-length"]&&!req.headers["transfer-encoding"])throw new AppError("conflict",409,"Choose whether to detach or delete subtasks.");
    const p=await body(),operations:Record<string,unknown>[]=[];
    if(p.children==="detach")for(const child of direct){const next=writeTask({parentUid:""},child.row.content);operations.push({operation:"put",collectionId:c.id,href:child.row.href,id:child.row.id,uid:child.row.uid,revision:child.row.revision,content:next.content,projection:JSON.stringify(next.data)});}
    else if(p.children==="delete"){const selected=[found],seen=new Set([row.uid]);for(let i=0;i<selected.length;i++)for(const child of rows)if(child.c.id===c.id&&!seen.has(child.row.uid)&&(JSON.parse(child.row.projection) as TaskData).parentUid===selected[i].row.uid){seen.add(child.row.uid);selected.push(child);if(selected.length>499)invalid("Task subtree exceeds 500 items.");}for(const entry of selected.slice(1).reverse())operations.push({operation:"delete",collectionId:c.id,href:entry.row.href,revision:entry.row.revision});}
    else invalid("Choose detach or delete for subtasks.");
    operations.push({operation:"delete",collectionId:c.id,href:row.href,revision:row.revision});return json(await workspace("batch",{collections:{[c.id]:c.revision},operations}));
  }
  if(["move","copy"].includes(action)&&method==="POST"){
    const p=await body();requireMatch(req,row);const target=collections.find(t=>t.id===p.collectionId);if(!target)throw new AppError("notFound",404);if(action==="move"&&target.ownerId!==c.ownerId)throw new AppError("forbidden",403);const operations:Record<string,unknown>[]=[];
    if(notes){if(action==="copy"){const d=JSON.parse(row.projection);const result=await service.put(target,`${randomUUID()}.md`,{...d,title:`${d.title.slice(0,248)} (copy)`,content:row.content},0);return json(result,201,result);}operations.push({operation:"move",collectionId:c.id,targetCollectionId:target.id,href:row.href,revision:row.revision});}
    else{const selected=[found],seen=new Set([row.uid]);for(let i=0;i<selected.length;i++)for(const candidate of rows)if(candidate.c.id===c.id&&!seen.has(candidate.row.uid)&&JSON.parse(candidate.row.projection).parentUid===selected[i].row.uid){seen.add(candidate.row.uid);selected.push(candidate);if(selected.length>500)invalid("Task subtree exceeds 500 items.");}const ids=new Map(selected.map(x=>[x.row.uid,action==="copy"?randomUUID():x.row.uid]));for(const [index,entry]of selected.entries()){const old=readTask(entry.row.content),next=writeTask({...old.data,parentUid:index===0?"":ids.get(old.data.parentUid)??""},entry.row.content);if(action==="copy"){for(const component of next.root.getAllSubcomponents("vtodo"))component.updatePropertyWithValue("uid",ids.get(old.uid)!);const {serialize}=await import("./standards.ts");operations.push({operation:"put",collectionId:target.id,id:randomUUID(),href:`${randomUUID()}.ics`,revision:0,uid:ids.get(old.uid),content:serialize(next.root),projection:JSON.stringify(next.data)});}else operations.push({operation:"move",collectionId:c.id,targetCollectionId:target.id,href:entry.row.href,revision:entry.row.revision,content:next.content,projection:JSON.stringify(next.data)});}}
    return json(await workspace("batch",{collections:{[c.id]:c.revision,[target.id]:target.revision},operations}));
  }
  throw new AppError("methodNotAllowed",405);
}
