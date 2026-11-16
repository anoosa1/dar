import {useEffect,useRef,useState,type FormEvent} from "react";
import type {FileEntries as FileEntry} from "./module_bindings/types.ts";
import {request,readProcedure,type BrowserSession} from "./api.ts";
import {useLiveConnection} from "./live.ts";
import {Icon} from "./icons.tsx";
import {ActionMenu,NewItemButton,Select,type MenuAction} from "./controls.tsx";
import {Dialog} from "./dialog.tsx";
import {EditorPane} from "./pane.tsx";
import {TextEditor,writeTextFile} from "./text-editor.tsx";
import {PimShell,usePim} from "./pim/common.tsx";
import {usePreferences,savePreferences} from "./app-settings.tsx";
import {edited} from "./pim/presentation.ts";
import type {NotesFile,NotesFolder,NotesLibrary} from "../shared/workspace.ts";
import type {PimCollection} from "../shared/pim.ts";
import type {Preferences} from "../shared/preferences.ts";

interface Detail {entry:FileEntry;role:string}
type Scope="all"|"favorites"|string;
type Pane={type:"folder";parentId:string}|{type:"renameFolder";folder:NotesFolder}|{type:"renameNote";note:NotesFile}|{type:"move";note:NotesFile};
const message=(cause:unknown)=>{const text=cause instanceof Error?cause.message:"The request could not be completed.";return /conflict/i.test(text)?"That name is already used here, or the item changed. Refresh and try again.":/forbidden|unauthenticated/i.test(text)?"Your access changed. Refresh the page or sign in again.":text;};
/** File names keep the title readable and drop characters Files does not allow. */
const fileName=(title:string)=>`${title.replace(/[\x00-\x1f/\\]+/g," ").replace(/\s+/g," ").trim().slice(0,200)||"Untitled"}.md`;
function unique(name:string,taken:Set<string>){if(!taken.has(name.toLowerCase()))return name;const base=name.replace(/\.md$/i,"");for(let n=2;n<1000;n++){const next=`${base} ${n}.md`;if(!taken.has(next.toLowerCase()))return next;}return `${base} ${Date.now()}.md`;}

/**
 * Notes are Markdown files in a Files folder (Files › docs/notes by default), like an
 * Obsidian vault: folders are notebooks, any Markdown editor or WebDAV client edits the
 * same files, and Files' versions, favorites, sharing and deleted files apply.
 */
export function Notes({session,onSignedOut}:{session:BrowserSession;onSignedOut:()=>void}){
  const pim=usePim(session,onSignedOut),live=useLiveConnection(session),connection=live.connection;
  const params=new URLSearchParams(location.search);
  const [library,setLibrary]=useState<NotesLibrary>(),[error,setError]=useState(""),[notice,setNotice]=useState(""),[query,setQuery]=useState(""),[scope,setScope]=useState<Scope>(params.get("folder")??"all");
  const [open,setOpen]=useState<FileEntry>(),[pane,setPane]=useState<Pane>(),[confirm,setConfirm]=useState<{title:string;text:string;action:string;run:()=>Promise<void>}>(),[busy,setBusy]=useState(false),[reload,setReload]=useState(0);
  const lastSaved=useRef<{id:string;content:string}|undefined>(undefined);
  const stored=usePreferences(),sort=stored.preferences.notesSort??"modified",initialNote=useRef(params.get("note")??params.get("file"));
  // Notes change in Files too (WebDAV, other apps), so the list also refreshes when the page regains focus.
  useEffect(()=>{const abort=new AbortController(),timer=setTimeout(()=>{void request<NotesLibrary>(`/api/v1/notes/library?${new URLSearchParams({q:query})}`,{signal:abort.signal}).then(value=>{setLibrary(value);setError("");}).catch(cause=>{if(!abort.signal.aborted)setError(message(cause));});},query?200:0);return()=>{abort.abort();clearTimeout(timer);};},[query,reload,pim.epoch]);
  useEffect(()=>{const focus=()=>setReload(n=>n+1);addEventListener("focus",focus);return()=>removeEventListener("focus",focus);},[]);
  useEffect(()=>{const url=new URL(location.href);url.searchParams.delete("file");if(open)url.searchParams.set("note",open.id);else url.searchParams.delete("note");if(scope==="all")url.searchParams.delete("folder");else url.searchParams.set("folder",scope);history.replaceState(null,"",url);},[open?.id,scope]);
  const details=async(id:string)=>{if(!connection)throw Error("Notes are reconnecting. Try again in a moment.");return readProcedure<Detail>(await connection.procedures.fileDetails({id}));};
  useEffect(()=>{const id=initialNote.current;if(!id||!connection)return;initialNote.current=null;void details(id).then(value=>setOpen(value.entry)).catch(cause=>setError(message(cause)));},[connection]);
  /** Saves only the changed preference; the server keeps every other value. */
  const save=(patch:Partial<Preferences>)=>savePreferences({revision:stored.preferences.revision,...patch},session);
  async function run(work:()=>Promise<void>,done=""){setBusy(true);setError("");try{await work();if(done)setNotice(done);setReload(n=>n+1);return true;}catch(cause){setError(message(cause));return false;}finally{setBusy(false);}}

  const folders=library?.folders??[],byId=new Map(folders.map(folder=>[folder.id,folder]));
  const currentFolder=scope!=="all"&&scope!=="favorites"?byId.get(scope):undefined;
  const target=currentFolder?.id??library?.rootId??"";
  const inScope=(note:NotesFile)=>scope==="all"?true:scope==="favorites"?note.favorite:currentFolder?note.path.startsWith(currentFolder.path+"/"):true;
  const rows=(library?.notes??[]).filter(inScope).sort((a,b)=>sort==="title"?a.title.localeCompare(b.title):b.modifiedAt-a.modifiedAt);
  const folderName=(id:string)=>id===library?.rootId?"Notes":byId.get(id)?.name??"Notes";
  const siblings=(parentId:string)=>new Set((library?.notes??[]).filter(note=>note.parentId===parentId).map(note=>note.name.toLowerCase()));

  async function newNote(){
    if(!library)return;
    await run(async()=>{const name=unique("Untitled.md",siblings(target));const id=await writeTextFile(session,{name,parentId:target,content:"",mediaType:"text/markdown"});setOpen((await details(id)).entry);});
  }
  /** A new note keeps its Untitled name only until it has a heading; the file then takes the heading as its name. */
  async function nameFromHeading(entry:FileEntry){
    const saved=lastSaved.current;lastSaved.current=undefined;
    if(!connection||saved?.id!==entry.id||!/^Untitled( \d+)?\.md$/i.test(entry.name))return;
    const heading=/^#\s+(.+?)\s*#*\s*$/m.exec(saved.content)?.[1]?.trim();if(!heading)return;
    try{const current=(await details(entry.id)).entry,name=unique(fileName(heading),siblings(current.parentId));if(name!==current.name)await connection.reducers.renameEntry({id:current.id,name,expectedRevision:current.revision});}
    catch{/* The note keeps its name; renaming is a convenience. */}
  }
  async function openNote(note:NotesFile){try{setOpen((await details(note.id)).entry);}catch(cause){setError(message(cause));}}
  async function favorite(note:NotesFile){await run(async()=>{const entry=(await details(note.id)).entry;await connection!.reducers.favoriteEntry({id:entry.id,favorite:!entry.favorite,expectedRevision:entry.revision});});}
  function remove(note:NotesFile){setConfirm({title:"Move note to deleted files?",action:"Move to deleted files",text:`“${note.title}” can be restored from Files › Deleted files.`,run:async()=>{const entry=(await details(note.id)).entry;await connection!.reducers.trashEntry({id:entry.id,expectedRevision:entry.revision});if(open?.id===note.id)setOpen(undefined);}});}
  function removeFolder(folder:NotesFolder){const count=(library?.notes??[]).filter(note=>note.path.startsWith(folder.path+"/")).length;setConfirm({title:"Move folder to deleted files?",action:"Move to deleted files",text:`“${folder.name}” and its ${count} ${count===1?"note":"notes"} can be restored from Files › Deleted files.`,run:async()=>{const entry=(await details(folder.id)).entry;await connection!.reducers.trashEntry({id:entry.id,expectedRevision:entry.revision});if(scope===folder.id)setScope("all");}});}
  const noteActions=(note:NotesFile):MenuAction[]=>[
    {label:note.favorite?"Remove from favorites":"Add to favorites",icon:"star",disabled:busy,run:()=>{void favorite(note);}},
    {label:"Rename",icon:"pencil",disabled:busy,run:()=>setPane({type:"renameNote",note})},
    {label:"Move to…",icon:"move",disabled:busy||!folders.length,run:()=>setPane({type:"move",note})},
    {label:"Show in Files",icon:"folder",href:`/?folder=${note.parentId}&file=${note.id}`},
    {label:"Move to deleted files",icon:"trash",danger:true,separator:true,disabled:busy,run:()=>remove(note)},
  ];
  const openNoteRow=library?.notes.find(note=>note.id===open?.id);

  const sidebar=<>
    <NewItemButton disabled={busy||!library} onClick={()=>{void newNote();}}>New note</NewItemButton>
    <nav aria-label="Notes views">{([["all","All notes","list"],["favorites","Favorites","star"]] as const).map(([value,label,icon])=><button key={value} className={`nav-item ${scope===value?"active":""}`} aria-current={scope===value?"page":undefined} onClick={()=>setScope(value)}><Icon name={icon}/>{label}</button>)}</nav>
    <div className="sidebar-heading">Folders<button className="text-button" aria-label="New folder" disabled={!library} onClick={()=>setPane({type:"folder",parentId:target})}>＋</button></div>
    <div className="notes-folders" role="list" aria-label="Folders">{[...folders].sort((a,b)=>a.path.localeCompare(b.path)).map(folder=>{const depth=Math.min(folder.path.split("/").length-1,5);return <div role="listitem" key={folder.id} className={`notes-folder-row depth-${depth}`}>
      <button className={`nav-item ${scope===folder.id?"active":""}`} aria-current={scope===folder.id?"page":undefined} onClick={()=>setScope(folder.id)}><Icon name="folder"/><span>{folder.name}</span></button>
      <ActionMenu label={`Actions for folder ${folder.name}`} className="contacts-book-actions" actions={[{label:"New folder inside",icon:"plus",run:()=>setPane({type:"folder",parentId:folder.id})},{label:"Rename folder",icon:"pencil",run:()=>setPane({type:"renameFolder",folder})},{label:"Show in Files",icon:"external",href:`/?folder=${folder.id}`},{label:"Move to deleted files",icon:"trash",danger:true,separator:true,run:()=>removeFolder(folder)}]}><Icon name="more"/></ActionMenu>
    </div>;})}{library&&!folders.length&&<p className="muted small notes-folders-empty">Folders in Files › {library.path} appear here.</p>}</div>
  </>;

  const editor=open&&<TextEditor key={open.id} entry={open} session={session} load={details} context={folderName(open.parentId)} startInPreview={stored.preferences.notesEditorMode==="preview"}
    onClose={()=>{const closed=open;setOpen(undefined);void nameFromHeading(closed).finally(()=>setReload(n=>n+1));}} onSaved={(_revision,content)=>{lastSaved.current={id:open.id,content};setReload(n=>n+1);}}
    actions={openNoteRow?noteActions(openNoteRow):[]}/>;
  const title=scope==="favorites"?"Favorites":currentFolder?.name??"All notes";
  return <PimShell kind="notes" session={session} onSignedOut={onSignedOut} query={query} onQuery={setQuery} sidebar={sidebar} details={editor||undefined} noteEditor={!!open} connectionState={pim.connectionState}>
    <div className="workspace-list notes-list">
      <div className="workspace-heading"><h1>{title}</h1><ActionMenu label="Sort notes" className="workspace-sort" actions={([["modified","Recently edited"],["title","Title"]] as const).map(([value,label])=>({label,current:sort===value,run:()=>{void save({notesSort:value}).catch(cause=>setError(message(cause)));}}))}><Icon name="sort"/><span>{sort==="title"?"Title":"Edited"}</span></ActionMenu></div>
      {library&&library.legacy>0&&!stored.preferences.notesImported&&<LegacyImport session={session} library={library} save={save} connection={connection} onDone={text=>{setNotice(text);setReload(n=>n+1);}} onError={text=>setError(text)}/>}
      {error&&<div role="alert" className="error">{error}<button className="text-button" onClick={()=>{setError("");setReload(n=>n+1);}}>Reload</button></div>}
      {notice&&<p role="status" className="notes-notice">{notice}<button className="icon-button" aria-label="Dismiss" onClick={()=>setNotice("")}><Icon name="close"/></button></p>}
      {!library&&!error&&<p role="status">Loading notes…</p>}
      {library&&!rows.length&&<div className="empty-state"><span className="empty-icon"><Icon name={query?"search":scope==="favorites"?"star":"file"}/></span><h2>{query?"No matching notes":scope==="favorites"?"No favorite notes":"No notes yet"}</h2><p>{query?"Search covers note names, headings and text.":scope==="favorites"?"Star a note to find it here.":`Notes are Markdown files in Files › ${library.path}${currentFolder?` › ${currentFolder.path}`:""}.`}</p>{!query&&scope!=="favorites"&&<button className="primary-button" disabled={busy} onClick={()=>{void newNote();}}><Icon name="plus"/>New note</button>}</div>}
      {rows.map(note=><div className="workspace-entry" key={note.id}><article className={`workspace-row ${open?.id===note.id?"selected":""}`}>
        <button className="workspace-row-content" onClick={()=>{if(open?.id!==note.id)void openNote(note);}}><strong>{note.title||"Untitled"}{note.favorite&&<Icon name="star" className="row-favorite"/>}</strong><small className="note-excerpt">{note.excerpt||"No additional text"}</small><span className="row-meta"><span>{edited(note.modifiedAt)}</span>{note.parentId!==library?.rootId&&<span><Icon name="folder"/>{folderName(note.parentId)}</span>}</span></button>
        <div className="workspace-row-tools"><ActionMenu label={`Actions for ${note.title}`} className="ui-file-actions" actions={noteActions(note)}><Icon name="more"/></ActionMenu></div>
      </article></div>)}
      {library?.truncated&&<p className="muted small">This folder is large; only the first notes are shown. Use search to find others.</p>}
    </div>
    {pane&&<NotesPane pane={pane} folders={folders} library={library} busy={busy} onClose={()=>setPane(undefined)} onSubmit={async value=>{
      const ok=await run(async()=>{
        if(!connection)throw Error("Notes are reconnecting. Try again in a moment.");
        if(pane.type==="folder")await connection.reducers.createFolder({id:crypto.randomUUID(),parentId:pane.parentId,name:value});
        else if(pane.type==="renameFolder"){const entry=(await details(pane.folder.id)).entry;await connection.reducers.renameEntry({id:entry.id,name:value,expectedRevision:entry.revision});}
        else if(pane.type==="renameNote"){const entry=(await details(pane.note.id)).entry;await connection.reducers.renameEntry({id:entry.id,name:/\.(md|markdown)$/i.test(value)?value:`${value}.md`,expectedRevision:entry.revision});if(open?.id===entry.id)setOpen((await details(entry.id)).entry);}
        else{const entry=(await details(pane.note.id)).entry;await connection.reducers.moveEntry({id:entry.id,parentId:value,expectedRevision:entry.revision});if(open?.id===entry.id)setOpen((await details(entry.id)).entry);}
      },pane.type==="move"?`Moved to ${folderName(value)}.`:"");
      if(ok)setPane(undefined);
    }}/>}
    {confirm&&<Dialog title={confirm.title} onClose={()=>setConfirm(undefined)}><p>{confirm.text}</p>{error&&<p className="error" role="alert">{error}</p>}<div className="dialog-actions"><button className="text-button" onClick={()=>setConfirm(undefined)}>Cancel</button><button className="danger-button" disabled={busy} onClick={()=>{void run(confirm.run,"Moved to deleted files.").then(ok=>{if(ok)setConfirm(undefined);});}}>{confirm.action}</button></div></Dialog>}
  </PimShell>;
}

/** New folder, rename and move use the right editing pane. */
function NotesPane({pane,folders,library,busy,onClose,onSubmit}:{pane:Pane;folders:NotesFolder[];library?:NotesLibrary;busy:boolean;onClose:()=>void;onSubmit:(value:string)=>Promise<void>}){
  const initial=pane.type==="renameFolder"?pane.folder.name:pane.type==="renameNote"?pane.note.name.replace(/\.(md|markdown)$/i,""):pane.type==="move"?pane.note.parentId:"";
  const [value,setValue]=useState(initial);
  const title=pane.type==="folder"?"New folder":pane.type==="renameFolder"?"Rename folder":pane.type==="renameNote"?"Rename note":"Move note";
  function submit(event:FormEvent){event.preventDefault();if(value.trim())void onSubmit(value.trim());}
  return <EditorPane title={title} onClose={onClose}><form onSubmit={submit}>
    {pane.type==="move"?<label>Folder<Select value={value} onChange={event=>setValue(event.target.value)}><option value={library?.rootId??""}>Notes (top level)</option>{[...folders].sort((a,b)=>a.path.localeCompare(b.path)).map(folder=><option key={folder.id} value={folder.id}>{folder.path.split("/").join(" › ")}</option>)}</Select></label>
      :<label>Name<input autoFocus required maxLength={255} value={value} onChange={event=>setValue(event.target.value)}/>{pane.type==="renameNote"&&<small className="field-hint">Saved as {fileName(value)}</small>}</label>}
    <div className="dialog-actions"><button type="button" className="text-button" onClick={onClose}>Cancel</button><button className="primary-button" disabled={busy||!value.trim()||value===initial}>{busy?"Saving…":pane.type==="folder"?"Create folder":pane.type==="move"?"Move here":"Save"}</button></div>
  </form></EditorPane>;
}

/**
 * Notes from before notes became files are copied once into the Notes folder: one folder per
 * notebook, one Markdown file per note (title as a heading, tags as front matter). The old
 * copies are left untouched.
 */
function LegacyImport({session,library,save,connection,onDone,onError}:{session:BrowserSession;library:NotesLibrary;save:(patch:Partial<Preferences>)=>Promise<unknown>;connection:ReturnType<typeof useLiveConnection>["connection"];onDone:(text:string)=>void;onError:(text:string)=>void}){
  const [progress,setProgress]=useState<string>();
  async function copy(){
    if(!connection)return onError("Notes are reconnecting. Try again in a moment.");
    try{
      const books=(await request<{items:PimCollection[]}>("/api/v1/notes/notebooks")).items.filter(book=>book.role==="owner");let copied=0;
      for(const book of books){
        const folderName=book.name.replace(/[\x00-\x1f/\\]+/g," ").trim().slice(0,200)||"Notebook";
        let folder=library.folders.find(row=>row.parentId===library.rootId&&row.name.toLowerCase()===folderName.toLowerCase())?.id;
        if(!folder){folder=crypto.randomUUID();await connection.reducers.createFolder({id:folder,parentId:library.rootId,name:folderName});}
        const taken=new Set<string>();let cursor="";
        do{
          const page=await request<{items:{id:string}[];nextCursor:string}>(`/api/v1/notes/items?${new URLSearchParams({collection:book.id,cursor})}`);
          for(const row of page.items){
            const item=await request<{content:string;data:{title:string;tags:string[];favorite:boolean}}>(`/api/v1/notes/items/${row.id}`);
            const heading=item.data.title&&!new RegExp(`^#\\s+${item.data.title.replace(/[.*+?^${}()|[\]\\]/g,"\\$&")}\\s*$`,"m").test(item.content.split("\n",1)[0])?`# ${item.data.title}\n\n`:"";
            const front=item.data.tags.length?`---\ntags: [${item.data.tags.map(tag=>JSON.stringify(tag)).join(", ")}]\n---\n`:"";
            const name=unique(fileName(item.data.title||"Untitled"),taken);taken.add(name.toLowerCase());
            const id=await writeTextFile(session,{name,parentId:folder,content:front+heading+item.content,mediaType:"text/markdown"});
            if(item.data.favorite)await connection.reducers.favoriteEntry({id,favorite:true,expectedRevision:1}).catch(()=>undefined);
            setProgress(`Copied ${++copied} of ${library.legacy}…`);
          }
          cursor=page.nextCursor;
        }while(cursor);
      }
      await save({notesImported:true});
      onDone(`Copied ${copied} ${copied===1?"note":"notes"} into Files › ${library.path}.`);
    }catch(cause){setProgress(undefined);onError(message(cause));}
  }
  return <div className="notes-legacy" role="status"><p>{progress??`${library.legacy} ${library.legacy===1?"note is":"notes are"} still in the old notes storage. Copy them into Files › ${library.path} as Markdown files, one folder per notebook.`}</p>
    {!progress&&<><button className="primary-button" onClick={()=>{setProgress("Copying…");void copy();}}>Copy into Files</button><button className="text-button" onClick={()=>{void save({notesImported:true}).then(()=>onDone("The old notes stay available for export in Notes settings."));}}>Not now</button></>}
  </div>;
}
