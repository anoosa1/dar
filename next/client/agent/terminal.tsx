import {useEffect,useLayoutEffect,useReducer,useRef,useState,type KeyboardEvent} from "react";
import {request,type BrowserSession} from "../api.ts";
import {Icon} from "../icons.tsx";

type Cell={char:string;style:string};
/**
 * Minimal terminal screen: printable text, CR/LF/BS/TAB, cursor movement, erase line/screen
 * and SGR bold/colour. Other escape sequences are consumed without effect, so full-screen
 * programs degrade to readable text rather than garbage.
 */
export class Screen{
  lines:Cell[][]=[[]];row=0;col=0;style="";rows=24;private pending="";
  private line(){while(this.lines.length<=this.row)this.lines.push([]);return this.lines[this.row];}
  private top(){return Math.max(0,this.lines.length-this.rows);}
  write(text:string){
    let data=this.pending+text;this.pending="";
    for(let i=0;i<data.length;i++){
      const char=data[i];
      if(char==="\x1b"){
        const rest=data.slice(i);
        const csi=/^\x1b\[([0-9;?]*)([ -\/]*)([@-~])/.exec(rest),osc=/^\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/.exec(rest);
        if(csi){this.control(csi[1],csi[3]);i+=csi[0].length-1;continue;}
        if(osc){i+=osc[0].length-1;continue;}
        if(/^\x1b(?:\[[0-9;?]*[ -\/]*|\][^\x07]*)?$/.test(rest)){this.pending=rest;break;}
        i+=1;continue;
      }
      if(char==="\r")this.col=0;
      else if(char==="\n"){this.row++;this.line();}
      else if(char==="\b")this.col=Math.max(0,this.col-1);
      else if(char==="\t")this.col=(Math.floor(this.col/8)+1)*8;
      else if(char==="\x07"||char<" ")continue;
      else{const line=this.line();while(line.length<this.col)line.push({char:" ",style:""});line[this.col]={char,style:this.style};this.col++;}
    }
    if(this.lines.length>2000){const drop=this.lines.length-2000;this.lines.splice(0,drop);this.row=Math.max(0,this.row-drop);}
  }
  private control(raw:string,final:string){
    const values=raw.replace("?","").split(";").map(value=>Number(value)||0),n=Math.max(1,values[0]);
    if(final==="m"){for(const code of raw?values:[0]){if(code===0)this.style="";else if(code===1)this.style=(this.style.replace(/\bb\b/,"")+" b").trim();else if(code>=30&&code<=37||code>=90&&code<=97)this.style=(this.style.replace(/\bc\d+\b/,"")+` c${code>=90?code-82:code-30}`).trim();else if(code===39)this.style=this.style.replace(/\bc\d+\b/,"").trim();}}
    else if(final==="K"){const line=this.line();if(values[0]===2)line.length=0;else if(values[0]===1)for(let i=0;i<=this.col&&i<line.length;i++)line[i]={char:" ",style:""};else line.length=Math.min(line.length,this.col);}
    else if(final==="J"){if(values[0]===2||values[0]===3){this.lines=[[]];this.row=0;this.col=0;}else{this.line().length=Math.min(this.line().length,this.col);this.lines.length=this.row+1;}}
    else if(final==="A")this.row=Math.max(this.top(),this.row-n);
    else if(final==="B"){this.row+=n;this.line();}
    else if(final==="C")this.col+=n;
    else if(final==="D")this.col=Math.max(0,this.col-n);
    else if(final==="G")this.col=n-1;
    else if(final==="H"||final==="f"){this.row=this.top()+Math.max(1,values[0])-1;this.col=Math.max(1,values[1]??1)-1;this.line();}
  }
}
const keyMap:Record<string,string>={Enter:"\r",Backspace:"\x7f",Tab:"\t",Escape:"\x1b",ArrowUp:"\x1b[A",ArrowDown:"\x1b[B",ArrowRight:"\x1b[C",ArrowLeft:"\x1b[D",Home:"\x1b[H",End:"\x1b[F",Delete:"\x1b[3~",PageUp:"\x1b[5~",PageDown:"\x1b[6~"};

/** Interactive shell for a thread's working copy, sandboxed on the server like agent commands. */
export function ThreadTerminal({threadId,session,visible}:{threadId:string;session:BrowserSession;visible:boolean}){
  const screen=useRef(new Screen()),[,redraw]=useReducer((n:number)=>n+1,0),[state,setState]=useState<"idle"|"opening"|"open"|"closed">("idle"),[error,setError]=useState("");
  const view=useRef<HTMLDivElement>(null),measure=useRef<HTMLSpanElement>(null),offset=useRef(0),loop=useRef<AbortController|undefined>(undefined),queue=useRef(""),flushing=useRef(false),decoder=useRef(new TextDecoder());
  const url=`/api/v1/agent/threads/${threadId}/terminal`;
  function size(){const box=view.current?.getBoundingClientRect(),cell=measure.current?.getBoundingClientRect();return {rows:Math.max(4,Math.floor((box?.height??300)/((cell?.height||17)))-1),cols:Math.max(20,Math.floor(((box?.width??600)-16)/((cell?.width||7.2))))};}
  async function read(controller:AbortController){
    while(!controller.signal.aborted){
      try{
        const value=await request<{open:boolean;start:number;end:number;data:string;exited:boolean;exitCode?:number}>(`${url}?after=${offset.current}`,{signal:controller.signal});
        if(controller.signal.aborted)return;
        if(value.data){screen.current.write(decoder.current.decode(Uint8Array.from(atob(value.data),char=>char.charCodeAt(0)),{stream:true}));redraw();}
        offset.current=value.end;
        if(!value.open||value.exited){setState("closed");return;}
      }catch(e){if(controller.signal.aborted)return;setError(e instanceof Error?e.message:"The terminal connection failed.");setState("closed");return;}
    }
  }
  async function open(){
    setState("opening");setError("");screen.current=new Screen();screen.current.rows=size().rows;offset.current=0;decoder.current=new TextDecoder();redraw();
    try{const value=await request<{start:number;end:number}>(url,{method:"POST",body:JSON.stringify(size())},session.csrfToken);offset.current=value.start;setState("open");loop.current?.abort();const controller=new AbortController();loop.current=controller;void read(controller);requestAnimationFrame(()=>view.current?.focus());}
    catch(e){setError(e instanceof Error?e.message:"The terminal could not be opened.");setState("idle");}
  }
  async function flush(){
    if(flushing.current||!queue.current)return;flushing.current=true;
    try{while(queue.current){const data=queue.current;queue.current="";await request(`${url}/input`,{method:"POST",body:JSON.stringify({data})},session.csrfToken);}}
    catch(e){setError(e instanceof Error?e.message:"Input could not be sent.");}finally{flushing.current=false;}
  }
  const send=(data:string)=>{queue.current+=data;void flush();};
  function keys(event:KeyboardEvent<HTMLDivElement>){
    if(state!=="open"||event.metaKey||event.altKey&&event.key.length>1)return;
    let data:string|undefined;
    if(event.ctrlKey&&event.key.length===1&&/[a-z@\[\\\]^_]/i.test(event.key)){if(event.key.toLowerCase()==="c"&&window.getSelection()?.toString())return;data=String.fromCharCode(event.key.toUpperCase().charCodeAt(0)&31);}
    else if(event.ctrlKey&&event.shiftKey)return;
    else data=keyMap[event.key]??(event.key.length===1?event.key:undefined);
    if(data===undefined)return;event.preventDefault();send(data);
  }
  useEffect(()=>()=>loop.current?.abort(),[]);
  useEffect(()=>{loop.current?.abort();setState("idle");setError("");screen.current=new Screen();redraw();},[threadId]);
  useLayoutEffect(()=>{if(view.current)view.current.scrollTop=view.current.scrollHeight;});
  useEffect(()=>{
    if(state!=="open"||!view.current)return;let timer:ReturnType<typeof setTimeout>;
    const observer=new ResizeObserver(()=>{clearTimeout(timer);timer=setTimeout(()=>{const next=size();screen.current.rows=next.rows;void request(`${url}/resize`,{method:"POST",body:JSON.stringify(next)},session.csrfToken).catch(()=>{});},200);});
    observer.observe(view.current);return()=>{observer.disconnect();clearTimeout(timer);};
  },[state,visible]);
  const lines=screen.current.lines,cursor={row:screen.current.row,col:screen.current.col};
  return <div className="agent-shell-terminal">
    <span ref={measure} className="agent-shell-measure" aria-hidden="true">M</span>
    {state==="idle"||state==="closed"&&!lines.some(line=>line.length)?<div className="agent-shell-start"><p>{state==="closed"?"The terminal session ended.":"A shell in this thread’s working copy. It runs in the same sandbox as Codex: no network, workspace writes only."}</p>{error&&<p role="alert" className="error">{error}</p>}<button className="secondary-button" onClick={()=>{void open();}}><Icon name="terminal"/>{state==="closed"?"Start a new terminal":"Open terminal"}</button></div>:<>
      <div ref={view} className="agent-shell-screen" role="log" aria-label="Terminal" aria-live="off" tabIndex={0} onKeyDown={keys} onPaste={event=>{if(state==="open"){event.preventDefault();send(event.clipboardData.getData("text").replace(/\r?\n/g,"\r"));}}}>
        {lines.map((line,row)=>{
          const cursorHere=row===cursor.row&&state==="open",cells=cursorHere?[...line,...Array.from({length:Math.max(0,cursor.col+1-line.length)},()=>({char:" ",style:""}))]:line;
          const parts:{style:string;text:string;cursor?:boolean}[]=[];
          cells.forEach((cell,col)=>{const isCursor=cursorHere&&col===cursor.col,last=parts.at(-1);if(!isCursor&&last&&!last.cursor&&last.style===cell.style)last.text+=cell.char;else parts.push({style:cell.style,text:cell.char,cursor:isCursor});});
          return <div key={row} className="agent-shell-line">{parts.map((part,index)=><span key={index} className={[...part.style?part.style.split(" ").map(name=>`t-${name}`):[],...part.cursor?["agent-shell-cursor"]:[]].join(" ")||undefined}>{part.text}</span>)}{"\u200b"}</div>;})}
      </div>
      <div className="agent-shell-status"><span>{state==="opening"?"Starting…":state==="open"?"Sandboxed shell · no network":"Session ended"}</span>{error&&<span role="alert" className="error">{error}</span>}{state==="open"?<button className="text-button" onClick={()=>{loop.current?.abort();void request(url,{method:"DELETE"},session.csrfToken).finally(()=>setState("closed"));}}>End session</button>:state==="closed"&&<button className="text-button" onClick={()=>{void open();}}>Restart</button>}</div>
    </>}
  </div>;
}
