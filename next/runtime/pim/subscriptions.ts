import ICAL from "ical.js";
import {createHash,randomUUID} from "node:crypto";
import type {Database} from "../database.ts";
import {AppError} from "../errors.ts";
import {safeFetch} from "../news/net.ts";
import {readEvent} from "./calendar.ts";
import {readTask} from "./tasks.ts";
import {serialize} from "./standards.ts";

interface Due {collectionId:string;url:string;etag:string;lastModified:string}
interface Item {id:string;uid:string;href:string;content:string;projection:string}
const maxBytes=8*1024*1024,maxResources=10000,maxResource=262144;

/**
 * Splits a fetched calendar into one resource per UID (master plus overrides), each with
 * the calendar's timezones, parsing the source once. Unreadable entries are skipped and counted.
 */
export function splitSubscription(text:string){
  let root:ICAL.Component;
  try{const raw=ICAL.parse(text.replace(/^﻿/,""));if(raw[0]!=="vcalendar")throw Error();root=new ICAL.Component(raw);}
  catch{throw new AppError("invalidCalendar",422,"The address did not return an iCalendar (.ics) file.");}
  const zones=root.getAllSubcomponents("vtimezone").map(zone=>zone.toJSON()),groups=new Map<string,ICAL.Component[]>();let skipped=0;
  for(const component of root.getAllSubcomponents().filter(c=>c.name==="vevent"||c.name==="vtodo")){
    const uid=String(component.getFirstPropertyValue("uid")??"").trim();if(!uid||uid.length>1024){skipped++;continue;}
    const key=`${component.name}/${uid}`;groups.set(key,[...groups.get(key)??[],component]);
  }
  if(groups.size>maxResources)throw new AppError("operationTooLarge",413,`The calendar has more than ${maxResources.toLocaleString("en")} events.`);
  const items:Item[]=[],seen=new Set<string>();
  for(const [key,components] of groups){
    const uid=key.slice(key.indexOf("/")+1);if(seen.has(uid)){skipped++;continue;}
    try{
      const calendar=new ICAL.Component("vcalendar");calendar.updatePropertyWithValue("version","2.0");calendar.updatePropertyWithValue("prodid","-//Dar//Calendar subscription//EN");
      for(const zone of zones)calendar.addSubcomponent(new ICAL.Component(JSON.parse(JSON.stringify(zone))));
      for(const component of components)calendar.addSubcomponent(new ICAL.Component(JSON.parse(JSON.stringify(component.toJSON()))));
      const content=serialize(calendar);if(Buffer.byteLength(content)>maxResource)throw Error();
      const result=components[0].name==="vtodo"?readTask(content):readEvent(content);
      items.push({id:randomUUID(),uid,href:`${createHash("sha1").update(uid).digest("hex")}.ics`,content:result.content,projection:JSON.stringify(result.data)});seen.add(uid);
    }catch{skipped++;}
  }
  return {items,skipped};
}
/** Batches stay well under the database call limit. */
function batches(items:Item[],max=900*1024,count=200){
  const out:Item[][]=[[]];let size=0;
  for(const item of items){const bytes=item.content.length+item.projection.length+200;if((size+bytes>max||out.at(-1)!.length>=count)&&out.at(-1)!.length){out.push([]);size=0;}out.at(-1)!.push(item);size+=bytes;}
  return out;
}

/**
 * Owns calendar subscription refreshes: a loop picks due calendars, fetches each address
 * through the News SSRF guard (public addresses only unless allowed), and syncs by UID.
 * Addresses can carry secrets and are never logged. Closing stops the loop and aborts fetches.
 */
export function createCalendarSubscriptions(db:Database,{allowPrivate=false}:{allowPrivate?:boolean}={}){
  const controller=new AbortController();let closing=false,running:Promise<void>|undefined;
  const worker=<T>(operation:string,value:object)=>db.call<T>("service_pim_subscriptions",[operation,JSON.stringify(value)]);
  async function sync(row:Due){
    const collectionId=row.collectionId;
    try{
      const options={allowPrivate,signal:controller.signal,accept:"text/calendar, */*;q=0.1",etag:row.etag||undefined,lastModified:row.lastModified||undefined,maxBytes,timeout:30000};
      // webcal:// does not name a scheme: HTTPS first, plain HTTP only when HTTPS cannot connect.
      const webcal=/^webcal:\/\//i.test(row.url),secure=row.url.replace(/^webcal:\/\//i,"https://");
      const response=await safeFetch(secure,options).catch(error=>{if(webcal&&error instanceof AppError&&error.code==="feedUnavailable"&&!controller.signal.aborted)return safeFetch(row.url.replace(/^webcal:\/\//i,"http://"),options);throw error;});
      if(response.status===304){await worker("finish",{collectionId,uids:null,etag:row.etag,lastModified:row.lastModified});return;}
      if(response.status===401||response.status===403)throw new AppError("calendarUnavailable",502,`The calendar address refused access (${response.status}). Check that the link is still shared.`);
      if(response.status<200||response.status>=300)throw new AppError("calendarUnavailable",502,`The calendar address answered ${response.status}.`);
      const {items,skipped}=splitSubscription(response.body.toString("utf8"));
      for(const batch of batches(items))if(batch.length)await worker("put",{collectionId,items:batch});
      // Only a calendar where nothing could be read is an error; partly readable sources still sync.
      const error=!items.length&&skipped?`Dar could not read the ${skipped} ${skipped===1?"entry":"entries"} in this calendar.`:"";
      await worker("finish",{collectionId,uids:error?null:items.map(item=>item.uid),etag:error?"":response.etag,lastModified:error?"":response.lastModified,error});
    }catch(error){
      if(closing)return;
      const message=error instanceof AppError&&error.message!==error.code?error.message:"The calendar could not be refreshed.";
      await worker("finish",{collectionId,uids:null,error:message}).catch(()=>undefined);
    }
  }
  async function tick(){
    if(closing||running)return;
    running=(async()=>{try{const due=await worker<Due[]>("due",{});for(const row of due){if(closing)break;await sync(row);}}catch{/* retried on the next tick */}})().finally(()=>{running=undefined;});
    await running;
  }
  const timer=setInterval(()=>{void tick();},30000),first=setTimeout(()=>{void tick();},2000);
  return {
    /** Runs a refresh soon, for example right after subscribing. */
    poke(){setTimeout(()=>{void tick();},300);},
    async close(){closing=true;clearInterval(timer);clearTimeout(first);controller.abort();await running;},
  };
}
export type CalendarSubscriptions=ReturnType<typeof createCalendarSubscriptions>;
