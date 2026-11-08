import type {TaskData} from "../../shared/workspace.ts";

export const taskStatus:Record<TaskData["status"],string>={"NEEDS-ACTION":"To do","IN-PROCESS":"In progress",COMPLETED:"Completed",CANCELLED:"Cancelled"};
export function taskPriority(priority:number){return priority===0?"None":priority<5?"High":priority===5?"Normal":"Low";}
/** Only actionable, explicitly supported contact schemes become links. */
export function contactHref(kind:string,value:string){
  value=value.trim();
  if(kind==="Email")return /^[^\s<>@]+@[^\s<>@]+$/.test(value)?`mailto:${encodeURIComponent(value).replace("%40","@")}`:undefined;
  if(kind==="Phone")return /^[+\d().\s-]+$/.test(value)&&/\d/.test(value)?`tel:${value.replace(/[().\s-]/g,"")}`:undefined;
  if(kind!=="Website")return undefined;
  try{const url=new URL(/^[a-z][a-z\d+.-]*:/i.test(value)?value:`https://${value}`);return ["https:","http:"].includes(url.protocol)&&!url.username&&!url.password?url.href:undefined;}catch{return undefined;}
}
/** Format stored wall dates without moving date-only values into another day. */
export function taskDate(value:string,allDay:boolean){
  if(!value)return "No due date";
  const date=new Date((value.length===10?value+"T12:00:00":value.replace(/Z$/, ""))+"Z");
  if(!Number.isFinite(+date))return value;
  return new Intl.DateTimeFormat(undefined,{year:"numeric",month:"short",day:"numeric",...allDay?{}:{hour:"numeric",minute:"2-digit"},timeZone:"UTC"}).format(date);
}
export function recurrenceLabel(rule:string,recurring=false){
  if(!rule)return recurring?"Custom schedule":"Does not repeat";
  const parts=Object.fromEntries(rule.toUpperCase().split(";").map(part=>part.split("="))),units:Record<string,string>={DAILY:"day",WEEKLY:"week",MONTHLY:"month",YEARLY:"year",HOURLY:"hour"};
  if(!units[parts.FREQ]||Object.keys(parts).some(key=>!["FREQ","INTERVAL","COUNT","UNTIL"].includes(key)))return "Custom schedule";
  const interval=Number(parts.INTERVAL??1),base=interval===1?`Every ${units[parts.FREQ]}`:`Every ${interval} ${units[parts.FREQ]}s`;
  const until=/^(\d{4})(\d\d)(\d\d)(?:T\d{6}Z?)?$/.exec(parts.UNTIL??"");
  if(parts.UNTIL&&!until)return "Custom schedule";
  return base+(parts.COUNT?` · ${parts.COUNT} occurrences`:until?` · Until ${taskDate(`${until[1]}-${until[2]}-${until[3]}`,true)}`:"");
}
