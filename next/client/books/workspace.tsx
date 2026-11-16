import {useCallback,useEffect,useMemo,useRef,useState,type ReactNode} from "react";
import {request,RequestError,formatBytes,type BrowserSession} from "../api.ts";
import {WorkspaceHeader,useMobileNavigation} from "../shell.tsx";
import {Icon,type IconName} from "../icons.tsx";
import {ActionMenu,NewItemButton,Select,useFloating,type MenuAction} from "../controls.tsx";
import {Dialog} from "../dialog.tsx";
import {SettingsSheet,SettingRow,Segmented} from "../settings-sheet.tsx";
import {usePreferences,savePreferences,usePreferenceSaver,SaveProblems} from "../app-settings.tsx";
import {useLiveConnection} from "../live.ts";
import {edited} from "../pim/presentation.ts";
import {photoFolderParts} from "../../shared/photos.ts";
import type {Preferences} from "../../shared/preferences.ts";
import {bookFormat,bookFormatLabel,isComic,defaultBooksFolder,type BookDetail,type BookItem,type BookLibrary,type BookMark,type BookPosition,type BookState,type BookStatus,type BookTocEntry} from "../../shared/books.ts";
import {EpubView,type EpubHandle,type ReaderLook} from "./epub.tsx";
import {ComicView,type ComicHandle} from "./comic.tsx";
import {PdfView,type PdfHandle,type PdfOutlineEntry} from "./pdf.tsx";

type ListView="home"|"reading"|"want"|"all"|"finished";
type View={type:ListView}|{type:"library";id:string};
type Sort=NonNullable<Preferences["booksSort"]>;
interface Notice {message:string;undo?:()=>void}
const api="/api/v1/books";
const views:[ListView,string,IconName][]=[["home","Home","library"],["reading","Reading","reading"],["want","Want to read","bookmark"],["all","All books","book"],["finished","Finished","check"]];
const sorts:[Sort,string][]=[["recent","Recently read"],["title","Title"],["author","Author"],["added","Recently added"]];
const accept=".epub,.pdf,.cbz,.cbr,.cb7,.cbt";
const mediaTypes:Record<string,string>={epub:"application/epub+zip",pdf:"application/pdf",cbz:"application/vnd.comicbook+zip",cbr:"application/vnd.comicbook-rar",cb7:"application/x-7z-compressed",cbt:"application/x-tar"};
const message=(cause:unknown)=>cause instanceof Error?cause.message:"The request could not be completed.";
const percent=(value:number)=>`${Math.round(value*100)}%`;
const authors=(book:BookItem)=>book.authors.join(", ")||(book.format==="pdf"?"PDF":isComic(book.format)?"Comic":"Unknown author");
/** A short name for this browser, shown as where a book was last read. */
function deviceName(){const ua=navigator.userAgent,browser=/Edg\//.test(ua)?"Edge":/Firefox\//.test(ua)?"Firefox":/Chrome\//.test(ua)?"Chrome":/Safari\//.test(ua)?"Safari":"Browser";
  const os=/Android/.test(ua)?"Android":/iPhone|iPad/.test(ua)?"iOS":/Mac OS X/.test(ua)?"macOS":/Windows/.test(ua)?"Windows":/Linux/.test(ua)?"Linux":"";return os?`${browser} on ${os}`:browser;}
function readView():{view:View;book?:string;read:boolean}{
  const p=new URLSearchParams(location.search),v=p.get("view")??"home",library=p.get("library");
  const view:View=library?{type:"library",id:library}:views.some(([id])=>id===v)?{type:v as ListView}:{type:"home"};
  return {view,book:p.get("book")??undefined,read:p.get("read")==="1"};
}
const statusLabel=(status:BookStatus|undefined)=>status==="want"?"Want to read":status==="reading"?"Reading":status==="finished"?"Finished":"";
/** Covers with no image get a typographic cover in one of a fixed set of hues. */
const hue=(text:string)=>{let h=0;for(const c of text)h=(h*31+c.codePointAt(0)!)%9973;return h%10;};

/**
 * Books (selected mock A, docs/spacetimedb/books-mocks.html): a Kavita-style library over a
 * Files folder with shelves, libraries (its top folders) and a details sheet; books open in
 * the middle sheet with contents and bookmarks on the right. Reading state syncs per account.
 */
export function BooksWorkspace({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const start=useMemo(readView,[]);
  const [library,setLibrary]=useState<BookLibrary>(),[error,setError]=useState(""),[view,setView]=useState<View>(start.view),[selected,setSelected]=useState<string|undefined>(start.book);
  const [reading,setReading]=useState<string|undefined>(start.read?start.book:undefined),[settings,setSettings]=useState(false),[notice,setNotice]=useState<Notice|null>(null),[reload,setReload]=useState(0);
  const [transfers,setTransfers]=useState<{id:string;name:string;progress:number;status:"uploading"|"complete"|"error";message?:string}[]>([]),[confirm,setConfirm]=useState<BookItem>();
  const {open:navigation,setOpen:setNavigation,id:navigationId,ref:navigationRef}=useMobileNavigation();
  const stored=usePreferences(),sort=stored.preferences.booksSort??"recent",live=useLiveConnection(session),fileInput=useRef<HTMLInputElement>(null);
  const load=useCallback(async(signal?:AbortSignal)=>{try{const value=await request<BookLibrary>(`${api}/library`,{signal});setLibrary(value);setError("");return value;}catch(cause){if(!signal?.aborted)setError(message(cause));return undefined;}},[]);
  useEffect(()=>{const abort=new AbortController();void load(abort.signal);return()=>abort.abort();},[reload,stored.preferences.booksFolder]);
  // New books are read in the background; ask again until every book has its details.
  useEffect(()=>{if(!library?.indexing)return;const timer=setTimeout(()=>setReload(n=>n+1),3000);return()=>clearTimeout(timer);},[library]);
  useEffect(()=>{const focus=()=>{if(!reading)setReload(n=>n+1);};addEventListener("focus",focus);return()=>removeEventListener("focus",focus);},[reading]);
  useEffect(()=>{const url=new URL(location.href);for(const key of ["view","library","book","read"])url.searchParams.delete(key);
    if(view.type==="library")url.searchParams.set("library",view.id);else if(view.type!=="home")url.searchParams.set("view",view.type);
    if(selected)url.searchParams.set("book",selected);if(reading)url.searchParams.set("read","1");history.replaceState(null,"",url);},[view,selected,reading]);
  useEffect(()=>{if(!notice)return;const timer=setTimeout(()=>setNotice(null),6000);return()=>clearTimeout(timer);},[notice]);
  useEffect(()=>{document.body.classList.toggle("is-reading-book",!!reading);return()=>document.body.classList.remove("is-reading-book");},[reading]);

  const books=library?.books??[],byId=new Map(books.map(b=>[b.id,b])),book=selected?byId.get(selected):undefined,open=reading?byId.get(reading):undefined;
  const save=(patch:Partial<Preferences>)=>savePreferences({revision:stored.preferences.revision,...patch},session).catch(cause=>setError(message(cause)));
  const ordered=(list:BookItem[])=>[...list].sort((a,b)=>sort==="title"?a.title.localeCompare(b.title):sort==="author"?authors(a).localeCompare(authors(b))||a.title.localeCompare(b.title):sort==="added"?b.modifiedAt-a.modifiedAt:(b.state?.updatedAt??0)-(a.state?.updatedAt??0)||b.modifiedAt-a.modifiedAt);
  const list=view.type==="library"?books.filter(b=>b.library===view.id):view.type==="reading"?books.filter(b=>b.state?.status==="reading"):view.type==="want"?books.filter(b=>b.state?.status==="want"):view.type==="finished"?books.filter(b=>b.state?.status==="finished"):books;
  const counts={reading:books.filter(b=>b.state?.status==="reading").length,want:books.filter(b=>b.state?.status==="want").length,all:books.length,finished:books.filter(b=>b.state?.status==="finished").length};
  const currentLibrary=view.type==="library"?library?.libraries.find(l=>l.id===view.id):undefined;
  const title=view.type==="library"?currentLibrary?.name??"Library":views.find(([id])=>id===view.type)![1];
  const choose=(next:View)=>{setView(next);setNavigation(false);};

  /** Saves a status change; the local copy updates at once and Undo restores the previous status. */
  async function setStatus(target:BookItem,status:BookStatus,undoable=true){
    const before=target.state;
    try{
      const state=await request<BookState>(`${api}/${target.id}/state`,{method:"PUT",body:JSON.stringify({revision:before?.revision??0,status,...status==="finished"?{progress:1}:{}})},session.csrfToken);
      patch(target.id,state);
      if(undoable)setNotice({message:status?`Marked as ${statusLabel(status).toLowerCase()}`:"Removed from your lists",undo:()=>void request<BookState>(`${api}/${target.id}/state`,{method:"PUT",body:JSON.stringify({revision:state.revision,status:before?.status??"",progress:before?.progress??0})},session.csrfToken).then(value=>patch(target.id,value)).catch(cause=>setError(message(cause)))});
    }catch(cause){if(cause instanceof RequestError&&cause.status===409){setReload(n=>n+1);setError("This book changed on another device. Try again.");}else setError(message(cause));}
  }
  const patch=(id:string,state:BookState|null)=>setLibrary(current=>current&&{...current,books:current.books.map(b=>b.id===id?{...b,state}:b)});
  function folderFor(){return view.type==="library"?view.id:library?.rootId??"";}
  async function upload(files:FileList|null){
    if(!files?.length||!library)return;
    const parentId=folderFor(),taken=new Set(books.filter(b=>b.parentId===parentId).map(b=>b.name.toLowerCase()));
    for(const file of [...files]){
      const format=bookFormat(file.name),id=crypto.randomUUID();
      if(!format){setTransfers(t=>[...t,{id,name:file.name,progress:0,status:"error",message:"Books opens EPUB, PDF and comic archives (CBZ, CBR, CB7, CBT)."}]);continue;}
      let name=file.name;for(let n=2;taken.has(name.toLowerCase())&&n<1000;n++)name=file.name.replace(/(\.[^.]+)$/,` ${n}$1`);taken.add(name.toLowerCase());
      setTransfers(t=>[...t,{id,name,progress:0,status:"uploading"}]);
      try{
        const reservation=await request<{id:string;fileId:string}>("/api/v1/files/uploads",{method:"POST",body:JSON.stringify({name,parentId,size:file.size,mediaType:mediaTypes[format]})},session.csrfToken);
        await new Promise<void>((resolve,reject)=>{const xhr=new XMLHttpRequest();xhr.open("PUT",`/api/v1/files/uploads/${reservation.id}`);xhr.setRequestHeader("x-csrf-token",session.csrfToken);
          xhr.upload.onprogress=event=>setTransfers(t=>t.map(x=>x.id===id?{...x,progress:event.total?event.loaded/event.total:0}:x));
          xhr.onload=()=>{if(xhr.status<300)resolve();else{let text="The upload failed.";try{text=JSON.parse(xhr.responseText).error?.message??text;}catch{/* keep */}reject(new Error(text));}};
          xhr.onerror=()=>reject(new Error("The connection was lost during the upload."));xhr.send(file);});
        setTransfers(t=>t.map(x=>x.id===id?{...x,progress:1,status:"complete"}:x));
      }catch(cause){setTransfers(t=>t.map(x=>x.id===id?{...x,status:"error",message:message(cause)}:x));}
    }
    setReload(n=>n+1);
  }
  async function trash(target:BookItem){
    const connection=live.connection;if(!connection)throw new Error("Books is reconnecting. Try again in a moment.");
    await connection.reducers.trashEntry({id:target.id,expectedRevision:target.revision});
    if(selected===target.id)setSelected(undefined);if(reading===target.id)setReading(undefined);
    setNotice({message:`Moved “${target.title}” to deleted files`});setReload(n=>n+1);
  }
  const bookActions=(target:BookItem):MenuAction[]=>[
    {label:target.state?.position?"Continue reading":"Read",icon:"reading",disabled:!!target.error,run:()=>{setSelected(target.id);setReading(target.id);}},
    {label:"Details",icon:"info",run:()=>setSelected(target.id)},
    {label:target.state?.status==="want"?"Remove from Want to read":"Want to read",icon:"bookmark",separator:true,run:()=>void setStatus(target,target.state?.status==="want"?"":"want")},
    {label:target.state?.status==="finished"?"Mark as unread":"Mark as finished",icon:"check",run:()=>void setStatus(target,target.state?.status==="finished"?"":"finished")},
    {label:"Download",icon:"download",href:`/api/v1/files/${target.id}/content`,download:true,separator:true},
    {label:"Show in Files",icon:"folder",href:`/?folder=${target.parentId}&file=${target.id}`},
    {label:"Move to deleted files",icon:"trash",danger:true,separator:true,run:()=>setConfirm(target)},
  ];

  const sidebar=<>
    <NewItemButton disabled={!library} onClick={()=>fileInput.current?.click()}>Add books</NewItemButton>
    <input ref={fileInput} type="file" multiple hidden accept={accept} aria-label="Add books" onChange={event=>{void upload(event.target.files);event.target.value="";}}/>
    <nav aria-label="Books views">{views.map(([id,label,icon])=><button key={id} className={`nav-item${view.type===id?" active":""}`} aria-current={view.type===id?"page":undefined} onClick={()=>choose({type:id})}><Icon name={icon}/><span className="books-row-title">{label}</span>{id!=="home"&&!!counts[id as keyof typeof counts]&&<span className="books-count">{counts[id as keyof typeof counts]}</span>}</button>)}</nav>
    <div className="sidebar-heading books-heading"><h2>Libraries</h2></div>
    <nav aria-label="Libraries" className="books-libraries">
      {library?.libraries.map(l=><button key={l.id} className={`nav-item${view.type==="library"&&view.id===l.id?" active":""}`} aria-current={view.type==="library"&&view.id===l.id?"page":undefined} onClick={()=>choose({type:"library",id:l.id})}><Icon name="library"/><span className="books-row-title">{l.name}</span>{!!l.count&&<span className="books-count">{l.count}</span>}</button>)}
      {library&&!library.libraries.length&&<p className="books-hint">Folders in Files › {library.path} become libraries.</p>}
    </nav>
    {!!library?.indexing&&<p className="books-indexing" role="status"><span className="books-spinner" aria-hidden="true"/>Reading {library.indexing} new {library.indexing===1?"book":"books"}…</p>}
    <button type="button" className="nav-item ui-app-settings-entry" onClick={()=>setSettings(true)}><Icon name="settings"/>Books settings</button>
  </>;

  const sortMenu=<ActionMenu label="Sort books" className="workspace-sort" actions={sorts.map(([value,label])=>({label,current:sort===value,run:()=>void save({booksSort:value})}))}><Icon name="sort"/><span>{sorts.find(([value])=>value===sort)![1]}</span></ActionMenu>;
  const empty=(icon:IconName,heading:string,text:string,action?:ReactNode)=><div className="books-empty"><span className="empty-icon"><Icon name={icon}/></span><h2>{heading}</h2><p>{text}</p>{action}</div>;
  const uploadButton=<button className="primary-button" onClick={()=>fileInput.current?.click()}><Icon name="upload"/>Add books</button>;
  let content:ReactNode;
  if(error&&!library)content=<div className="books-empty" role="alert"><h2>Books could not be loaded</h2><p>{error}</p><button className="secondary-button" onClick={()=>setReload(n=>n+1)}>Try again</button></div>;
  else if(!library)content=<p className="books-empty" role="status">Loading your library…</p>;
  else if(!books.length)content=empty("book",`No books in ${library.path} yet`,"Add EPUB, PDF or comic files (CBZ, CBR), or move them into this folder in Files.",<div className="books-empty-actions">{uploadButton}<a className="secondary-button" href={`/?folder=${library.rootId}`}>Open in Files</a></div>);
  else if(view.type==="home"){
    const continuing=ordered(books.filter(b=>b.state?.status==="reading")).slice(0,6),added=[...books].sort((a,b)=>b.modifiedAt-a.modifiedAt).slice(0,16),wanted=ordered(books.filter(b=>b.state?.status==="want")).slice(0,16);
    content=<div className="books-scroll">
      {continuing.length>0&&<section aria-label="Continue reading"><div className="books-shelf-head"><h2>Continue reading</h2><button className="text-button" onClick={()=>choose({type:"reading"})}>See all</button></div>
        <div className="books-continue">{continuing.map(b=><ContinueCard key={b.id} book={b} selected={b.id===selected} onOpen={()=>{setSelected(b.id);setReading(b.id);}} onDetails={()=>setSelected(b.id)}/>)}</div></section>}
      <Shelf title="Recently added" hint={`in ${library.path}`} books={added} selected={selected} onSelect={setSelected} onAll={()=>choose({type:"all"})} actions={bookActions}/>
      {wanted.length>0&&<Shelf title="Want to read" books={wanted} selected={selected} onSelect={setSelected} onAll={()=>choose({type:"want"})} actions={bookActions}/>}
    </div>;
  }else if(!list.length)content=view.type==="library"?empty("library","This library is empty",`Add books here, or move them into Files › ${library.path} › ${currentLibrary?.name??""}.`,uploadButton)
    :view.type==="reading"?empty("reading","Nothing in progress","Books you start reading appear here."):view.type==="want"?empty("bookmark","Your list is empty","Choose Want to read on a book to keep it here.")
    :empty("check","No finished books","Books you finish, or mark as finished, appear here.");
  else content=<div className="books-scroll"><ul className="books-grid" aria-label={title}>{ordered(list).map(b=><li key={b.id}><BookCard book={b} selected={b.id===selected} onSelect={()=>setSelected(b.id)} actions={bookActions(b)}/></li>)}</ul>
    {library.truncated&&<p className="books-hint">This folder is very large; only the first books are shown.</p>}</div>;

  const details=book&&!open&&<aside className="pim-details books-details" tabIndex={-1} aria-label="Book details"><BookDetails key={book.id} book={book} books={books} session={session} onClose={()=>setSelected(undefined)} onRead={()=>setReading(book.id)} onStatus={status=>void setStatus(book,status)} onSelect={setSelected} actions={bookActions(book)}/></aside>;
  return <div className={`app-shell pim-shell books-shell${details?" has-pim-details":""}${open?" is-reading":""}`}>
    <WorkspaceHeader app="books" session={session} onSignedOut={onSignedOut} onNavigation={()=>setNavigation(!navigation)} navigationOpen={navigation} navigationId={navigationId} onSettings={()=>setSettings(true)}/>
    <div className="pim-workspace books-workspace">{navigation&&<button className="nav-scrim" aria-label="Close navigation" onClick={()=>setNavigation(false)}/>}
      <aside id={navigationId} ref={navigationRef} className={`sidebar books-sidebar${navigation?" is-open":""}`} aria-label="Books navigation">{sidebar}</aside>
      {open?<Reader key={open.id} book={open} session={session} preferences={stored.preferences} onPreferences={save} onClose={()=>{setReading(undefined);setReload(n=>n+1);}} onState={state=>patch(open.id,state)} actions={bookActions(open).slice(2)}/>
        :<main className="pim-main books-main" tabIndex={-1} aria-label={title}>
          <div className="books-heading-bar"><h1>{title}</h1>{library&&view.type!=="home"&&<span className="books-heading-count">{list.length}</span>}<div className="books-tools">{view.type!=="home"&&books.length>0&&sortMenu}</div></div>
          {error&&library&&<div role="alert" className="error books-error">{error}<button className="text-button" onClick={()=>{setError("");setReload(n=>n+1);}}>Reload</button></div>}
          {content}
        </main>}
      {details}
    </div>
    {transfers.length>0&&<section className="transfer-panel books-transfers" aria-label="Uploads"><div className="transfer-heading"><strong>{transfers.some(t=>t.status==="uploading")?`Adding ${transfers.filter(t=>t.status==="uploading").length}`:"Added books"}</strong><button className="icon-button" aria-label="Dismiss finished uploads" disabled={transfers.some(t=>t.status==="uploading")} onClick={()=>setTransfers([])}><Icon name="close"/></button></div>
      <ul>{transfers.map(t=><li key={t.id} className={`books-transfer is-${t.status}`}><span>{t.name}</span><progress max={1} value={t.progress} aria-label={`${t.name} upload progress`}/>{t.message?<small role="alert">{t.message}</small>:<small>{t.status==="complete"?"Done":`${Math.round(t.progress*100)}%`}</small>}</li>)}</ul></section>}
    {confirm&&<Dialog title="Move book to deleted files?" onClose={()=>setConfirm(undefined)}><ConfirmTrash book={confirm} onCancel={()=>setConfirm(undefined)} onConfirm={()=>trash(confirm).then(()=>setConfirm(undefined))}/></Dialog>}
    {settings&&<BooksSettings session={session} library={library} onClose={()=>{setSettings(false);setReload(n=>n+1);}}/>}
    <div className="books-toast-region" role="status" aria-live="polite">{notice&&<div className="settings-toast books-toast"><span>{notice.message}</span>{notice.undo&&<button type="button" className="text-button" onClick={()=>{const undo=notice.undo!;setNotice(null);undo();}}>Undo</button>}</div>}</div>
  </div>;
}

function ConfirmTrash({book,onCancel,onConfirm}:{book:BookItem;onCancel:()=>void;onConfirm:()=>Promise<void>}){
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  return <><p>“{book.title}” ({book.name}) moves to Files › Deleted files, where it can be restored. Your reading progress is kept.</p>{error&&<p className="error" role="alert">{error}</p>}
    <div className="dialog-actions"><button className="text-button" onClick={onCancel}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{setBusy(true);void onConfirm().catch(cause=>{setError(message(cause));setBusy(false);});}}>Move to deleted files</button></div></>;
}

/** The cover image, or a typographic cover when the book has none or it cannot load. */
function Cover({book,size="m"}:{book:BookItem;size?:"s"|"m"|"l"}){
  const [failed,setFailed]=useState(!book.cover);
  const done=book.state?.status==="finished",progress=book.state?.status==="reading"?book.state.progress:0;
  return <span className={`books-cover is-${size}${failed?` is-generated hue-${hue(book.title)}`:""}`}>
    {failed?<><b>{book.title}</b><i>{book.authors[0]??""}</i></>:<img src={`${api}/${book.id}/cover?r=${book.revision}`} alt="" loading="lazy" decoding="async" onError={()=>setFailed(true)}/>}
    {book.format!=="epub"&&size!=="s"&&<em className="books-format">{bookFormatLabel(book.format)}</em>}
    {progress>0&&progress<1&&<span className="books-progress"><progress max={1} value={progress} aria-label={`${percent(progress)} read`}/></span>}
    {done&&<span className="books-done" title="Finished"><Icon name="check"/><span className="sr-only">Finished</span></span>}
  </span>;
}
function BookCard({book,selected,onSelect,actions}:{book:BookItem;selected:boolean;onSelect:()=>void;actions:MenuAction[]}){
  return <div className={`books-card${selected?" is-selected":""}`}>
    <button type="button" className="books-card-open" aria-pressed={selected} onClick={onSelect}><Cover book={book}/><strong>{book.title}</strong><small>{book.series?`${book.series}${book.seriesIndex!==null?` · ${book.seriesIndex}`:""}`:authors(book)}</small></button>
    <ActionMenu label={`Actions for ${book.title}`} className="books-card-menu" actions={actions}><Icon name="more"/></ActionMenu>
  </div>;
}
function Shelf({title,hint,books,selected,onSelect,onAll,actions}:{title:string;hint?:string;books:BookItem[];selected?:string;onSelect:(id:string)=>void;onAll:()=>void;actions:(book:BookItem)=>MenuAction[]}){
  return <section aria-label={title}><div className="books-shelf-head"><h2>{title}</h2>{hint&&<small>{hint}</small>}<button className="text-button" onClick={onAll}>See all</button></div>
    <ul className="books-shelf">{books.map(b=><li key={b.id}><BookCard book={b} selected={b.id===selected} onSelect={()=>onSelect(b.id)} actions={actions(b)}/></li>)}</ul></section>;
}
function ContinueCard({book,selected,onOpen,onDetails}:{book:BookItem;selected:boolean;onOpen:()=>void;onDetails:()=>void}){
  const progress=book.state?.progress??0;
  return <div className={`books-continue-card${selected?" is-selected":""}`}>
    <button type="button" className="books-continue-cover" aria-label={`Details for ${book.title}`} onClick={onDetails}><Cover book={book} size="s"/></button>
    <button type="button" className="books-continue-body" onClick={onOpen}><strong>{book.title}</strong><small>{authors(book)}</small>
      <span className="books-continue-place">{place(book)}</span><progress max={1} value={progress} aria-label={`${percent(progress)} read`}/><span className="books-continue-meta"><span>{book.state?.updatedAt?edited(book.state.updatedAt):""}</span><span>Continue</span></span></button>
  </div>;
}
function place(book:BookItem){
  const p=book.state?.position;if(!p)return "Not started";
  if(p.page!==undefined)return book.pages?`Page ${p.page+1} of ${book.pages}`:`Page ${p.page+1}`;
  return `${percent(book.state!.progress)} through`;
}

/** The right details sheet: cover, progress, actions, description, series and file facts. */
function BookDetails({book,books,session,onClose,onRead,onStatus,onSelect,actions}:{book:BookItem;books:BookItem[];session:BrowserSession;onClose:()=>void;onRead:()=>void;onStatus:(status:BookStatus)=>void;onSelect:(id:string)=>void;actions:MenuAction[]}){
  const [more,setMore]=useState(false),root=useRef<HTMLDivElement>(null);void session;
  useEffect(()=>{root.current?.focus({preventScroll:true});},[]);
  const series=book.series?books.filter(b=>b.series===book.series).sort((a,b)=>(a.seriesIndex??1e9)-(b.seriesIndex??1e9)||a.title.localeCompare(b.title)):[];
  const progress=book.state?.progress??0,status=book.state?.status??"";
  const problem=book.error==="drm"?"This book is protected with DRM by the store it came from, so Dar cannot open it. Download it to read it in that store’s app."
    :book.error==="unsupported"?"This archive type could not be opened on this server. Download it to read it elsewhere.":book.error==="damaged"?"This file could not be read. It may be damaged or not a real "+bookFormatLabel(book.format)+" file.":"";
  return <div className="books-detail" ref={root} tabIndex={-1} onKeyDown={event=>{if(event.key==="Escape"&&!(event.target as HTMLElement).closest("[role=menu]")){event.preventDefault();onClose();}}}>
    <div className="books-detail-head"><button className="icon-button books-detail-back" aria-label="Close details" title="Close (Escape)" onClick={onClose}><Icon name="back"/></button><h2>Details</h2><ActionMenu label={`More actions for ${book.title}`} actions={actions.slice(1)}><Icon name="more"/></ActionMenu><button className="icon-button books-detail-close" aria-label="Close details" onClick={onClose}><Icon name="close"/></button></div>
    <div className="books-detail-body">
      <div className="books-hero"><Cover book={book} size="l"/><div><h3>{book.title}</h3><p>{authors(book)}</p>{book.series&&<p className="books-series">{book.series}{book.seriesIndex!==null&&` · Book ${book.seriesIndex}`}{series.length>1&&` of ${series.length}`}</p>}
        {status&&<p className="books-status-chip"><Icon name={status==="finished"?"check":status==="want"?"bookmark":"reading"}/>{statusLabel(status)}</p>}
        {status==="reading"&&<><progress max={1} value={progress} aria-label={`${percent(progress)} read`}/><small>{percent(progress)} · {place(book)}</small></>}</div></div>
      {problem?<p className="error" role="alert">{problem}</p>:<div className="books-quick">
        <button className="primary-button" onClick={onRead}><Icon name="reading"/>{status==="finished"?"Read again":book.state?.position?"Continue reading":"Read"}</button>
        <button className={`icon-button books-round${status==="want"?" is-on":""}`} aria-pressed={status==="want"} aria-label="Want to read" title="Want to read" onClick={()=>onStatus(status==="want"?"":"want")}><Icon name="bookmark"/></button>
        <a className="icon-button books-round" href={`/api/v1/files/${book.id}/content`} download aria-label="Download" title="Download"><Icon name="download"/></a>
        <button className={`icon-button books-round${status==="finished"?" is-on":""}`} aria-pressed={status==="finished"} aria-label={status==="finished"?"Mark as unread":"Mark as finished"} title={status==="finished"?"Mark as unread":"Mark as finished"} onClick={()=>onStatus(status==="finished"?"":"finished")}><Icon name="check"/></button>
      </div>}
      {book.description&&<div className={`books-description${more?" is-open":""}`}>{book.description.split("\n").map((line,i)=><p key={i}>{line}</p>)}{book.description.length>320&&<button className="text-button" aria-expanded={more} onClick={()=>setMore(!more)}>{more?"Less":"More"}</button>}</div>}
      {series.length>1&&<section aria-label={`${book.series} series`} className="books-series-strip"><h4>{book.series}</h4><div>{series.map(b=><button key={b.id} type="button" aria-current={b.id===book.id?"true":undefined} aria-label={`${b.title}${b.seriesIndex!==null?`, book ${b.seriesIndex}`:""}`} onClick={()=>onSelect(b.id)}><Cover book={b} size="s"/></button>)}</div></section>}
      <div className="card books-facts">
        <div><Icon name="file"/><strong>{bookFormatLabel(book.format)} · {formatBytes(book.size)}</strong>{book.pages>0&&<span>{book.format==="epub"?`≈ ${book.pages} pages`:`${book.pages} pages`}</span>}</div>
        {(book.published||book.publisher)&&<div><Icon name="calendar"/><strong>{book.published?`Published ${book.published}`:"Publisher"}</strong><span>{book.publisher}</span></div>}
        {book.language&&<div><Icon name="globe"/><strong>{languageName(book.language)}</strong></div>}
        {book.state?.position?<div><Icon name="clock"/><strong>Last read {edited(book.state.updatedAt)}</strong>{book.state.device&&<span>{book.state.device}</span>}</div>:null}
        <div><Icon name="folder"/><strong className="books-path">{book.path}</strong><a href={`/?folder=${book.parentId}&file=${book.id}`}>Show in Files</a></div>
      </div>
      {book.subjects.length>0&&<div className="books-tags">{book.subjects.map(s=><span key={s} className="books-tag">{s}</span>)}</div>}
    </div>
  </div>;
}
function languageName(code:string){try{return new Intl.DisplayNames([navigator.language],{type:"language"}).of(code)??code;}catch{return code;}}

const themes={paper:{background:"#ffffff",color:"#1f1a17",link:"#7a3b6f",dark:false},sepia:{background:"#f4ecd8",color:"#4b3a2c",link:"#8a4b23",dark:false},night:{background:"#141112",color:"#d9d0c5",link:"#e8a0a8",dark:true}} as const;
const lineHeights={tight:1.35,normal:1.6,loose:1.9} as const;
const fontSizes=[14,15,16,17,18,19,20,21,22,24,26,28];
interface Conflict {state:BookState;position:BookPosition|null}

/**
 * The reader in the middle sheet: a bar (back, title, place, Aa, bookmark, contents, full
 * screen, more), the book, and a footer with the place and a scrubber. Progress saves a
 * moment after each page turn with the state's revision, so a newer place from another
 * device is never overwritten without asking.
 */
function Reader({book,session,preferences,onPreferences,onClose,onState,actions}:{book:BookItem;session:BrowserSession;preferences:Preferences;onPreferences:(patch:Partial<Preferences>)=>Promise<unknown>;onClose:()=>void;onState:(state:BookState)=>void;actions:MenuAction[]}){
  const [detail,setDetail]=useState<BookDetail>(),[failed,setFailed]=useState(""),[panel,setPanel]=useState<"contents"|"bookmarks"|undefined>(()=>matchMedia("(min-width: 1101px)").matches&&book.format==="epub"?"contents":undefined);
  const [where,setWhere]=useState<{position:BookPosition;progress:number}>(),[pdf,setPdf]=useState<{pages:number;outline:PdfOutlineEntry[]}>(),[conflict,setConflict]=useState<Conflict>(),[look,setLook]=useState<ReaderLook>();
  const [typography,setTypography]=useState(false),[marks,setMarks]=useState<BookMark[]>([]),[saveError,setSaveError]=useState("");
  const root=useRef<HTMLElement>(null),stage=useRef<HTMLDivElement>(null),epub=useRef<EpubHandle>(null),comic=useRef<ComicHandle>(null),pdfView=useRef<PdfHandle>(null),aa=useRef<HTMLButtonElement>(null),aaPanel=useRef<HTMLDivElement>(null);
  const revision=useRef(book.state?.revision??0),pending=useRef<{position:BookPosition;progress:number}|undefined>(undefined),timer=useRef<ReturnType<typeof setTimeout>|undefined>(undefined),status=useRef<BookStatus>(book.state?.status??"");
  useFloating(typography,aa,aaPanel,"end");
  useEffect(()=>{const abort=new AbortController();void request<BookDetail>(`${api}/${book.id}`,{signal:abort.signal}).then(value=>{
    setDetail(value);setMarks(value.marks);if(value.state){revision.current=value.state.revision;status.current=value.state.status;}
  }).catch(cause=>{if(!abort.signal.aborted)setFailed(message(cause));});return()=>abort.abort();},[book.id]);
  useEffect(()=>{root.current?.focus({preventScroll:true});},[]);
  // The Dar theme follows the app's own colours; the others are fixed paper colours.
  const theme=preferences.booksTheme??"dar",font=preferences.booksFont??"serif",size=preferences.booksFontSize??18,lineHeight=lineHeights[preferences.booksLineHeight??"normal"],layout=preferences.booksLayout??"pages";
  useEffect(()=>{
    const node=stage.current;if(!node)return;
    const css=getComputedStyle(node),probe=node.querySelector<HTMLElement>(".books-accent-probe");
    const base=theme==="dar"?{background:css.backgroundColor,color:css.color,link:probe?getComputedStyle(probe).color:css.color,dark:document.documentElement.dataset.theme==="mocha"}:themes[theme];
    setLook({...base,font,size,lineHeight,layout});
  },[theme,font,size,lineHeight,layout,!!detail]);

  /** Saves the latest place after a short pause; a conflict shows the newer place instead. */
  const flush=useCallback(async()=>{
    const next=pending.current;if(!next)return;pending.current=undefined;
    const finished=next.progress>=0.999,status_=finished?"finished":status.current==="finished"?"finished":"reading";
    try{
      const state=await request<BookState>(`${api}/${book.id}/state`,{method:"PUT",body:JSON.stringify({revision:revision.current,position:next.position,progress:next.progress,status:status_,device:deviceName()})},session.csrfToken);
      revision.current=state.revision;status.current=state.status;onState(state);setSaveError("");
    }catch(cause){
      if(cause instanceof RequestError&&cause.status===409){
        const current=await request<BookDetail>(`${api}/${book.id}`).catch(()=>undefined);
        if(current?.state){revision.current=current.state.revision;status.current=current.state.status;
          // Only ask when the other device is somewhere else; otherwise adopt its revision quietly.
          if(current.state.position&&Math.abs(current.state.progress-next.progress)>0.005)setConflict({state:current.state,position:next.position});else{pending.current=next;void flush();}}
      }else setSaveError("Your place could not be saved. It will be saved with your next page turn.");
    }
  },[book.id]);
  const moved=useCallback((position:BookPosition,progress:number)=>{
    setWhere({position,progress});if(conflict)return;
    pending.current={position,progress};clearTimeout(timer.current);timer.current=setTimeout(()=>void flush(),1200);
  },[flush,conflict]);
  useEffect(()=>()=>{clearTimeout(timer.current);void flush();},[flush]);
  useEffect(()=>{const hide=()=>{if(document.visibilityState==="hidden")void flush();};document.addEventListener("visibilitychange",hide);return()=>document.removeEventListener("visibilitychange",hide);},[flush]);

  const manifest=detail?.manifest,start=detail?.state?.position??null;
  const spineSizes=manifest?.kind==="epub"?manifest.spine.map(s=>s.size):[],spineTotal=spineSizes.reduce((a,b)=>a+b,0)||1;
  const spineBefore=(index:number)=>spineSizes.slice(0,index).reduce((a,b)=>a+b,0);
  const toc:BookTocEntry[]=manifest?.kind==="epub"?manifest.toc:[];
  const chapter=(position:BookPosition|undefined)=>{if(!position)return "";
    if(position.spine!==undefined){let label="";for(const entry of toc)if(entry.spine>=0&&entry.spine<=position.spine)label=entry.label;return label;}
    let label="";for(const entry of pdf?.outline??[])if(entry.page>=0&&entry.page<=(position.page??0))label=entry.label;return label;};
  const pages=manifest?.kind==="comic"?manifest.pages:pdf?.pages??book.pages;
  const rightToLeft=manifest?.kind==="comic"?(preferences.booksComicDirection==="rtl"||(preferences.booksComicDirection??"auto")==="auto"&&manifest.rightToLeft):manifest?.kind==="epub"?manifest.rightToLeft:false;
  function goTo(position:BookPosition){
    if(position.spine!==undefined)epub.current?.goTo(position.spine,position.fraction);
    else if(manifest?.kind==="comic")comic.current?.goTo(position.page??0);
    else pdfView.current?.goTo(position.page??0,position.fraction);
  }
  function step(direction:1|-1){if(manifest?.kind==="epub")direction>0?epub.current?.next():epub.current?.previous();else if(manifest?.kind==="comic")direction>0?comic.current?.next():comic.current?.previous();else pdfView.current?.step(direction);}
  const marked=where?marks.find(m=>m.position.spine===where.position.spine&&m.position.page===where.position.page&&(manifest?.kind==="comic"||Math.abs(m.position.fraction-where.position.fraction)<0.02)):undefined;
  async function toggleMark(){
    if(!where)return;
    try{
      if(marked){await request(`${api}/${book.id}/marks/${marked.id}`,{method:"DELETE"},session.csrfToken);setMarks(list=>list.filter(m=>m.id!==marked.id));}
      else{const label=chapter(where.position)||(where.position.page!==undefined?`Page ${where.position.page+1}`:`${percent(where.progress)}`);
        const mark=await request<BookMark>(`${api}/${book.id}/marks`,{method:"POST",body:JSON.stringify({id:crypto.randomUUID(),position:where.position,label})},session.csrfToken);setMarks(list=>[...list,mark]);}
    }catch(cause){setSaveError(message(cause));}
  }
  async function fullscreen(){try{if(document.fullscreenElement)await document.exitFullscreen();else await root.current?.requestFullscreen();}catch{/* not allowed here */}}
  function keys(event:React.KeyboardEvent){
    if(event.defaultPrevented||(event.target as HTMLElement).closest("input,select,textarea,[role=menu],[role=dialog],.books-reader-panel,.books-typography"))return;
    if(event.key==="Escape"){event.preventDefault();if(typography)setTypography(false);else if(document.fullscreenElement)void document.exitFullscreen();else onClose();}
    // PDFs scroll natively with Space and Page keys; books and comics turn pages with them.
    else if(event.key===(rightToLeft?"ArrowLeft":"ArrowRight")||(manifest?.kind!=="pdf"&&(event.key==="PageDown"||event.key===" "&&!event.shiftKey))){event.preventDefault();step(1);}
    else if(event.key===(rightToLeft?"ArrowRight":"ArrowLeft")||(manifest?.kind!=="pdf"&&(event.key==="PageUp"||event.key===" "&&event.shiftKey))){event.preventDefault();step(-1);}
    else if(event.key==="f"&&!event.ctrlKey&&!event.metaKey){event.preventDefault();void fullscreen();}
  }
  const progressValue=where?.progress??book.state?.progress??0;
  const footer=where?where.position.page!==undefined?`Page ${where.position.page+1} of ${pages||"…"}`:chapter(where.position)||"":"";
  const set=(patch:Partial<Preferences>)=>void onPreferences(patch);
  const contents=manifest?.kind==="epub"?toc.map((entry,index)=><button key={index} type="button" className={`books-toc-row depth-${entry.depth}${where?.position.spine===entry.spine&&chapter(where.position)===entry.label?" is-current":""}`} disabled={entry.spine<0} onClick={()=>{const anchor=entry.href.split("#")[1];epub.current?.goTo(entry.spine,0,anchor);if(matchMedia("(max-width: 1100px)").matches)setPanel(undefined);}}>{entry.label}</button>)
    :manifest?.kind==="pdf"&&pdf?.outline.length?pdf.outline.map((entry,index)=><button key={index} type="button" className={`books-toc-row depth-${entry.depth}`} disabled={entry.page<0} onClick={()=>{pdfView.current?.goTo(entry.page);if(matchMedia("(max-width: 1100px)").matches)setPanel(undefined);}}><span>{entry.label}</span><small>{entry.page+1}</small></button>)
    :<p className="books-hint">{manifest?.kind==="comic"?"Comics have no table of contents. Use the scrubber below to move through the pages.":"This book has no table of contents."}</p>;
  return <main ref={root} className={`pim-main books-reader books-theme-${theme}${panel?" has-panel":""}`} tabIndex={-1} aria-label={`Reading ${book.title}`} onKeyDown={keys}>
    <div className="books-reader-bar">
      <button className="icon-button" aria-label="Back to library" title="Back to library (Escape)" onClick={onClose}><Icon name="back"/></button>
      <h1>{book.title}</h1>{where&&chapter(where.position)&&<span className="books-reader-chapter"><Icon name="chevron"/>{chapter(where.position)}</span>}
      <div className="books-tools">
        {manifest?.kind==="pdf"&&<span className="books-zoom" role="group" aria-label="Zoom"><button className="icon-button" aria-label="Zoom out" onClick={()=>pdfView.current?.zoom("out")}><Icon name="minus"/></button><button className="icon-button" aria-label="Fit width" title="Fit width" onClick={()=>pdfView.current?.zoom("width")}><Icon name="fitWidth"/></button><button className="icon-button" aria-label="Zoom in" onClick={()=>pdfView.current?.zoom("in")}><Icon name="plus"/></button></span>}
        {manifest?.kind==="epub"&&<button ref={aa} className={`icon-button${typography?" is-active":""}`} aria-label="Text and layout" title="Text and layout" aria-expanded={typography} onClick={()=>setTypography(!typography)}><Icon name="typography"/></button>}
        <button className={`icon-button${marked?" is-on":""}`} aria-pressed={!!marked} aria-label={marked?"Remove bookmark":"Add bookmark"} title={marked?"Remove bookmark":"Bookmark this page"} disabled={!where} onClick={()=>void toggleMark()}><Icon name="bookmark"/></button>
        <button className={`icon-button${panel?" is-active":""}`} aria-pressed={!!panel} aria-label="Contents and bookmarks" title="Contents and bookmarks" onClick={()=>setPanel(panel?undefined:"contents")}><Icon name="panel"/></button>
        <button className="icon-button books-fullscreen" aria-label="Full screen" title="Full screen (F)" onClick={()=>void fullscreen()}><Icon name="expand"/></button>
        <ActionMenu label="More reading actions" actions={actions}><Icon name="more"/></ActionMenu>
      </div>
    </div>
    {typography&&look&&<div ref={aaPanel} className="books-typography" role="dialog" aria-label="Text and layout" onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();event.stopPropagation();setTypography(false);aa.current?.focus();}}}>
      <fieldset><legend>Theme</legend><div className="books-themes">{(["dar","paper","sepia","night"] as const).map(value=><button key={value} type="button" className={`books-theme-swatch is-${value}`} aria-pressed={theme===value} onClick={()=>set({booksTheme:value})}><i>Aa</i>{value==="dar"?"Dar":value[0].toUpperCase()+value.slice(1)}</button>)}</div></fieldset>
      <label>Font<Select value={font} onChange={event=>set({booksFont:event.target.value as "serif"|"sans"|"book"})}><option value="serif">Serif</option><option value="sans">Sans serif</option><option value="book">The book’s own</option></Select></label>
      <div className="books-size"><span>Size <small>{size} px</small></span><div><button className="icon-button" aria-label="Smaller text" disabled={size<=fontSizes[0]} onClick={()=>set({booksFontSize:fontSizes[Math.max(0,fontSizes.indexOf(size)-1)]})}><Icon name="minus"/></button><button className="icon-button" aria-label="Larger text" disabled={size>=fontSizes.at(-1)!} onClick={()=>set({booksFontSize:fontSizes[Math.min(fontSizes.length-1,fontSizes.indexOf(size)+1)]})}><Icon name="plus"/></button></div></div>
      <span className="books-typography-label">Line spacing</span><Segmented label="Line spacing" value={preferences.booksLineHeight??"normal"} options={[["tight","Tight"],["normal","Normal"],["loose","Loose"]] as const} onChange={value=>set({booksLineHeight:value})}/>
      <span className="books-typography-label">Layout</span><Segmented label="Layout" value={layout} options={[["pages","Pages"],["scroll","Scroll"]] as const} onChange={value=>set({booksLayout:value})}/>
      <p className="books-hint">Saved to your account for every book.</p>
    </div>}
    <div className="books-reader-body">
      <div ref={stage} className="books-stage"><span className="books-accent-probe" aria-hidden="true"/>
        {failed?<div className="books-empty" role="alert"><h2>This book could not be opened</h2><p>{failed}</p><button className="secondary-button" onClick={onClose}>Back to library</button></div>
          :!detail||!look?<p className="books-empty" role="status">Opening {book.title}…</p>
          :manifest?.kind==="epub"?<EpubView ref={epub} base={`${api}/${book.id}/epub/r${book.revision}/`} spine={manifest.spine} rightToLeft={rightToLeft} look={look} label={book.title}
              start={{spine:start?.spine??0,fraction:start?.fraction??0}} onError={setFailed} onLink={url=>{const target=new URL(url,location.href);if(target.origin===location.origin&&target.pathname.startsWith(`${api}/${book.id}/epub/`)){const path=decodeURIComponent(target.pathname.split("/").slice(7).join("/")),index=manifest.spine.findIndex(s=>s.href===path);if(index>=0)epub.current?.goTo(index,0,target.hash.slice(1)||undefined);}else window.open(target.href,"_blank","noopener,noreferrer");}}
              onPosition={(spine,fraction)=>moved({spine,fraction},Math.min(1,(spineBefore(spine)+fraction*spineSizes[spine])/spineTotal))}/>
          :manifest?.kind==="comic"?<ComicView ref={comic} base={`${api}/${book.id}/pages/`} pages={manifest.pages} start={start?.page??0} rightToLeft={rightToLeft} label={book.title} onPage={page=>moved({page,fraction:0},manifest.pages>1?page/(manifest.pages-1):1)}/>
          :<PdfView ref={pdfView} url={`/api/v1/files/${book.id}/content?v=${book.revision}`} page={start?.page??0} fraction={start?.fraction??0} label={book.title} onReady={setPdf} onError={setFailed}
              onPosition={(page,fraction,count)=>moved({page,fraction},count>1?Math.min(1,(page+fraction)/(count-1)):1)}/>}
      </div>
      {panel&&<aside className="books-reader-panel" aria-label="Contents and bookmarks">
        <div className="books-panel-tabs" role="tablist"><button role="tab" aria-selected={panel==="contents"} onClick={()=>setPanel("contents")}>Contents</button><button role="tab" aria-selected={panel==="bookmarks"} onClick={()=>setPanel("bookmarks")}>Bookmarks{marks.length>0&&<small>{marks.length}</small>}</button><button className="icon-button" aria-label="Close contents" onClick={()=>setPanel(undefined)}><Icon name="close"/></button></div>
        <div className="books-panel-body" role="tabpanel">{panel==="contents"?contents:marks.length?marks.map(m=><div key={m.id} className="books-mark"><button type="button" onClick={()=>goTo(m.position)}><Icon name="bookmark"/><span>{m.label||"Bookmark"}<small>{m.position.page!==undefined?`Page ${m.position.page+1}`:""} · {edited(m.createdAt)}</small></span></button><button className="icon-button" aria-label={`Remove bookmark ${m.label}`} onClick={()=>void request(`${api}/${book.id}/marks/${m.id}`,{method:"DELETE"},session.csrfToken).then(()=>setMarks(list=>list.filter(x=>x.id!==m.id))).catch(cause=>setSaveError(message(cause)))}><Icon name="close"/></button></div>)
          :<p className="books-hint">Bookmark a page with the bookmark button. Bookmarks are kept with your account.</p>}</div>
      </aside>}
    </div>
    <div className="books-reader-foot">
      <span>{footer}</span>
      <input type="range" min={0} max={1000} step={1} value={Math.round(progressValue*1000)} aria-label="Place in book" aria-valuetext={`${percent(progressValue)} read`} onChange={event=>{
        const value=Number(event.target.value)/1000;
        if(manifest?.kind==="epub"){let at=0,index=0;for(;index<spineSizes.length-1&&at+spineSizes[index]<value*spineTotal;index++)at+=spineSizes[index];epub.current?.goTo(index,Math.max(0,Math.min(1,(value*spineTotal-at)/(spineSizes[index]||1))));}
        else if(manifest?.kind==="comic")comic.current?.goTo(Math.round(value*(manifest.pages-1)));
        else if(pdf)pdfView.current?.goTo(Math.round(value*(pdf.pages-1)));
      }}/>
      <span>{percent(progressValue)}</span>
    </div>
    {saveError&&<p className="books-save-error" role="alert">{saveError}</p>}
    {conflict&&<div className="books-conflict" role="alertdialog" aria-label="Your place changed on another device">
      <strong>{conflict.state.progress>progressValue?"You read further":"Your place changed"}{conflict.state.device?` on ${conflict.state.device}`:" on another device"}</strong>
      <p>There: {percent(conflict.state.progress)}{conflict.state.position?.page!==undefined?`, page ${conflict.state.position.page+1}`:""} ({edited(conflict.state.updatedAt)}). Here: {percent(progressValue)}.</p>
      <div className="dialog-actions"><button className="secondary-button" onClick={()=>{setConflict(undefined);if(where){pending.current=where;void flush();}}}>Stay here</button><button className="primary-button" autoFocus onClick={()=>{const target=conflict.state.position;setConflict(undefined);if(target)goTo(target);}}>Go there</button></div>
    </div>}
  </main>;
}

function BooksSettings({session,library,onClose}:{session:BrowserSession;library?:BookLibrary;onClose:()=>void}){
  const saver=usePreferenceSaver(session),[section,setSection]=useState("General"),p=saver.preferences,state=saver.states;
  const saved=p?.booksFolder??library?.path??defaultBooksFolder,[draft,setDraft]=useState<string>(),value=draft??saved,valid=!!photoFolderParts(value);
  return <SettingsSheet title="Books settings" icon="book" sections={["General","Reading","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={onClose} notice={saver.notice} onNotice={saver.setNotice} footer="Books keeps reading progress, lists and bookmarks with your account. The books themselves stay in Files.">
    <SaveProblems error={saver.error} conflict={saver.conflict} onReload={()=>void saver.reload()}/>
    {!p?<p role="status">Loading settings…</p>:section==="General"?<>
      <section className="settings-group" aria-label="Library"><h3>Library</h3>
        <SettingRow label="Books folder" hint={`Books finds EPUB, PDF and comic files in Files › ${saved}. Its top folders are libraries.`} state={state.booksFolder} stacked>
          <form className="photos-folder-form" onSubmit={event=>{event.preventDefault();const parts=photoFolderParts(value);if(!parts)return;setDraft(undefined);saver.change("booksFolder",parts.join("/"),`Books now come from ${parts.join("/")}`);}}><label><span className="sr-only">Books folder</span><input value={value} maxLength={500} onChange={event=>setDraft(event.target.value)} aria-invalid={!valid}/></label><button className="secondary-button" disabled={!valid||value===saved}>Save</button></form>
          {!valid&&<p className="field-error" role="alert">Use folder names separated by /, like docs/books.</p>}
        </SettingRow>
        <SettingRow label="Sort books by" hint="Order of the grids" state={state.booksSort} stacked><Segmented label="Sort books by" value={p.booksSort??"recent"} options={sorts} onChange={next=>saver.change("booksSort",next,`Books sorted by ${sorts.find(([v])=>v===next)![1].toLowerCase()}`)}/></SettingRow>
      </section></>
    :section==="Reading"?<>
      <section className="settings-group" aria-label="Text"><h3>Text</h3>
        <SettingRow label="Theme" hint="Page colours while reading. Dar follows your appearance setting." state={state.booksTheme} stacked><Segmented label="Theme" value={p.booksTheme??"dar"} options={[["dar","Dar"],["paper","Paper"],["sepia","Sepia"],["night","Night"]] as const} onChange={next=>saver.change("booksTheme",next,"Reading theme changed")}/></SettingRow>
        <SettingRow label="Font" hint="Choose the book’s own to keep the publisher’s fonts" state={state.booksFont} stacked><Segmented label="Font" value={p.booksFont??"serif"} options={[["serif","Serif"],["sans","Sans serif"],["book","The book’s own"]] as const} onChange={next=>saver.change("booksFont",next,"Reading font changed")}/></SettingRow>
        <SettingRow label="Text size" state={state.booksFontSize}><Select aria-label="Text size" value={String(p.booksFontSize??18)} onChange={event=>saver.change("booksFontSize",Number(event.target.value),`Text size set to ${event.target.value} px`)}>{fontSizes.map(n=><option key={n} value={n}>{n} px</option>)}</Select></SettingRow>
        <SettingRow label="Line spacing" state={state.booksLineHeight}><Segmented label="Line spacing" value={p.booksLineHeight??"normal"} options={[["tight","Tight"],["normal","Normal"],["loose","Loose"]] as const} onChange={next=>saver.change("booksLineHeight",next,"Line spacing changed")}/></SettingRow>
        <SettingRow label="Layout" hint="Turn pages, or scroll through each chapter" state={state.booksLayout}><Segmented label="Layout" value={p.booksLayout??"pages"} options={[["pages","Pages"],["scroll","Scroll"]] as const} onChange={next=>saver.change("booksLayout",next,`Books open in ${next==="pages"?"Pages":"Scroll"} layout`)}/></SettingRow>
      </section>
      <section className="settings-group" aria-label="Comics"><h3>Comics</h3>
        <SettingRow label="Reading direction" hint="Automatic uses the comic’s own setting (manga read right to left)" state={state.booksComicDirection} stacked><Segmented label="Reading direction" value={p.booksComicDirection??"auto"} options={[["auto","Automatic"],["ltr","Left to right"],["rtl","Right to left"]] as const} onChange={next=>saver.change("booksComicDirection",next,"Comic direction changed")}/></SettingRow>
      </section></>
    :<dl className="ui-shortcuts"><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd><dt>→ / Space / Page Down</dt><dd>Next page (← in right-to-left books)</dd><dt>← / Shift Space / Page Up</dt><dd>Previous page</dd><dt>F</dt><dd>Full screen (PDFs and comics)</dd><dt>Escape</dt><dd>Close menus, then the book, then details</dd><dt>Tab / Shift Tab</dt><dd>Move between controls</dd></dl>}
  </SettingsSheet>;
}
