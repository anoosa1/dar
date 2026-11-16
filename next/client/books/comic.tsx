import {forwardRef,useEffect,useImperativeHandle,useRef,useState} from "react";

export interface ComicHandle {next():void;previous():void;goTo(page:number):void}
interface Props {base:string;pages:number;start:number;rightToLeft:boolean;label:string;onPage:(page:number)=>void}

/**
 * Comic pages one at a time, fitted to the sheet. Taps on the outer thirds and arrow keys
 * turn pages in the reading direction (right-to-left for manga); the next pages preload.
 */
export const ComicView=forwardRef<ComicHandle,Props>(function ComicView({base,pages,start,rightToLeft,label,onPage},ref){
  const [index,setIndex]=useState(Math.max(0,Math.min(pages-1,start))),[failed,setFailed]=useState(false),[hint,setHint]=useState(()=>!localStorage.getItem("dar:books:comic-hint"));
  const report=useRef(onPage);report.current=onPage;
  const go=(value:number)=>setIndex(Math.max(0,Math.min(pages-1,value)));
  useImperativeHandle(ref,()=>({next:()=>go(index+1),previous:()=>go(index-1),goTo:go}));
  useEffect(()=>{setFailed(false);report.current(index);
    // Preload the next two pages so turning feels instant.
    for(const n of [index+1,index+2])if(n<pages){const image=new Image();image.src=`${base}${n}`;}
  },[index]);
  useEffect(()=>{if(!hint)return;const timer=setTimeout(()=>{setHint(false);localStorage.setItem("dar:books:comic-hint","1");},3500);return()=>clearTimeout(timer);},[hint]);
  function click(event:React.MouseEvent<HTMLDivElement>){
    const box=event.currentTarget.getBoundingClientRect(),x=(event.clientX-box.left)/box.width;
    if(x<0.33)go(rightToLeft?index+1:index-1);else if(x>0.67)go(rightToLeft?index-1:index+1);
  }
  return <div className="comic-view" role="group" aria-label={label} onClick={click}>
    {failed?<p className="pdf-view-message" role="alert">Page {index+1} could not be shown.</p>
      :<img key={index} src={`${base}${index}`} alt={`Page ${index+1} of ${pages}`} onError={()=>setFailed(true)} draggable={false}/>}
    {hint&&<div className="comic-zones" aria-hidden="true"><span>{rightToLeft?"Next page":"Previous page"}</span><span>Tap the sides to turn pages</span><span>{rightToLeft?"Previous page":"Next page"}</span></div>}
  </div>;
});
