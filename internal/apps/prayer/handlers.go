package prayer

import (
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

func (a *App) handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	if err := renderPage(w, r, user.DisplayName); err != nil {
		http.Error(w, "Internal server error", http.StatusInternalServerError)
	}
}

func renderPage(w io.Writer, r *http.Request, displayName string) error {
	analytics := r.URL.Path == "/prayer/analytics"
	sidebar := webui.BuildSidebar(webui.SidebarConfig{
		StateKey: "prayer", AriaLabel: "Prayer Times navigation",
		Items: []webui.SidebarItem{
			{ID: "prayer-today", Label: "Today", Icon: "fa-solid fa-house", Href: "/prayer", Active: !analytics},
			{ID: "prayer-analytics-link", Label: "Analytics", Icon: "fa-solid fa-chart-column", Href: "/prayer/analytics", Active: analytics},
		},
		SettingsHref: "#prayer-settings", SettingsLabel: "Prayer settings",
	})
	content, _ := webui.ExternalizeLastScript(prayerHTML(), "/prayer/assets/app.js")
	return webui.RenderRequest(w, r, webui.PageData{
		Title: "Prayer Times — Dar", CurrentApp: "prayer", AppBar: server.AppBarHTML(r),
		UserMenu: webui.UserMenuHTML(displayName), Sidebar: sidebar, Content: content,
	})
}

func handlePrayerAsset(w http.ResponseWriter, r *http.Request) {
	_, script := webui.ExternalizeLastScript(prayerHTML(), "/prayer/assets/app.js")
	webui.ServeAppJavaScript(w, r, "app.js", script)
}

func prayerHTML() string {
	return `
<div id="prayer-app" data-dar-shell class="flex h-full min-h-0 overflow-hidden bg-surface">
  <main data-dar-master-pane class="min-w-0 flex-1 overflow-y-auto overscroll-contain p-4 md:p-6">
    <section id="prayer-schedule" class="prayer-page">
      <header class="mb-5 flex items-start gap-3">
        <div data-dar-sidebar-toggle-host class="min-w-0 flex-1">
          <h1 id="prayer-date-title" class="truncate text-2xl font-semibold">Prayer Times</h1>
          <p id="prayer-location" class="mt-1 text-sm text-on-surface-variant"></p>
        </div>
        <button id="prayer-today-button" type="button" class="h-10 rounded-full border border-outline-variant px-4 text-sm font-medium hover:bg-surface-container-high">Today</button>
      </header>
      <div id="prayer-warning" class="mb-4 hidden rounded-2xl bg-error-container p-4 text-sm text-on-error-container" role="status"></div>
      <div id="prayer-loading" class="py-20 text-center text-on-surface-variant"><i class="fa-solid fa-circle-notch fa-spin mb-3 block text-3xl"></i>Loading prayer times…</div>
      <div id="prayer-setup" class="hidden rounded-3xl bg-surface-container-low p-8 text-center">
        <i class="fa-solid fa-location-dot mb-4 text-4xl text-primary"></i><h2 class="text-xl font-semibold">Choose your location</h2>
        <p class="mx-auto mt-2 max-w-md text-sm text-on-surface-variant">Prayer times use your location, timezone, calculation method, and Asr school.</p>
        <button type="button" onclick="openSettings()" class="mt-5 h-11 rounded-full bg-primary px-6 font-medium text-on-primary">Open settings</button>
      </div>
      <div id="prayer-content" class="prayer-schedule-grid hidden">
        <div>
          <section id="next-prayer-card" class="mb-4 rounded-3xl bg-primary p-5 text-on-primary" aria-label="Next prayer">
            <p class="text-sm font-medium opacity-80">Next prayer</p>
            <div class="mt-1 flex items-end justify-between gap-4"><h2 id="next-prayer-name" class="text-3xl font-semibold"></h2><p id="next-prayer-time" class="text-2xl font-semibold"></p></div>
            <p id="next-prayer-countdown" class="mt-4 border-t border-on-primary/20 pt-3 text-sm"></p>
          </section>
          <section class="rounded-3xl bg-surface-container-low p-3" aria-label="Daily prayer timeline">
            <ol id="prayer-timeline" class="divide-y divide-outline-variant/40"></ol>
          </section>
        </div>
        <aside id="calendar" class="space-y-4">
          <section class="rounded-3xl border border-outline-variant bg-surface-container-low p-4" aria-label="Prayer date calendar">
            <header class="mb-3 flex items-center gap-2"><button id="calendar-prev" type="button" class="h-9 w-9 rounded-full hover:bg-surface-container-high" aria-label="Previous month"><i class="fa-solid fa-chevron-left"></i></button><h2 id="calendar-title" class="flex-1 text-center font-semibold"></h2><button id="calendar-next" type="button" class="h-9 w-9 rounded-full hover:bg-surface-container-high" aria-label="Next month"><i class="fa-solid fa-chevron-right"></i></button></header>
            <div class="grid grid-cols-7 text-center text-xs text-on-surface-variant"><span>Sun</span><span>Mon</span><span>Tue</span><span>Wed</span><span>Thu</span><span>Fri</span><span>Sat</span></div>
            <div id="calendar-grid" class="mt-2 grid grid-cols-7 gap-1"></div>
          </section>
          <section class="rounded-3xl border border-outline-variant p-4 text-sm">
            <p class="text-xs font-semibold uppercase tracking-wider text-on-surface-variant">Calculation</p><p id="prayer-method" class="mt-1 font-medium"></p>
            <p class="mt-4 text-xs font-semibold uppercase tracking-wider text-on-surface-variant">Timezone</p><p id="prayer-timezone" class="mt-1 font-medium"></p>
          </section>
        </aside>
      </div>
    </section>

    <section id="prayer-analytics" class="prayer-page hidden">
      <header class="mb-6" data-dar-sidebar-toggle-host><h1 class="text-2xl font-semibold">Prayer analytics</h1><p id="analytics-since" class="mt-1 text-sm text-on-surface-variant"></p></header>
      <div id="analytics-loading" class="py-20 text-center text-on-surface-variant"><i class="fa-solid fa-circle-notch fa-spin mb-3 block text-3xl"></i>Loading analytics…</div>
      <div id="analytics-empty" class="hidden rounded-3xl bg-surface-container-low p-10 text-center"><i class="fa-solid fa-chart-column mb-4 text-4xl text-primary"></i><h2 class="text-xl font-semibold">No tracked prayers yet</h2><p class="mt-2 text-sm text-on-surface-variant">Enable Todo sync in Prayer settings to begin tracking.</p><button type="button" onclick="openSettings()" class="mt-5 h-11 rounded-full bg-primary px-6 font-medium text-on-primary">Open settings</button></div>
      <div id="analytics-content" class="hidden space-y-5">
        <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <article class="rounded-3xl bg-primary-container p-5"><p class="text-sm text-on-primary-container">On time</p><p id="analytics-on-time" class="mt-2 text-3xl font-semibold text-primary"></p></article>
          <article class="rounded-3xl bg-secondary-container p-5"><p class="text-sm text-on-secondary-container">Off time</p><p id="analytics-off-time" class="mt-2 text-3xl font-semibold"></p></article>
          <article class="rounded-3xl bg-error-container p-5"><p class="text-sm text-on-error-container">Missed</p><p id="analytics-missed" class="mt-2 text-3xl font-semibold text-error"></p></article>
          <article class="rounded-3xl bg-surface-container-high p-5"><p class="text-sm text-on-surface-variant">Completion rate</p><p id="analytics-rate" class="mt-2 text-3xl font-semibold"></p></article>
        </div>
        <section class="overflow-hidden rounded-3xl border border-outline-variant"><header class="bg-surface-container-low px-5 py-4"><h2 class="font-semibold">By prayer</h2></header><div id="analytics-breakdown" class="divide-y divide-outline-variant"></div></section>
      </div>
    </section>
  </main>
</div>

<div id="prayer-settings-modal" class="fixed inset-0 z-[70] hidden items-center justify-center bg-scrim/40 p-4" role="dialog" aria-modal="true" aria-labelledby="prayer-settings-title">
  <form id="prayer-settings-form" class="flex max-h-[90vh] w-full max-w-lg flex-col overflow-hidden rounded-3xl bg-surface shadow-2xl">
    <header class="flex items-center gap-3 border-b border-outline-variant px-5 py-4"><h2 id="prayer-settings-title" class="flex-1 text-xl font-semibold">Prayer settings</h2><button id="prayer-settings-close" type="button" class="h-9 w-9 rounded-full hover:bg-surface-container-high" aria-label="Close settings"><i class="fa-solid fa-xmark"></i></button></header>
    <div class="space-y-5 overflow-y-auto p-5">
      <label class="block text-sm font-medium">Location<input id="prayer-setting-location" required maxlength="200" placeholder="City, country" class="mt-2 h-11 w-full rounded-xl bg-surface-container px-3 font-normal outline-none focus:ring-2 focus:ring-primary/30"><span class="mt-1 block text-xs font-normal text-on-surface-variant">Stored separately from Personal info.</span></label>
      <label class="block text-sm font-medium">Calculation method<select id="prayer-setting-method" class="mt-2 h-11 w-full rounded-xl bg-surface-container px-3 font-normal"><option value="mwl">Muslim World League</option><option value="isna">ISNA</option><option value="egyptian">Egyptian Authority</option><option value="karachi">Karachi</option></select></label>
      <label class="block text-sm font-medium">Asr school<select id="prayer-setting-asr" class="mt-2 h-11 w-full rounded-xl bg-surface-container px-3 font-normal"><option value="standard">Standard (Shafi'i, Maliki, Hanbali)</option><option value="hanafi">Hanafi</option></select></label>
      <label class="flex items-start justify-between gap-4 rounded-2xl bg-surface-container-low p-4"><span><span class="block text-sm font-medium">Sync with Todo</span><span class="mt-1 block text-xs text-on-surface-variant">Create the five daily prayer tasks in a dedicated Prayers list.</span></span><input id="prayer-setting-sync" type="checkbox" class="mt-1 h-5 w-5 accent-primary"></label>
      <p id="prayer-settings-status" class="min-h-5 text-sm text-on-surface-variant" role="status" aria-live="polite"></p>
    </div>
    <footer class="flex justify-end gap-2 border-t border-outline-variant p-4"><button id="prayer-settings-cancel" type="button" class="h-10 rounded-full px-5 hover:bg-surface-container-high">Cancel</button><button id="prayer-settings-save" class="h-10 rounded-full bg-primary px-5 font-medium text-on-primary">Save</button></footer>
  </form>
</div>

<script>
(() => {
  const $=id=>document.getElementById(id), analyticsView=location.pathname.endsWith('/analytics');
  const prayerNames=[['fajr','Fajr','fa-moon'],['sunrise','Sunrise','fa-sun'],['dhuhr','Dhuhr','fa-sun'],['asr','Asr','fa-clock'],['maghrib','Maghrib','fa-sun'],['isha','Isha','fa-moon'],['midnight','Midnight','fa-star']];
  const requestedDate=new URLSearchParams(location.search).get('date');let settings=null,current=null,selected=requestedDate||localDate(new Date()),calendarDate=parseDate(selected),time24=false,countdownTimer=null;
  function localDate(d,zone){return new Intl.DateTimeFormat('en-CA',{year:'numeric',month:'2-digit',day:'2-digit',timeZone:zone}).format(d)}
  function parseDate(v){const [y,m,d]=v.split('-').map(Number);return new Date(y,m-1,d,12)}
  function fmt(ts,options={}){return new Intl.DateTimeFormat([],{hour:'numeric',minute:'2-digit',hour12:time24?false:undefined,timeZone:current?.timezone,...options}).format(new Date(ts*1000))}
  async function api(url,options={}){const r=await fetch(url,options),body=await r.json().catch(()=>({}));if(!r.ok)throw new Error(body.error||'Request failed');return body}
  function show(id,visible){$(id)?.classList.toggle('hidden',!visible)}
  function warning(message){const el=$('prayer-warning');if(!el)return;el.textContent=message||'';show('prayer-warning',!!message)}
  async function loadSettings(){settings=await api('/api/prayer/settings');$('prayer-setting-location').value=settings.location||settings.location_label||'';$('prayer-setting-method').value=settings.method||'mwl';$('prayer-setting-asr').value=settings.asr_school||'standard';$('prayer-setting-sync').checked=!!settings.todo_sync;warning(settings.sync_warning||'')}
  window.openSettings=async function(){if(!settings)await loadSettings();const modal=$('prayer-settings-modal');modal.classList.remove('hidden');modal.classList.add('flex');$('prayer-settings-status').textContent=settings.sync_warning||'';$('prayer-setting-location').focus()};
  function closeSettings(){const modal=$('prayer-settings-modal');modal.classList.add('hidden');modal.classList.remove('flex')}
  async function saveSettings(e){e.preventDefault();const status=$('prayer-settings-status');status.textContent='Saving…';$('prayer-settings-save').disabled=true;try{settings=await api('/api/prayer/settings',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({location:$('prayer-setting-location').value,method:$('prayer-setting-method').value,asr_school:$('prayer-setting-asr').value,todo_sync:$('prayer-setting-sync').checked})});status.textContent=settings.sync_warning||'Settings saved.';warning(settings.sync_warning||'');DarUI.toast('Prayer settings saved');setTimeout(()=>{closeSettings();location.href=analyticsView?'/prayer/analytics':'/prayer'},350)}catch(error){status.textContent=error.message;status.className='min-h-5 text-sm text-error'}finally{$('prayer-settings-save').disabled=false}}
  function renderCalendar(){const year=calendarDate.getFullYear(),month=calendarDate.getMonth(),first=new Date(year,month,1),days=new Date(year,month+1,0).getDate(),leading=first.getDay();$('calendar-title').textContent=new Intl.DateTimeFormat([],{month:'long',year:'numeric'}).format(first);const grid=$('calendar-grid');grid.replaceChildren();for(let i=0;i<leading;i++)grid.appendChild(document.createElement('span'));for(let day=1;day<=days;day++){const value=year+'-'+String(month+1).padStart(2,'0')+'-'+String(day).padStart(2,'0'),button=document.createElement('button');button.type='button';button.textContent=day;button.className='h-9 rounded-full text-sm hover:bg-surface-container-high '+(value===selected?'bg-primary text-on-primary font-semibold':'');button.setAttribute('aria-label',new Intl.DateTimeFormat([],{dateStyle:'full'}).format(new Date(year,month,day,12)));if(value===selected)button.setAttribute('aria-current','date');button.onclick=()=>selectDate(value);grid.appendChild(button)}}
  function selectDate(value){selected=value;calendarDate=parseDate(value);const url=new URL(location.href);url.searchParams.set('date',value);history.pushState({},'',url);renderCalendar();loadTimes()}
  function renderTimes(){const date=parseDate(current.date),today=current.date===localDate(new Date(),current.timezone);$('prayer-date-title').textContent=new Intl.DateTimeFormat([],{weekday:'long',month:'long',day:'numeric',year:date.getFullYear()!==new Date().getFullYear()?'numeric':undefined}).format(date);$('prayer-location').textContent=current.location;$('prayer-method').textContent=current.method_label+' · '+(current.asr_school==='hanafi'?'Hanafi':'Standard')+' Asr';$('prayer-timezone').textContent=current.timezone;const list=$('prayer-timeline');list.replaceChildren();for(const [key,label,icon] of prayerNames){const row=document.createElement('li'),timestamp=current.times[key],next=current.next_prayer?.name===label;row.className='flex items-center gap-3 rounded-2xl px-3 py-4 '+(next?'bg-primary-container text-primary':'');const midnightDay=key==='midnight'&&localDate(new Date(timestamp*1000),current.timezone)!==current.date;row.innerHTML='<span class="flex h-9 w-9 items-center justify-center rounded-full '+(next?'bg-primary text-on-primary':'bg-surface-container-high text-on-surface-variant')+'"><i class="fa-solid '+icon+'"></i></span><span class="flex-1 font-medium">'+label+'</span><span class="text-right font-semibold">'+fmt(timestamp)+(midnightDay?'<small class="block text-[10px] font-normal text-on-surface-variant">next day</small>':'')+'</span>';list.appendChild(row)}show('next-prayer-card',today&&!!current.next_prayer);if(today&&current.next_prayer){$('next-prayer-name').textContent=current.next_prayer.name;$('next-prayer-time').textContent=fmt(current.next_prayer.time);updateCountdown();clearInterval(countdownTimer);countdownTimer=setInterval(updateCountdown,30000)}}
  function updateCountdown(){if(!current?.next_prayer)return;const seconds=Math.max(0,current.next_prayer.time-Math.floor(Date.now()/1000)),hours=Math.floor(seconds/3600),minutes=Math.ceil((seconds%3600)/60);$('next-prayer-countdown').textContent=hours?'In '+hours+'h '+minutes+'m':'In '+minutes+'m'}
  async function loadTimes(){show('prayer-loading',true);show('prayer-content',false);show('prayer-setup',false);try{current=await api('/api/prayer/times?date='+encodeURIComponent(selected));renderTimes();show('prayer-content',true);warning(settings?.sync_warning||'')}catch(error){if(error.message.startsWith('Set a prayer location')){show('prayer-setup',true)}else warning(error.message)}finally{show('prayer-loading',false)}}
  async function loadAnalytics(){try{const data=await api('/api/prayer/analytics'),total=data.totals.on_time+data.totals.off_time+data.totals.missed+data.totals.pending;if(!data.tracking_started_at&&total===0){show('analytics-empty',true);return}show('analytics-content',true);$('analytics-since').textContent=data.tracking_started_at?'Tracking since '+new Intl.DateTimeFormat([],{dateStyle:'medium'}).format(new Date(data.tracking_started_at*1000)):'';$('analytics-on-time').textContent=data.totals.on_time;$('analytics-off-time').textContent=data.totals.off_time;$('analytics-missed').textContent=data.totals.missed;$('analytics-rate').textContent=Math.round(data.completion_rate*100)+'%';const body=$('analytics-breakdown');body.replaceChildren();for(const item of data.by_prayer){const row=document.createElement('article');row.className='grid grid-cols-[1fr_repeat(3,minmax(3.5rem,auto))] items-center gap-3 px-5 py-4 text-sm';row.innerHTML='<h3 class="font-medium">'+item.name+'</h3><span class="text-center"><strong class="block text-lg text-primary">'+item.on_time+'</strong><small class="text-on-surface-variant">On time</small></span><span class="text-center"><strong class="block text-lg">'+item.off_time+'</strong><small class="text-on-surface-variant">Off time</small></span><span class="text-center"><strong class="block text-lg text-error">'+item.missed+'</strong><small class="text-on-surface-variant">Missed</small></span>';body.appendChild(row)}}catch(error){$('analytics-loading').textContent=error.message}finally{show('analytics-loading',false)}}
  $('prayer-settings-close').onclick=closeSettings;$('prayer-settings-cancel').onclick=closeSettings;$('prayer-settings-form').onsubmit=saveSettings;$('prayer-settings-modal').onclick=e=>{if(e.target===$('prayer-settings-modal'))closeSettings()};
  $('calendar-prev').onclick=()=>{calendarDate=new Date(calendarDate.getFullYear(),calendarDate.getMonth()-1,1,12);renderCalendar()};$('calendar-next').onclick=()=>{calendarDate=new Date(calendarDate.getFullYear(),calendarDate.getMonth()+1,1,12);renderCalendar()};$('prayer-today-button').onclick=()=>selectDate(localDate(new Date(),current?.timezone||settings?.timezone));
  addEventListener('popstate',()=>{selected=new URLSearchParams(location.search).get('date')||localDate(new Date());calendarDate=parseDate(selected);renderCalendar();loadTimes()});document.addEventListener('keydown',e=>{if(e.key==='Escape')closeSettings()});
  (async()=>{try{const tf=await fetch('/api/settings/time-format').then(r=>r.ok?r.json():null);time24=!!tf?.time_format_24h}catch{}await loadSettings();if(!requestedDate&&settings?.timezone){selected=localDate(new Date(),settings.timezone);calendarDate=parseDate(selected)}if(analyticsView){show('prayer-schedule',false);show('prayer-analytics',true);loadAnalytics()}else{renderCalendar();loadTimes();if(location.hash==='#calendar')$('calendar').scrollIntoView({behavior:'smooth'})}})();
})();
</script>`
}
