export function shiftCalendarDate(anchor:string,view:string,direction:-1|1){
  const [year,month,day]=anchor.split("-").map(Number);
  if(view==="month"){
    const target=new Date(Date.UTC(year,month-1+direction,1));
    const last=new Date(Date.UTC(target.getUTCFullYear(),target.getUTCMonth()+1,0)).getUTCDate();
    target.setUTCDate(Math.min(day,last));return target.toISOString().slice(0,10);
  }
  return new Date(Date.parse(`${anchor}T12:00:00Z`)+direction*(view==="day"?1:view==="agenda"?30:7)*86400000).toISOString().slice(0,10);
}
