import {forwardRef,useEffect,useImperativeHandle,useRef,useState} from "react";
import * as pdfjs from "pdfjs-dist";
import type {PDFDocumentProxy,PDFPageProxy,RenderTask} from "pdfjs-dist";

// Keep in step with the copy made by scripts/build-client.mjs.
const assets="/pdfjs/6.5.438/";
pdfjs.GlobalWorkerOptions.workerSrc=`${assets}pdf.worker.min.mjs`;

export interface PdfOutlineEntry {label:string;page:number;depth:number}
export interface PdfHandle {goTo(page:number,fraction?:number):void;step(direction:1|-1):void;zoom(change:"in"|"out"|"width"|"page"):void}
export type PdfZoom={mode:"width"|"page"}|{mode:"scale";scale:number};
interface Props {url:string;page?:number;fraction?:number;className?:string;label:string;
  onReady?:(info:{pages:number;outline:PdfOutlineEntry[]})=>void;onPosition?:(page:number,fraction:number,pages:number)=>void;onZoom?:(zoom:PdfZoom)=>void;onError?:(message:string)=>void}

/**
 * A PDF drawn with pdf.js: pages are laid out at their real proportions in one scrolling
 * column, and only pages near the viewport are drawn (and kept) on canvases, so long
 * documents stay light. Files' preview and the Books reader share it.
 */
export const PdfView=forwardRef<PdfHandle,Props>(function PdfView({url,page=0,fraction=0,className="",label,onReady,onPosition,onZoom,onError},ref){
  const scroller=useRef<HTMLDivElement>(null),[doc,setDoc]=useState<PDFDocumentProxy>(),[sizes,setSizes]=useState<{w:number;h:number}[]>([]),[width,setWidth]=useState(0),[height,setHeight]=useState(0);
  const [zoom,setZoom]=useState<PdfZoom>({mode:"width"}),[failed,setFailed]=useState(""),start=useRef({page,fraction}),restored=useRef(false);
  const callbacks=useRef({onReady,onPosition,onZoom,onError});callbacks.current={onReady,onPosition,onZoom,onError};
  useEffect(()=>{
    let alive=true;setFailed("");setDoc(undefined);restored.current=false;
    const task=pdfjs.getDocument({url,cMapUrl:`${assets}cmaps/`,cMapPacked:true,standardFontDataUrl:`${assets}standard_fonts/`,wasmUrl:`${assets}wasm/`,iccUrl:`${assets}iccs/`,enableXfa:false,isOffscreenCanvasSupported:true});
    void task.promise.then(async value=>{
      if(!alive)return;
      // Every page's size is read up front (cheap: no drawing), so the column has its final height.
      const first=(await value.getPage(1)).getViewport({scale:1}),list=Array.from({length:value.numPages},()=>({w:first.width,h:first.height}));
      setSizes(list);setDoc(value);
      for(let i=2;i<=value.numPages&&alive;i+=1){const v=(await value.getPage(i)).getViewport({scale:1});if(v.width!==first.width||v.height!==first.height){list[i-1]={w:v.width,h:v.height};if(i%25===0||i===value.numPages)setSizes([...list]);}}
      if(alive)setSizes([...list]);
      const outline:PdfOutlineEntry[]=[];
      const walk=async(items:Awaited<ReturnType<PDFDocumentProxy["getOutline"]>>|null,depth:number)=>{for(const item of items??[]){if(outline.length>=2000)return;
        let target=-1;try{const dest=typeof item.dest==="string"?await value.getDestination(item.dest):item.dest;const first=dest?.[0];target=typeof first==="number"?first:first?await value.getPageIndex(first):-1;}catch{target=-1;}
        if(item.title.trim())outline.push({label:item.title.trim().slice(0,200),page:target,depth:Math.min(depth,6)});await walk(item.items,depth+1);}};
      await walk(await value.getOutline().catch(()=>null),0);
      if(alive)callbacks.current.onReady?.({pages:value.numPages,outline});
    },cause=>{if(!alive)return;const message=(cause as {name?:string}).name==="PasswordException"?"This PDF is protected with a password. Download it to open it in another app.":(cause as {name?:string}).name==="InvalidPDFException"?"This file is not a readable PDF.":"The PDF could not be loaded.";setFailed(message);callbacks.current.onError?.(message);});
    return()=>{alive=false;void task.destroy();};
  },[url]);
  useEffect(()=>{const node=scroller.current;if(!node)return;const observer=new ResizeObserver(()=>{setWidth(node.clientWidth);setHeight(node.clientHeight);});observer.observe(node);return()=>observer.disconnect();},[]);
  const gap=12,pad=16,widest=Math.max(1,...sizes.map(s=>s.w));
  const scale=zoom.mode==="scale"?zoom.scale:zoom.mode==="width"?Math.max(0.2,(width-pad*2)/widest):Math.max(0.2,Math.min((width-pad*2)/widest,(height-pad*2)/Math.max(1,...sizes.map(s=>s.h))));
  const tops:number[]=[];let total=pad;for(const size of sizes){tops.push(total);total+=size.h*scale+gap;}
  const position=()=>{const node=scroller.current;if(!node||!tops.length)return {page:0,fraction:0};const middle=node.scrollTop+Math.min(node.clientHeight/3,200);
    let index=0;while(index+1<tops.length&&tops[index+1]<=middle)index++;const h=sizes[index].h*scale;return {page:index,fraction:Math.max(0,Math.min(1,(node.scrollTop-tops[index])/h))};};
  const goTo=(target:number,frac=0)=>{const node=scroller.current;if(!node||!tops.length)return;const index=Math.max(0,Math.min(tops.length-1,target));node.scrollTop=tops[index]+frac*sizes[index].h*scale-(frac?0:pad/2);};
  // Keep the same place across zoom and resize; restore the saved place once sizes are known.
  const kept=useRef({page:0,fraction:0});
  useEffect(()=>{if(!doc||!width)return;if(!restored.current){restored.current=true;goTo(start.current.page,start.current.fraction);}else goTo(kept.current.page,kept.current.fraction);},[doc,scale,sizes.length]);
  useEffect(()=>{const node=scroller.current;if(!node)return;let frame=0;const on=()=>{cancelAnimationFrame(frame);frame=requestAnimationFrame(()=>{if(!restored.current)return;const at=position();kept.current=at;callbacks.current.onPosition?.(at.page,at.fraction,tops.length);});};node.addEventListener("scroll",on,{passive:true});return()=>{node.removeEventListener("scroll",on);cancelAnimationFrame(frame);};});
  const change=(next:PdfZoom)=>{setZoom(next);callbacks.current.onZoom?.(next);};
  useImperativeHandle(ref,()=>({goTo,step:direction=>{const node=scroller.current;if(node)node.scrollBy({top:direction*node.clientHeight*0.9,behavior:matchMedia("(prefers-reduced-motion: reduce)").matches?"auto":"smooth"});},
    zoom:value=>{if(value==="width"||value==="page")change({mode:value});else change({mode:"scale",scale:Math.max(0.25,Math.min(5,scale*(value==="in"?1.25:0.8)))});}}));
  // Draw pages near the viewport; let distant canvases go.
  const [visible,setVisible]=useState<[number,number]>([0,2]);
  useEffect(()=>{const node=scroller.current;if(!node||!tops.length)return;const update=()=>{const top=node.scrollTop-node.clientHeight,bottom=node.scrollTop+node.clientHeight*2;let a=0;while(a+1<tops.length&&tops[a+1]<top)a++;let b=a;while(b+1<tops.length&&tops[b+1]<bottom)b++;setVisible(v=>v[0]===a&&v[1]===b?v:[a,b]);};update();node.addEventListener("scroll",update,{passive:true});return()=>node.removeEventListener("scroll",update);},[tops.length,scale,height]);
  return <div ref={scroller} className={`pdf-view ${className}`} tabIndex={0} role="document" aria-label={label} aria-busy={!doc&&!failed}>
    {failed?<div className="pdf-view-message" role="alert">{failed}</div>:!doc?<div className="pdf-view-message" role="status">Opening PDF…</div>:
      <div className="pdf-view-pages" ref={node=>{node?.style.setProperty("--pdf-height",`${Math.round(total)}px`);}}>
        {sizes.map((size,index)=>index>=visible[0]&&index<=visible[1]?<PdfPage key={index} doc={doc} index={index} top={tops[index]} width={size.w*scale} height={size.h*scale} scale={scale}/>:null)}
      </div>}
  </div>;
});

/** One page box; its canvas is drawn at the screen's pixel density. */
function PdfPage({doc,index,top,width,height,scale}:{doc:PDFDocumentProxy;index:number;top:number;width:number;height:number;scale:number}){
  const box=useRef<HTMLDivElement>(null),canvas=useRef<HTMLCanvasElement>(null),[drawn,setDrawn]=useState(false);
  useEffect(()=>{const node=box.current;if(!node)return;node.style.setProperty("--pdf-page-top",`${top}px`);node.style.setProperty("--pdf-page-width",`${width}px`);node.style.setProperty("--pdf-page-height",`${height}px`);},[top,width,height]);
  useEffect(()=>{
    let task:RenderTask|undefined,alive=true,page:PDFPageProxy|undefined;
    const timer=setTimeout(()=>void doc.getPage(index+1).then(value=>{
      if(!alive||!canvas.current)return;page=value;
      const ratio=Math.min(window.devicePixelRatio||1,2),viewport=value.getViewport({scale:scale*ratio}),target=canvas.current;
      target.width=Math.floor(viewport.width);target.height=Math.floor(viewport.height);
      task=value.render({canvas:target,viewport});
      return task.promise.then(()=>{if(alive)setDrawn(true);});
    }).catch(()=>{}),60);
    return()=>{alive=false;clearTimeout(timer);task?.cancel();page?.cleanup();};
  },[doc,index,scale]);
  return <div ref={box} className={`pdf-view-page${drawn?" is-drawn":""}`} aria-label={`Page ${index+1}`} role="img"><canvas ref={canvas}/></div>;
}
