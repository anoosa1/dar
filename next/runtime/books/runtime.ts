import {execFile} from "node:child_process";
import {lstat,mkdir,mkdtemp,readFile,readdir,rename,rm,stat,writeFile,open} from "node:fs/promises";
import {constants} from "node:fs";
import path from "node:path";
import type {Database} from "../database.ts";
import {blobPath} from "../files/blobs.ts";
import {openZip,type Zip} from "./zip.ts";
import {readEpub,comicPages,readComicInfo,emptyMeta} from "./formats.ts";
import {bookFormat,bookPosition,type BookFormat,type BookItem,type BookLibrary,type BookMeta,type BookState,type BookTocEntry} from "../../shared/books.ts";

/**
 * Tools from the Nix package run without a shell, with a minimal environment and limits:
 * libvips for covers and PDF facts, bsdtar (libarchive) for RAR, 7z and tar comics.
 */
const tool=(name:string)=>process.env[`DAR_${name.toUpperCase()}`]||name;
function run(name:string,args:string[],timeout=60_000,maxBuffer=4*1024*1024){
  return new Promise<{stdout:string;ok:boolean}>(resolve=>execFile(tool(name),args,{timeout,maxBuffer,env:{PATH:process.env.PATH??"",HOME:"/nonexistent",LC_ALL:"C.UTF-8"},windowsHide:true},(error,stdout)=>resolve({stdout:String(stdout??""),ok:!error})));
}
const exists=async(file:string)=>{try{return (await stat(file)).size>0;}catch{return false;}};
const imageType=(name:string)=>({jpg:"image/jpeg",jpeg:"image/jpeg",png:"image/png",gif:"image/gif",webp:"image/webp",avif:"image/avif",bmp:"image/bmp",svg:"image/svg+xml"} as Record<string,string>)[/\.([a-z0-9]+)$/i.exec(name)?.[1]?.toLowerCase()??""];
/** Types an EPUB may hold; others are served as plain bytes the browser will not run. */
export function epubType(name:string){
  const ext=/\.([a-z0-9]+)$/i.exec(name)?.[1]?.toLowerCase()??"";
  return imageType(name)??({xhtml:"application/xhtml+xml",html:"application/xhtml+xml",htm:"application/xhtml+xml",xml:"application/xml",css:"text/css",ttf:"font/ttf",otf:"font/otf",woff:"font/woff",woff2:"font/woff2",mp3:"audio/mpeg",mp4:"video/mp4",m4a:"audio/mp4",ncx:"application/xml",opf:"application/xml"} as Record<string,string>)[ext]??"application/octet-stream";
}

interface EpubInfo {opf:string;spine:{href:string;size:number}[];toc:BookTocEntry[];cover?:string}
interface Info {meta:BookMeta;epub?:EpubInfo;pages?:string[]}
const infoVersion=1;
export interface BookFile {id:string;parentId:string;name:string;blobId:string;size:number;revision:number;modifiedAt:number;path:string;library:string}

/**
 * Books' derived data, all regenerable and keyed by immutable blob ID in book-cache/:
 * parsed metadata (memory and disk), cover thumbnails and the pages of comics that need
 * bsdtar. Tool runs are limited to two at a time.
 */
export async function createBooksRuntime(db:Database,root:string){
  const cache=path.join(root,"book-cache");await mkdir(cache,{recursive:true,mode:0o700});
  const file=(blobId:string,suffix:string)=>{if(!/^[a-f0-9-]{36}$/.test(blobId))throw new Error("Unexpected blob ID.");return path.join(cache,blobId.slice(0,2),`${blobId}-${suffix}`);};
  let active=0;const waiting:(()=>void)[]=[];
  async function slot<T>(work:()=>Promise<T>){if(active>=2)await new Promise<void>(resolve=>waiting.push(resolve));active++;try{return await work();}finally{active--;waiting.shift()?.();}}
  const infos=new Map<string,Promise<Info>>(),tasks=new Map<string,Promise<unknown>>();
  const once=<T,>(key:string,work:()=>Promise<T>)=>{let task=tasks.get(key) as Promise<T>|undefined;if(!task){task=work().finally(()=>tasks.delete(key));tasks.set(key,task);}return task;};
  // Open ZIPs are kept briefly: a reader asks for many resources of the same book.
  const zips=new Map<string,{zip:Promise<Zip>;used:number}>();
  function zipFor(blobId:string){
    const open=zips.get(blobId);if(open){open.used=Date.now();return open.zip;}
    const zip=openZip(blobPath(root,blobId));zip.catch(()=>zips.delete(blobId));zips.set(blobId,{zip,used:Date.now()});
    if(zips.size>16){const oldest=[...zips.entries()].sort((a,b)=>a[1].used-b[1].used)[0];zips.delete(oldest[0]);void oldest[1].zip.then(z=>z.close()).catch(()=>{});}
    return zip;
  }
  const sweep=setInterval(()=>{for(const [id,open] of zips)if(Date.now()-open.used>60_000){zips.delete(id);void open.zip.then(z=>z.close()).catch(()=>{});}},30_000);sweep.unref();

  /** Comic archives other than ZIP are unpacked once, after checking their listed size. */
  async function unpacked(blobId:string){
    const target=file(blobId,"pages");
    if(await lstat(target).then(s=>s.isDirectory(),()=>false))return target;
    return once(`unpack:${blobId}`,()=>slot(async()=>{
      const source=blobPath(root,blobId),listing=await run("bsdtar",["-tvf",source],120_000,32*1024*1024);
      if(!listing.ok)throw new Error("unsupported");
      let total=0,count=0;for(const line of listing.stdout.split("\n")){const size=/^\S+\s+\d+\s+\S+\s+\S+\s+(\d+)\s/.exec(line);if(size){total+=Number(size[1]);count++;}}
      if(total>4*1024**3||count>20000)throw new Error("unsupported");
      await mkdir(path.dirname(target),{recursive:true,mode:0o700});
      const work=await mkdtemp(path.join(cache,".work-"));
      try{
        // bsdtar refuses absolute paths and .. by default; ownership and modes are not kept.
        if(!(await run("bsdtar",["-xf",source,"-C",work,"--no-same-owner","--no-same-permissions"],600_000)).ok)throw new Error("damaged");
        await rename(work,target).catch(async error=>{if(!await lstat(target).then(s=>s.isDirectory(),()=>false))throw error;});
        return target;
      }finally{await rm(work,{recursive:true,force:true});}
    }));
  }
  async function listFiles(directory:string,prefix=""):Promise<string[]>{
    const out:string[]=[];
    for(const entry of await readdir(path.join(directory,prefix),{withFileTypes:true})){
      const name=prefix?`${prefix}/${entry.name}`:entry.name;
      if(entry.isDirectory()&&!entry.isSymbolicLink())out.push(...await listFiles(directory,name));else if(entry.isFile())out.push(name);
      if(out.length>20000)break;
    }
    return out;
  }
  async function readPage(directory:string,name:string,max:number){
    const handle=await open(path.join(directory,name),constants.O_RDONLY|constants.O_NOFOLLOW);
    try{const {size}=await handle.stat();if(size>max)throw new Error("Page too large.");return await handle.readFile();}finally{await handle.close();}
  }

  async function parse(blobId:string,format:BookFormat,name:string):Promise<Info>{
    const title=name.replace(/\.[^.]+$/,"").replace(/[_]+/g," ").trim()||name,source=blobPath(root,blobId);
    if(format==="pdf"){
      const meta=emptyMeta(title),header=await slot(()=>run("vipsheader",["-a",source],60_000));
      if(!header.ok){meta.error="damaged";return {meta};}
      const field=(key:string)=>new RegExp(`^${key}: (.*)$`,"m").exec(header.stdout)?.[1]?.trim()??"";
      meta.pages=Number(field("pdf-n_pages"))||0;meta.title=field("pdf-title").slice(0,300)||title;
      const author=field("pdf-author");if(author)meta.authors=author.split(/\s*[;&]\s*|\s+and\s+/).filter(Boolean).slice(0,10).map(a=>a.slice(0,200));
      meta.cover=true;return {meta};
    }
    if(format==="epub"){
      try{
        const zip=await zipFor(blobId),read=async(entry:string,max:number)=>{const found=zip.get(entry);if(!found)throw new Error("Missing entry.");return zip.read(found,max);};
        const epub=await readEpub(entry=>zip.get(entry),read,title);
        if(epub.encrypted)epub.meta.error="drm";
        return {meta:epub.meta,epub:{opf:epub.opf,spine:epub.spine,toc:epub.toc,cover:epub.cover}};
      }catch{const meta=emptyMeta(title);meta.error="damaged";return {meta};}
    }
    const meta=emptyMeta(title);
    try{
      let names:string[],info:Buffer|undefined;
      if(format==="cbz"){const zip=await zipFor(blobId);names=zip.entries.filter(e=>!e.encrypted).map(e=>e.name);const ci=zip.entries.find(e=>/(^|\/)ComicInfo\.xml$/i.test(e.name));info=ci?await zip.read(ci,1024*1024):undefined;}
      else{const directory=await unpacked(blobId);names=await listFiles(directory);const ci=names.find(n=>/(^|\/)ComicInfo\.xml$/i.test(n));info=ci?await readPage(directory,ci,1024*1024):undefined;}
      const pages=comicPages(names);if(!pages.length){meta.error="damaged";return {meta};}
      if(info)readComicInfo(info.toString("utf8"),meta);
      meta.pages=pages.length;meta.cover=true;return {meta,pages};
    }catch(error){meta.error=(error as Error).message==="unsupported"?"unsupported":"damaged";return {meta};}
  }
  /** Metadata for a blob: memory, then the disk cache, then the file itself. */
  function info(blobId:string,format:BookFormat,name:string){
    let task=infos.get(blobId);
    if(!task){
      task=(async()=>{
        const saved=file(blobId,"info.json");
        try{const value=JSON.parse(await readFile(saved,"utf8"));if(value.version===infoVersion)return value.info as Info;}catch{/* parse below */}
        const value=await parse(blobId,format,name);
        if(!value.meta.error||value.meta.error==="drm"){await mkdir(path.dirname(saved),{recursive:true,mode:0o700});await writeFile(saved,JSON.stringify({version:infoVersion,info:value}),{mode:0o600});}
        return value;
      })().catch(()=>({meta:{...emptyMeta(name),error:"damaged" as const}}));
      if(infos.size>=5000)infos.delete(infos.keys().next().value!);
      infos.set(blobId,task);
    }
    return task;
  }

  /** A WebP cover about 480 px tall, made from the EPUB cover image, the first PDF page or the first comic page. */
  async function cover(blobId:string,format:BookFormat,name:string){
    const target=file(blobId,"cover.webp");if(await exists(target))return target;
    const book=await info(blobId,format,name);if(!book.meta.cover)return undefined;
    return once(`cover:${blobId}`,async()=>{
      await mkdir(path.dirname(target),{recursive:true,mode:0o700});
      const work=await mkdtemp(path.join(cache,".work-"));
      try{
        let input=blobPath(root,blobId);
        if(format==="pdf")input=`${input}[page=0]`;
        else{
          let bytes:Buffer|undefined,ext="";
          if(format==="epub"&&book.epub?.cover){bytes=await (await zipFor(blobId)).read((await zipFor(blobId)).get(book.epub.cover)!,32*1024*1024);ext=path.extname(book.epub.cover);}
          else if(book.pages?.length){const first=book.pages[0];bytes=(await page(blobId,format,name,0))?.bytes;ext=path.extname(first);}
          if(!bytes)return undefined;
          input=path.join(work,`cover${ext.toLowerCase()||".img"}`);await writeFile(input,bytes,{mode:0o600});
        }
        const out=path.join(work,"out.webp");
        const ok=await slot(()=>run("vipsthumbnail",[input,"--size","320x480>","-o",`${out}[Q=78,strip]`],120_000));
        if(!ok.ok||!await exists(out))return undefined;
        await rename(out,target);return target;
      }finally{await rm(work,{recursive:true,force:true});}
    });
  }
  /** One comic page as stored in the archive. */
  async function page(blobId:string,format:BookFormat,name:string,index:number){
    const book=await info(blobId,format,name),entry=book.pages?.[index];if(!entry)return undefined;
    const type=imageType(entry)??"application/octet-stream";
    if(format==="cbz"){const zip=await zipFor(blobId),found=zip.get(entry);return found?{bytes:await zip.read(found,64*1024*1024),type}:undefined;}
    return {bytes:await readPage(await unpacked(blobId),entry,64*1024*1024),type};
  }
  /** An EPUB resource by its path inside the archive. */
  async function resource(blobId:string,entry:string){
    const zip=await zipFor(blobId),found=zip.get(entry);if(!found||found.encrypted)return undefined;
    return {bytes:await zip.read(found,32*1024*1024),type:epubType(entry)};
  }

  /** The library with metadata; books still being read for the first time show their file name until ready. */
  async function library(sessionId:string):Promise<BookLibrary>{
    const listing=await db.call<{rootId:string;path:string;libraries:{id:string;name:string}[];files:BookFile[];states:BookState[];truncated:boolean}>("service_books",[sessionId,"library","{}"]);
    const states=new Map(listing.states.map(s=>[s.fileId,{...s,position:bookPosition(s.position)}]));
    const pending=listing.files.map(f=>info(f.blobId,bookFormat(f.name)!,f.name)),settled=new Map<number,Info>();
    pending.forEach((task,index)=>void task.then(value=>settled.set(index,value)));
    await Promise.race([Promise.all(pending),new Promise(resolve=>setTimeout(resolve,2500))]);
    let indexing=0;
    const books:BookItem[]=listing.files.map((f,index)=>{
      const value=settled.get(index);if(!value)indexing++;
      const meta=value?.meta??emptyMeta(f.name.replace(/\.[^.]+$/,""));
      return {...meta,id:f.id,parentId:f.parentId,name:f.name,format:bookFormat(f.name)!,size:f.size,revision:f.revision,modifiedAt:f.modifiedAt,library:f.library,path:f.path,state:states.get(f.id)??null};
    });
    const counts=new Map<string,number>();for(const b of books)if(b.library)counts.set(b.library,(counts.get(b.library)??0)+1);
    return {rootId:listing.rootId,path:listing.path,libraries:listing.libraries.map(l=>({...l,count:counts.get(l.id)??0})),books,truncated:listing.truncated,indexing};
  }

  return {
    library,info,cover,page,resource,
    async close(){clearInterval(sweep);await Promise.allSettled([...tasks.values()]);for(const open of zips.values())await open.zip.then(z=>z.close()).catch(()=>{});zips.clear();},
  };
}
export type BooksRuntime=Awaited<ReturnType<typeof createBooksRuntime>>;
