package files

import (
	"html/template"
	"io"
	"net/http"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

type App struct{}

func (a App) Name() string                     { return "files" }
func (a App) DisplayName() string              { return "Files" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-folder"></i>` }
func (a App) Description() string              { return "File browser — upload, download, manage" }
func (a App) AppBarOrder() int                 { return 2 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/files", Handler: handlePage},
		{Method: "GET", Pattern: "/files/*", Handler: handlePage},
	}
}

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil { http.Redirect(w, r, "/login", http.StatusSeeOther); return }
	renderPage(w, r, user.DisplayName)
}

func renderPage(w io.Writer, r *http.Request, displayName string) error {
	urlPath := strings.TrimPrefix(r.URL.Path, "/files")
	filter := r.URL.Query().Get("filter")
	activeAll := urlPath == "" && filter == ""
	activeFiles := urlPath == "/files"
	activeTrash := urlPath == "/trash"
	activeFav := filter == "favorites"

	return webui.Render(w, webui.PageData{
		Title: "Files — Dar", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "All files", Icon: "fa-solid fa-folder", Href: "/files", Active: activeAll},
			{Label: "Files", Icon: "fa-solid fa-file", Href: "/files/files", Active: activeFiles},
			{Label: "Favorites", Icon: "fa-solid fa-star", Href: "/files?filter=favorites", Active: activeFav},
			{Label: "Trash", Icon: "fa-solid fa-trash", Href: "/files/trash", Active: activeTrash},
		}, "/settings"),
		Content: template.HTML(filesHTML()),
	})
}

func filesHTML() string {
	return `
<div id="files-app" class="h-[calc(100vh-120px)] flex flex-col -m-4 md:-m-6">
	<!-- Toolbar -->
	<div class="flex items-center gap-3 px-4 md:px-6 py-3 border-b border-outline-variant flex-wrap shrink-0">
		<button onclick="newFolder()" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest transition-colors inline-flex items-center gap-2">
			<i class="fa-solid fa-folder-plus"></i> New folder</button>
		<button onclick="newTextFile()" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest transition-colors inline-flex items-center gap-2">
			<i class="fa-solid fa-file-lines"></i> New file</button>
		<label class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-upload"></i> Upload
			<input type="file" id="upload-input" onchange="uploadFiles()" class="hidden" multiple>
		</label>
		<button onclick="toggleView()" id="view-toggle" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center gap-2">
			<i id="view-icon" class="fa-solid fa-list"></i> <span id="view-label">List</span></button>
		<div class="flex-1"></div>
		<span id="selected-count" class="text-sm text-primary font-medium hidden"></span>
		<button id="bulk-delete-btn" onclick="bulkDelete()" class="hidden h-9 px-4 rounded-full bg-error-container text-error text-sm font-medium border-0 cursor-pointer hover:opacity-90 items-center gap-1"><i class="fa-solid fa-trash"></i> Delete</button>
		<button id="bulk-move-btn" onclick="bulkMove()" class="hidden h-9 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest items-center gap-1"><i class="fa-solid fa-arrows-up-down-left-right"></i> Move</button>
		<div class="relative">
			<input id="search-input" type="text" placeholder="Search..." onkeydown="if(event.key==='Enter')searchFiles()"
				class="h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 w-40 md:w-56 placeholder:text-on-surface-variant">
			<button onclick="searchFiles()" class="absolute right-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant"><i class="fa-solid fa-search"></i></button>
		</div>
	</div>

	</div>
		<button onclick="showTagManager()" class="h-10 px-3 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center gap-1">
			<i class="fa-solid fa-tags"></i></button>
	</div>

	<!-- Breadcrumbs -->
	<nav id="breadcrumbs" class="flex items-center gap-1 text-sm px-4 md:px-6 py-2 border-b border-outline-variant flex-wrap shrink-0"></nav>

	<!-- File list (fills remaining height) -->
	<div id="file-list" class="flex-1 overflow-y-auto">
		<div class="text-center text-on-surface-variant py-12">Loading...</div>
	</div>

	<!-- Drag-drop overlay -->
	<div id="drop-zone" class="hidden fixed inset-0 bg-primary/10 z-50 flex items-center justify-center border-4 border-dashed border-primary rounded-2xl m-4">
		<div class="text-center"><i class="fa-solid fa-cloud-arrow-up text-5xl text-primary mb-3 block"></i><p class="text-lg font-semibold text-primary">Drop files to upload</p></div>
	</div>

	<!-- Preview panel -->
	<div id="preview-panel" class="hidden fixed inset-y-0 right-0 w-full sm:w-96 bg-surface border-l border-outline-variant z-40 shadow-2 flex flex-col" onclick="event.stopPropagation()">
		<div class="flex items-center gap-2 p-4 border-b border-outline-variant">
			<h3 id="preview-title" class="text-sm font-semibold flex-1 truncate"></h3>
			<button onclick="closePreview()" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:bg-surface-container-highest">&times;</button>
		</div>
		<div id="preview-content" class="flex-1 overflow-y-auto p-4"></div>
	</div>

	<!-- Image lightbox -->
	<div id="img-lightbox" class="hidden fixed inset-0 bg-black/90 z-50 flex items-center justify-center" onclick="closeLightbox()">
		<button class="absolute top-4 left-4 text-white text-sm bg-white/20 rounded-full px-4 py-2 border-0 cursor-pointer hover:bg-white/30" onclick="event.stopPropagation();closeLightbox()">Close</button>
		<button id="lightbox-prev" class="absolute left-4 top-1/2 -translate-y-1/2 text-white text-3xl bg-transparent border-0 cursor-pointer hover:opacity-70" onclick="event.stopPropagation();navLightbox(-1)"><i class="fa-solid fa-chevron-left"></i></button>
		<img id="lightbox-img" class="max-w-[90vw] max-h-[85vh] object-contain rounded-lg" src="" onclick="event.stopPropagation()">
		<button id="lightbox-next" class="absolute right-4 top-1/2 -translate-y-1/2 text-white text-3xl bg-transparent border-0 cursor-pointer hover:opacity-70" onclick="event.stopPropagation();navLightbox(1)"><i class="fa-solid fa-chevron-right"></i></button>
	</div>

	<!-- Tag manager modal -->
	<div id="tag-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)document.getElementById('tag-modal').classList.add('hidden')">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-sm shadow-2">
			<h2 class="text-lg font-semibold mb-4">Tags</h2>
			<div id="tag-list" class="space-y-2 mb-4 max-h-64 overflow-y-auto"></div>
			<div class="flex gap-2">
				<input id="new-tag-name" placeholder="Tag name" class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
				<input id="new-tag-color" type="color" value="#6366f1" class="w-10 h-10 rounded-full border-0 cursor-pointer p-0">
				<button onclick="createTag()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90"><i class="fa-solid fa-plus"></i></button>
			</div>
		</div>
	</div>

	<!-- Rename modal -->
	<div id="rename-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeRename()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-sm shadow-2">
			<h2 class="text-lg font-semibold mb-4">Rename</h2>
			<input id="rename-input" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30" onkeydown="if(event.key==='Enter')doRename()">
			<div class="flex gap-2 justify-end mt-4">
				<button onclick="closeRename()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer">Cancel</button>
				<button onclick="doRename()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Rename</button>
			</div>
		</div>
	</div>

	<!-- Move modal -->
	<div id="move-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeMove()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-sm shadow-2">
			<h2 class="text-lg font-semibold mb-4">Move to...</h2>
			<select id="move-target" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none">
				<option value="/files">Personal Files</option>
				<option value="/">Home</option>
			</select>
			<div class="flex gap-2 justify-end mt-4">
				<button onclick="closeMove()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer">Cancel</button>
				<button onclick="doMove()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Move</button>
			</div>
		</div>
	</div>
</div>

<script>
const apiBase="/api/files";
let currentPath="/",selectedPaths=new Set(),renameTarget=null,moveTargets=[],viewMode="list",lightboxFiles=[],lightboxIdx=0;

// Parse URL to determine initial path
function init(){
	let fp=window.location.pathname.replace(/\/files\/?/,"");
	let q=new URLSearchParams(window.location.search);
	let filter=q.get("filter");
	if(filter==="favorites"){currentPath="/";window._filter="favorites"}
	else if(fp==="trash")currentPath="/trash";
	else if(fp)currentPath="/files/"+fp;
	else if(window.location.pathname==="/files")currentPath="/";
	else currentPath="/files";
	loadTagMap();loadFiles();
	setupDragDrop();
}
function loadTagMap(){api("tags",{}).then(d=>{window._tagMap=d.tags||{}})

async function api(method,params){
	let url=apiBase+"/"+method+"?"+new URLSearchParams(params);
	let r=await fetch(url,{method:method==="upload"?"POST":(params.src?"POST":"GET")});
	return r.json();
}
async function apiPost(path,params){
	let r=await fetch(apiBase+path+"?"+new URLSearchParams(params),{method:"POST"});
	return r.json();
}

function loadFiles(){
	let params={path:currentPath};
	if(window._filter)params.filter=window._filter;if(window._tagFilter)params.tag=window._tagFilter;
	api("list",params).then(d=>renderFiles(d.files||[],d.path));
}

function renderBreadcrumbs(){
	let parts=currentPath.replace("/files","Files").split("/").filter(p=>p);
	let html='<a href="#" onclick="navTo(\'/\')" class="text-primary no-underline hover:underline">Home</a>';
	let acc="";
	for(let i=0;i<parts.length;i++){
		let isFiles=parts[i]==="Files";
		acc+=isFiles?"/files":"/"+parts[i];
		html+=' <span class="text-on-surface-variant">/</span> ';
		if(i===parts.length-1)html+='<span class="text-on-surface font-medium">'+esc(parts[i])+'</span>';
		else html+='<a href="#" onclick="navTo(\''+acc+'\')" class="text-primary no-underline hover:underline">'+esc(parts[i])+'</a>';
	}
	document.getElementById("breadcrumbs").innerHTML=html;
}

function renderFiles(files,path){
	currentPath=path;renderBreadcrumbs();
	let div=document.getElementById("file-list");
	if(files.length===0){
		div.innerHTML='<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-folder-open text-4xl mb-2 block"></i>'+(currentPath==="/trash"?"Trash is empty":"This folder is empty")+'</div>';
		return;
	}
	let isTrash=currentPath==="/trash";
	let isGrid=viewMode==="grid"&&!isTrash;
	let html=isGrid?'<div class="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-6 lg:grid-cols-8 gap-2 p-2">':'<div class="divide-y divide-outline-variant">';
	for(const f of files){
		let icon=f.is_dir?'fa-solid fa-folder text-primary':'fa-solid fa-file text-on-surface-variant';
		let size=f.is_dir?'—':formatSize(f.size);
		let checked=selectedPaths.has(f.path)?'checked':'';
		let favIcon=f.favorited?'<i class="fa-solid fa-star text-yellow-500 text-xs cursor-pointer" onclick="event.stopPropagation();toggleFav(\''+f.path+'\')"></i>':'<i class="fa-regular fa-star text-on-surface-variant text-xs cursor-pointer opacity-0 group-hover:opacity-100" onclick="event.stopPropagation();toggleFav(\''+f.path+'\')"></i>';
		html+='<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high transition-colors group" draggable="false">';
		if(!isTrash)html+='<input type="checkbox" '+checked+' onchange="toggleSelect(\''+f.path+'\')" class="w-4 h-4 accent-primary cursor-pointer">';
		html+='<i class="'+icon+' text-lg w-6 text-center"></i>';
		if(isGrid){let isImg=f.mime_type&&f.mime_type.startsWith("image/");html+='<div class="aspect-square bg-surface-container rounded-xl overflow-hidden cursor-pointer hover:opacity-90 relative group" onclick="'+(!f.is_dir?'previewFile(\''+f.path+'\',\''+escJS(f.name)+'\',\''+(f.mime_type||"")+'\')':'navTo(\''+f.path+'\')')+'">';
			if(isImg)html+='<img src="'+apiBase+'/thumb?path='+encodeURIComponent(f.path)+'" alt="" class="w-full h-full object-cover" loading="lazy">';
			else{let ico=f.is_dir?'fa-folder text-primary text-3xl':'fa-file text-on-surface-variant text-3xl';html+='<div class="w-full h-full flex items-center justify-center"><i class="fa-solid '+ico+'"></i></div>'}
			html+='<div class="absolute bottom-0 left-0 right-0 bg-gradient-to-t from-black/60 to-transparent p-2 pt-6"><p class="text-white text-[11px] truncate leading-tight">'+esc(f.name)+'</p></div>';
			if(f.favorited)html+='<i class="fa-solid fa-star text-yellow-400 text-xs absolute top-1.5 right-1.5"></i>';
			html+='</div>';}
		else if(f.is_dir&&!isTrash)html+='<a href="#" onclick="navTo(\''+f.path+'\')" class="flex-1 text-sm text-on-surface no-underline hover:text-primary truncate">'+esc(f.name)+'</a>';
		else if(isTrash)html+='<span class="flex-1 text-sm text-on-surface truncate">'+esc(f.name)+'</span>';
		else html+='<a href="'+apiBase+'/download?path='+encodeURIComponent(f.path)+'" class="flex-1 text-sm text-on-surface no-underline hover:text-primary truncate" download>'+esc(f.name)+'</a>';
		html+='<span class="text-xs text-on-surface-variant w-20 text-right hidden sm:inline">'+size+'</span>';
		html+=favIcon;
		if(!isGrid&&f.tags&&f.tags.length>0){html+='<div class="flex gap-1 ml-2">';f.tags.forEach(tid=>{
			let tg=window._tagMap?.[tid];if(tg)html+='<span class="text-[10px] px-2 py-0.5 rounded-full text-white cursor-pointer" style="background:'+tg.color+'" onclick="event.stopPropagation();filterByTag(''+tid+'')">'+esc(tg.name)+'</span>'});html+='</div>'}
		if(isTrash){
			html+='<button onclick="restoreItem(\''+f.trash_path+'\')" class="text-xs text-primary cursor-pointer border-0 bg-transparent hover:underline">Restore</button>';
			html+='<button onclick="deletePermanent(\''+f.trash_path+'\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error opacity-0 group-hover:opacity-100"><i class="fa-solid fa-trash text-xs"></i></button>';
		}else{
			html+='<button onclick="startRename(\''+f.path+'\',\''+escJS(f.name)+'\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-primary opacity-0 group-hover:opacity-100"><i class="fa-solid fa-pen text-xs"></i></button>';
			html+='<button onclick="assignTag(\''+f.path+'\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-primary opacity-0 group-hover:opacity-100"><i class="fa-solid fa-tag text-xs"></i></button>';
			html+='<button onclick="deleteItem(\''+f.path+'\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error opacity-0 group-hover:opacity-100"><i class="fa-solid fa-trash text-xs"></i></button>';
		}
		html+='</div>';
	}
	html+='</div>';
	div.innerHTML=html;
	updateBulkUI();
}

function navTo(path){window._filter=null;currentPath=path;if(path==="/trash")window.history.pushState({},"","/files/trash");else if(path==="/")window.history.pushState({},"","/files");else if(path==="/files")window.history.pushState({},"","/files/files");else window.history.pushState({},"","/files?path="+encodeURIComponent(path));loadFiles()}

function toggleSelect(path){selectedPaths.has(path)?selectedPaths.delete(path):selectedPaths.add(path);updateBulkUI()}
function updateBulkUI(){
	let n=selectedPaths.size;
	let countEl=document.getElementById("selected-count"),delEl=document.getElementById("bulk-delete-btn"),movEl=document.getElementById("bulk-move-btn");
	if(n>0){countEl.textContent=n+" selected";countEl.classList.remove("hidden");delEl.classList.remove("hidden");delEl.classList.add("inline-flex");movEl.classList.remove("hidden");movEl.classList.add("inline-flex")}
	else{countEl.classList.add("hidden");delEl.classList.add("hidden");delEl.classList.remove("inline-flex");movEl.classList.add("hidden");movEl.classList.remove("inline-flex")}
}

function newFolder(){let n=prompt("Folder name:");if(!n)return;apiPost("/mkdir",{path:currentPath,name:n}).then(loadFiles)}
function newTextFile(){let n=prompt("File name:","Untitled.txt");if(!n)return;apiPost("/touch",{path:currentPath,name:n}).then(loadFiles)}
function uploadFiles(){let input=document.getElementById("upload-input");if(!input.files.length)return;let form=new FormData();for(let f of input.files)form.append("file",f);fetch(apiBase+"/upload?path="+encodeURIComponent(currentPath),{method:"POST",body:form}).then(()=>{input.value="";loadFiles()})}

function startRename(path,name){renameTarget=path;document.getElementById("rename-input").value=name;document.getElementById("rename-modal").classList.remove("hidden");document.getElementById("rename-input").focus()}
function closeRename(){document.getElementById("rename-modal").classList.add("hidden");renameTarget=null}
function doRename(){let n=document.getElementById("rename-input").value.trim();if(!n||!renameTarget)return;let dir=renameTarget.substring(0,renameTarget.lastIndexOf("/"));let dst=dir+"/"+n;if(dir==="")dst="/"+n;apiPost("/rename",{src:renameTarget,dst:dst}).then(()=>{closeRename();loadFiles()})}

function deleteItem(path){if(!confirm("Move '"+path+"' to trash?"))return;apiPost("/delete",{path:path}).then(loadFiles)}
function restoreItem(id){apiPost("/restore",{id:id}).then(loadFiles)}
function deletePermanent(id){if(!confirm("Delete permanently?"))return;apiPost("/delete-permanent",{id:id}).then(loadFiles)}

function bulkDelete(){if(selectedPaths.size===0)return;if(!confirm("Delete "+selectedPaths.size+" items?"))return;let promises=[];selectedPaths.forEach(p=>promises.push(apiPost("/delete",{path:p})));Promise.all(promises).then(()=>{selectedPaths.clear();loadFiles()})}
function bulkMove(){moveTargets=[...selectedPaths];document.getElementById("move-modal").classList.remove("hidden")}
function closeMove(){document.getElementById("move-modal").classList.add("hidden")}
function doMove(){let dest=document.getElementById("move-target").value;let promises=[];moveTargets.forEach(p=>{let name=p.substring(p.lastIndexOf("/")+1);promises.push(apiPost("/move",{src:p,dst:dest+"/"+name}))});Promise.all(promises).then(()=>{selectedPaths.clear();closeMove();loadFiles()})}

function assignTag(path){let tags=Object.values(window._tagMap||{}).map(t=>'<button onclick="toggleFileTag(\''+path+'\',\''+t.id+'\')" class="w-full text-left px-3 py-2 text-sm hover:bg-surface-container-high rounded-lg cursor-pointer border-0 bg-transparent flex items-center gap-2"><span class="w-3 h-3 rounded-full shrink-0" style="background:'+t.color+'"></span>'+esc(t.name)+'</button>').join("");let d=document.createElement("div");d.className="fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4";d.onclick=function(e){if(e.target===this)d.remove()};d.innerHTML='<div class="bg-surface rounded-2xl p-4 w-48 shadow-2"><h3 class="text-sm font-semibold mb-2">Assign tag</h3><div class="space-y-1">'+tags+'</div><button onclick="this.parentElement.parentElement.parentElement.remove()" class="mt-2 w-full h-9 rounded-full text-sm text-on-surface-variant border-0 bg-transparent cursor-pointer">Cancel</button></div>';document.body.appendChild(d)}
function toggleFileTag(path,tid){tagApi("/tag",{path:path,tag:tid,add:true}).then(loadFiles)}
function toggleFav(path){apiPost("/favorite",{path:path}).then(loadFiles)}
function filterByTag(tid){window._filter="tag";window._tagFilter=tid;window.history.pushState({},"","/files?filter=tag&tag="+tid);currentPath="/";loadFiles()}
function showTagManager(){document.getElementById("tag-modal").classList.remove("hidden");loadTagList()}
function loadTagList(){api("tags",{}).then(d=>{window._tagMap=d.tags||{};let div=document.getElementById("tag-list");let tags=Object.values(d.tags||{});div.innerHTML=tags.map(t=>'<div class="flex items-center gap-2"><span class="w-4 h-4 rounded-full shrink-0" style="background:'+t.color+'"></span><span class="text-sm flex-1">'+esc(t.name)+'</span><button onclick="deleteTag(''+t.id+'')" class="w-6 h-6 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error text-xs"><i class="fa-solid fa-trash"></i></button></div>').join("")||'<p class="text-sm text-on-surface-variant">No tags</p>';loadFiles()})}
function createTag(){let n=document.getElementById("new-tag-name").value.trim();let c=document.getElementById("new-tag-color").value;if(!n)return;tagApi("/tags",{name:n,color:c}).then(r=>{document.getElementById("new-tag-name").value="";loadTagList()})}
function deleteTag(id){tagApi("/tags?id="+id,{}).then(loadTagList)}
async function tagApi(path,body){let r=await fetch(apiBase+path,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(body)});return r.json()}
function searchFiles(){let q=document.getElementById("search-input").value.trim();if(!q){loadFiles();return}api("search",{q:q}).then(d=>renderFiles(d.files||[],"/"))}
function toggleView(){viewMode=viewMode==="list"?"grid":"list";document.getElementById("view-icon").className=viewMode==="grid"?"fa-solid fa-grip":"fa-solid fa-list";document.getElementById("view-label").textContent=viewMode==="grid"?"Grid":"List";loadFiles()}
function previewFile(path,name,mime){document.getElementById("preview-title").textContent=name;let pc=document.getElementById("preview-content");document.getElementById("preview-panel").classList.remove("hidden");
	if(mime&&mime.startsWith("image/")){pc.innerHTML='<img src="'+apiBase+'/download?path='+encodeURIComponent(path)+'" class="max-w-full rounded-lg cursor-pointer" onclick="openLightbox(''+path+'')">'}
	else if(mime&&mime.startsWith("audio/")){pc.innerHTML='<audio controls class="w-full mt-4"><source src="'+apiBase+'/download?path='+encodeURIComponent(path)+'" type="'+mime+'"></audio>'}
	else if(mime&&mime.startsWith("video/")){pc.innerHTML='<video controls class="max-w-full rounded-lg mt-4"><source src="'+apiBase+'/download?path='+encodeURIComponent(path)+'" type="'+mime+'"></video>'}
	else{api("preview",{path:path}).then(d=>{let t=d.type||"";let c=esc(d.content||"");if(t.includes("markdown")||name.endsWith(".md"))pc.innerHTML='<div class="prose prose-sm text-sm">'+mdToHtml(c)+'</div>';else pc.innerHTML='<pre class="text-xs whitespace-pre-wrap font-mono bg-surface-container p-3 rounded-xl overflow-x-auto">'+c+'</pre>'})}}
function closePreview(){document.getElementById("preview-panel").classList.add("hidden")}
function openLightbox(path){let imgs=[];document.querySelectorAll("#file-list a[download]").forEach(a=>{let p=new URL(a.href).searchParams.get("path");if(p&&p.match(/\.(png|jpg|jpeg|gif|webp|svg)/i))imgs.push(p)});lightboxFiles=imgs;lightboxIdx=lightboxFiles.indexOf(path);if(lightboxIdx<0)lightboxIdx=0;showLightbox()}
function showLightbox(){document.getElementById("lightbox-img").src=apiBase+"/download?path="+encodeURIComponent(lightboxFiles[lightboxIdx]||"");document.getElementById("img-lightbox").classList.remove("hidden")}
function closeLightbox(){document.getElementById("img-lightbox").classList.add("hidden")}
function navLightbox(d){lightboxIdx+=d;if(lightboxIdx<0)lightboxIdx=lightboxFiles.length-1;if(lightboxIdx>=lightboxFiles.length)lightboxIdx=0;showLightbox()}
function mdToHtml(md){return md.replace(/^### (.+)/gm,'<h3 class="text-base font-semibold mt-3 mb-1">$1</h3>').replace(/^## (.+)/gm,'<h2 class="text-lg font-semibold mt-3 mb-1">$1</h2>').replace(/^# (.+)/gm,'<h1 class="text-xl font-bold mt-3 mb-2">$1</h1>').replace(/\*\*(.+?)\*\*/g,'<strong>$1</strong>').replace(/\*(.+?)\*/g,'<em>$1</em>').replace(/\\x60(.+?)\\x60/g,'<code class="bg-surface-container-high px-1 rounded text-xs">$1</code>').replace(/^- (.+)/gm,'<li class="ml-4 list-disc">$1</li>').replace(/\n/g,'<br>')}

function setupDragDrop(){
	let dz=document.getElementById("drop-zone");
	document.addEventListener("dragover",e=>{e.preventDefault();dz.classList.remove("hidden")});
	dz.addEventListener("dragleave",()=>dz.classList.add("hidden"));
	dz.addEventListener("drop",e=>{e.preventDefault();dz.classList.add("hidden");
		let form=new FormData();for(let f of e.dataTransfer.files)form.append("file",f);
		fetch(apiBase+"/upload?path="+encodeURIComponent(currentPath),{method:"POST",body:form}).then(loadFiles)});
}

function formatSize(b){if(b<1024)return b+" B";if(b<1048576)return(b/1024).toFixed(1)+" KB";if(b<1073741824)return(b/1048576).toFixed(1)+" MB";return(b/1073741824).toFixed(1)+" GB"}
function esc(s){let d=document.createElement("div");d.textContent=s;return d.innerHTML}
function escJS(s){return s.replace(/'/g,"\\'")}
init();
</script>`
}
