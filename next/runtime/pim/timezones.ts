import ICAL from "ical.js";
import { parsed, escapeText } from "./standards.ts";

const cache = new Map<string, string>();
const weekdays = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
function stamp(epoch: number) { return new Date(epoch).toISOString().slice(0,19).replace(/[-:]/g, ""); }
function offsetText(seconds: number) {
  const n = Math.abs(seconds); return `${seconds < 0 ? "-" : "+"}${String(Math.floor(n/3600)).padStart(2,"0")}${String(Math.floor(n%3600/60)).padStart(2,"0")}${n%60 ? String(n%60).padStart(2,"0") : ""}`;
}
interface Transition { at: number; from: number; to: number }
function pattern(t: Transition) {
  const d = new Date(t.at + t.from * 1000), day=d.getUTCDate(), month=d.getUTCMonth()+1, last=new Date(Date.UTC(d.getUTCFullYear(),month,0)).getUTCDate();
  return `${month}|${day+7>last?-1:Math.ceil(day/7)}${weekdays[d.getUTCDay()]}|${stamp(+d).slice(9)}`;
}

/** Export a self-contained VTIMEZONE from the runtime's IANA database. Explicit
 * transitions cover two centuries; stable annual tails remain recurring. No
 * network timezone service or machine-local TZ environment is involved. */
export function timezoneComponent(zone: string, year: number): ICAL.Component {
  const key=`${zone}/${year}`, cached=cache.get(key);
  if(cached)return parsed(cached,"vtimezone");
  const formatter=new Intl.DateTimeFormat("en-CA",{timeZone:zone,year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hourCycle:"h23"});
  const offset=(seconds:number)=>{
    const p=new Map(formatter.formatToParts(seconds*1000).map(v=>[v.type,v.value]));
    return Date.parse(`${p.get("year")}-${p.get("month")}-${p.get("day")}T${p.get("hour")}:${p.get("minute")}:${p.get("second")}Z`)/1000-seconds;
  };
  const start=Date.parse(`${String(Math.max(1,year-1)).padStart(4,"0")}-01-01T00:00:00Z`)/1000,end=Date.parse(`${String(Math.min(9999,year+201)).padStart(4,"0")}-01-01T00:00:00Z`)/1000;
  let previous=start, previousOffset=offset(start);
  const initial=previousOffset, transitions:Transition[]=[];
  for(let probe=start+86400;probe<=end;probe+=86400){
    const current=offset(probe);
    if(current!==previousOffset){let lo=previous,hi=probe;while(hi-lo>1){const mid=Math.floor((lo+hi)/2);if(offset(mid)===previousOffset)lo=mid;else hi=mid;}transitions.push({at:hi*1000,from:previousOffset,to:current});}
    previous=probe;previousOffset=current;
  }
  const groups=new Map<string,Transition[]>();
  for(const t of transitions){const k=`${t.from}/${t.to}`,values=groups.get(k)??[];values.push(t);groups.set(k,values);}
  const lines=["BEGIN:VTIMEZONE",`TZID:${escapeText(zone)}`,"X-DAR-GENERATED:1",`BEGIN:STANDARD`,`DTSTART:${stamp(start*1000)}`,`TZOFFSETFROM:${offsetText(initial)}`,`TZOFFSETTO:${offsetText(initial)}`,"END:STANDARD"];
  for(const values of groups.values()){
    const first=values[0],kind=first.to>first.from?"DAYLIGHT":"STANDARD";
    lines.push(`BEGIN:${kind}`,`DTSTART:${stamp(first.at+first.from*1000)}`,`TZOFFSETFROM:${offsetText(first.from)}`,`TZOFFSETTO:${offsetText(first.to)}`);
    if(values.length>1)lines.push(`RDATE:${values.slice(1).map(t=>stamp(t.at+t.from*1000)).join(",")}`);
    lines.push(`END:${kind}`);
    const tail=values.slice(-5),last=values.at(-1)!;
    if(tail.length===5&&tail.every((t,i)=>pattern(t)===pattern(last)&&(!i||new Date(t.at).getUTCFullYear()===new Date(tail[i-1].at).getUTCFullYear()+1))&&new Date(last.at).getUTCFullYear()>=year+199){
      const [month,day]=pattern(last).split("|");
      lines.push(`BEGIN:${kind}`,`DTSTART:${stamp(last.at+last.from*1000)}`,`TZOFFSETFROM:${offsetText(last.from)}`,`TZOFFSETTO:${offsetText(last.to)}`,`RRULE:FREQ=YEARLY;BYMONTH=${month};BYDAY=${day}`,`END:${kind}`);
    }
  }
  lines.push("END:VTIMEZONE");const content=lines.join("\r\n")+"\r\n";
  if(cache.size<32)cache.set(key,content);
  return parsed(content,"vtimezone");
}
