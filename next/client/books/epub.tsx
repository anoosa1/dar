import {forwardRef,useEffect,useImperativeHandle,useRef,useState} from "react";

export interface ReaderLook {background:string;color:string;link:string;dark:boolean;font:"serif"|"sans"|"book";size:number;lineHeight:number;layout:"pages"|"scroll"}
export interface EpubHandle {next():void;previous():void;goTo(spine:number,fraction?:number,anchor?:string):void}
interface Props {base:string;spine:{href:string;size:number}[];rightToLeft:boolean;look:ReaderLook;start:{spine:number;fraction:number};label:string;
  onPosition:(spine:number,fraction:number)=>void;onLink:(url:string)=>void;onError:(message:string)=>void}
interface Target {spine:number;fraction:number;anchor?:string;html:boolean;load:number}

const families={serif:'"Iowan Old Style","Charter","Literata","Georgia",serif',sans:'system-ui,-apple-system,"Segoe UI","Noto Sans",sans-serif'};
const encodePath=(href:string)=>href.split("/").map(encodeURIComponent).join("/");

/**
 * An EPUB chapter in a same-origin iframe whose document has no script access (its CSP is
 * script-src 'none'). The reader styles the chapter (theme, font, size, spacing) and, in
 * Pages layout, splits it into screen-wide CSS columns that it scrolls one page at a time.
 * Positions are a spine index and a fraction through that chapter.
 */
export const EpubView=forwardRef<EpubHandle,Props>(function EpubView({base,spine,rightToLeft,look,start,label,onPosition,onLink,onError},ref){
  const frame=useRef<HTMLIFrameElement>(null),[target,setTarget]=useState<Target>({...start,html:false,load:0});
  const page=useRef({index:0,count:1,width:1}),current=useRef({spine:start.spine,fraction:start.fraction}),turning=useRef(0);
  const props=useRef({look,onPosition,onLink,onError,rightToLeft});props.current={look,onPosition,onLink,onError,rightToLeft};
  const paged=look.layout==="pages";
  // Only the loaded chapter is touched, never the iframe's initial blank document (it carries the app's own policy).
  const ready=useRef<Document|undefined>(undefined);
  const doc=()=>{try{const value=frame.current?.contentDocument??undefined;return value&&value===ready.current?value:undefined;}catch{return undefined;}};
  const report=()=>{const value=current.current;props.current.onPosition(value.spine,value.fraction);};

  /** Puts the reader's stylesheet into the chapter, replacing an earlier one. */
  function style(document:Document){
    const {look:l}=props.current,frameNode=frame.current!,width=frameNode.clientWidth,height=frameNode.clientHeight;
    // Comfortable lines: a column is at most about 34em wide; wider screens get two columns or wider margins.
    const columns=paged&&width>=1100?2:1,widest=Math.round(l.size*(columns===2?30:36)),padX=Math.max(width<600?20:width<1000?40:56,Math.floor((width/columns-widest)/2)),padY=width<600?24:40,gap=padX*2,column=paged?Math.floor(width/columns-gap):0;
    const font=l.font==="book"?"":`font-family:${families[l.font]}!important;`;
    const css=[
      `html{background:${l.background}!important;color:${l.color}!important;color-scheme:${l.dark?"dark":"light"};font-size:${l.size}px!important;-webkit-text-size-adjust:none;text-size-adjust:none}`,
      `body{background:transparent!important;color:inherit!important;line-height:${l.lineHeight}!important;${font}overflow-wrap:break-word}`,
      l.font==="book"?"":`body :not(pre,code,kbd,samp,pre *,code *){font-family:inherit!important}`,
      `body *{color:inherit!important;background-color:transparent!important;border-color:color-mix(in srgb,currentColor 25%,transparent)!important}`,
      `a,a *{color:${l.link}!important}`,
      `img,svg,video,canvas{max-width:100%!important;height:auto;object-fit:contain;break-inside:avoid}`,
      `::selection{background:color-mix(in srgb,${l.link} 30%,transparent)}`,
      paged?`html{height:100%!important;overflow:hidden!important}body{margin:0!important;box-sizing:border-box!important;height:100vh!important;padding:${padY}px ${padX}px!important;column-width:${column}px!important;column-gap:${gap}px!important;column-fill:auto!important;max-width:none!important}img,svg,video{max-height:${height-padY*2}px!important}`
        :`html{overflow-y:auto!important;overflow-x:hidden!important;height:auto!important}body{margin:0 auto!important;max-width:42em!important;padding:32px ${padX}px 40vh!important;box-sizing:border-box!important}`,
    ].join("\n");
    let node=document.getElementById("dar-reader-style");
    if(!node){node=document.createElementNS("http://www.w3.org/1999/xhtml","style");node.id="dar-reader-style";(document.head??document.documentElement).appendChild(node);}
    node.textContent=css;
  }
  const scroller=(document:Document)=>document.scrollingElement??document.documentElement;
  const rtlFlow=(document:Document)=>getComputedStyle(document.body??document.documentElement).direction==="rtl";
  function measure(document:Document){
    const width=frame.current!.clientWidth,el=scroller(document);
    page.current.width=width;page.current.count=Math.max(1,Math.ceil((el.scrollWidth-4)/width));
  }
  function show(document:Document,index:number){
    const p=page.current;p.index=Math.max(0,Math.min(p.count-1,index));
    scroller(document).scrollLeft=(rtlFlow(document)?-1:1)*p.index*p.width;
    current.current={spine:current.current.spine,fraction:p.count>1?p.index/(p.count-1):0};
  }
  /** Moves to a fraction of the chapter, or to an element by ID. */
  function place(document:Document,fraction:number,anchor?:string){
    const el=scroller(document);
    if(paged){
      measure(document);
      const found=anchor?document.getElementById(anchor):null;
      if(found){const box=found.getBoundingClientRect(),left=Math.abs(box.left+(rtlFlow(document)?-1:1)*el.scrollLeft);show(document,Math.floor(left/page.current.width));}
      else show(document,Math.round(fraction*(page.current.count-1)));
    }else{
      const found=anchor?document.getElementById(anchor):null;
      if(found)found.scrollIntoView({block:"start"});else el.scrollTop=fraction*Math.max(0,el.scrollHeight-el.clientHeight);
      current.current={spine:current.current.spine,fraction:Math.max(0,Math.min(1,el.scrollTop/Math.max(1,el.scrollHeight-el.clientHeight)))};
    }
  }
  function go(spineIndex:number,fraction:number,anchor?:string){
    if(spineIndex<0||spineIndex>=spine.length)return;
    if(spineIndex===current.current.spine&&doc()?.body){const document=doc()!;place(document,fraction,anchor);report();return;}
    current.current={spine:spineIndex,fraction};setTarget({spine:spineIndex,fraction,anchor,html:false,load:Date.now()});
  }
  function step(direction:1|-1){
    const document=doc();if(!document)return;
    if(paged){
      const p=page.current;
      if(p.index+direction>=0&&p.index+direction<p.count){show(document,p.index+direction);report();return;}
    }else{
      const el=scroller(document),before=el.scrollTop;el.scrollTop+=direction*el.clientHeight*0.9;
      if(Math.abs(el.scrollTop-before)>2){current.current={spine:current.current.spine,fraction:Math.max(0,Math.min(1,el.scrollTop/Math.max(1,el.scrollHeight-el.clientHeight)))};report();return;}
    }
    go(current.current.spine+direction,direction>0?0:1);
  }
  useImperativeHandle(ref,()=>({next:()=>step(1),previous:()=>step(-1),goTo:(s,f=0,a)=>go(s,f,a)}));

  // Once a chapter loads: fall back to the HTML parser if the XML is malformed, style it, place it, listen.
  function loaded(){
    let document:Document|undefined;try{document=frame.current?.contentDocument??undefined;}catch{document=undefined;}
    if(!document||document.URL==="about:blank")return;ready.current=document;
    const broken=document.documentElement?.nodeName==="parsererror"||!!document.querySelector("parsererror");
    if(broken&&!target.html){setTarget(t=>({...t,html:true,load:Date.now()}));return;}
    if(!document.body){props.current.onError("This chapter could not be read.");return;}
    style(document);
    const restore=()=>place(document,current.current.fraction);
    place(document,target.fraction,target.anchor);report();
    // Images and fonts change the layout after load; keep the reader in the same place.
    for(const image of document.images)if(!image.complete)image.addEventListener("load",restore,{once:true});
    void document.fonts?.ready.then(restore);
    document.addEventListener("keydown",event=>keys(event));
    document.addEventListener("click",event=>{
      const link=(event.target as Element).closest?.("a[href]");
      if(link){event.preventDefault();props.current.onLink((link as HTMLAnchorElement).href);return;}
      if(!paged||document.getSelection()?.toString())return;
      const x=event.clientX/frame.current!.clientWidth,forward=props.current.rightToLeft?x<0.3:x>0.7,back=props.current.rightToLeft?x>0.7:x<0.3;
      if(forward)step(1);else if(back)step(-1);
    });
    let swipe:{x:number;y:number}|undefined;
    document.addEventListener("touchstart",event=>{const t=event.touches[0];swipe={x:t.clientX,y:t.clientY};},{passive:true});
    document.addEventListener("touchend",event=>{const t=event.changedTouches[0];if(!swipe||!paged)return;const dx=t.clientX-swipe.x,dy=t.clientY-swipe.y;swipe=undefined;if(Math.abs(dx)>50&&Math.abs(dx)>Math.abs(dy)*1.5)step((dx<0)!==props.current.rightToLeft?1:-1);},{passive:true});
    document.addEventListener("wheel",event=>{if(!paged)return;const now=Date.now();if(now-turning.current<350)return;const delta=Math.abs(event.deltaX)>Math.abs(event.deltaY)?event.deltaX:event.deltaY;if(Math.abs(delta)<20)return;turning.current=now;event.preventDefault();step(delta>0?1:-1);},{passive:false});
    if(!paged)document.addEventListener("scroll",()=>{const el=scroller(document);current.current={spine:current.current.spine,fraction:Math.max(0,Math.min(1,el.scrollTop/Math.max(1,el.scrollHeight-el.clientHeight)))};report();},{passive:true});
  }
  function keys(event:KeyboardEvent){
    if(event.defaultPrevented||event.altKey||event.ctrlKey||event.metaKey)return;
    const rtl=props.current.rightToLeft,forward=["PageDown"," "].includes(event.key)&&!event.shiftKey||event.key===(rtl?"ArrowLeft":"ArrowRight")||(paged&&event.key==="ArrowDown"),back=event.key==="PageUp"||(event.key===" "&&event.shiftKey)||event.key===(rtl?"ArrowRight":"ArrowLeft")||(paged&&event.key==="ArrowUp");
    if(forward){event.preventDefault();step(1);}else if(back){event.preventDefault();step(-1);}
    else if(event.key==="Escape"||event.key==="Home"||event.key==="End"){
      // Let the reader's own shortcuts (Escape closes) see keys pressed inside the chapter.
      frame.current?.dispatchEvent(new KeyboardEvent("keydown",{key:event.key,bubbles:true}));
    }
  }
  // Theme, font, size or layout changes restyle in place; resizing re-measures.
  useEffect(()=>{const document=doc();if(!document?.body)return;const fraction=current.current.fraction;style(document);place(document,fraction);},[look.background,look.color,look.font,look.size,look.lineHeight,look.layout]);
  useEffect(()=>{const node=frame.current;if(!node)return;let last=node.clientWidth+"x"+node.clientHeight;const observer=new ResizeObserver(()=>{const size=node.clientWidth+"x"+node.clientHeight;if(size===last)return;last=size;const document=doc();if(document?.body){const fraction=current.current.fraction;style(document);place(document,fraction);}});observer.observe(node);return()=>observer.disconnect();},[]);
  const src=`${base}${encodePath(spine[target.spine]?.href??"")}${target.html?"?as=html":""}`;
  return <iframe key={`${target.spine}:${target.load}`} ref={frame} className={`epub-view${paged?" is-paged":""}`} src={src} title={label} sandbox="allow-same-origin allow-scripts" onLoad={loaded}
    onKeyDown={event=>keys(event.nativeEvent)}/>;
});
