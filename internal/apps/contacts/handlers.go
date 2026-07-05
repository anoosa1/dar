package contacts

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
		Title:    "Contacts — Dar",
		AppBar:   server.AppBarHTML(r),
		UserMenu: template.HTML(userMenuHTML(displayName)),
		Content:  template.HTML(contactsHTML()),
	})
}

func userMenuHTML(name string) string {
	initial := "U"
	if len(name) > 0 { initial = name[:1] }
	return `<div class="relative">
		<button id="user-menu-button" type="button" onclick="toggleUserMenu()"
			class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer">
			<span>` + initial + `</span>
		</button>
		<div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
			<div class="px-4 py-2 text-sm font-medium text-on-surface">` + name + `</div>
			<a href="/home" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Home</a>
			<a href="/logout" class="block px-4 py-2 text-sm text-error no-underline hover:bg-error-container">Logout</a>
		</div></div><script>
	function toggleUserMenu(){let m=document.getElementById("user-menu");if(m)m.classList.toggle("hidden")}
	document.addEventListener("click",function(e){let b=document.getElementById("user-menu-button"),m=document.getElementById("user-menu");if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add("hidden")});</script>`
}

func contactsHTML() string {
	return `
<div id="contacts-app" class="max-w-3xl mx-auto">
	<div class="flex items-center gap-3 mb-4">
		<button onclick="showCreate()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90 transition-opacity inline-flex items-center gap-2">
			<i class="fa-solid fa-plus"></i> New contact
		</button>
		<div class="flex-1"></div>
		<div class="relative">
			<input id="search-input" type="text" placeholder="Search contacts..."
				class="h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 w-48 placeholder:text-on-surface-variant"
				oninput="searchContacts()">
		</div>
	</div>
	<div id="contact-list"></div>

	<!-- Edit modal -->
	<div id="edit-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)closeModal()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-md shadow-2">
			<h2 id="modal-title" class="text-lg font-semibold mb-4">New Contact</h2>
			<input id="edit-first" placeholder="First name" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="edit-last" placeholder="Last name" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="edit-email" placeholder="Email" type="email" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="edit-phone" placeholder="Phone" type="tel" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<input id="edit-org" placeholder="Organization" class="w-full h-10 px-4 mb-3 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30">
			<div class="flex gap-2 justify-end mt-4">
				<button onclick="closeModal()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
				<button onclick="saveContact()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Save</button>
			</div>
		</div>
	</div>

	<!-- Detail modal -->
	<div id="detail-modal" class="hidden fixed inset-0 bg-black/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)closeDetail()">
		<div class="bg-surface rounded-2xl p-6 w-full max-w-sm shadow-2 text-center">
			<div id="detail-avatar" class="w-20 h-20 rounded-full inline-flex items-center justify-center text-white text-2xl font-bold mb-3"></div>
			<h2 id="detail-name" class="text-xl font-semibold mb-1"></h2>
			<p id="detail-email" class="text-sm text-primary mb-1"></p>
			<p id="detail-phone" class="text-sm text-on-surface-variant mb-1"></p>
			<p id="detail-org" class="text-sm text-on-surface-variant mb-4"></p>
			<div class="flex gap-2 justify-center">
				<button onclick="editContact(currentDetail)" class="h-10 px-4 rounded-full bg-primary-container text-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90">Edit</button>
				<button onclick="deleteContact(currentDetail)" class="h-10 px-4 rounded-full bg-error-container text-error text-sm font-medium border-0 cursor-pointer hover:opacity-90">Delete</button>
			</div>
		</div>
	</div>
</div>

<script>
const apiBase = "/api/contacts";
let contacts = [];
let currentEdit = null;
let currentDetail = null;

async function api(method, path, body) {
	let opts = {method};
	if (body) { opts.headers = {"Content-Type":"application/json"}; opts.body = JSON.stringify(body); }
	let r = await fetch(apiBase + path, opts);
	return r.json();
}

function loadContacts() {
	api("GET", "/list").then(d => {
		contacts = d.contacts || [];
		renderList(contacts);
	});
}

function renderList(list) {
	let div = document.getElementById("contact-list");
	if (list.length === 0) {
		div.innerHTML = '<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-address-book text-4xl mb-2 block"></i>No contacts yet</div>';
		return;
	}
	let groups = {};
	for (const c of list) {
		let key = (c.first_name || c.last_name || "?")[0].toUpperCase();
		if (!groups[key]) groups[key] = [];
		groups[key].push(c);
	}
	let html = '';
	for (const key of Object.keys(groups).sort()) {
		html += '<div class="text-xs font-bold text-on-surface-variant px-2 py-1 mt-2">' + key + '</div>';
		for (const c of groups[key]) {
			let name = c.first_name || c.last_name || c.email || "?";
			html += '<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high rounded-xl cursor-pointer transition-colors" onclick="showDetail(\'' + c.id + '\')">';
			html += '<div class="w-10 h-10 rounded-full inline-flex items-center justify-center text-white text-sm font-bold" style="background:' + (c.color || '#6750a4') + '">' + (c.initial || name[0]) + '</div>';
			html += '<div class="flex-1"><div class="text-sm font-medium text-on-surface">' + esc(name) + '</div>';
			if (c.email) html += '<div class="text-xs text-on-surface-variant">' + esc(c.email) + '</div>';
			html += '</div></div>';
		}
	}
	div.innerHTML = html;
}

function showCreate() {
	currentEdit = null;
	document.getElementById("modal-title").textContent = "New Contact";
	["edit-first","edit-last","edit-email","edit-phone","edit-org"].forEach(id => document.getElementById(id).value = "");
	document.getElementById("edit-modal").classList.remove("hidden");
}

function editContact(c) {
	currentEdit = c.id;
	document.getElementById("modal-title").textContent = "Edit Contact";
	document.getElementById("edit-first").value = c.first_name || "";
	document.getElementById("edit-last").value = c.last_name || "";
	document.getElementById("edit-email").value = c.email || "";
	document.getElementById("edit-phone").value = c.phone || "";
	document.getElementById("edit-org").value = c.org || "";
	document.getElementById("edit-modal").classList.remove("hidden");
	closeDetail();
}

function closeModal() { document.getElementById("edit-modal").classList.add("hidden"); }

function saveContact() {
	let data = {
		id: currentEdit || "",
		first_name: document.getElementById("edit-first").value,
		last_name: document.getElementById("edit-last").value,
		email: document.getElementById("edit-email").value,
		phone: document.getElementById("edit-phone").value,
		org: document.getElementById("edit-org").value,
	};
	let path = currentEdit ? "/update" : "/create";
	api("POST", path, data).then(() => { closeModal(); loadContacts(); });
}

function showDetail(id) {
	currentDetail = contacts.find(c => c.id === id);
	if (!currentDetail) return;
	document.getElementById("detail-avatar").textContent = currentDetail.initial || "?";
	document.getElementById("detail-avatar").style.background = currentDetail.color || "#6750a4";
	document.getElementById("detail-name").textContent = (currentDetail.first_name || "") + " " + (currentDetail.last_name || "");
	document.getElementById("detail-email").textContent = currentDetail.email || "";
	document.getElementById("detail-phone").textContent = currentDetail.phone || "";
	document.getElementById("detail-org").textContent = currentDetail.org || "";
	document.getElementById("detail-modal").classList.remove("hidden");
}

function closeDetail() { document.getElementById("detail-modal").classList.add("hidden"); }

function deleteContact(c) {
	if (!confirm("Delete this contact?")) return;
	api("POST", "/delete?id=" + c.id).then(() => { closeDetail(); loadContacts(); });
}

function searchContacts() {
	let q = document.getElementById("search-input").value.toLowerCase();
	if (!q) { renderList(contacts); return; }
	let filtered = contacts.filter(c => 
		(c.first_name+" "+c.last_name+" "+c.email+" "+c.phone).toLowerCase().includes(q)
	);
	renderList(filtered);
}

function esc(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }

loadContacts();
</script>`
}
