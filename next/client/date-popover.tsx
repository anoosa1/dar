import {useEffect,useId,useRef,useState} from "react";
import {shiftCalendarDate} from "../shared/calendar-navigation.ts";
import {Icon} from "./icons.tsx";
import {Select} from "./controls.tsx";
import "./date-popover.css";
const add=(date:string,days:number)=>new Date(Date.parse(date+"T12:00:00Z")+days*86400000).toISOString().slice(0,10);
const format=(date:string,options:Intl.DateTimeFormatOptions)=>new Date(date+"T12:00:00Z").toLocaleDateString(undefined,{...options,timeZone:"UTC"});
/** One themed navigation control; dates remain civil dates in the calendar's timezone. */
export function DatePopover({value,today,weekStart,onChange}:{value:string;today:string;weekStart:number;onChange:(date:string)=>void}){
  const id=useId(),panel=useRef<HTMLDivElement>(null),trigger=useRef<HTMLButtonElement>(null),[open,setOpen]=useState(false),[month,setMonth]=useState(value.slice(0,7)+"-01"),[focused,setFocused]=useState(value);
  const year=Number(month.slice(0,4)),monthIndex=Number(month.slice(5,7))-1;
  useEffect(()=>{if(open)panel.current?.querySelector<HTMLButtonElement>(`[data-date="${focused}"]`)?.focus();},[open,focused,month]);
  function close(){panel.current?.hidePopover();trigger.current?.focus();}
  function select(date:string){onChange(date);close();}
  function move(date:string){setFocused(date);setMonth(date.slice(0,7)+"-01");}
  return <><button ref={trigger} type="button" className="text-button calendar-anchor date-popover-trigger" aria-label="Choose date" aria-haspopup="dialog" aria-expanded={open} aria-controls={id} popoverTarget={id} data-date={value}>{value===today?"Today":format(value,{month:"short",day:"numeric",year:value.slice(0,4)!==today.slice(0,4)?"numeric":undefined})}<Icon name="chevron"/></button>
  <div id={id} ref={panel} popover="auto" role="dialog" aria-label="Choose calendar date" className="date-popover" onToggle={event=>{const visible=(event.nativeEvent as ToggleEvent).newState==="open";setOpen(visible);if(visible){setMonth(value.slice(0,7)+"-01");setFocused(value);}}} onKeyDown={event=>{if(event.key==="Escape"){event.preventDefault();event.stopPropagation();close();}}}>
    <div className="date-popover-heading"><button className="icon-button" aria-label="Previous month" onClick={()=>move(shiftCalendarDate(focused,"month",-1))}><Icon name="back"/></button><strong aria-live="polite">{format(month,{month:"long",year:"numeric"})}</strong><button className="icon-button" aria-label="Next month" onClick={()=>move(shiftCalendarDate(focused,"month",1))}><Icon name="chevron"/></button></div>
    <div className="date-popover-selectors"><Select aria-label="Choose month" value={monthIndex} onChange={event=>move(`${month.slice(0,4)}-${String(Number(event.target.value)+1).padStart(2,"0")}-01`)}>{Array.from({length:12},(_,i)=><option key={i} value={i}>{new Date(Date.UTC(2026,i,1)).toLocaleDateString(undefined,{month:"long",timeZone:"UTC"})}</option>)}</Select><Select aria-label="Choose year" value={year} onChange={event=>move(`${event.target.value.padStart(4,"0")}-${month.slice(5,7)}-01`)}>{Array.from({length:201},(_,i)=>Number(value.slice(0,4))-100+i).filter(y=>y>=1&&y<=9999).map(y=><option key={y} value={y}>{y}</option>)}</Select></div>
    <MonthGrid month={month} value={value} today={today} weekStart={weekStart} focused={focused} label="Pick a day" onFocus={setFocused} onMove={move} onSelect={select}/>
    <div className="date-popover-actions"><button className="text-button" onClick={()=>select(today)}>Go to today</button></div>
  </div></>;
}

/** A six-week grid of civil dates with roving focus and arrow/Home/End/Page keys. */
function MonthGrid({month,value,today,weekStart,focused,label,week,onFocus,onMove,onSelect}:{month:string;value:string;today:string;weekStart:number;focused:string;label:string;week?:string[];onFocus:(date:string)=>void;onMove:(date:string)=>void;onSelect:(date:string)=>void}){
  const offset=(new Date(month+"T12:00:00Z").getUTCDay()-weekStart+7)%7,dates=Array.from({length:42},(_,i)=>add(month,i-offset)),weekday=(date:string)=>(new Date(date+"T12:00:00Z").getUTCDay()-weekStart+7)%7;
  return <div className="date-popover-grid" role="grid" aria-label={label} onKeyDown={event=>{
    let next:string|undefined;if(event.key==="ArrowLeft")next=add(focused,-1);if(event.key==="ArrowRight")next=add(focused,1);if(event.key==="ArrowUp")next=add(focused,-7);if(event.key==="ArrowDown")next=add(focused,7);if(event.key==="Home")next=add(focused,-weekday(focused));if(event.key==="End")next=add(focused,6-weekday(focused));if(event.key==="PageUp"||event.key==="PageDown")next=shiftCalendarDate(focused,"month",event.key==="PageUp"?-1:1);if(next){event.preventDefault();onMove(next);}
  }}><div role="row">{dates.slice(0,7).map(date=><span role="columnheader" key={date}>{format(date,{weekday:"narrow"})}</span>)}</div>{Array.from({length:6},(_,row)=><div role="row" key={row}>{dates.slice(row*7,row*7+7).map(date=>{const inWeek=week?.includes(date);return <button type="button" role="gridcell" key={date} data-date={date} aria-label={format(date,{weekday:"long",month:"long",day:"numeric",year:"numeric"})} aria-selected={date===value} aria-current={date===today?"date":undefined} tabIndex={date===focused?0:-1} className={[date.slice(0,7)===month.slice(0,7)?"":"outside-month",inWeek?"in-range":"",inWeek&&date===week![0]?"range-start":"",inWeek&&date===week!.at(-1)?"range-end":""].filter(Boolean).join(" ")||undefined} onClick={()=>onSelect(date)} onFocus={()=>onFocus(date)}>{Number(date.slice(8))}</button>;})}</div>)}</div>;
}
/** The sidebar month: follows the calendar anchor and highlights the visible range. */
export function MiniMonth({value,today,weekStart,range,onChange}:{value:string;today:string;weekStart:number;range?:string[];onChange:(date:string)=>void}){
  const [month,setMonth]=useState(value.slice(0,7)+"-01"),[focused,setFocused]=useState(value),root=useRef<HTMLDivElement>(null);
  useEffect(()=>{setMonth(value.slice(0,7)+"-01");setFocused(value);},[value]);
  function move(date:string){setFocused(date);setMonth(date.slice(0,7)+"-01");requestAnimationFrame(()=>root.current?.querySelector<HTMLButtonElement>(`[data-date="${date}"]`)?.focus());}
  return <div className="mini-month" ref={root}><div className="mini-month-heading"><strong aria-live="polite">{format(month,{month:"long",year:"numeric"})}</strong><button type="button" className="icon-button" aria-label="Previous month in sidebar" onClick={()=>{const next=shiftCalendarDate(month,"month",-1);setMonth(next);setFocused(next);}}><Icon name="back"/></button><button type="button" className="icon-button" aria-label="Next month in sidebar" onClick={()=>{const next=shiftCalendarDate(month,"month",1);setMonth(next);setFocused(next);}}><Icon name="chevron"/></button></div>
    <MonthGrid month={month} value={value} today={today} weekStart={weekStart} focused={focused} label="Month overview" week={range} onFocus={setFocused} onMove={move} onSelect={onChange}/></div>;
}
