import type {IncomingMessage,ServerResponse} from "node:http";
import type {Database} from "../database.ts";
import type {BlobFile} from "../files/blobs.ts";
import {AppError} from "../errors.ts";
import {writeJson} from "../json-response.ts";
import {sendFile} from "../photos/http.ts";
import type {BooksRuntime} from "./runtime.ts";
import {booksReadOperations} from "../../shared/read-operations.ts";
import {bookFormat,unsupportedBook,type BookDetail,type BookManifest,type BookMark,type BookState} from "../../shared/books.ts";

const opaque=/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
// Book documents may style themselves and load their own images and fonts, never scripts, forms, popups or other
// origins. The sandbox keeps allow-scripts only so the reader's own listeners (page turns) work in WebKit; script-src
// 'none' still stops every script in the book.
const documentPolicy="default-src 'none'; script-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; media-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'; sandbox allow-same-origin allow-scripts";

/**
 * The Books API for browser sessions. Every request resolves the file through Files'
 * download check (owner or grant, not deleted, Locked folder rules), so a book is readable
 * here exactly when Files would serve it.
 */
export async function handleBooksHttp(req:IncomingMessage,res:ServerResponse,url:URL,db:Database,sessionId:string,books:BooksRuntime,body:()=>Promise<Record<string,unknown>>){
  const method=req.method??"GET",route=url.pathname.slice("/api/v1/books".length);
  const send=(value:unknown,status=200)=>writeJson(res,status,JSON.stringify(value));
  const call=<T,>(operation:string,value:object)=>db.operation<T>("service_books",operation,booksReadOperations,[sessionId,operation,JSON.stringify(value)]);
  if(route==="/library"&&method==="GET")return send(await books.library(sessionId));
  const match=/^\/([a-f0-9-]{36})(?:\/(cover|state|marks|pages|epub)(?:\/(.+))?)?$/.exec(route);
  if(!match||!opaque.test(match[1]))throw new AppError("notFound",404);
  const [,id,part,rest]=match;
  const entry=await db.read<BlobFile>("service_download",[sessionId,id,""]),format=bookFormat(entry.name);
  if(!format)throw new AppError(unsupportedBook(entry.name)?"unsupportedBook":"notFound",unsupportedBook(entry.name)?415:404);
  const reading=["GET","HEAD"].includes(method),tag=(suffix:string)=>`"${entry.blobId}-${suffix}"`,cached="private, max-age=86400";
  const bytes=(value:{bytes:Buffer;type:string},extra:Record<string,string>={})=>{
    const etag=tag(String(value.bytes.length));if(req.headers["if-none-match"]===etag){res.writeHead(304,{etag,"cache-control":cached});res.end();return;}
    res.writeHead(200,{"content-type":value.type,"content-length":value.bytes.length,etag,"cache-control":cached,"x-content-type-options":"nosniff","content-security-policy":"default-src 'none'; sandbox",...extra});res.end(method==="HEAD"?undefined:value.bytes);
  };
  if(!part&&method==="GET"){
    const info=await books.info(entry.blobId,format,entry.name),detail=await call<{state:BookState|null;marks:BookMark[]}>("detail",{fileId:id});
    const manifest:BookManifest=format==="pdf"?{kind:"pdf"}:format==="epub"?{kind:"epub",spine:info.epub?.spine??[],toc:info.epub?.toc??[],rightToLeft:info.meta.rightToLeft,language:info.meta.language}:{kind:"comic",pages:info.pages?.length??0,rightToLeft:info.meta.rightToLeft};
    return send({...detail,manifest,meta:info.meta} satisfies BookDetail&{meta:unknown});
  }
  if(part==="cover"&&reading){
    const file=await books.cover(entry.blobId,format,entry.name);if(!file)throw new AppError("notFound",404);
    return sendFile(req,res,file,"image/webp",tag("cover"),entry.locked===false?cached:"private, no-store");
  }
  if(part==="pages"&&reading){
    const index=Number(rest);if(!Number.isInteger(index)||index<0||format==="epub"||format==="pdf")throw new AppError("notFound",404);
    const page=await books.page(entry.blobId,format,entry.name,index);if(!page)throw new AppError("notFound",404);
    return bytes(page);
  }
  if(part==="epub"&&reading&&rest){
    // The first segment carries the file revision so a replaced book is never served from the browser cache.
    const name=rest.split("/").slice(1).map(segment=>{try{return decodeURIComponent(segment);}catch{throw new AppError("notFound",404);}}).join("/");
    if(format!=="epub"||!name)throw new AppError("notFound",404);
    const value=await books.resource(entry.blobId,name);if(!value)throw new AppError("notFound",404);
    // Sloppy XHTML that the XML parser rejects can be read with the HTML parser instead.
    if(value.type==="application/xhtml+xml"&&url.searchParams.get("as")==="html")value.type="text/html; charset=utf-8";
    return bytes(value,/xhtml|html|svg/.test(value.type)?{"content-security-policy":documentPolicy}:{});
  }
  if(part==="state"&&method==="PUT"){
    const p=await body();
    try{return send(await call<BookState>("saveState",{...p,fileId:id}));}
    catch(error){
      // The newer state comes back so the reader can offer to jump to it.
      if(error instanceof AppError&&error.code==="conflict"){const current=await call<{state:BookState|null}>("detail",{fileId:id});return send({error:{code:"conflict",message:"Your place in this book changed on another device."},state:current.state},409);}
      throw error;
    }
  }
  if(part==="marks"&&method==="POST"&&!rest)return send(await call<BookMark>("addMark",{...await body(),fileId:id}),201);
  if(part==="marks"&&method==="DELETE"&&rest&&opaque.test(rest))return send(await call("deleteMark",{id:rest}));
  throw new AppError(reading||!part?"notFound":"methodNotAllowed",reading||!part?404:405);
}
