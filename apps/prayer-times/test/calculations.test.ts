import assert from "node:assert/strict";
import {test} from "node:test";
import {prayerTimes,nextPrayer,prayers,validateSettings,dateInZone,addDays,type Settings} from "../calculations/index.ts";
const toronto:Settings={schemaVersion:1,location:{label:"Toronto",latitude:43.6532,longitude:-79.3832,timezone:"America/Toronto"},calculation:{method:"ISNA",asrRule:"standard",highLatitudeRule:"none",offsetMinutes:Object.fromEntries(prayers.map(p=>[p,0])) as Settings["calculation"]["offsetMinutes"]}};
function event(date:string,s:Settings,p:string){return prayerTimes(date,s).find(e=>e.prayer===p)!;}
test("independent USNO rise/transit/set fixtures agree within two minutes in both hemispheres",()=>{
  // Retrieved 2026-11-08 from USNO API v4.0.1 (fixed UTC offsets in these dates).
  // https://aa.usno.navy.mil/api/rstt/oneday?date=2026-11-08&coords=43.6532,-79.3832&tz=-5
  // https://aa.usno.navy.mil/api/rstt/oneday?date=2026-06-21&coords=-33.8688,151.2093&tz=10
  const fixtures=[{date:"2026-11-08",settings:toronto,times:{sunrise:"12:03",dhuhr:"17:01",maghrib:"21:59"}},{date:"2026-06-21",settings:{...toronto,location:{label:"Sydney",latitude:-33.8688,longitude:151.2093,timezone:"Australia/Sydney"}},times:{sunrise:"21:00",dhuhr:"01:57",maghrib:"06:54"}}];
  for(const f of fixtures)for(const [p,hhmm] of Object.entries(f.times)){
    const date=f.settings.location.label==="Sydney"&&p==="sunrise"?"2026-06-20":f.date;
    const expected=Date.parse(`${date}T${hhmm}:00Z`)+(p==="dhuhr"?60000:0),actual=event(f.date,f.settings,p).at;
    assert.ok(actual!==null&&Math.abs(actual-expected)<=120000,JSON.stringify({p,date:f.date,actual,expected}));
  }
});
test("independent PrayTimes published sample checks ISNA sunrise; convention ordering and offsets hold",()=>{
  // https://github.com/abodehq/Pray-Times/blob/master/Code/v2/python/praytimes.py
  // Published example: 2011-02-09, latitude 43, longitude -80, UTC-5, sunrise 07:26.
  const s=structuredClone(toronto);s.location.latitude=43;s.location.longitude=-80;
  assert.ok(Math.abs(event("2011-02-09",s,"sunrise").at!-Date.parse("2011-02-09T12:26:00Z"))<=120000);
  const normal=prayerTimes("2026-11-08",toronto);assert.ok(normal.every((e,i)=>e.at!==null&&(i===0||e.at>normal[i-1].at!)));
  const hanafi=structuredClone(toronto);hanafi.calculation.asrRule="hanafi";assert.ok(event("2026-11-08",hanafi,"asr").at!>event("2026-11-08",toronto,"asr").at!);
  const mwl=structuredClone(toronto);mwl.calculation.method="MWL";assert.ok(event("2026-11-08",mwl,"fajr").at!<normal[0].at!);assert.ok(event("2026-11-08",mwl,"isha").at!>normal[5].at!);
  const shifted=structuredClone(toronto);shifted.calculation.offsetMinutes.fajr=7;assert.equal(event("2026-11-08",shifted,"fajr").at,normal[0].at!+7*60000);
});
test("DST, dateline, year rollover and unrelated process timezones keep target civil dates",()=>{
  const saved=process.env.TZ;
  try{const expected=prayerTimes("2026-03-08",toronto);for(const tz of ["UTC","Asia/Tokyo","America/Los_Angeles"]){process.env.TZ=tz;assert.deepEqual(prayerTimes("2026-03-08",toronto),expected);}}
  finally{if(saved===undefined)delete process.env.TZ;else process.env.TZ=saved;}
  for(const date of ["2026-03-07","2026-03-08","2026-11-01","2000-01-01","2099-12-31"]){for(const e of prayerTimes(date,toronto))assert.equal(dateInZone(e.at!,toronto.location.timezone),date);}
  const kiritimati={...toronto,location:{label:"Kiritimati",latitude:1.8721,longitude:-157.4278,timezone:"Pacific/Kiritimati"}};for(const e of prayerTimes("2026-01-01",kiritimati))assert.equal(dateInZone(e.at!,kiritimati.location.timezone),"2026-01-01");
  const last=event("2026-12-31",toronto,"isha").at!;assert.equal(nextPrayer(last+1,toronto)?.prayer,"fajr");assert.equal(dateInZone(nextPrayer(last+1,toronto)!.at!,toronto.location.timezone),"2027-01-01");
  const madrid={...structuredClone(toronto),location:{label:"Madrid",latitude:40.4168,longitude:-3.7038,timezone:"Europe/Madrid"}};madrid.calculation.offsetMinutes.isha=120;
  const lateIsha=event("2026-06-21",madrid,"isha").at!;assert.equal(dateInZone(lateIsha,madrid.location.timezone),"2026-06-22");assert.equal(nextPrayer(lateIsha-60000,madrid)?.at,lateIsha);
});
test("high latitude rules are explicit and polar missing events never become fabricated times",()=>{
  const london={...structuredClone(toronto),location:{label:"London",latitude:51.5074,longitude:-0.1278,timezone:"Europe/London"}};
  london.calculation.method="MWL";assert.equal(event("2026-06-21",london,"fajr").at,null);
  for(const rule of ["middle","seventh","angle"] as const){london.calculation.highLatitudeRule=rule;for(const p of ["fajr","isha"]){const e=event("2026-06-21",london,p);assert.equal(e.status,"adjusted");assert.ok(Number.isFinite(e.at));}}
  const polar={...london,location:{label:"Tromsø",latitude:69.6492,longitude:18.9553,timezone:"Europe/Oslo"}};for(const p of ["sunrise","maghrib"])assert.equal(event("2026-06-21",polar,p).at,null);
  assert.equal(event("2026-12-21",polar,"sunrise").at,null);
});
test("invalid payloads and calendar dates fail deliberately; next prayer excludes sunrise",()=>{
  for(const value of [null,{}, {...toronto,location:{...toronto.location,latitude:NaN}},{...toronto,location:{...toronto.location,timezone:"Somewhere/Invalid"}}])assert.throws(()=>validateSettings(value));
  for(const date of ["2026-02-30","2026-13-01","not-a-date","1999-01-01"])assert.throws(()=>prayerTimes(date,toronto));
  assert.equal(addDays("2024-02-28",1),"2024-02-29");const dawn=event("2026-11-08",toronto,"fajr").at!;assert.equal(nextPrayer(dawn+1,toronto)?.prayer,"dhuhr");
});
