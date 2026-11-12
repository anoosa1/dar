/** @jsxImportSource @dar/app-sdk */
import {createRoot,useState,useEffect,useMemo,useRef,AppWorkspace,Select,SettingsSheet,SettingRow,Segmented,SaveStatus,Icon,type AppHost,type AppRecord,type SaveState,type SettingsNotice} from "@dar/app-sdk";
import {methods,prayers,prayerTimes,nextPrayer,dateInZone,addDays,civilDate,validateSettings,type Settings,type Prayer,type HighLatitude} from "../calculations/index.ts";
import "./style.css";
const labels:Record<Prayer,string>={fajr:"Fajr",sunrise:"Sunrise",dhuhr:"Dhuhr",asr:"Asr",maghrib:"Maghrib",isha:"Isha"};
const message=(e:unknown)=>e instanceof Error?e.message:"The request failed. Please try again.";
type Times=Record<Prayer,number|null>;
function timesFor(date:string,s:Settings):Times|null{try{return Object.fromEntries(prayerTimes(date,s).map(e=>[e.prayer,e.at])) as Times;}catch{return null;}}
function duration(ms:number){const minutes=Math.max(0,Math.ceil(ms/60000)),hours=Math.floor(minutes/60);return hours?`${hours} h ${minutes%60} min`:`${minutes} min`;}

/** Today band (settings sheet B): the next prayer, a countdown and the day's timeline. */
function TodayBand({today,label,times,next,tomorrow,now,time,dayName}:{today:string;label:string;tomorrow:boolean;times:Times|null;next:{prayer:Prayer;at:number}|null;now:number;time:(at:number|null)=>string;dayName:(date:string)=>string}){
  const points=prayers.flatMap(prayer=>times?.[prayer]==null?[]:[{prayer,at:times[prayer]!}]);
  const first=(points[0]?.at??now)-3600000,last=(points.at(-1)?.at??now)+3600000,at=(value:number)=>`${((value-first)/(last-first)*100).toFixed(2)}%`;
  return <section className="prayer-today" aria-labelledby="prayer-today-heading">
    <div className="prayer-today-summary"><div><p>{dayName(today)} · {label}</p><h1 id="prayer-today-heading">{next?`${labels[next.prayer]}${tomorrow?" tomorrow":""} at ${time(next.at)}`:"No upcoming prayer"}</h1></div>{next&&<p className="prayer-countdown"><small>Starts in</small><strong>{duration(next.at-now)}</strong></p>}</div>
    {points.length>0&&<svg className="prayer-timeline" width="100%" height="78" role="img" aria-label={`Today: ${points.map(p=>`${labels[p.prayer]} ${time(p.at)}${p.at<=now?" (passed)":""}`).join(", ")}`}>
      <line className="prayer-timeline-track" x1="0" x2="100%" y1="42" y2="42"/>
      {times?.sunrise!=null&&times.maghrib!=null&&<line className="prayer-timeline-daylight" x1={at(times.sunrise)} x2={at(times.maghrib)} y1="42" y2="42"/>}
      {now>first&&now<last&&<line className="prayer-timeline-now" x1={at(now)} x2={at(now)} y1="28" y2="56"/>}
      {points.map(p=><g key={p.prayer} className={`prayer-timeline-point ${p.at<=now?"is-past":""} ${p.at===next?.at?"is-next":""} ${p.prayer==="sunrise"?"is-solar":""}`}>
        <text x={at(p.at)} y="18" textAnchor="middle" className="prayer-timeline-name">{labels[p.prayer]}</text><text x={at(p.at)} y="18" textAnchor="middle" className="prayer-timeline-initial">{labels[p.prayer][0]}</text>
        <circle cx={at(p.at)} cy="42" r={p.prayer==="sunrise"?6:8}/><text x={at(p.at)} y="74" textAnchor="middle" className="prayer-timeline-time">{time(p.at)}</text>
      </g>)}
    </svg>}
  </section>;
}

function PrayerApp({host}:{host:AppHost}){
  const [record,setRecord]=useState<AppRecord<Settings>|null>(),[error,setError]=useState(""),[query,setQuery]=useState(""),[editor,setEditor]=useState(false),[baseline,setBaseline]=useState<Settings|null>(null),[suggestedZone,setSuggestedZone]=useState(Intl.DateTimeFormat().resolvedOptions().timeZone),[now,setNow]=useState(Date.now()),[start,setStart]=useState(""),[selected,setSelected]=useState<string>();
  const settings=record?.value;
  async function load(){setError("");try{const value=await host.data.get<Settings>("settings");if(value)validateSettings(value.value);setRecord(value);}catch(e){if(!host.signal.aborted)setError(message(e));}}
  useEffect(()=>{void load();void host.regional().then(p=>{if(p.timezone)setSuggestedZone(p.timezone);}).catch(()=>{});const refresh=()=>setNow(Date.now()),timer=setInterval(refresh,1000);document.addEventListener("visibilitychange",refresh);return()=>{clearInterval(timer);document.removeEventListener("visibilitychange",refresh);};},[]);
  const minute=Math.floor(now/60000),today=dateInZone(now,settings?.location.timezone??suggestedZone),week=start||today;
  const days=useMemo(()=>Array.from({length:7},(_,i)=>addDays(week,i)),[week]);
  const next=useMemo(()=>{try{return settings?nextPrayer(minute*60000,settings):null;}catch{return null;}},[settings,minute]);
  const rows=useMemo(()=>settings?days.map(date=>({date,times:timesFor(date,settings),before:baseline?timesFor(date,baseline):null})):[],[settings,baseline,days]);
  const todayTimes=useMemo(()=>settings?timesFor(today,settings):null,[settings,today]);
  const remaining=next?.at?Math.max(0,next.at-now):0;
  const time=(at:number|null)=>at===null?"Unavailable":new Intl.DateTimeFormat(undefined,{timeZone:settings!.location.timezone,hour:"numeric",minute:"2-digit"}).format(at);
  const dayName=(date:string)=>new Intl.DateTimeFormat(undefined,{timeZone:"UTC",weekday:"long",month:"short",day:"numeric"}).format(civilDate(date));
  const weekday=(date:string)=>new Intl.DateTimeFormat(undefined,{timeZone:"UTC",weekday:"long"}).format(civilDate(date));
  const shortDate=(date:string)=>new Intl.DateTimeFormat(undefined,{timeZone:"UTC",month:"short",day:"numeric"}).format(civilDate(date));
  let events:ReturnType<typeof prayerTimes>=[],calculationError="";
  try{if(settings&&selected)events=prayerTimes(selected,settings);}catch(e){calculationError=message(e);}
  const matching=events.filter(e=>labels[e.prayer].toLowerCase().includes(query.toLowerCase()));
  function openDay(date:string){setSelected(date);const url=new URL(location.href);url.searchParams.set("date",date);history.replaceState(null,"",url);}
  function closeDay(){setSelected(undefined);const url=new URL(location.href);url.searchParams.delete("date");history.replaceState(null,"",url);}
  function openSettings(){setBaseline(settings??null);setEditor(true);}
  function closeSettings(){setEditor(false);setBaseline(null);}
  useEffect(()=>{if(settings&&!start){const requested=new URLSearchParams(location.search).get("date");try{if(requested){if(!/^20\d\d-/.test(requested))throw Error("Unsupported date");civilDate(requested);setStart(requested);setSelected(requested);}}catch{setError("The requested date is invalid. Choose a day from the week.");}}},[!!settings]);
  return <AppWorkspace host={host} searchLabel="Search prayer names" onSettings={openSettings} query={query} onQuery={value=>{setQuery(value);if(value&&!selected)openDay(today);}} sidebar={<><button className={`nav-item ${selected===today?"active":""}`} onClick={()=>{setStart(today);openDay(today);}}>Today</button><button className={`nav-item ${!selected?"active":""}`} onClick={()=>{setStart(today);closeDay();}}>This week</button>{settings&&<p className="prayer-sidebar-location"><strong>{settings.location.label}</strong><span>{settings.location.timezone}</span><span>{methods[settings.calculation.method].label} · {settings.calculation.asrRule==="hanafi"?"Hanafi":"Standard"} Asr</span></p>}<button className="nav-item ui-app-settings-entry" onClick={openSettings}><Icon name="settings"/>Prayer Times settings</button></>} details={settings&&selected?<><div className="pim-detail-top"><button className="text-button" onClick={closeDay}>Back to week</button><button className="icon-button" aria-label="Close day" onClick={closeDay}><Icon name="close"/></button></div><h2>{dayName(selected)}</h2><p className="muted small">{settings.location.label} · {settings.location.timezone}<br/>{methods[settings.calculation.method].label} · {settings.calculation.asrRule==="hanafi"?"Hanafi":"Standard"} Asr</p>{selected===today&&<p className="prayer-next">{next?`Next: ${labels[next.prayer]} · in ${duration(remaining)}`:"No upcoming prayer can be calculated for this location."}</p>}{calculationError&&<p className="error" role="alert">{calculationError}</p>}<div aria-label="Prayer timetable">{matching.map(event=><div key={event.prayer} className={`prayer-time-row ${event.at===next?.at?"is-next":""} ${selected===today&&event.at!==null&&event.at<=now?"is-past":""}`}><div><strong>{labels[event.prayer]}</strong>{event.prayer==="sunrise"&&<small>Solar event</small>}{event.status==="adjusted"&&<small>Adjusted</small>}</div><time dateTime={event.at===null?undefined:new Date(event.at).toISOString()}>{time(event.at)}</time></div>)}</div>{query&&!matching.length&&<p className="muted">No matching prayers.</p>}{events.some(e=>e.at===null)&&<p className="prayer-explanation">Some solar events do not occur with these settings. Edit the location or high-latitude rule; unavailable times are not estimated.</p>}<button className="text-button" onClick={openSettings}>Edit prayer settings</button><p className="prayer-explanation">Calculated estimates. Check your local mosque's timetable.</p></>:undefined}>
    {error&&<div className="error" role="alert">{error}<button className="text-button" onClick={()=>void load()}>Retry</button></div>}
    {record===undefined?<p role="status">Loading prayer settings…</p>:record===null||!settings?<div className="prayer-empty"><h1>Set your prayer location</h1><p>Choose coordinates, timezone and calculation method to see your timetable.</p><button className="primary-button" onClick={openSettings}>Set up Prayer Times</button></div>:<>
      <TodayBand today={today} label={settings.location.label} times={todayTimes} next={next?.at!=null?{prayer:next.prayer,at:next.at}:null} tomorrow={next?.at!=null&&dateInZone(next.at,settings.location.timezone)!==today} now={now} time={time} dayName={dayName}/>
      <div className="prayer-week-heading"><h2>{shortDate(days[0])} – {shortDate(days[6])}</h2><div className="prayer-week-controls"><button className="icon-button" aria-label="Previous week" onClick={()=>setStart(addDays(week,-7))}><Icon name="back"/></button><button className="secondary-button" onClick={()=>{setStart(today);openDay(today);}}>Today</button><button className="icon-button" aria-label="Next week" onClick={()=>setStart(addDays(week,7))}><Icon name="chevron"/></button></div></div>
      <table className="prayer-week"><caption className="sr-only">Prayer times from {dayName(days[0])} to {dayName(days[6])}</caption>
        <thead><tr><th scope="col">Day</th>{prayers.map(prayer=><th key={prayer} scope="col" className={prayer==="sunrise"?"is-solar":""}>{labels[prayer]}</th>)}</tr></thead>
        <tbody>{rows.map(({date,times,before})=><tr key={date} className={[date===today&&"is-today",selected===date&&"is-selected",date<today&&"is-past"].filter(Boolean).join(" ")} onClick={()=>openDay(date)}>
          <th scope="row"><button type="button" className="prayer-day" aria-label={dayName(date)} aria-pressed={selected===date} onClick={event=>{event.stopPropagation();openDay(date);}}><strong>{weekday(date)}</strong><small>{date===today?"Today":shortDate(date)}</small></button></th>
          {prayers.map(prayer=>{const at=times?.[prayer]??null,changed=!!before&&before[prayer]!==at;return <td key={prayer} data-label={labels[prayer]} className={[prayer==="sunrise"&&"is-solar",at!==null&&at===next?.at&&"is-next",changed&&"is-changed"].filter(Boolean).join(" ")}><time dateTime={at===null?undefined:new Date(at).toISOString()}>{times?time(at):"—"}</time>{changed&&<span className="sr-only"> (changed)</span>}</td>;})}
        </tr>)}</tbody></table>
      <p className="prayer-explanation">Calculated estimates. Check your local mosque's timetable.</p>
    </>}
    {editor&&<PrayerSettings host={host} record={record??null} timezone={suggestedZone} onClose={closeSettings} onSaved={value=>{if(!record)setBaseline(value.value);setRecord(value);}} onReload={()=>void load()}/>}
  </AppWorkspace>;
}

type Draft={label:string;latitude:string;longitude:string;zone:string;method:keyof typeof methods|"";asr:"standard"|"hanafi";high:HighLatitude;offsets:Record<Prayer,number>};
type Field="label"|"latitude"|"longitude"|"zone"|"method"|"asr"|"high"|"offsets";
const zeros=Object.fromEntries(prayers.map(p=>[p,0])) as Record<Prayer,number>;
const draftOf=(s:Settings|undefined,zone:string):Draft=>({label:s?.location.label??"",latitude:s?String(s.location.latitude):"",longitude:s?String(s.location.longitude):"",zone:s?.location.timezone??zone,method:s?.calculation.method??"",asr:s?.calculation.asrRule??"standard",high:s?.calculation.highLatitudeRule??"none",offsets:s?.calculation.offsetMinutes??zeros});
function problems(d:Draft):Partial<Record<Field,string>>{
  const found:Partial<Record<Field,string>>={},latitude=Number(d.latitude),longitude=Number(d.longitude);
  if(!d.label.trim())found.label="Enter a location name.";else if(d.label.length>100)found.label="Use 100 characters or fewer.";
  if(!d.latitude.trim()||!Number.isFinite(latitude)||Math.abs(latitude)>90)found.latitude="Between −90 and 90. Not saved.";
  if(!d.longitude.trim()||!Number.isFinite(longitude)||Math.abs(longitude)>180)found.longitude="Between −180 and 180. Not saved.";
  try{if(!d.zone.trim())throw Error();new Intl.DateTimeFormat("en",{timeZone:d.zone.trim()}).format();}catch{found.zone="Enter an IANA timezone, such as America/Toronto.";}
  if(!d.method)found.method="Choose a calculation method.";
  if(!prayers.every(p=>Number.isInteger(d.offsets[p])&&Math.abs(d.offsets[p])<=120))found.offsets="Offsets range from −120 to 120 minutes. Not saved.";
  return found;
}
const toSettings=(d:Draft):Settings=>({schemaVersion:1,location:{label:d.label.trim(),latitude:Number(d.latitude),longitude:Number(d.longitude),timezone:d.zone.trim()},calculation:{method:d.method as keyof typeof methods,asrRule:d.asr,highLatitudeRule:d.high,offsetMinutes:d.offsets}});
const highRules:[HighLatitude,string][]=[["none","No adjustment"],["middle","Middle of the night"],["seventh","One seventh of the night"],["angle","Angle-based night portion"]];

/** Settings sheet: each valid change saves with the record revision; invalid text never saves. */
function PrayerSettings({host,record,timezone,onClose,onSaved,onReload}:{host:AppHost;record:AppRecord<Settings>|null;timezone:string;onClose:()=>void;onSaved:(value:AppRecord<Settings>)=>void;onReload:()=>void}){
  const [draft,setDraft]=useState(()=>draftOf(record?.value,timezone)),[touched,setTouched]=useState<Field[]>([]),[states,setStates]=useState<Partial<Record<Field,SaveState>>>({}),[notice,setNotice]=useState<SettingsNotice|null>(null),[error,setError]=useState(""),[conflict,setConflict]=useState(false),[locating,setLocating]=useState(false),[section,setSection]=useState("Location");
  const current=useRef(record),queue=useRef(Promise.resolve());
  // Adopt a version saved elsewhere, such as after Reload saved settings.
  useEffect(()=>{if(record&&record.revision!==current.current?.revision){current.current=record;setDraft(draftOf(record.value,timezone));setTouched([]);setStates({});setConflict(false);setError("");}},[record?.revision]);
  const issues=problems(draft),shown=(field:Field)=>touched.includes(field)?issues[field]:undefined;
  const mark=(fields:Field[],state:SaveState|undefined)=>setStates(s=>({...s,...Object.fromEntries(fields.map(f=>[f,state]))}));
  const group=(fields:Field[]):SaveState|undefined=>fields.some(f=>states[f]==="saving")?"saving":fields.some(f=>states[f]==="error")?"error":fields.some(f=>states[f]==="saved")?"saved":undefined;
  function commit(next:Draft,fields:Field[],summary:string,undoable=true){
    setTouched(t=>[...new Set([...t,...fields])]);
    const found=problems(next);
    if(fields.some(f=>found[f])){mark(fields,"error");return;}
    if(Object.keys(found).length)return;
    const value=toSettings(next),previous=current.current;
    if(previous&&JSON.stringify(previous.value)===JSON.stringify(value))return;
    mark(fields,"saving");setNotice(null);setError("");
    queue.current=queue.current.then(async()=>{
      try{
        const saved=await host.data.put("settings",validateSettings(value),current.current?.revision??0);current.current=saved;onSaved(saved);mark(fields,"saved");
        setNotice({message:summary,undo:undoable&&previous?()=>{const back=draftOf(previous.value,timezone);setDraft(back);commit(back,fields,"Change undone",false);}:undefined});
      }catch(e){mark(fields,"error");if((e as {status?:number}).status===409)setConflict(true);else setError(message(e));}
    });
  }
  const text=(field:"label"|"latitude"|"longitude"|"zone",summary:string)=>({value:draft[field],"aria-invalid":!!shown(field)||undefined,"aria-describedby":shown(field)?`prayer-${field}-error`:undefined,onChange:(event:{target:{value:string}})=>setDraft(d=>({...d,[field]:event.target.value})),onBlur:()=>commit(draft,[field],summary),onKeyDown:(event:{key:string;preventDefault:()=>void})=>{if(event.key==="Enter"){event.preventDefault();commit(draft,[field],summary);}}});
  const problem=(field:Field)=>shown(field)&&<span id={`prayer-${field}-error`} className="field-error">{shown(field)}</span>;
  function locate(){if(!navigator.geolocation){setError("Location is unavailable in this browser. Enter coordinates manually.");return;}setLocating(true);setError("");navigator.geolocation.getCurrentPosition(position=>{if(host.signal.aborted)return;const next={...draft,latitude:String(position.coords.latitude),longitude:String(position.coords.longitude)};setDraft(next);setLocating(false);commit(next,["latitude","longitude"],"Location updated");},()=>{if(!host.signal.aborted){setError("Location was not available. Enter coordinates manually.");setLocating(false);}},{timeout:10000,maximumAge:0});}
  function offset(prayer:Prayer,value:number){const next={...draft,offsets:{...draft.offsets,[prayer]:value}};setDraft(next);commit(next,["offsets"],`${labels[prayer]} offset set to ${value>0?"+":""}${value} min`);}
  return <SettingsSheet title="Prayer Times settings" icon="clock" sections={["Location","Calculation","Offsets","Keyboard shortcuts"]} section={section} onSection={setSection} onClose={()=>{commit(draft,["label","latitude","longitude","zone","offsets"],"Prayer settings saved",false);onClose();}} notice={notice} onNotice={setNotice} footer={record?"Changes save as you make them. Invalid values are not saved.":"Your timetable starts once the location and method are complete."}>
    {error&&<p className="error settings-problem" role="alert">{error}</p>}
    {conflict&&<div role="alert" className="settings-problem settings-conflict"><p><strong>Changed in another tab</strong>Your prayer settings were saved elsewhere. Reload to see them before changing them again.</p><button type="button" className="text-button" onClick={onReload}>Reload saved settings</button></div>}
    {!record&&Object.keys(issues).length>0&&<p className="settings-setup-hint">Complete the location and choose a calculation method to see your timetable.</p>}
    {section==="Location"&&<section className="settings-group" aria-label="Location"><h3>Location <SaveStatus state={group(["label","latitude","longitude","zone"])}/></h3>
      <div className="settings-field"><label>Location name<input maxLength={100} {...text("label","Location name saved")}/></label>{problem("label")}</div>
      <button type="button" className="secondary-button prayer-locate" disabled={locating} onClick={locate}><Icon name="globe"/>{locating?"Locating…":"Use my location"}</button>
      <div className="prayer-coordinate-grid"><div className="settings-field"><label>Latitude<input type="number" min={-90} max={90} step="any" {...text("latitude","Latitude saved")}/></label>{problem("latitude")}</div><div className="settings-field"><label>Longitude<input type="number" min={-180} max={180} step="any" {...text("longitude","Longitude saved")}/></label>{problem("longitude")}</div></div>
      <div className="settings-field"><label>Timezone<input placeholder="America/Toronto" {...text("zone","Timezone saved")}/></label>{problem("zone")||<span className="field-hint">The timezone of this location, including daylight saving.</span>}</div>
    </section>}
    {section==="Calculation"&&<><section className="settings-group" aria-label="Method"><h3>Method <SaveStatus state={states.method}/></h3>
      <div role="radiogroup" aria-label="Calculation method" className="prayer-methods">{(Object.entries(methods) as [keyof typeof methods,(typeof methods)[keyof typeof methods]][]).map(([id,m])=><label key={id} className={draft.method===id?"is-checked":""}><input type="radio" name="prayer-method" checked={draft.method===id} onChange={()=>{const next={...draft,method:id};setDraft(next);commit(next,["method"],`Method set to ${m.label}`);}}/><span>{m.label}</span><small>Fajr {m.fajr}° · Isha {m.isha}°</small></label>)}</div>{problem("method")}</section>
      <section className="settings-group" aria-label="Conventions"><h3>Conventions</h3>
        <SettingRow label="Asr convention" hint="Hanafi waits for two shadow lengths" state={states.asr}><Segmented label="Asr convention" value={draft.asr} options={[["standard","Standard"],["hanafi","Hanafi"]] as const} onChange={value=>{const next={...draft,asr:value};setDraft(next);commit(next,["asr"],`Asr set to ${value==="hanafi"?"Hanafi":"Standard"}`);}}/></SettingRow>
        <SettingRow label="High-latitude rule" hint="For places where twilight never fully ends" state={states.high} stacked><Select aria-label="High-latitude rule" value={draft.high} onChange={event=>{const value=event.target.value as HighLatitude,next={...draft,high:value};setDraft(next);commit(next,["high"],`High-latitude rule: ${highRules.find(([id])=>id===value)?.[1]}`);}}>{highRules.map(([id,label])=><option key={id} value={id}>{label}</option>)}</Select></SettingRow>
      </section></>}
    {section==="Offsets"&&<section className="settings-group" aria-label="Minute offsets"><h3>Minute offsets <SaveStatus state={states.offsets}/></h3><p className="field-hint">Applied after calculation, from −120 to 120 minutes. Each step saves.</p>
      {prayers.map(prayer=><SettingRow key={prayer} label={labels[prayer]}><span className="prayer-stepper"><button type="button" className="icon-button" aria-label={`Decrease ${labels[prayer]} offset`} disabled={draft.offsets[prayer]<=-120} onClick={()=>offset(prayer,draft.offsets[prayer]-1)}>−</button><input type="number" min={-120} max={120} step={1} aria-label={`${labels[prayer]} offset`} value={draft.offsets[prayer]} onChange={event=>setDraft(d=>({...d,offsets:{...d.offsets,[prayer]:event.target.value===""?Number.NaN:Number(event.target.value)}}))} onBlur={()=>commit(draft,["offsets"],`${labels[prayer]} offset saved`)} onKeyDown={event=>{if(event.key==="Enter"){event.preventDefault();commit(draft,["offsets"],`${labels[prayer]} offset saved`);}}}/><button type="button" className="icon-button" aria-label={`Increase ${labels[prayer]} offset`} disabled={draft.offsets[prayer]>=120} onClick={()=>offset(prayer,draft.offsets[prayer]+1)}>+</button><small>min</small></span></SettingRow>)}{problem("offsets")}
    </section>}
    {section==="Keyboard shortcuts"&&<dl className="ui-shortcuts"><dt>Ctrl / Cmd K</dt><dd>Search all apps</dd><dt>Left / Right</dt><dd>Move between settings tabs</dd><dt>Enter</dt><dd>Save the focused field</dd><dt>Escape</dt><dd>Close settings</dd><dt>Tab / Shift Tab</dt><dd>Move between controls</dd></dl>}
  </SettingsSheet>;
}
export function mount(element:HTMLElement,host:AppHost){const root=createRoot(element,{onUncaughtError:host.onError});root.render(<PrayerApp host={host}/>);return()=>root.unmount();}
