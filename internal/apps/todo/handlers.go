package todo

import (
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

func handlePage(w http.ResponseWriter, r *http.Request) {
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
	collection := r.URL.Query().Get("collection")
	trash := r.URL.Query().Get("trash") == "1"
	sidebar := webui.BuildSidebar(webui.SidebarConfig{
		StateKey:  "todo",
		AriaLabel: "Todo navigation",
		Items: []webui.SidebarItem{
			{ID: "todo-collection-important", Label: "Important", Icon: "fa-solid fa-star", Href: "/todo?collection=important", Active: !trash && collection == "important"},
			{ID: "todo-collection-today", Label: "Today", Icon: "fa-solid fa-calendar-day", Href: "/todo?collection=today", Active: !trash && collection == "today"},
			{ID: "todo-collection-week", Label: "Week", Icon: "fa-solid fa-calendar-week", Href: "/todo?collection=week", Active: !trash && collection == "week"},
			{ID: "todo-collection-all", Label: "All", Icon: "fa-solid fa-list-check", Href: "/todo?collection=all", Active: !trash && (collection == "" || collection == "all")},
			{ID: "todo-collection-current", Label: "Current", Icon: "fa-solid fa-clock", Href: "/todo?collection=current", Active: !trash && collection == "current"},
			{ID: "todo-collection-completed", Label: "Completed", Icon: "fa-solid fa-check", Href: "/todo?collection=completed", Active: !trash && collection == "completed"},
			{ID: "todo-collection-trash", Label: "Trash", Icon: "fa-solid fa-trash", Href: "/todo?trash=1", Active: trash},
		},
		Sections: []webui.SidebarSection{{
			Label: "Task lists",
			NavID: "task-lists",
			Actions: []webui.SidebarAction{{
				ID: "new-list", Label: "New list", Icon: "fa-solid fa-plus",
			}},
		}},
		SettingsHref:  "#todo-settings",
		SettingsLabel: "Todo settings",
	})
	content, _ := webui.ExternalizeLastScript(todoHTML(), "/todo/assets/app.js")
	return webui.RenderRequest(w, r, webui.PageData{
		Title:      "Tasks — Dar",
		CurrentApp: "todo",
		AppBar:     server.AppBarHTML(r),
		UserMenu:   webui.UserMenuHTML(displayName),
		Sidebar:    sidebar,
		Content:    content,
	})
}

func handleTodoAsset(w http.ResponseWriter, r *http.Request) {
	_, script := webui.ExternalizeLastScript(todoHTML(), "/todo/assets/app.js")
	webui.ServeAppJavaScript(w, r, "app.js", script)
}

func todoHTML() string {
	return `
<div id="todo-app" data-dar-shell class="flex h-full min-h-0 overflow-hidden bg-surface">
  <div data-dar-master-pane class="min-w-0 flex-1 overflow-y-auto overscroll-contain bg-transparent p-4 [scrollbar-gutter:stable] md:p-6">
    <div class="w-full">
      <header class="mb-4 flex items-center justify-between gap-3">
        <div data-dar-sidebar-toggle-host class="flex min-w-0 flex-1 items-start gap-3"><div class="min-w-0"><h1 id="task-heading" class="truncate text-2xl font-semibold">All tasks</h1><p id="task-subtitle" class="text-sm text-on-surface-variant"></p></div></div>
        <button id="manage-list" class="hidden h-10 rounded-full bg-surface-container px-4 hover:bg-surface-container-high" aria-label="Manage current list"><i class="fa-solid fa-gear mr-2"></i>List</button>
      </header>
      <section class="mb-3 flex gap-2">
        <input id="quick-title" aria-label="Add a task" class="h-11 min-w-0 flex-1 rounded-full bg-surface-container px-4 outline-none focus:ring-2 focus:ring-primary/30" placeholder="Add a task to this list…">
        <button id="sort-tasks" type="button" class="h-11 w-11 shrink-0 rounded-full bg-surface-container hover:bg-surface-container-high" aria-label="Sort tasks by priority" title="Sort tasks by priority"><i class="fa-solid fa-arrow-down-wide-short"></i></button>
        <button id="quick-add" type="button" class="h-11 w-11 shrink-0 rounded-full bg-primary text-on-primary hover:opacity-90" aria-label="Add task"><i class="fa-solid fa-plus"></i></button>
      </section>
      <section id="list-editor" class="mb-4 hidden space-y-3 rounded-2xl bg-surface-container p-4" aria-label="List settings">
        <div class="flex justify-between"><h2 class="font-semibold">List settings</h2><button type="button" id="close-list-editor" class="h-8 w-8 rounded-full hover:bg-surface-container-high" aria-label="Close list settings"><i class="fa-solid fa-xmark"></i></button></div>
        <label class="block text-sm">List name<input id="list-name" class="mt-1 h-10 w-full rounded-xl bg-surface px-3 outline-none" aria-label="List name"></label>
        <div class="flex gap-2"><button id="save-list" type="button" class="h-10 rounded-full bg-primary px-5 text-on-primary">Save list</button><button id="delete-list" type="button" class="h-10 rounded-full px-5 text-error hover:bg-error-container">Move list to trash</button></div>
        <div class="border-t border-outline-variant/30 pt-2"><h3 class="mb-2 font-medium">Sharing</h3><div class="grid gap-2 sm:grid-cols-[minmax(0,1fr)_8rem_auto]"><input id="share-email" type="email" class="h-10 rounded-xl bg-surface px-3 outline-none" aria-label="Share with email" placeholder="person@example.com"><select id="share-permission" class="h-10 rounded-xl bg-surface px-3" aria-label="Share permission"><option value="write">Can edit</option><option value="read">Can view</option></select><button id="add-share" type="button" class="h-10 rounded-full bg-secondary-container px-5">Share</button></div><div id="share-list" class="mt-2 space-y-1 text-sm"></div></div>
      </section>
      <section id="task-list" class="space-y-1" aria-live="polite"></section>
      <div id="empty-state" class="hidden py-16 text-center text-on-surface-variant"><i class="fa-solid fa-check-double mb-3 block text-4xl"></i><p>All clear! Add a task above.</p></div>
    </div>
  </div>

  <section id="detail-pane" data-dar-detail-pane class="hidden bg-transparent lg:w-80 lg:min-w-72 lg:shrink-0 lg:border-l lg:border-outline-variant" role="region" aria-label="Task details">
    <div id="detail-surface" class="flex h-full w-full flex-col overflow-hidden bg-transparent">
      <div id="detail-pane-resizer" role="separator" aria-orientation="vertical" aria-label="Resize task details" tabindex="0" class="absolute inset-y-0 left-0 z-20 hidden w-2 cursor-col-resize touch-none hover:bg-primary/20 focus:bg-primary/30 focus:outline-none lg:block"></div>
      <form id="task-editor" class="hidden min-h-0 flex-1 flex-col" aria-label="Task details">
        <header class="flex items-center gap-3 border-b border-outline-variant bg-transparent px-5 py-4"><button type="button" data-dar-detail-back id="close-editor" class="h-9 w-9 rounded-full hover:bg-surface-container-high" aria-label="Back to tasks"><i class="fa-solid fa-arrow-left lg:hidden"></i><i class="fa-solid fa-xmark hidden lg:inline"></i></button><h2 class="flex-1 text-xl font-semibold">Task details</h2></header>
        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-5">
          <input id="editor-title" required maxlength="512" class="h-11 w-full rounded-xl bg-surface px-3 outline-none focus:ring-2 focus:ring-primary/30" placeholder="Task title" aria-label="Task title">
          <textarea id="editor-notes" class="min-h-24 w-full rounded-xl bg-surface p-3 outline-none focus:ring-2 focus:ring-primary/30" placeholder="Notes" aria-label="Notes"></textarea>
          <label class="block text-sm">List<select id="editor-list" class="mt-1 h-10 w-full rounded-xl bg-surface px-3"></select></label>
          <label class="block text-sm">Due date<input id="editor-due" type="datetime-local" class="mt-1 h-10 w-full rounded-xl bg-surface px-3"></label>
          <label class="block text-sm">Priority<select id="editor-priority" class="mt-1 h-10 w-full rounded-xl bg-surface px-3"><option value="0">None</option><option value="1">High</option><option value="5">Medium</option><option value="9">Low</option></select></label>
          <label class="block text-sm">Repeat<select id="editor-repeat" class="mt-1 h-10 w-full rounded-xl bg-surface px-3"><option value="">Does not repeat</option><option value="FREQ=DAILY">Every day</option><option value="FREQ=WEEKLY">Every week</option><option value="FREQ=MONTHLY">Every month</option><option value="FREQ=YEARLY">Every year</option></select></label>
          <label class="inline-flex items-center gap-2 text-sm"><input id="editor-habit" type="checkbox" class="h-4 w-4 accent-primary"> This task is a habit <span class="text-on-surface-variant">(tracks a streak)</span></label>
        </div>
        <footer class="flex gap-2 border-t border-outline-variant p-4"><button class="h-10 rounded-full bg-primary px-5 text-on-primary" type="submit">Save task</button><button id="delete-task" class="hidden h-10 rounded-full px-5 text-error hover:bg-error-container" type="button">Delete</button></footer>
      </form>
    </div>
  </section>
</div>

<div id="todo-settings-modal" class="fixed inset-0 z-[70] hidden items-center justify-center bg-scrim/40 p-4" role="dialog" aria-modal="true" aria-labelledby="todo-settings-title" onclick="if(event.target===this)closeSettings()">
  <div class="flex max-h-[90vh] w-full max-w-lg flex-col overflow-hidden rounded-3xl bg-surface shadow-2xl">
    <header class="flex items-center gap-3 border-b border-outline-variant px-5 py-4"><h2 id="todo-settings-title" class="flex-1 text-xl font-semibold">Todo settings</h2><span id="todo-settings-status" class="text-xs text-on-surface-variant" aria-live="polite"></span><button type="button" id="close-settings" class="h-9 w-9 rounded-full hover:bg-surface-container-high" aria-label="Close Todo settings"><i class="fa-solid fa-xmark"></i></button></header>
    <div class="space-y-5 overflow-y-auto p-5">
      <label class="block text-sm">Default list<select id="todo-setting-default-list" class="mt-1 h-10 w-full rounded-xl bg-surface-container px-3"></select></label>
      <section><h3 class="mb-3 font-semibold">Visibility of Smart Collections</h3><div id="todo-smart-settings" class="space-y-2"></div></section>
    </div>
    <footer class="flex justify-end gap-2 border-t border-outline-variant p-4"><button id="todo-settings-cancel" type="button" class="h-10 rounded-full px-5 hover:bg-surface-container-high">Cancel</button><button id="todo-settings-save" type="button" class="h-10 rounded-full bg-primary px-5 text-on-primary">Save</button></footer>
  </div>
</div>

<script>
(() => {
  const apiBase='/api/todo';
  const pageParams=new URLSearchParams(location.search);
  const smartNames=['important','today','week','all','current','completed'];
  let lists=[], tasks=[];
  let preferences={default_list_id:'',smart_collections:{important:'automatic',today:'automatic',week:'automatic',all:'automatic',current:'automatic',completed:'automatic'},sort_mode:'due',collection_counts:{}};
  let current={collection:pageParams.get('collection')||'',list:'',trash:pageParams.get('trash')==='1'};
  let editing=null, sort='due', initialSelection=true, detailResizeActive=false;
  const $=id=>document.getElementById(id);
  const esc=s=>{const d=document.createElement('div');d.textContent=s||'';return d.innerHTML};

  async function api(method,path,body){const o={method,headers:{Accept:'application/json'}};if(body!==undefined){o.headers['Content-Type']='application/json';o.body=JSON.stringify(body)}const r=await fetch(apiBase+path,o);if(!r.ok){const e=await r.json().catch(()=>({error:'Request failed'}));throw new Error(e.error||'Request failed')}return r.status===204?null:r.json()}
  function settingsStatus(message,isError){const status=$('todo-settings-status');status.textContent=message||'';status.className='text-xs '+(isError?'text-error':'text-on-surface-variant')}
  function updateSmartVisibility(){smartNames.forEach(name=>{const mode=(preferences.smart_collections||{})[name]||'automatic';const count=(preferences.collection_counts||{})[name]||0;const hidden=mode==='hidden'||(mode==='automatic'&&count===0);const item=$('todo-collection-'+name);if(item)item.classList.toggle('hidden',hidden)})}
  function updateSidebarSelection(){smartNames.forEach(name=>{const item=$('todo-collection-'+name);if(!item)return;const active=!current.list&&!current.trash&&(current.collection===name||(name==='all'&&(current.collection===''||current.collection==='all')));item.classList.toggle('bg-primary-container',active);item.classList.toggle('text-primary',active);item.classList.toggle('font-medium',active)});const trash=$('todo-collection-trash');if(trash){trash.classList.toggle('bg-primary-container',current.trash);trash.classList.toggle('text-primary',current.trash);trash.classList.toggle('font-medium',current.trash)}}
  function updateSortButton(){const button=$('sort-tasks');const next=sort==='due'?'priority':'due date';button.setAttribute('aria-label','Sort tasks by '+next);button.title='Sort tasks by '+next}
  async function loadPreferences(){preferences=Object.assign(preferences,await api('GET','/preferences'));sort=preferences.sort_mode||'due';renderPreferenceControls();updateSmartVisibility();updateSortButton()}
  function renderPreferenceControls(){const defaultSelect=$('todo-setting-default-list');defaultSelect.innerHTML='<option value="">First available list</option>'+lists.map(l=>'<option value="'+esc(l.id)+'">'+esc(l.name)+'</option>').join('');defaultSelect.value=preferences.default_list_id||'';$('todo-smart-settings').innerHTML=smartNames.map(name=>'<label class="flex items-center justify-between gap-3 rounded-2xl bg-surface-container-low p-3 text-sm"><span>'+name[0].toUpperCase()+name.slice(1)+'</span><select data-smart="'+name+'" class="h-9 rounded-xl bg-surface-container px-2 text-sm"><option value="hidden">Hidden</option><option value="visible">Visible</option><option value="automatic">Automatic</option></select></label>').join('');document.querySelectorAll('[data-smart]').forEach(select=>{select.value=(preferences.smart_collections||{})[select.dataset.smart]||'automatic'})}
  async function savePreferences(){settingsStatus('Saving…');try{const saved=await api('PUT','/preferences',{default_list_id:preferences.default_list_id||'',smart_collections:preferences.smart_collections,sort_mode:sort});preferences=Object.assign(preferences,saved);const fresh=await api('GET','/preferences');preferences=Object.assign(preferences,fresh);updateSmartVisibility();settingsStatus('Saved')}catch(e){settingsStatus(e.message||String(e),true);renderPreferenceControls();updateSmartVisibility()}}
  window.openSettings=function(){renderPreferenceControls();settingsStatus('');const modal=$('todo-settings-modal');modal.classList.remove('hidden');modal.classList.add('flex');$('todo-setting-default-list').focus()};
  window.closeSettings=function(){const modal=$('todo-settings-modal');modal.classList.add('hidden');modal.classList.remove('flex')};
  async function saveSettingsDraft(){const smart={};document.querySelectorAll('[data-smart]').forEach(select=>smart[select.dataset.smart]=select.value);settingsStatus('Saving…');try{const saved=await api('PUT','/preferences',{default_list_id:$('todo-setting-default-list').value,smart_collections:smart,sort_mode:sort});preferences=Object.assign(preferences,saved);updateSmartVisibility();closeSettings();DarUI.toast('Todo settings saved')}catch(e){settingsStatus(e.message||String(e),true)}}

  function fmt(ts){return ts?new Intl.DateTimeFormat(undefined,{month:'short',day:'numeric',hour:'numeric',minute:'2-digit'}).format(new Date(ts*1000)):''}
  function listName(){if(current.collection&&current.collection!=='all')return current.collection[0].toUpperCase()+current.collection.slice(1);const l=lists.find(x=>x.id===current.list);return l?l.name:'All tasks'}
  async function refresh(){lists=(await api('GET','/lists')).lists||[];if(initialSelection){if(!current.trash&&!current.collection&&preferences.default_list_id&&lists.some(l=>l.id===preferences.default_list_id))current.list=preferences.default_list_id;initialSelection=false}const q=new URLSearchParams();if(current.list)q.set('list_id',current.list);if(current.collection)q.set('collection',current.collection||'all');if(current.trash)q.set('trash','1');tasks=(await api('GET','/tasks?'+q)).tasks||[];await loadPreferences();render()}
  function render(){const activeList=lists.find(l=>l.id===current.list);$('task-heading').textContent=current.trash?'Trash':listName();$('task-subtitle').textContent=tasks.length===1?'1 task':tasks.length+' tasks';$('manage-list').classList.toggle('hidden',!activeList||current.trash);updateSidebarSelection();const nav=$('task-lists');nav.innerHTML=lists.map(l=>'<button type="button" data-list="'+esc(l.id)+'" class="flex h-10 w-full items-center gap-3 rounded-full px-4 text-left text-sm '+(current.list===l.id?'bg-primary-container font-medium text-primary':'hover:bg-surface-container-highest')+'"><span class="inline-block h-2 w-2 shrink-0 rounded-full bg-primary"></span><span class="min-w-0 flex-1 truncate">'+esc(l.name)+'</span>'+(l.permission!=='owner'?'<span class="text-xs text-on-surface-variant">shared</span>':'')+'</button>').join('');nav.querySelectorAll('button').forEach(b=>b.onclick=()=>{closeTaskEditor();current.list=b.dataset.list;current.collection='';current.trash=false;refresh()});$('editor-list').innerHTML=lists.filter(l=>l.permission!=='read').map(l=>'<option value="'+esc(l.id)+'">'+esc(l.name)+'</option>').join('');const body=$('task-list');body.innerHTML='';const ordered=[...tasks].sort((a,b)=>sort==='priority'?b.priority-a.priority:(a.due_at||Infinity)-(b.due_at||Infinity));ordered.forEach(t=>{const row=document.createElement('article');row.id='task-'+t.id;row.className='group flex items-center gap-3 rounded-2xl p-3 transition-colors hover:bg-surface-container';const destructive=current.trash?'Restore':'Delete';row.innerHTML='<input aria-label="Complete '+esc(t.title)+'" type="checkbox" '+(t.status==='COMPLETED'?'checked':'')+' class="h-5 w-5 accent-primary"><button class="star h-7 w-7 rounded-full '+(t.starred?'text-primary':'text-on-surface-variant')+'" aria-label="Toggle important"><i class="fa-'+(t.starred?'solid':'regular')+' fa-star"></i></button><button class="detail min-w-0 flex-1 text-left"><span class="block text-sm font-medium '+(t.status==='COMPLETED'?'line-through text-on-surface-variant':'')+'">'+esc(t.title)+'</span><span class="mt-1 block text-xs text-on-surface-variant">'+(t.due_at?'<i class="fa-regular fa-calendar mr-1"></i>'+fmt(t.due_at):'')+(t.is_habit?' <i class="fa-solid fa-fire ml-2 text-primary"></i> '+(t.streak||0)+' streak':'')+(t.priority?' <i class="fa-solid fa-flag ml-2"></i>':'')+'</span></button><button class="delete h-8 w-8 rounded-full text-error opacity-0 hover:bg-error-container group-hover:opacity-100" aria-label="'+destructive+' '+esc(t.title)+'"><i class="fa-solid fa-'+(current.trash?'arrow-rotate-left':'trash')+'"></i></button>';row.querySelector('input').onchange=async()=>{try{if(row.querySelector('input').checked)await api('POST','/tasks/'+t.id+'?action=complete');else await api('PATCH','/tasks/'+t.id,{...t,status:'NEEDS-ACTION',percent_complete:0})}catch(e){DarUI.toast(e.message,{tone:'error'})}refresh()};row.querySelector('.star').onclick=async()=>{await api('PATCH','/tasks/'+t.id,{...t,starred:!t.starred});refresh()};row.querySelector('.detail').onclick=()=>openEditor(t);row.querySelector('.delete').onclick=async()=>{try{if(current.trash){await api('POST','/tasks/'+t.id+'?action=restore')}else if(await DarUI.confirmDialog({title:'Delete task?',message:'This task will be moved to Trash.',confirmLabel:'Delete',tone:'danger'}))await api('DELETE','/tasks/'+t.id);closeTaskEditor();refresh()}catch(e){DarUI.toast(e.message,{tone:'error'})}};body.appendChild(row)});$('empty-state').classList.toggle('hidden',tasks.length!==0);updateSmartVisibility()}

  function openEditor(t){editing=t||null;if(DarUI.shell.isCompact()&&!history.state?.darDetail)history.pushState({darDetail:'todo'},'',t?'#task-'+encodeURIComponent(t.id):'#task-new');$('task-editor').classList.remove('hidden');$('task-editor').classList.add('flex');$('editor-title').value=t?t.title:'';$('editor-notes').value=t?t.notes||'':'';$('editor-list').value=t?t.list_id:(current.list||lists[0]?.id||'');$('editor-due').value=t&&t.due_at?new Date(t.due_at*1000).toISOString().slice(0,16):'';$('editor-priority').value=t?t.priority:0;$('editor-repeat').value=t?t.rrule||'':'';$('editor-habit').checked=!!(t&&t.is_habit);$('delete-task').classList.toggle('hidden',!t);DarUI.shell.openDetail($('detail-pane'),{master:document.querySelector('[data-dar-master-pane]')});$('editor-title').focus()}
  window.closeTaskEditor=function(fromHistory){if(!fromHistory&&DarUI.shell.isCompact()&&history.state?.darDirect==='todo'){history.replaceState({},'',location.pathname+location.search)}else if(!fromHistory&&DarUI.shell.isCompact()&&history.state?.darDetail){history.back();return}editing=null;$('task-editor').classList.add('hidden');$('task-editor').classList.remove('flex');DarUI.shell.closeDetail($('detail-pane'),{master:document.querySelector('[data-dar-master-pane]')})};
  async function openListEditor(){const l=lists.find(x=>x.id===current.list);if(!l)return;$('list-editor').classList.remove('hidden');$('list-name').value=l.name;await loadShares()}
  async function loadShares(){const d=await api('GET','/lists/'+current.list+'/share');$('share-list').innerHTML=(d.shares||[]).map(s=>'<div class="flex items-center justify-between gap-2 rounded-lg bg-surface px-2 py-1"><span>'+esc(s.email)+' <span class="text-on-surface-variant">'+esc(s.permission)+'</span></span><button data-user="'+esc(s.user_id)+'" class="remove-share text-error" aria-label="Remove share '+esc(s.email)+'"><i class="fa-solid fa-xmark"></i></button></div>').join('')||'<p class="text-on-surface-variant">Not shared yet.</p>';document.querySelectorAll('.remove-share').forEach(b=>b.onclick=async()=>{await api('DELETE','/lists/'+current.list+'/share?user_id='+encodeURIComponent(b.dataset.user));loadShares()})}
  async function quickAdd(){const title=$('quick-title').value.trim();if(!title)return;try{await api('POST','/tasks',{title,list_id:current.list||lists[0]?.id||''});$('quick-title').value='';await refresh()}catch(e){DarUI.toast(e.message,{tone:'error'})}}

  $('close-settings').onclick=closeSettings;
  $('todo-settings-cancel').onclick=closeSettings;
  $('todo-settings-save').onclick=saveSettingsDraft;
  $('close-editor').onclick=closeTaskEditor;
  $('quick-add').onclick=quickAdd;
  $('quick-title').onkeydown=e=>{if(e.key==='Enter')quickAdd()};
  $('task-editor').onsubmit=async e=>{e.preventDefault();const due=$('editor-due').value;const task={title:$('editor-title').value,notes:$('editor-notes').value,list_id:$('editor-list').value,due_at:due?Math.floor(new Date(due).getTime()/1000):0,priority:Number($('editor-priority').value),rrule:$('editor-repeat').value,is_habit:$('editor-habit').checked,status:editing?editing.status:'NEEDS-ACTION',percent_complete:editing?editing.percent_complete:0,starred:editing?editing.starred:false,tags:editing?editing.tags:[]};try{if(editing)await api('PATCH','/tasks/'+editing.id,{...editing,...task});else await api('POST','/tasks',task);closeTaskEditor();await refresh()}catch(err){DarUI.toast(err.message,{tone:'error'})}};
  $('delete-task').onclick=async()=>{if(editing&&await DarUI.confirmDialog({title:'Delete task?',message:'This task will be moved to Trash.',confirmLabel:'Delete',tone:'danger'})){await api('DELETE','/tasks/'+editing.id);closeTaskEditor();refresh()}};
  $('new-list').onclick=async()=>{const name=await DarUI.inputDialog({title:'New list',label:'List name',confirmLabel:'Create'});if(name){try{const l=await api('POST','/lists',{name});current.list=l.id;current.collection='';refresh()}catch(e){DarUI.toast(e.message,{tone:'error'})}}};
  $('manage-list').onclick=()=>openListEditor();
  $('close-list-editor').onclick=()=>$('list-editor').classList.add('hidden');
  $('save-list').onclick=async()=>{try{await api('PATCH','/lists/'+current.list,{name:$('list-name').value});await refresh()}catch(e){DarUI.toast(e.message,{tone:'error'})}};
  $('delete-list').onclick=async()=>{if(await DarUI.confirmDialog({title:'Delete list?',message:'This list and its tasks will be moved to Trash.',confirmLabel:'Delete',tone:'danger'})){try{await api('DELETE','/lists/'+current.list);current.list='';$('list-editor').classList.add('hidden');refresh()}catch(e){DarUI.toast(e.message,{tone:'error'})}}};
  $('add-share').onclick=async()=>{const email=$('share-email').value.trim();if(!email)return;try{await api('POST','/lists/'+current.list+'/share',{email,permission:$('share-permission').value});$('share-email').value='';await loadShares()}catch(e){DarUI.toast(e.message,{tone:'error'})}};
  $('sort-tasks').onclick=async()=>{sort=sort==='due'?'priority':'due';preferences.sort_mode=sort;render();updateSortButton();await savePreferences()};

  const detailPane=$('detail-pane'), detailPaneResizer=$('detail-pane-resizer');
  function resizeDetailPane(clientX){const minWidth=288;const maxWidth=Math.min(720,Math.max(minWidth,Math.floor(window.innerWidth*.7)));detailPane.style.width=Math.max(minWidth,Math.min(maxWidth,window.innerWidth-clientX))+'px'}
  detailPaneResizer.addEventListener('pointerdown',e=>{if(e.button!==0||window.innerWidth<1024)return;e.preventDefault();detailResizeActive=true;detailPaneResizer.setPointerCapture(e.pointerId);detailPane.classList.add('select-none');resizeDetailPane(e.clientX)});
  detailPaneResizer.addEventListener('pointermove',e=>{if(detailResizeActive)resizeDetailPane(e.clientX)});
  detailPaneResizer.addEventListener('pointerup',()=>{detailResizeActive=false;detailPane.classList.remove('select-none')});
  detailPaneResizer.addEventListener('keydown',e=>{if(window.innerWidth<1024||!['ArrowLeft','ArrowRight'].includes(e.key))return;e.preventDefault();const width=detailPane.getBoundingClientRect().width+(e.key==='ArrowLeft'?16:-16);detailPane.style.width=Math.max(288,Math.min(720,width))+'px'});
  window.addEventListener('resize',()=>{if(window.innerWidth<1024)detailPane.style.width=''});
  window.addEventListener('popstate',()=>{if(DarUI.shell.isCompact()&&!history.state?.darDetail)closeTaskEditor(true)});
  document.addEventListener('keydown',e=>{if(e.key!=='Escape')return;if(!$('todo-settings-modal').classList.contains('hidden'))closeSettings();else if(!$('task-editor').classList.contains('hidden'))closeTaskEditor();else $('list-editor').classList.add('hidden')});

  async function openSearchTask(){if(!location.hash.startsWith('#task-'))return;const id=decodeURIComponent(location.hash.slice(6));const task=await api('GET','/tasks/'+encodeURIComponent(id));current={collection:'',list:task.list_id,trash:false};await refresh();if(DarUI.shell.isCompact())history.replaceState({darDetail:'todo',darDirect:'todo'},'',location.href);openEditor(task)}
  loadPreferences().then(refresh).then(openSearchTask).catch(e=>{$('task-list').innerHTML='<p class="text-error">'+esc(e.message)+'</p>'});
})();
</script>`
}
