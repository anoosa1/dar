package files

import (
	"html/template"
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
		http.Error(w, "Internal server error", 500)
	}
}

func renderPage(w io.Writer, r *http.Request, displayName string) error {
	return webui.Render(w, webui.PageData{
		Title:    "Files — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: template.HTML(userMenuHTML(displayName)),
		Content:  template.HTML(filesHTML()),
	})
}

func userMenuHTML(name string) string {
	initial := "U"
	if len(name) > 0 {
		initial = name[:1]
	}
	return `<div class="relative">
		<button id="user-menu-button" type="button" onclick="toggleUserMenu()"
			class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer">
			<span>` + initial + `</span>
		</button>
		<div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
			<div class="px-4 py-2 text-sm font-medium text-on-surface">` + name + `</div>
			<a href="/home" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Home</a>
			<a href="/logout" class="block px-4 py-2 text-sm text-error no-underline hover:bg-error-container">Logout</a>
		</div>
	</div>
	<script>
	function toggleUserMenu(){let m=document.getElementById("user-menu");if(m)m.classList.toggle("hidden")}
	document.addEventListener("click",function(e){let b=document.getElementById("user-menu-button"),m=document.getElementById("user-menu");if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add("hidden")});
	</script>`
}

func filesHTML() string {
	return `
<div id="files-app" class="max-w-4xl mx-auto">
	<!-- Toolbar -->
	<div class="flex items-center gap-3 mb-4 flex-wrap">
		<button onclick="newFolder()" class="h-10 px-4 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest transition-colors inline-flex items-center gap-2">
			<i class="fa-solid fa-folder-plus"></i> New folder
		</button>
		<label class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium cursor-pointer hover:opacity-90 transition-opacity inline-flex items-center gap-2">
			<i class="fa-solid fa-upload"></i> Upload
			<input type="file" id="upload-input" onchange="uploadFile()" class="hidden" multiple>
		</label>
		<div class="flex-1"></div>
		<div class="relative">
			<input id="search-input" type="text" placeholder="Search files..."
				class="h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 w-48 placeholder:text-on-surface-variant"
				onkeydown="if(event.key==='Enter')searchFiles()">
			<button onclick="searchFiles()" class="absolute right-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant">
				<i class="fa-solid fa-search"></i>
			</button>
		</div>
	</div>

	<!-- Breadcrumbs -->
	<nav id="breadcrumbs" class="flex items-center gap-1 text-sm mb-4 flex-wrap"></nav>

	<!-- File list -->
	<div id="file-list" class="bg-surface-container rounded-xl overflow-hidden">
		<div class="text-center text-on-surface-variant py-12">Loading...</div>
	</div>
</div>

<script>
let currentPath = "/";
const apiBase = "/api/files";

async function api(method, params) {
	let url = apiBase + "?" + new URLSearchParams(params);
	if (method === "POST") {
		let r = await fetch(url, {method:"POST"});
		return r.json();
	}
	let r = await fetch(url);
	return r.json();
}

function loadFiles() {
	const q = new URLSearchParams(window.location.search);
	currentPath = q.get("path") || "/";
	document.getElementById("search-input").value = "";
	renderBreadcrumbs();
	api("GET", {path: currentPath}).then(data => renderFileList(data.files || []));
}

function renderBreadcrumbs() {
	let parts = currentPath.split("/").filter(p => p);
	let html = '<a href="#" onclick="navigate(\'/\')" class="text-primary no-underline hover:underline">Home</a>';
	let accumulated = "";
	for (let i = 0; i < parts.length; i++) {
		accumulated += "/" + parts[i];
		html += ' <span class="text-on-surface-variant">/</span> ';
		if (i === parts.length - 1) {
			html += '<span class="text-on-surface font-medium">' + esc(parts[i]) + '</span>';
		} else {
			html += '<a href="#" onclick="navigate(\'' + accumulated + '\')" class="text-primary no-underline hover:underline">' + esc(parts[i]) + '</a>';
		}
	}
	document.getElementById("breadcrumbs").innerHTML = html;
}

function renderFileList(files) {
	const div = document.getElementById("file-list");
	if (files.length === 0) {
		div.innerHTML = '<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-folder-open text-4xl mb-2 block"></i>This folder is empty</div>';
		return;
	}
	let html = '<div class="divide-y divide-outline-variant">';
	for (const f of files) {
		let icon = f.is_dir ? 'fa-solid fa-folder text-primary' : 'fa-solid fa-file text-on-surface-variant';
		let size = f.is_dir ? '—' : formatSize(f.size);
		html += '<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high transition-colors group">';
		html += '<i class="' + icon + ' text-lg w-6 text-center"></i>';
		if (f.is_dir) {
			html += '<a href="#" onclick="navigate(\'' + f.path + '\')" class="flex-1 text-sm text-on-surface no-underline hover:text-primary truncate">' + esc(f.name) + '</a>';
		} else {
			html += '<a href="' + apiBase + '/download?path=' + encodeURIComponent(f.path) + '" class="flex-1 text-sm text-on-surface no-underline hover:text-primary truncate" download>' + esc(f.name) + '</a>';
		}
		html += '<span class="text-xs text-on-surface-variant w-20 text-right hidden sm:inline">' + size + '</span>';
		html += '<button onclick="deleteItem(\'' + f.path + '\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error hover:bg-error-container opacity-0 group-hover:opacity-100 transition-all"><i class="fa-solid fa-trash text-xs"></i></button>';
		html += '</div>';
	}
	html += '</div>';
	div.innerHTML = html;
}

function navigate(path) {
	window.history.pushState({}, "", "/files?path=" + encodeURIComponent(path));
	currentPath = path;
	loadFiles();
}

function newFolder() {
	const name = prompt("Folder name:");
	if (!name) return;
	api("POST", {path: currentPath, name: name}).then(() => loadFiles());
}

function uploadFile() {
	const input = document.getElementById("upload-input");
	const file = input.files[0];
	if (!file) return;
	const form = new FormData();
	form.append("file", file);
	fetch(apiBase + "/upload?path=" + encodeURIComponent(currentPath), {method:"POST", body:form})
		.then(() => { input.value = ""; loadFiles(); });
}

function deleteItem(path) {
	if (!confirm("Delete " + path + "?")) return;
	api("POST", {path: path}).then(() => loadFiles());
}

function searchFiles() {
	const q = document.getElementById("search-input").value.trim();
	if (!q) { loadFiles(); return; }
	api("GET", {q: q}).then(data => renderFileList(data.files || []));
}

function formatSize(bytes) {
	if (bytes < 1024) return bytes + " B";
	if (bytes < 1048576) return (bytes / 1024).toFixed(1) + " KB";
	return (bytes / 1048576).toFixed(1) + " MB";
}
function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

loadFiles();
</script>`
}
