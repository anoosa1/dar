import path from "node:path";
import {createReadStream} from "node:fs";
import {mkdir,readdir,unlink,stat} from "node:fs/promises";
import {randomUUID} from "node:crypto";
import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {blobPath,type blobStore} from "../files/blobs.ts";
import type {Bridge} from "./bridge.ts";
import {localName} from "../../shared/chat.ts";

interface MediaRow {id:string;accountId:string;chat:string;messageId:string;kind:string;mime:string;size:number;fileName:string;blobId:string;state:string;at:number;chatName:string;platform:string}
type Blobs=ReturnType<typeof blobStore>;
type Call=<T>(sessionId:string,operation:string,value:object)=>Promise<T>;
type Worker=<T>(operation:string,value:object)=>Promise<T>;
const messages:Record<string,[number,string]>={
  retrying:[409,"WhatsApp no longer has this file. Your phone was asked to send it again; try again in a moment."],
  expired:[410,"This file is no longer available from the service, and could not be fetched again."],
  failed:[502,"The attachment could not be downloaded. Try again."],
};

/**
 * Attachments: the bridge decrypts each into the runtime's download directory, then it joins the
 * blob store under its digest and the database records it, which keeps it through maintenance and
 * puts it in backups. One download per attachment at a time; automatic ones run two at once.
 */
export async function createChatMedia({db,root,blobs,bridge,worker,call}:{db:Database;root:string;blobs:Blobs;bridge:Bridge;worker:Worker;call:Call}){
  const downloads=path.join(root,"chat-downloads");
  await mkdir(downloads,{recursive:true,mode:0o700});
  // A download interrupted by a stop leaves a partial file nothing refers to.
  for(const name of await readdir(downloads))if(name.endsWith(".part"))await unlink(path.join(downloads,name)).catch(()=>{});
  const running=new Map<string,Promise<void>>(),queue:string[]=[];let active=0,closing=false;

  /** Downloads one attachment, identified by its message row ID "<account>/<chat>/<message>". */
  function fetch(id:string):Promise<void>{
    const current=running.get(id);if(current)return current;
    const [accountId,chat,...rest]=id.split("/"),messageId=rest.join("/");
    const work=(async()=>{
      const file=path.join(downloads,`${randomUUID()}.part`);
      try{
        await bridge.call("download",{account:accountId,chat,id:messageId,path:file},15*60_000);
        const {size}=await stat(file);
        const blobId=await blobs.receive(createReadStream(file),randomUUID(),size);
        await worker("mediaStored",{accountId,id,blobId,size});
      }catch(error){
        const code=error instanceof AppError?error.code:"";
        if(!closing)await worker("mediaState",{accountId,id,state:code==="retrying"?"retrying":code==="expired"||code==="noMedia"?"expired":"failed"}).catch(()=>{});
        throw error;
      }finally{await unlink(file).catch(()=>{});}
    })();
    running.set(id,work);void work.catch(()=>{}).finally(()=>{if(running.get(id)===work)running.delete(id);});
    return work;
  }
  function pump(){
    while(active<2&&queue.length&&!closing){
      const id=queue.shift()!;active++;
      void fetch(id).catch(()=>{}).finally(()=>{active--;pump();});
    }
  }
  async function ready(sessionId:string,threadId:string,messageId:string,wait=true):Promise<MediaRow>{
    let row=await call<MediaRow>(sessionId,"media",{threadId,messageId});
    if(row.state==="stored")return row;
    if(!wait)throw new AppError(row.state,...(messages[row.state]??messages.failed));
    try{await fetch(row.id);}
    catch(error){if(error instanceof AppError&&messages[error.code])throw new AppError(error.code,...messages[error.code]);if(error instanceof AppError&&error.code==="notConnected")throw new AppError("notConnected",409,`${row.platform==="discord"?"Discord":"WhatsApp"} is not connected, so the attachment cannot be downloaded now.`);throw new AppError("failed",...messages.failed);}
    row=await call<MediaRow>(sessionId,"media",{threadId,messageId});
    if(row.state!=="stored")throw new AppError("failed",...messages.failed);
    return row;
  }

  return {
    /** New incoming photos, stickers and voice notes. */
    queue(ids:string[]){for(const id of ids)if(!queue.includes(id))queue.push(id);pump();},
    /** The phone sent expired media again. */
    async retried(accountId:string,chat:string,messageId:string,ok:boolean){
      const found=await worker<{id:string;state:string}>("mediaFind",{accountId,chat,messageId});if(!found.id)return;
      if(ok){queue.push(found.id);pump();}else await worker("mediaState",{accountId,id:found.id,state:"expired"});
    },
    /** Serves an attachment, downloading it first if needed; HTML and other active types always download. */
    async serve(req:IncomingMessage,res:ServerResponse,sessionId:string,threadId:string,messageId:string,attachment:boolean){
      const row=await ready(sessionId,threadId,messageId);
      await blobs.download(req,res,{id:row.id,name:localName(row),blobId:row.blobId,size:row.size,mediaType:row.mime,revision:0},!attachment);
    },
    /** Copies an attachment into a Files folder as a new file, numbering the name if it is taken. */
    async save(sessionId:string,threadId:string,messageId:string,folderId:string){
      const row=await ready(sessionId,threadId,messageId),name=localName(row),dot=name.lastIndexOf("."),stem=dot>0?name.slice(0,dot):name,extension=dot>0?name.slice(dot):"";
      for(let attempt=1;attempt<=50;attempt++){
        const candidate=attempt===1?name:`${stem} (${attempt})${extension}`,uploadId=randomUUID(),fileId=randomUUID();
        try{await db.call("service_begin_upload",[sessionId,uploadId,fileId,folderId,candidate,row.size,row.mime,0]);}
        catch(error){if(error instanceof AppError&&error.code==="conflict")continue;throw error;}
        const digest=await blobs.receive(createReadStream(blobPath(root,row.blobId)),uploadId,row.size);
        await db.call("service_finish_upload",[sessionId,uploadId,digest,0]);
        return {fileId,name:candidate};
      }
      throw new AppError("conflict",409,"Too many files with this name in that folder.");
    },
    close(){closing=true;queue.length=0;},
  };
}
