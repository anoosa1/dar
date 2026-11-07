import ICAL from "ical.js";
import {randomUUID} from "node:crypto";
import type {TaskData} from "../../shared/workspace.ts";
import type {PimResource} from "../../shared/pim.ts";
import {field,invalid,parsed,serialize} from "./standards.ts";
import {expand,timezone,wallEpoch,wall} from "./calendar.ts";
import {timezoneComponent} from "./timezones.ts";
const val=(c:ICAL.Component,k:string)=>String(c.getFirstPropertyValue(k)??"");
const time=(c:ICAL.Component,k:string)=>c.getFirstPropertyValue(k) as ICAL.Time|null;
function effectiveDue(c:ICAL.Component){const due=time(c,"due"),start=time(c,"dtstart"),duration=c.getFirstPropertyValue("duration");if(due)return due;if(start&&duration instanceof ICAL.Duration){const result=start.clone();result.addDuration(duration);return result;}return null;}
function data(c:ICAL.Component):TaskData{const start=time(c,"dtstart"),due=effectiveDue(c),anchor=start??due,property=c.getFirstProperty(start?"dtstart":"due"),alarm=c.getAllSubcomponents("valarm").find(a=>val(a,"action")==="DISPLAY"),trigger=alarm?.getFirstPropertyValue("trigger");return {kind:"task",title:val(c,"summary"),description:val(c,"description"),start:start?.toString().replace(/Z$/,"")??"",due:due?.toString().replace(/Z$/,"")??"",timezone:anchor?.isDate?"floating":String(property?.getParameter("tzid")??(anchor?.zone.tzid==="UTC"?"UTC":"floating")),allDay:anchor?.isDate??false,priority:Number(c.getFirstPropertyValue("priority")??0),status:(val(c,"status")||"NEEDS-ACTION") as TaskData["status"],progress:Number(c.getFirstPropertyValue("percent-complete")??0),completedAt:time(c,"completed")?.toJSDate().toISOString()??"",categories:c.getAllProperties("categories").flatMap(p=>p.getValues().map(String)),parentUid:c.getAllProperties("related-to").filter(p=>!p.getParameter("reltype")||p.getParameter("reltype")==="PARENT").map(p=>String(p.getFirstValue()))[0]??"",recurrence:val(c,"rrule"),recurring:c.hasProperty("rrule")||c.hasProperty("rdate"),reminderMinutes:trigger instanceof ICAL.Duration?-trigger.toSeconds()/60:null};}
export function isTask(content:string){return /(?:^|\n)BEGIN:VTODO(?:\r?\n|$)/i.test(content);}
export function readTask(content:string){
  const root=parsed(content,"vcalendar"),todos=root.getAllSubcomponents("vtodo"),masters=todos.filter(c=>!c.hasProperty("recurrence-id"));
  if(val(root,"version")!=="2.0"||masters.length!==1||root.getAllSubcomponents().some(c=>!["vtodo","vtimezone"].includes(c.name)))invalid("Expected one VTODO series and its timezone definitions.");
  const master=masters[0],uid=val(master,"uid"),seen=new Set<string>();
  if(!uid||uid.length>1024)invalid("A task needs a stable UID.");
  for(const c of todos){
    const p=data(c),start=time(c,"dtstart"),due=time(c,"due"),duration=c.getFirstPropertyValue("duration") as ICAL.Duration|null;
    if(val(c,"uid")!==uid)invalid("Task components must share a UID.");
    if(!["NEEDS-ACTION","IN-PROCESS","COMPLETED","CANCELLED"].includes(p.status)||!Number.isInteger(p.progress)||p.progress<0||p.progress>100||!Number.isInteger(p.priority)||p.priority<0||p.priority>9)invalid("Invalid task status, progress or priority.");
    if(p.recurring&&!p.start)invalid("Recurring tasks need a start date.");
    if(duration&&(!start||due||duration.toSeconds()<=0||start.isDate&&(duration.hours||duration.minutes||duration.seconds)))invalid("A positive duration requires a start and cannot accompany DUE.");
    for(const key of ["dtstart","due","recurrence-id","completed","created"]){
      const t=time(c,key);if(!t)continue;
      wallEpoch(t.toString().replace(/Z$/,""),"UTC");
      const zone=String(c.getFirstProperty(key)?.getParameter("tzid")??"");
      if(zone&&!root.getAllSubcomponents("vtimezone").some(z=>val(z,"tzid")===zone))timezone(zone);
    }
    if(start&&due&&(start.isDate!==due.isDate||due.compare(start)<0))invalid("The due date must not precede the start.");
    if(c.hasProperty("recurrence-id")){const id=val(c,"recurrence-id");if(seen.has(id))invalid("Duplicate task exception.");seen.add(id);}
    if(p.parentUid===uid)invalid("A task cannot be its own parent.");
    if(p.recurrence)try{ICAL.Recur.fromString(p.recurrence);}catch{invalid("Invalid recurrence rule.");}
  }
  return {uid,content:serialize(root),data:data(master),root,master};
}
function setTime(c:ICAL.Component,key:string,value:string,allDay:boolean,zone:string){
  c.removeAllProperties(key);if(!value)return;value=value.replace(/Z$/,"");if(allDay?!/^\d{4}-\d\d-\d\d$/.test(value):value.length===10)invalid("Use a date-only value or date/time matching the task type.");
  let iana=true;try{timezone(zone);}catch{iana=false;}
  // Embedded custom zones remain valid imports; native IANA edits reject gaps.
  wallEpoch(value,allDay||zone==="floating"||!iana?"UTC":zone,true);
  const t=allDay?ICAL.Time.fromDateString(value):ICAL.Time.fromDateTimeString(value.length===16?value+":00":value);
  if(!allDay&&zone==="UTC")t.zone=ICAL.Timezone.utcTimezone;
  const p=new ICAL.Property(key);p.setValue(t);if(!allDay&&zone!=="UTC"&&zone!=="floating")p.setParameter("tzid",zone);c.addProperty(p);
}
/** Edit the source component in place so unknown properties and exceptions survive. */
export function writeTask(input:unknown,previous?:string,occurrenceId?:string){if(!input||typeof input!=="object"||Array.isArray(input))invalid("Invalid task.");const p=input as Record<string,unknown>,old=previous?readTask(previous):undefined,root=old?.root??new ICAL.Component("vcalendar");if(!old){root.updatePropertyWithValue("version","2.0");root.updatePropertyWithValue("prodid","-//Dar//Tasks//EN");root.addSubcomponent(new ICAL.Component("vtodo"));}let c=old?.master??root.getFirstSubcomponent("vtodo")!;
  if(occurrenceId){if(!old?.data.recurring)invalid("Not a recurring task.");c=root.getAllSubcomponents("vtodo").find(t=>val(t,"recurrence-id").replace(/Z$/,"")===occurrenceId.replace(/Z$/,""))??new ICAL.Component(ICAL.parse(serialize(old.master)));if(c===old.master)invalid("Invalid occurrence.");if(!root.getAllSubcomponents().includes(c)){c.removeAllProperties("rrule");c.removeAllProperties("rdate");c.removeAllProperties("exdate");const stamp=occurrenceId.replace(/Z$/,"");
      const slot=taskOccurrences({id:"slot",collectionId:"",href:"",uid:old.uid,content:previous!,projection:"{}",revision:0,modifiedAt:0},"UTC",new Date(wallEpoch(stamp,"UTC")-2*86400000),new Date(wallEpoch(stamp,"UTC")+2*86400000)).find(o=>o.recurrenceId.replace(/Z$/,"")===stamp);
      if(!slot)invalid("That occurrence is not part of this series.");
      root.addSubcomponent(c);setTime(c,"recurrence-id",occurrenceId,old.data.allDay,old.data.timezone);
      c.removeAllProperties("duration");setTime(c,"dtstart",slot.start,slot.allDay,slot.timezone);setTime(c,"due",slot.due,slot.allDay,slot.timezone);}}
  const baseline=data(c),title=field(p.title??baseline.title,1000).trim();if(!title)invalid("A task needs a title.");const zone=field(p.timezone??baseline.timezone,100),allDay=p.allDay===undefined?baseline.allDay:p.allDay===true;if(zone!=="floating"&&!root.getAllSubcomponents("vtimezone").some(z=>val(z,"tzid")===zone))timezone(zone);const status=field(p.status??baseline.status,30),progress=Number(p.progress??(status==="COMPLETED"?100:baseline.progress));
  c.updatePropertyWithValue("uid",old?.uid??randomUUID());c.updatePropertyWithValue("summary",title);c.updatePropertyWithValue("description",field(p.description??baseline.description,65536));c.updatePropertyWithValue("status",status);c.updatePropertyWithValue("percent-complete",status==="COMPLETED"?100:progress);c.updatePropertyWithValue("priority",Number(p.priority??baseline.priority));
  if(p.due!==undefined)c.removeAllProperties("duration");
  for(const key of ["start","due"] as const)if(key!=="due"||!c.hasProperty("duration"))setTime(c,key==="start"?"dtstart":"due",field(p[key]??baseline[key],100),allDay,zone);
  c.removeAllProperties("completed");if(status==="COMPLETED")c.updatePropertyWithValue("completed",ICAL.Time.fromJSDate(new Date(baseline.completedAt||new Date().toISOString()),true));
  if(p.parentUid!==undefined){c.getAllProperties("related-to").filter(t=>!t.getParameter("reltype")||t.getParameter("reltype")==="PARENT").forEach(t=>c.removeProperty(t));if(p.parentUid){const r=new ICAL.Property("related-to");r.setValue(field(p.parentUid,1024));r.setParameter("reltype","PARENT");c.addProperty(r);}}
  if(p.categories!==undefined){if(!Array.isArray(p.categories)||p.categories.length>50)invalid("Invalid categories.");c.removeAllProperties("categories");if(p.categories.length){const property=new ICAL.Property("categories");property.setValues(p.categories.map(v=>field(v,100)));c.addProperty(property);}}
  if(p.recurrence!==undefined&&!occurrenceId){c.removeAllProperties("rrule");if(p.recurrence)try{c.updatePropertyWithValue("rrule",ICAL.Recur.fromString(field(p.recurrence,1024)));}catch{invalid("Invalid recurrence rule.");}}
  if(p.reminderMinutes!==undefined&&!(p.reminderMinutes===null&&baseline.reminderMinutes===null)){c.getAllSubcomponents("valarm").filter(a=>val(a,"action")==="DISPLAY").forEach(a=>c.removeSubcomponent(a));if(p.reminderMinutes!==null){const minutes=Number(p.reminderMinutes);if(!Number.isInteger(minutes)||minutes<0||minutes>10080||(!time(c,"dtstart")&&!time(c,"due")))invalid("Invalid task reminder.");const alarm=new ICAL.Component("valarm");alarm.updatePropertyWithValue("action","DISPLAY");alarm.updatePropertyWithValue("description",title);const trigger=new ICAL.Property("trigger");trigger.setValue(ICAL.Duration.fromSeconds(-minutes*60));if(!time(c,"dtstart"))trigger.setParameter("related","END");alarm.addProperty(trigger);c.addSubcomponent(alarm);}}
  if(zone!=="UTC"&&zone!=="floating"&&!root.getAllSubcomponents("vtimezone").some(z=>val(z,"tzid")===zone)){const anchor=time(c,"dtstart")??time(c,"due");if(anchor)root.addSubcomponent(timezoneComponent(zone,anchor.year));}
  c.updatePropertyWithValue("dtstamp",ICAL.Time.now());
  if(occurrenceId&&old){if(status!=="COMPLETED"&&val(old.master,"status")==="COMPLETED"){old.master.updatePropertyWithValue("status","NEEDS-ACTION");old.master.updatePropertyWithValue("percent-complete",0);old.master.removeAllProperties("completed");}
    if(status==="COMPLETED"){const pending=nextTaskOccurrence({id:"pending",collectionId:"",href:"",uid:old.uid,content:serialize(root),projection:"{}",revision:0,modifiedAt:0},"UTC");if(!pending){old.master.updatePropertyWithValue("status","COMPLETED");old.master.updatePropertyWithValue("percent-complete",100);old.master.updatePropertyWithValue("completed",ICAL.Time.now());}}}
  return readTask(serialize(root));
}
export function nextTaskOccurrence(row:PimResource,zone:string){return taskOccurrences(row,zone,new Date("0001-01-01T00:00:00Z"),new Date("9999-12-31T23:59:59Z"),{pendingOnly:true,first:true})[0];}
export function taskOccurrences(row:PimResource,zone:string,from:Date,to:Date,options:{pendingOnly?:boolean;first?:boolean}={}){const task=readTask(row.content);if(options.pendingOnly&&["COMPLETED","CANCELLED"].includes(task.data.status))return [];if(!task.data.start&&!task.data.due)return [];const converted=new ICAL.Component("vcalendar");converted.updatePropertyWithValue("version","2.0");for(const component of task.root.getAllSubcomponents()){if(component.name==="vtimezone"){converted.addSubcomponent(new ICAL.Component(JSON.parse(JSON.stringify(component.toJSON()))));continue;}const event=new ICAL.Component(JSON.parse(JSON.stringify(component.toJSON())));event.jCal[0]="vevent";const start=time(event,"dtstart")??effectiveDue(event)!;if(!time(event,"dtstart")){event.updatePropertyWithValue("dtstart",start.clone());const z=event.getFirstProperty("due")?.getParameter("tzid");if(z)event.getFirstProperty("dtstart")!.setParameter("tzid",z);}const end=(effectiveDue(event)??start).clone();if(start.isDate)end.adjust(1,0,0,0);const endZone=event.getFirstProperty("due")?.getParameter("tzid")??event.getFirstProperty("dtstart")?.getParameter("tzid");event.removeAllProperties("due");event.removeAllProperties("duration");event.updatePropertyWithValue("dtend",end);if(endZone)event.getFirstProperty("dtend")!.setParameter("tzid",endZone);event.removeAllProperties("status");if(options.pendingOnly&&["COMPLETED","CANCELLED"].includes(data(component).status))event.updatePropertyWithValue("status","CANCELLED");converted.addSubcomponent(event);}
  return expand({...row,content:serialize(converted)},zone,from,to,!!options.first,!options.pendingOnly).map(o=>{const exception=task.root.getAllSubcomponents("vtodo").find(c=>val(c,"recurrence-id").replace(/Z$/,"")===o.recurrenceId.replace(/Z$/,"")),p=exception?data(exception):task.data,end=p.allDay?ICAL.Time.fromDateString(o.end):ICAL.Time.fromDateTimeString(o.end);if(p.allDay)end.adjust(-1,0,0,0);const due=p.due?end.toString().replace(/Z$/,""):"",start=p.start?o.start:"",component=exception??task.master,alarm=component.getAllSubcomponents("valarm").find(a=>val(a,"action")==="DISPLAY"),related=alarm?.getFirstProperty("trigger")?.getParameter("related"),endEpoch=p.allDay&&due?wallEpoch(due,zone):Date.parse(o.endAt);return {...o,...p,start,due,recurrenceId:o.recurrenceId,startAt:o.startAt,endAt:o.endAt,alarmAt:related==="END"&&p.start?endEpoch:Date.parse(o.startAt)};});
}
/** RFC 4791 section 9.9 distinguishes undated, due-only and completed tasks. */
export function taskMatchesRange(row:PimResource,from:Date,to:Date){
  const task=readTask(row.content),c=task.master,p=task.data,s=+from,e=+to;
  if(!Number.isFinite(s)||!Number.isFinite(e)||s>=e)invalid("Invalid task time range.");
  const epoch=(key:string)=>{const t=time(c,key);if(!t)return undefined;const zone=String(c.getFirstProperty(key)?.getParameter("tzid")??(t.zone.tzid==="UTC"?"UTC":"floating"));return !t.isDate&&t.zone.tzid!=="floating"&&t.zone.tzid!=="local"?t.toUnixTime()*1000:wallEpoch(t.toString().replace(/Z$/,""),zone==="floating"?"UTC":zone);};
  const start=epoch("dtstart"),due=epoch("due"),completed=epoch("completed"),created=epoch("created"),duration=c.getFirstPropertyValue("duration") as ICAL.Duration|null;
  if(p.recurring)return taskOccurrences(row,"UTC",from,to,{first:true}).length>0;
  if(start!==undefined){if(duration){const end=start+duration.toSeconds()*1000;return s<=end&&(e>start||e>=end);}if(due!==undefined)return(s<due||s<=start)&&(e>start||e>=due);return s<=start&&e>start;}
  if(due!==undefined)return s<due&&e>=due;
  if(completed!==undefined&&created!==undefined)return(s<=created||s<=completed)&&(e>=created||e>=completed);
  if(completed!==undefined)return s<=completed&&e>=completed;
  return created===undefined||e>created;
}

/** Date-only tasks retain their calendar day; timed tasks use the viewer's day. */
export function taskDue(row:PimResource,viewerZone:string,occurrence?:TaskData&{endAt:string}){
  const task=readTask(row.content),p=occurrence??task.data;if(!p.due)return {day:"",sort:"9999"};
  if(p.allDay)return {day:p.due.slice(0,10),sort:p.due};
  const due=effectiveDue(task.master)!,epoch=occurrence?Date.parse(occurrence.endAt):due.zone.tzid!=="floating"&&due.zone.tzid!=="local"?due.toUnixTime()*1000:wallEpoch(p.due,p.timezone==="floating"?viewerZone:p.timezone);
  return {day:wall(epoch,viewerZone).slice(0,10),sort:new Date(epoch).toISOString()};
}
