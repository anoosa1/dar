import {SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import {bookPosition,type BookMark,type BookState,type BookStatus} from "../../shared/books.ts";

function fail(code:string):never{throw new SenderError(code);}
type ReadCtx=Pick<Ctx,"db">;
type StateRow=NonNullable<ReturnType<Ctx["db"]["bookStates"]["id"]["find"]>>;
type MarkRow=NonNullable<ReturnType<Ctx["db"]["bookMarks"]["id"]["find"]>>;
const statuses:readonly BookStatus[]=["","want","reading","finished"];
const uuid=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
const parse=(text:string)=>{try{return text?bookPosition(JSON.parse(text)):null;}catch{return null;}};
const text=(value:unknown,max:number)=>{if(typeof value!=="string"||value.length>max||/[\x00-\x08\x0b\x0c\x0e-\x1f]/.test(value))fail("invalidInput");return value;};

export function exposeState(row:StateRow):BookState{
  return {fileId:row.fileId,status:statuses.includes(row.status as BookStatus)?row.status as BookStatus:"",position:parse(row.position),progress:row.progress,device:row.device,updatedAt:row.updatedAt,revision:row.revision};
}
const exposeMark=(row:MarkRow):BookMark=>({id:row.id,fileId:row.fileId,position:parse(row.position)??{page:0,fraction:0},label:row.label,createdAt:row.createdAt});
/** Every reading state of an account, for the library. */
export function bookStates(ctx:ReadCtx,userId:string){return [...ctx.db.bookStates.ownerId.filter(userId)].map(exposeState);}

/**
 * Reading state and bookmarks for one account. `readable` confirms the caller may read the
 * Files entry now (owner or grant, not deleted, not locked away), so a state can only be
 * kept for a book the account can open.
 */
export function booksOperation(ctx:Ctx,userId:string,operation:string,p:Record<string,unknown>,readable:(fileId:string)=>boolean){
  const now=Number(ctx.timestamp.toMillis());
  const fileId=()=>{const id=p.fileId;if(typeof id!=="string"||!uuid.test(id))fail("invalidInput");if(!readable(id))fail("notFound");return id;};
  if(operation==="detail"){
    const id=fileId(),row=ctx.db.bookStates.id.find(`${userId}/${id}`);
    const marks=[...ctx.db.bookMarks.key.filter(`${userId}/${id}`)].map(exposeMark).sort((a,b)=>(a.position.spine??a.position.page??0)-(b.position.spine??b.position.page??0)||a.position.fraction-b.position.fraction);
    return {state:row?exposeState(row):null,marks};
  }
  if(operation==="saveState"){
    if(Object.keys(p).some(key=>!["fileId","revision","status","position","progress","device"].includes(key)))fail("invalidInput");
    const id=fileId(),key=`${userId}/${id}`,old=ctx.db.bookStates.id.find(key);
    // A newer state from another device is never overwritten silently.
    if(p.revision!==(old?.revision??0))fail("conflict");
    let status=old?.status??"",position=old?.position??"",progress=old?.progress??0,device=old?.device??"";
    if(p.status!==undefined){if(!statuses.includes(p.status as BookStatus))fail("invalidInput");status=p.status as string;}
    if(p.position!==undefined){
      if(p.position===null)position="";else{const value=bookPosition(p.position);if(!value)fail("invalidInput");position=JSON.stringify(value);}
    }
    if(p.progress!==undefined){if(typeof p.progress!=="number"||!(p.progress>=0&&p.progress<=1))fail("invalidInput");progress=p.progress;}
    if(p.device!==undefined)device=text(p.device,80);
    if(!old&&[...ctx.db.bookStates.ownerId.filter(userId)].length>=20000)fail("quotaExceeded");
    const row={id:key,ownerId:userId,fileId:id,status,position,progress,device,updatedAt:now,revision:(old?.revision??0)+1};
    if(old)ctx.db.bookStates.id.update(row);else ctx.db.bookStates.insert(row);
    return exposeState(row);
  }
  if(operation==="addMark"){
    if(Object.keys(p).some(key=>!["fileId","id","position","label"].includes(key)))fail("invalidInput");
    const id=fileId(),markId=p.id,position=bookPosition(p.position);
    if(typeof markId!=="string"||!uuid.test(markId)||!position)fail("invalidInput");
    const existing=ctx.db.bookMarks.id.find(markId);if(existing){if(existing.ownerId!==userId)fail("conflict");return exposeMark(existing);}
    if([...ctx.db.bookMarks.key.filter(`${userId}/${id}`)].length>=1000)fail("quotaExceeded");
    const row={id:markId,ownerId:userId,key:`${userId}/${id}`,fileId:id,position:JSON.stringify(position),label:text(p.label??"",200).trim(),createdAt:now};
    ctx.db.bookMarks.insert(row);return exposeMark(row);
  }
  if(operation==="deleteMark"){
    const id=p.id;if(typeof id!=="string"||!uuid.test(id))fail("invalidInput");
    const row=ctx.db.bookMarks.id.find(id);if(!row||row.ownerId!==userId)fail("notFound");
    ctx.db.bookMarks.id.delete(id);return {ok:true};
  }
  fail("invalidInput");
}

export function booksSnapshot(ctx:ReadCtx){return {states:[...ctx.db.bookStates.iter()],marks:[...ctx.db.bookMarks.iter()]};}
export function restoreBooks(ctx:Ctx,value?:ReturnType<typeof booksSnapshot>){
  if(!value)return;
  for(const row of value.states){
    if(!ctx.db.accounts.id.find(row.ownerId)||row.id!==`${row.ownerId}/${row.fileId}`||!statuses.includes(row.status as BookStatus)||!Number.isInteger(row.revision)||row.revision<1)fail("invalidInput");
    ctx.db.bookStates.insert(row);
  }
  for(const row of value.marks){
    if(!ctx.db.accounts.id.find(row.ownerId)||row.key!==`${row.ownerId}/${row.fileId}`||!uuid.test(row.id))fail("invalidInput");
    ctx.db.bookMarks.insert(row);
  }
}
