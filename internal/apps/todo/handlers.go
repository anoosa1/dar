package todo

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
		Title:    "Todo — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar([]webui.SidebarItem{
			{Label: "Active", Icon: "fa-solid fa-list-check", Href: "/todo", Active: true},
			{Label: "Completed", Icon: "fa-solid fa-check", Href: "/todo?filter=completed"},
		}, "/settings"),
		Content: template.HTML(todoHTML()),
	})
}

func todoHTML() string {
	return `
<div id="todo-app" class="max-w-2xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<h2 id="list-title" class="text-lg font-semibold">Tasks</h2>
		<button onclick="showCreateList()" class="w-10 h-10 rounded-full bg-surface-container-high border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center justify-center text-on-surface-variant" title="New list">
			<i class="fa-solid fa-plus"></i></button>
	</div>

	<!-- Task input -->
	<div class="flex gap-2 mb-4">
		<input id="new-task-input" type="text" placeholder="Add a task..."
			class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant"
			onkeydown="if(event.key==='Enter')addTask()">
		<button onclick="addTask()" class="w-10 h-10 rounded-full bg-primary text-white border-0 cursor-pointer hover:opacity-90 inline-flex items-center justify-center shrink-0">
			<i class="fa-solid fa-plus"></i></button>
	</div>

	<!-- Task list -->
	<div id="task-list"></div>
</div>

<script>
const apiBase = "/api/todo";
let currentList = "default", currentFilter = "";

async function api(method, path, body) {
	let opts = {method};
	if (body) { opts.headers = {"Content-Type":"application/json"}; opts.body = JSON.stringify(body); }
	let r = await fetch(apiBase + path, opts);
	return r.json();
}

function init() {
	let q = new URLSearchParams(window.location.search);
	currentFilter = q.get("filter") || "";
	loadTasks();
}

function loadTasks() {
	let params = "?list_id=" + currentList;
	if (currentFilter) params += "&filter=" + currentFilter;
	api("GET", "/tasks" + params).then(d => renderTasks(d.tasks || []));
}

function renderTasks(tasks) {
	let div = document.getElementById("task-list");
	if (tasks.length === 0) {
		div.innerHTML = '<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-check-double text-4xl mb-2 block"></i>' +
			(currentFilter === "completed" ? 'No completed tasks' : 'All clear! Add a task above.') + '</div>';
		return;
	}
	let html = "";
	for (const t of tasks) {
		let checked = t.completed ? "checked" : "";
		let lineCls = t.completed ? "line-through text-on-surface-variant" : "text-on-surface";
		let dueStr = t.due_date ? '<span class="text-xs text-on-surface-variant ml-2">' + t.due_date + '</span>' : "";
		html += '<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high rounded-xl group transition-colors">';
		html += '<input type="checkbox" ' + checked + ' onchange="toggleTask(\'' + t.id + '\',this.checked)" class="w-5 h-5 rounded-full accent-primary cursor-pointer">';
		html += '<span class="flex-1 text-sm ' + lineCls + '">' + esc(t.title) + dueStr + '</span>';
		html += '<button onclick="deleteTask(\'' + t.id + '\')" class="w-8 h-8 rounded-full bg-transparent border-0 cursor-pointer text-on-surface-variant hover:text-error hover:bg-error-container opacity-0 group-hover:opacity-100 transition-all">';
		html += '<i class="fa-solid fa-trash text-xs"></i></button>';
		html += '</div>';
	}
	div.innerHTML = html;
}

function addTask() {
	let input = document.getElementById("new-task-input");
	let title = input.value.trim();
	if (!title) return;
	input.value = "";
	api("POST", "/task", {list_id: currentList, title: title}).then(() => loadTasks());
}

function toggleTask(id, completed) {
	api("PUT", "/task", {id: id, completed: completed}).then(() => loadTasks());
}

function deleteTask(id) {
	api("DELETE", "/task?id=" + id).then(() => loadTasks());
}

function showCreateList() {
	let name = prompt("List name:");
	if (!name) return;
	api("POST", "/lists", {name: name}).then(d => { currentList = d.id; loadTasks(); });
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

init();
</script>`
}
