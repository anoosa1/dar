import {useEffect,useRef,useState} from "react";
import type {FileEntries as FileEntry} from "./module_bindings/types.ts";
import {Icon} from "./icons.tsx";
import {ActionMenu,type MenuAction} from "./controls.tsx";
import {FileTile,fileTypeLabel} from "./file-types.tsx";
import {edited} from "./pim/presentation.ts";

/** What the browser can show from Files' inline content route (`?preview=1`). */
export function previewKind(entry:Pick<FileEntry,"kind"|"mediaType"|"size">):"image"|"pdf"|"video"|"audio"|"text"|undefined{
  if(entry.kind!=="file")return undefined;
  const media=entry.mediaType;
  if(/^image\/(png|jpeg|gif|webp|avif)$/.test(media))return "image";
  if(media==="application/pdf")return "pdf";
  if(/^video\/(mp4|webm|ogg)$/.test(media))return "video";
  if(/^audio\/(mpeg|mp4|ogg|wav|webm|flac|aac|x-wav|x-flac)$/.test(media))return "audio";
  if((/^text\//.test(media)||media==="application/json")&&entry.size<=1024*1024)return "text";
  return undefined;
}
const formatSize=(bytes:number)=>bytes<1024?`${bytes} B`:bytes<1024**2?`${(bytes/1024).toFixed(1)} KB`:bytes<1024**3?`${(bytes/1024**2).toFixed(1)} MB`:`${(bytes/1024**3).toFixed(1)} GB`;

/**
 * A file preview in the same right-hand sheet as the text editor: the bar (Close, type,
 * previous/next, Download, file menu) over a stage that fits the content to the sheet.
 * Arrow keys step through the folder's previewable files; Escape closes.
 */
export function FilePreview({entry,siblings,onOpen,onClose,actions=[]}:{entry:FileEntry;siblings:FileEntry[];onOpen:(entry:FileEntry)=>void;onClose:()=>void;actions?:MenuAction[]}){
  const kind=previewKind(entry),content=`/api/v1/files/${entry.id}/content`,source=`${content}?preview=1&v=${entry.revision}`;
  const [text,setText]=useState<string>(),[failed,setFailed]=useState(false),root=useRef<HTMLElement>(null);
  const index=siblings.findIndex(item=>item.id===entry.id),previous=index>0?siblings[index-1]:undefined,next=index>=0?siblings[index+1]:undefined;
  // Focus moves into the preview and returns to the control that opened it.
  useEffect(()=>{const opener=document.activeElement as HTMLElement|null;root.current?.focus({preventScroll:true});return()=>{if(opener?.isConnected&&(!document.activeElement||document.activeElement===document.body||root.current?.contains(document.activeElement)))opener.focus({preventScroll:true});};},[]);
  useEffect(()=>{
    setFailed(false);setText(undefined);if(kind!=="text")return;
    const abort=new AbortController();
    void fetch(source,{signal:abort.signal}).then(async response=>{if(!response.ok)throw Error();setText(await response.text());}).catch(()=>{if(!abort.signal.aborted)setFailed(true);});
    return()=>abort.abort();
  },[entry.id,entry.revision,kind]);
  const unavailable=<div className="empty-state file-preview-empty"><FileTile entry={entry} large/><h2>{failed?"This preview could not load":"No preview for this file type"}</h2><p>{failed?"The file may have changed or is not readable by this browser.":`${fileTypeLabel(entry)} files can’t be shown in the browser. Download the file to open it.`}</p><a className="primary-button" href={content} download><Icon name="download"/>Download</a></div>;
  return <section ref={root} tabIndex={-1} className={`file-text-editor document-editor file-preview file-preview-${kind??"none"}`} aria-label={`Preview of ${entry.name}`}
    onKeyDown={event=>{
      if(event.defaultPrevented||(event.target as HTMLElement).closest("input,textarea,video,audio,[role=menu]"))return;
      if(event.key==="Escape"){event.preventDefault();onClose();}
      else if(event.key==="ArrowLeft"&&previous){event.preventDefault();onOpen(previous);}
      else if(event.key==="ArrowRight"&&next){event.preventDefault();onOpen(next);}
    }}>
    <div className="workspace-editor-top">
      <button className="icon-button" aria-label="Close preview" title="Close (Escape)" onClick={onClose}><Icon name="close"/></button>
      <span className="note-notebook"><FileTile entry={entry}/>{fileTypeLabel(entry)}</span>
      <span className="file-preview-meta">{formatSize(entry.size)} · Edited {edited(entry.modifiedAt)}</span>
      {siblings.length>1&&index>=0&&<div className="file-preview-steps" role="group" aria-label="Browse files"><button className="icon-button" aria-label="Previous file" title="Previous (←)" disabled={!previous} onClick={()=>previous&&onOpen(previous)}><Icon name="back"/></button><span>{index+1} of {siblings.length}</span><button className="icon-button" aria-label="Next file" title="Next (→)" disabled={!next} onClick={()=>next&&onOpen(next)}><Icon name="chevron"/></button></div>}
      <a className="icon-button" href={content} download aria-label="Download" title="Download"><Icon name="download"/></a>
      <ActionMenu label="File options" actions={[{label:"Open in new tab",icon:"external",href:source},...actions]}><Icon name="more"/></ActionMenu>
    </div>
    <h1 className="file-preview-title">{entry.name}</h1>
    <div className="file-preview-stage">{failed||!kind?unavailable
      :kind==="image"?<img key={entry.id} src={source} alt={entry.name} onError={()=>setFailed(true)}/>
      :kind==="pdf"?<iframe key={entry.id} src={source} title={`PDF ${entry.name}`}/>
      :kind==="video"?<video key={entry.id} src={source} controls playsInline preload="metadata" aria-label={entry.name} onError={()=>setFailed(true)}/>
      :kind==="audio"?<div className="file-preview-audio"><FileTile entry={entry} large/><audio key={entry.id} src={source} controls preload="metadata" aria-label={entry.name} onError={()=>setFailed(true)}/></div>
      :text===undefined?<p className="muted" role="status">Loading preview…</p>:<pre className="file-preview-source">{text}</pre>}
    </div>
  </section>;
}
