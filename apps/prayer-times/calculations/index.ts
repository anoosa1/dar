/** Original implementation of the published USNO solar equations and prayer conventions.
 * All results are UTC milliseconds, rounded to the nearest minute after offsets.
 * References and supported limits: ../CALCULATIONS.md. No host-local Date accessors.
 */
export const prayers=["fajr","sunrise","dhuhr","asr","maghrib","isha"] as const;
export type Prayer=typeof prayers[number];
export const methods={MWL:{label:"Muslim World League",fajr:18,isha:17},ISNA:{label:"ISNA",fajr:15,isha:15},Egypt:{label:"Egyptian General Authority",fajr:19.5,isha:17.5},Karachi:{label:"University of Islamic Sciences, Karachi",fajr:18,isha:18}} as const;
export type HighLatitude="none"|"middle"|"seventh"|"angle";
export interface Settings {schemaVersion:1;location:{label:string;latitude:number;longitude:number;timezone:string};calculation:{method:keyof typeof methods;asrRule:"standard"|"hanafi";highLatitudeRule:HighLatitude;offsetMinutes:Record<Prayer,number>}}
export interface Event {prayer:Prayer;at:number|null;status:"calculated"|"adjusted"|"unavailable"}
const HOUR=3600000,DAY=24*HOUR,RAD=Math.PI/180;
const sin=(v:number)=>Math.sin(v*RAD),cos=(v:number)=>Math.cos(v*RAD),tan=(v:number)=>Math.tan(v*RAD);
const norm=(v:number,period:number)=>(v%period+period)%period;
export function dateInZone(instant:number,timezone:string):string{
  const parts=new Intl.DateTimeFormat("en-CA",{timeZone:timezone,year:"numeric",month:"2-digit",day:"2-digit"}).formatToParts(instant);
  const field=(type:string)=>parts.find(p=>p.type===type)!.value;return `${field("year")}-${field("month")}-${field("day")}`;
}
export function civilDate(date:string){
  if(!/^(?:1999|20\d\d|2100)-\d\d-\d\d$/.test(date))throw Error("Choose a date between 2000 and 2099.");
  const instant=Date.parse(date+"T00:00:00Z");if(!Number.isFinite(instant)||new Date(instant).toISOString().slice(0,10)!==date)throw Error("Invalid calendar date.");return instant;
}
export function addDays(date:string,days:number){if(!Number.isInteger(days))throw Error("Invalid day offset.");return new Date(civilDate(date)+days*DAY).toISOString().slice(0,10);}
export function validateSettings(value:unknown):Settings{
  const s=value as Settings;
  if(!s||s.schemaVersion!==1||!s.location||!s.calculation)throw Error("Unsupported prayer settings.");
  const {latitude,longitude,timezone,label}=s.location,c=s.calculation;
  if(typeof label!=="string"||!label.trim()||label.length>100||!Number.isFinite(latitude)||Math.abs(latitude)>90||!Number.isFinite(longitude)||Math.abs(longitude)>180||typeof timezone!=="string"||timezone.length>100)throw Error("Enter a location, valid coordinates and timezone.");
  try{new Intl.DateTimeFormat("en",{timeZone:timezone}).format();}catch{throw Error("Enter a valid IANA timezone, such as America/Toronto.");}
  if(!Object.hasOwn(methods,c.method)||!["standard","hanafi"].includes(c.asrRule)||!["none","middle","seventh","angle"].includes(c.highLatitudeRule)||!c.offsetMinutes||!prayers.every(p=>Number.isInteger(c.offsetMinutes[p])&&Math.abs(c.offsetMinutes[p])<=120))throw Error("Invalid calculation settings or offsets (−120 to 120 minutes).");
  return structuredClone(s);
}
/** USNO apparent solar longitude, obliquity, right ascension and equation of time. */
export function solarCoordinates(instant:number){
  const d=instant/DAY+2440587.5-2451545,anomaly=357.529+0.98560028*d,mean=norm(280.459+0.98564736*d,360);
  const longitude=mean+1.915*sin(anomaly)+0.020*sin(2*anomaly),obliquity=23.439-0.00000036*d;
  const ascension=norm(Math.atan2(cos(obliquity)*sin(longitude),cos(longitude))/RAD,360);
  return {declination:Math.asin(sin(obliquity)*sin(longitude))/RAD,equationHours:(norm(mean-ascension+180,360)-180)/15};
}
function altitude(instant:number,latitude:number,longitude:number){
  const {declination,equationHours}=solarCoordinates(instant),utcHour=norm(instant,DAY)/HOUR;
  const angle=15*(utcHour+longitude/15+equationHours-12);
  return Math.asin(Math.max(-1,Math.min(1,sin(latitude)*sin(declination)+cos(latitude)*cos(declination)*cos(angle))))/RAD;
}
/** Bracket a crossing, then bisect to subsecond precision. No crossing stays null. */
function crossing(noon:number,latitude:number,longitude:number,target:number,rising:boolean):number|null{
  const start=rising?noon-18*HOUR:noon,end=rising?noon:noon+18*HOUR,step=6*60000;let found:number|null=null;
  for(let left=start;left<end;left+=step){let a=left,b=Math.min(end,left+step),fa=altitude(a,latitude,longitude)-target,fb=altitude(b,latitude,longitude)-target;
    if(rising?fa<=0&&fb>0:fa>=0&&fb<0){
      for(let i=0;i<24;i++){const mid=(a+b)/2,fm=altitude(mid,latitude,longitude)-target;if(rising?fm>0:fm<0)b=mid;else a=mid;}
      found=(a+b)/2;if(!rising)return found;
    }
  }
  return found;
}
function solarDay(date:string,s:Settings){
  const {latitude,longitude,timezone}=s.location,base=civilDate(date);let noon:number|undefined;
  for(let shift=-2;shift<=2;shift++){
    const day=base+shift*DAY;let candidate=day+(12-longitude/15)*HOUR;
    for(let i=0;i<4;i++)candidate=day+(12-longitude/15-solarCoordinates(candidate).equationHours)*HOUR;
    if(dateInZone(candidate,timezone)===date){noon=candidate;break;}
  }
  if(noon===undefined)throw Error("This civil date does not exist in the selected timezone.");
  return {noon,sunrise:crossing(noon,latitude,longitude,-0.833333,true),sunset:crossing(noon,latitude,longitude,-0.833333,false)};
}
export function prayerTimes(date:string,input:Settings):Event[]{
  if(!/^20\d\d-/.test(date))throw Error("Choose a date between 2000 and 2099.");
  const s=validateSettings(input),{latitude,longitude}=s.location,c=s.calculation,method=methods[c.method],day=solarDay(date,s);
  const times:Record<Prayer,number|null>={fajr:crossing(day.noon,latitude,longitude,-method.fajr,true),sunrise:day.sunrise,dhuhr:day.noon+60000,asr:null,maghrib:day.sunset,isha:crossing(day.noon,latitude,longitude,-method.isha,false)};
  const diff=Math.abs(latitude-solarCoordinates(day.noon).declination);
  if(diff<90&&altitude(day.noon,latitude,longitude)>0){const target=Math.atan(1/((c.asrRule==="hanafi"?2:1)+tan(diff)))/RAD;times.asr=crossing(day.noon,latitude,longitude,target,false);}
  const adjusted=new Set<Prayer>();
  if(c.highLatitudeRule!=="none"){
    const previous=solarDay(addDays(date,-1),s),next=solarDay(addDays(date,1),s);
    const portion=(angle:number)=>c.highLatitudeRule==="middle"?0.5:c.highLatitudeRule==="seventh"?1/7:angle/60;
    if(day.sunrise!==null&&previous.sunset!==null){const bound=day.sunrise-(day.sunrise-previous.sunset)*portion(method.fajr);if(times.fajr===null||times.fajr<bound){times.fajr=bound;adjusted.add("fajr");}}
    if(day.sunset!==null&&next.sunrise!==null){const bound=day.sunset+(next.sunrise-day.sunset)*portion(method.isha);if(times.isha===null||times.isha>bound){times.isha=bound;adjusted.add("isha");}}
  }
  return prayers.map(prayer=>({prayer,at:times[prayer]===null?null:Math.round((times[prayer]!+c.offsetMinutes[prayer]*60000)/60000)*60000,status:times[prayer]===null?"unavailable":adjusted.has(prayer)||c.offsetMinutes[prayer]!==0?"adjusted":"calculated"}));
}
export function nextPrayer(now:number,s:Settings):Event|null{
  const today=dateInZone(now,s.location.timezone);
  // A late Isha or user offset can spill into the next civil day.
  return [addDays(today,-1),today,addDays(today,1)].filter(date=>/^20\d\d-/.test(date)).flatMap(date=>prayerTimes(date,s)).filter(e=>e.prayer!=="sunrise"&&e.at!==null&&e.at>now).sort((a,b)=>a.at!-b.at!)[0]??null;
}
