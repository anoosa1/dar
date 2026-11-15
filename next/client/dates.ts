/**
 * Cached date formatting. `toLocaleDateString(locale, options)` builds a new
 * formatter on every call, which is slow in long lists on phones. These match the
 * Date methods, including the fields they fill in when the options name none.
 */
const formats=new Map<string,Intl.DateTimeFormat>();
const dateFields=["weekday","year","month","day"] as const,timeFields=["dayPeriod","hour","minute","second","fractionalSecondDigits"] as const;
type Kind="date"|"time"|"any";
function format(value:number|Date,options:Intl.DateTimeFormatOptions,kind:Kind){
  const key=kind+JSON.stringify(options);let formatter=formats.get(key);
  if(!formatter){
    const styled=options.dateStyle!==undefined||options.timeStyle!==undefined,has=(fields:readonly string[])=>fields.some(field=>(options as Record<string,unknown>)[field]!==undefined);
    const bare=!styled&&(kind==="date"?!has(dateFields):kind==="time"?!has(timeFields):!has(dateFields)&&!has(timeFields));
    const filled={...options,...bare&&kind!=="time"?{year:"numeric",month:"numeric",day:"numeric"}:{},...bare&&kind!=="date"?{hour:"numeric",minute:"numeric",second:"numeric"}:{}} as Intl.DateTimeFormatOptions;
    formatter=new Intl.DateTimeFormat(undefined,filled);formats.set(key,formatter);
  }
  return formatter.format(value);
}
/** Like `date.toLocaleDateString(undefined, options)`. */
export const localDate=(value:number|Date,options:Intl.DateTimeFormatOptions={})=>format(value,options,"date");
/** Like `date.toLocaleTimeString(undefined, options)`. */
export const localTime=(value:number|Date,options:Intl.DateTimeFormatOptions={})=>format(value,options,"time");
/** Like `date.toLocaleString(undefined, options)`. */
export const localDateTime=(value:number|Date,options:Intl.DateTimeFormatOptions={})=>format(value,options,"any");
