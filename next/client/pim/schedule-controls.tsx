import {useEffect,useRef,useState} from "react";
import {recurrenceLabel} from "./presentation.ts";
import {Select} from "../controls.tsx";

// Keep FormData and recurrence-slot updates synchronized with themed controls.
function useField(initial:string,value?:string,onChange?:(value:string)=>void){
  const [local,setLocal]=useState(initial),ref=useRef<HTMLInputElement>(null),current=value??local;
  function change(next:string){setLocal(next);onChange?.(next);}
  useEffect(()=>{const field=ref.current;if(!field)return;const updated=()=>change(field.value);field.addEventListener("change",updated);return()=>field.removeEventListener("change",updated);},[onChange]);
  return {current,change,ref};
}
const repeats=["DAILY","WEEKLY","MONTHLY","YEARLY"];
export function RepeatField({defaultValue="",value,onChange}:{defaultValue?:string;value?:string;onChange?:(value:string)=>void}){
  const field=useField(defaultValue,value,onChange),[custom,setCustom]=useState(false),simple=field.current===""||repeats.some(freq=>field.current===`FREQ=${freq}`),mode=custom||!simple?"custom":field.current.replace("FREQ=","").toLowerCase();
  return <div><input ref={field.ref} type="hidden" name="recurrence" value={field.current}/><label>Repeats<Select aria-label="Repeats" value={mode} onChange={event=>{const next=event.target.value;setCustom(next==="custom");if(next!=="custom")field.change(next?`FREQ=${next.toUpperCase()}`:"");else if(!field.current)field.change("FREQ=WEEKLY;COUNT=10");}}><option value="">Does not repeat</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="monthly">Monthly</option><option value="yearly">Yearly</option><option value="custom">Custom schedule…</option></Select></label>{field.current&&<p className="field-hint">{recurrenceLabel(field.current)}</p>}{mode==="custom"&&<details open={custom} onToggle={event=>setCustom(event.currentTarget.open)}><summary>Custom recurrence</summary><label>Recurrence rule<input aria-label="Recurrence rule" value={field.current} maxLength={2048} onChange={event=>field.change(event.target.value)}/><small>For advanced schedules, for example FREQ=WEEKLY;BYDAY=MO,WE;COUNT=12.</small></label></details>}</div>;
}
export function PriorityField({defaultValue=0}:{defaultValue?:number}){
  const field=useField(String(defaultValue)),priority=Number(field.current),group=priority===0?"0":priority<5?"1":priority===5?"5":"9";
  return <div><input ref={field.ref} type="hidden" name="priority" value={field.current}/><label>Priority<Select aria-label="Priority" value={group} onChange={event=>field.change(event.target.value)}><option value="0">None</option><option value="1">High</option><option value="5">Normal</option><option value="9">Low</option></Select></label><details><summary>Exact priority</summary><label>Priority number<input type="number" min={0} max={9} value={field.current} onChange={event=>field.change(event.target.value)}/><small>0 means none; 1–4 high, 5 normal, 6–9 low. Imported priorities are preserved.</small></label></details></div>;
}
const reminders=[{value:"",label:"No reminder"},{value:"0",label:"At start"},{value:"5",label:"5 minutes before"},{value:"10",label:"10 minutes before"},{value:"15",label:"15 minutes before"},{value:"30",label:"30 minutes before"},{value:"60",label:"1 hour before"},{value:"1440",label:"1 day before"},{value:"10080",label:"1 week before"}];
export function ReminderField({defaultValue=null,value,onChange}:{defaultValue?:number|null;value?:number|null;onChange?:(value:number|null)=>void}){
  const field=useField(defaultValue===null?"":String(defaultValue),value===undefined?undefined:value===null?"":String(value),onChange?next=>onChange(next===""?null:Number(next)):undefined),[custom,setCustom]=useState(false),mode=custom||!reminders.some(option=>option.value===field.current)?"custom":field.current;
  return <div><input ref={field.ref} type="hidden" name="reminderMinutes" value={field.current}/><label>Reminder<Select aria-label="Reminder" value={mode} onChange={event=>{setCustom(event.target.value==="custom");if(event.target.value!=="custom")field.change(event.target.value);else if(field.current==="")field.change("20");}}>{reminders.map(option=><option key={option.value} value={option.value}>{option.label}</option>)}<option value="custom">Custom reminder…</option></Select></label>{mode==="custom"&&<label>Minutes before<input type="number" min={0} max={10080} value={field.current} onChange={event=>field.change(event.target.value)}/></label>}</div>;
}
