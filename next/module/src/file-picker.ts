import {Range,SenderError} from "spacetimedb/server";
import type {Ctx} from "./index.ts";
import type {DarFilePickerPage} from "../../shared/file-picker.ts";
type Entry=NonNullable<ReturnType<Ctx["db"]["fileEntries"]["id"]["find"]>>;

/** Shared, bounded browsing for native file/folder pickers in every Dar app. */
export function filePicker(ctx:Ctx,userId:string,parentId:string,cursor:string,kind:string,canRead:(row:Entry)=>boolean):DarFilePickerPage{
  if(!["file","folder"].includes(kind)||cursor.length>2048||parentId&&!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(parentId))throw new SenderError("invalidInput");
  const parent=parentId?ctx.db.fileEntries.id.find(parentId):null;
  if(parentId&&(!parent||parent.kind!=="folder"||!canRead(parent)))throw new SenderError("notFound");
  const prefix=`${parent?.ownerId??userId}/${parentId}/`;
  if(cursor&&!cursor.startsWith(prefix))throw new SenderError("invalidInput");
  const range=new Range<string>({tag:cursor?"excluded":"included",value:cursor||prefix},{tag:"excluded",value:prefix.slice(0,-1)+"0"});
  const items:DarFilePickerPage["items"]=[];let scanned=0,nextCursor:string|null=null;
  for(const row of ctx.db.fileEntries.listKey.filter(range)){
    if((row.kind==="folder"||kind==="file")&&canRead(row))items.push({id:row.id,name:row.name,kind:row.kind as "file"|"folder"});
    if(++scanned>=200){nextCursor=row.listKey;break;}
  }
  const ancestors:DarFilePickerPage["ancestors"]=[];let id=parent?.parentId;
  for(let depth=0;id&&depth<128;depth++){
    const row=ctx.db.fileEntries.id.find(id);if(!row||!canRead(row))break;
    ancestors.unshift({id:row.id,name:row.name});id=row.parentId;
  }
  return {folder:parent?{id:parent.id,name:parent.name,kind:"folder",parentId:parent.parentId}:null,ancestors,items,nextCursor};
}
