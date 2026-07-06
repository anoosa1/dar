package notes

import (
	"html/template"
	"io"
	"net/http"

	"github.com/anoosa/dar/internal/apps"
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
		Title:    "Notes — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "All notes", Icon: "fa-solid fa-note-sticky", Href: "/notes", Active: true},
			{Label: "Favorites", Icon: "fa-solid fa-star", Href: "/notes?filter=favorites"},
		}, "/settings"),
		Content: template.HTML(notesHTML()),
	})
}

func notesHTML() string {
	return `
<div id="notes-app" class="max-w-3xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<button onclick="createNote()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
			<i class="fa-solid fa-plus"></i> New note</button>
		<div class="flex-1"></div>
		<input id="search-input" type="text" placeholder="Search notes..." oninput="loadNotes()"
			class="h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 w-48 placeholder:text-on-surface-variant">
	</div>
	<div id="note-list" class="space-y-1"></div>

	<!-- Editor modal -->
	<div id="editor-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)closeEditor()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-lg shadow-2">
			<div class="flex items-center gap-2 mb-3">
				<input id="edit-title" placeholder="Note title" class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm font-semibold border-0 outline-none focus:ring-2 focus:ring-primary/30">
				<button onclick="toggleFavorite()" id="fav-btn" class="w-10 h-10 rounded-full bg-surface-container border-0 cursor-pointer text-on-surface-variant hover:text-yellow-500"><i class="fa-solid fa-star"></i></button>
			</div>
			<textarea id="edit-content" placeholder="Write something..." rows="8"
				class="w-full px-4 py-3 rounded-xl bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 resize-none"></textarea>
			<div class="flex gap-2 justify-between mt-4">
				<button onclick="deleteNote()" class="h-10 px-4 rounded-full text-sm font-medium text-error border-0 bg-error-container cursor-pointer hover:opacity-90"><i class="fa-solid fa-trash"></i></button>
				<div class="flex gap-2">
					<button onclick="closeEditor()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
					<button onclick="saveNote()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Save</button>
				</div>
			</div>
		</div>
	</div>
</div>

<script>
const apiBase = "/api/notes";
let notes = [], currentNote = null;
let saveTimeout = null;

async function api(m, p, b) {
	let o = {method:m};
	if (b) { o.headers = {"Content-Type":"application/json"}; o.body = JSON.stringify(b); }
	let r = await fetch(apiBase + p, o);
	return r.json();
}

function loadNotes() {
	let q = document.getElementById("search-input").value;
	let params = q ? "?q=" + encodeURIComponent(q) : "";
	api("GET", "/notes" + params).then(d => {
		notes = d.notes || [];
		renderList();
	});
}

function renderList() {
	let div = document.getElementById("note-list");
	if (notes.length === 0) {
		div.innerHTML = '<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-note-sticky text-4xl mb-2 block"></i>No notes yet</div>';
		return;
	}
	div.innerHTML = notes.map(n => {
		let preview = (n.content || "").substring(0, 80).replace(/\n/g, " ");
		let date = new Date(n.updated_at * 1000).toLocaleDateString();
		let star = n.favorited ? ' <i class="fa-solid fa-star text-yellow-500 text-xs"></i>' : '';
		return '<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high rounded-xl cursor-pointer transition-colors" onclick="openNote(\'' + n.id + '\')">' +
			'<div class="flex-1"><div class="text-sm font-medium text-on-surface">' + esc(n.title) + star + '</div>' +
			'<div class="text-xs text-on-surface-variant truncate">' + esc(preview) + '</div></div>' +
			'<span class="text-xs text-on-surface-variant shrink-0">' + date + '</span></div>';
	}).join("");
}

function createNote() {
	currentNote = {id: "", title: "", content: "", favorited: false};
	showEditor();
}

function openNote(id) {
	currentNote = notes.find(n => n.id === id);
	if (!currentNote) return;
	showEditor();
}

function showEditor() {
	document.getElementById("edit-title").value = currentNote.title || "";
	document.getElementById("edit-content").value = currentNote.content || "";
	document.getElementById("fav-btn").innerHTML = currentNote.favorited ?
		'<i class="fa-solid fa-star text-yellow-500"></i>' : '<i class="fa-solid fa-star"></i>';
	document.getElementById("editor-modal").classList.remove("hidden");
	document.getElementById("edit-title").focus();
}

function closeEditor() {
	document.getElementById("editor-modal").classList.add("hidden");
	currentNote = null;
}

function toggleFavorite() {
	currentNote.favorited = !currentNote.favorited;
	document.getElementById("fav-btn").innerHTML = currentNote.favorited ?
		'<i class="fa-solid fa-star text-yellow-500"></i>' : '<i class="fa-solid fa-star"></i>';
	autoSave();
}

function saveNote() {
	let title = document.getElementById("edit-title").value.trim();
	let content = document.getElementById("edit-content").value;
	let method = currentNote.id ? "PUT" : "POST";
	api(method, "/note", {id: currentNote.id, title: title || "Untitled", content: content, favorited: currentNote.favorited})
		.then(d => {
			closeEditor();
			loadNotes();
		});
}

function autoSave() {
	let title = document.getElementById("edit-title").value.trim();
	let content = document.getElementById("edit-content").value;
	if (!title && !content) return;
	let method = currentNote.id ? "PUT" : "POST";
	api(method, "/note", {id: currentNote.id, title: title || "Untitled", content: content, favorited: currentNote.favorited})
		.then(d => {
			if (!currentNote.id) { currentNote.id = d.id; notes.push(d); }
			else { let i = notes.findIndex(n => n.id === currentNote.id); if (i >= 0) notes[i] = d; }
		});
}

function deleteNote() {
	if (!currentNote.id) { closeEditor(); return; }
	if (!confirm("Delete this note?")) return;
	api("DELETE", "/note?id=" + currentNote.id).then(() => { closeEditor(); loadNotes(); });
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

loadNotes();
</script>`
}

type App struct{}

func (a App) Name() string                     { return "notes" }
func (a App) DisplayName() string              { return "Notes" }
func (a App) Icon() string                     { return `<i class="fa-solid fa-note-sticky"></i>` }
func (a App) Description() string              { return "Markdown notes" }
func (a App) AppBarOrder() int                 { return 6 }
func (a App) DaemonConfig() *apps.DaemonConfig { return nil }
func (a App) Routes() []apps.Route {
	return []apps.Route{
		{Method: "GET", Pattern: "/notes", Handler: handlePage},
	}
}
