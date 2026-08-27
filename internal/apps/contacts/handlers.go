package contacts

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
		http.Error(w, "Internal server error", 500)
	}
}

func renderPage(w io.Writer, r *http.Request, displayName string) error {
	content, _ := webui.ExternalizeLastScript(contactsHTML(), "/contacts/assets/app.js")
	return webui.RenderRequest(w, r, webui.PageData{
		Title:      "Contacts — Dar",
		CurrentApp: "contacts",
		AppBar:     server.AppBarHTML(r),
		UserMenu:   webui.UserMenuHTML(displayName),
		Sidebar:    "",
		Content:    content,
	})
}

func handleContactsAsset(w http.ResponseWriter, r *http.Request) {
	_, script := webui.ExternalizeLastScript(contactsHTML(), "/contacts/assets/app.js")
	webui.ServeAppJavaScript(w, r, "app.js", script)
}

func contactsHTML() string {
	return `
<div id="contacts-app" data-dar-shell class="flex h-full min-h-0 flex-col bg-surface">
	<div class="flex-1 min-h-0 flex">
		<aside id="contacts-sidebar" data-dar-sidebar data-dar-sidebar-key="contacts" aria-label="Contacts navigation" class="flex flex-col shrink-0 border-r border-outline-variant bg-surface-container-low h-full">
			<div class="flex-1 flex flex-col min-h-0">
				<div class="px-4 pb-3 pt-3 shrink-0">
					<button onclick="showCreate()" class="w-full h-10 px-4 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90 transition-opacity inline-flex items-center justify-center gap-2 shadow-1">
						<i class="fa-solid fa-plus"></i> New contact
					</button>
				</div>
				<div class="overflow-y-auto flex-1 min-h-0">
					<div class="px-3 pb-1">
						<a href="#" onclick="selectFilter('all');return false" class="rounded-full bg-primary-container text-primary font-medium no-underline px-4 h-10 flex items-center gap-3">
							<i class="fa-solid fa-address-book w-5 shrink-0 text-center"></i>
							<div class="flex-1 min-w-0"><div class="text-sm truncate">All contacts</div></div>
							<span id="all-count" class="text-xs opacity-60">0</span>
						</a>
					</div>
					<div class="px-3 pb-1">
						<a id="sidebar-not-grouped" href="#" onclick="selectFilter('ungrouped');return false" class="rounded-full no-underline px-4 h-10 flex items-center gap-3 text-on-surface-variant hover:bg-surface-container-highest">
							<i class="fa-solid fa-user-slash w-5 shrink-0 text-center"></i>
							<div class="flex-1 min-w-0"><div class="text-sm truncate">Not grouped</div></div>
							<span id="ungrouped-count" class="text-xs opacity-60">0</span>
						</a>
					</div>
					<div class="px-4 pb-1">
						<div class="flex items-center gap-3 px-3 py-1">
							<div class="text-xs font-bold text-on-surface-variant uppercase tracking-wider">Contact groups</div>
							<div class="flex-1"></div>
							<button type="button" onclick="toggleGroupMenu(event)" aria-label="Add contact group" class="w-6 h-6 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center">
								<i class="fa-solid fa-plus text-xs"></i>
							</button>
						</div>
						<div id="group-list" class="space-y-0.5">
							<div class="text-xs text-on-surface-variant px-3 py-2">No groups yet</div>
						</div>
						<div id="group-menu" class="hidden absolute z-30 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 min-w-[180px]">
							<input id="new-group-name" type="text" placeholder="New group name"
								class="w-full h-8 px-3 mb-1 text-sm bg-transparent border-0 outline-none placeholder:text-on-surface-variant"
								onkeydown="if(event.key==='Enter')createGroup()">
							<button onclick="createGroup()" class="w-full text-left px-3 py-1.5 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer">Create</button>
						</div>
					</div>
				</div>
			</div>
			<div class="p-3 border-t border-outline-variant shrink-0">
				<button onclick="openSettings()" class="w-full h-10 rounded-full flex items-center gap-3 px-4 text-sm text-on-surface-variant hover:bg-surface-container-highest transition-colors border-0 bg-transparent cursor-pointer">
					<i class="fa-solid fa-gear w-5 shrink-0 text-center"></i>
					<span>Contacts settings</span>
				</button>
			</div>
		<div id="group-item-menu" class="hidden fixed z-50 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 min-w-[180px]">
			<button onclick="addMembersToGroup()" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-user-plus w-4 text-xs"></i> Add member</button>
			<hr class="border-outline-variant my-1">
			<button onclick="renameGroup()" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-pen w-4 text-xs"></i> Rename</button>
			<button onclick="deleteGroup()" class="w-full text-left px-3 py-2 text-sm text-error hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-trash w-4 text-xs"></i> Delete</button>
		</div>

<!-- Add members to group modal -->
<div id="add-members-modal" class="hidden fixed inset-0 bg-scrim/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeAddMembersModal()">
	<div class="bg-surface rounded-2xl w-full max-w-md shadow-2 flex flex-col max-h-[80vh]">
		<div class="flex items-center justify-between p-4 pb-2 shrink-0">
			<h2 class="text-lg font-semibold" id="add-members-title">Add members</h2>
			<button onclick="closeAddMembersModal()" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center">
				<i class="fa-solid fa-xmark"></i>
			</button>
		</div>
		<div class="px-4 pb-2 shrink-0">
			<input id="add-members-search" type="text" placeholder="Search contacts..."
				class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30"
				oninput="filterAddMemberList()">
		</div>
		<div id="add-members-list" class="overflow-y-auto flex-1 px-4 py-2 space-y-1"></div>
		<div class="flex justify-end gap-2 p-4 pt-3 border-t border-outline-variant shrink-0">
			<button onclick="closeAddMembersModal()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
			<button onclick="saveGroupMembers()" class="h-10 px-4 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90">Add</button>
		</div>
	</div>
</div>
		</aside>

		<section id="contact-list-pane" data-dar-master-pane class="relative border-r border-outline-variant min-w-0 shrink-0 flex flex-col min-h-0 w-full max-w-full lg:w-[300px]" style="width:300px">
			<header data-dar-sidebar-toggle-host class="flex h-14 shrink-0 items-center gap-2 border-b border-outline-variant px-3">
				<h1 class="min-w-0 flex-1 truncate text-lg font-semibold">Contacts</h1>
			</header>
			<div data-dar-pane-header class="px-3 py-3 border-b border-outline-variant shrink-0">
				<input id="search-input" type="text" placeholder="Search contacts..."
					class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant"
					oninput="searchContacts()">
			</div>
			<div id="contact-list" class="flex-1 overflow-y-auto px-1 py-2"></div>
			<!-- Drag handle: resize list column (Nextcloud-style) -->
			<div id="list-resizer" role="separator" aria-orientation="vertical" aria-label="Resize contact list"
				class="hidden lg:block absolute top-0 right-0 w-1.5 h-full cursor-col-resize z-20 hover:bg-primary/40 active:bg-primary/50 transition-colors"
				title="Drag to resize"></div>
		</section>

		<section id="detail-pane" data-dar-detail-pane class="hidden lg:flex min-w-0 flex-1 flex-col overflow-y-auto bg-transparent">
			<header data-dar-pane-header class="sticky top-0 z-10 flex h-14 shrink-0 items-center gap-3 border-b border-outline-variant px-4 lg:hidden">
				<button type="button" data-dar-detail-back onclick="closeContactDetail()" class="h-10 w-10 items-center justify-center rounded-full hover:bg-surface-container-high" aria-label="Back to contacts"><i class="fa-solid fa-arrow-left"></i></button>
				<h2 class="font-semibold">Contact details</h2>
			</header>
			<div id="detail-empty" class="flex-1 flex flex-col items-center justify-center text-center p-6">
				<i class="fa-solid fa-address-book text-4xl text-on-surface-variant mb-3 opacity-30"></i>
				<p class="text-on-surface-variant text-sm">Select a contact</p>
				<p class="text-on-surface-variant text-xs mt-1 opacity-60">Contact details will appear here.</p>
			</div>

			<div id="detail-view" class="hidden p-6">
				<div class="flex flex-col items-center mb-6">
					<div class="w-24 h-24 rounded-full inline-flex items-center justify-center text-on-primary text-3xl font-bold mb-3" id="detail-avatar"></div>
					<h2 id="detail-name" class="text-xl font-semibold text-on-surface text-center break-words"></h2>
					<p id="detail-subtitle" class="text-sm text-on-surface-variant mt-1 text-center"></p>
					<div class="flex gap-2 mt-4">
						<button onclick="editCurrent()" class="h-10 px-4 rounded-full bg-primary-container text-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
							<i class="fa-solid fa-pen"></i> Edit
						</button>
						<button onclick="cloneCurrent()" class="h-10 px-4 rounded-full bg-tertiary-container text-tertiary text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
							<i class="fa-solid fa-clone"></i> Clone
						</button>
						<button onclick="downloadCurrent()" class="h-10 px-4 rounded-full bg-surface-container text-on-surface-variant text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-high inline-flex items-center gap-2">
							<i class="fa-solid fa-download"></i> Download
						</button>
						<button onclick="deleteCurrent()" class="h-10 px-4 rounded-full bg-error-container text-error text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center gap-2">
							<i class="fa-solid fa-trash"></i> Delete
						</button>
					</div>
				</div>
				<div id="detail-fields" class="space-y-4"></div>
			</div>

			<div id="detail-edit" class="hidden p-6">
				<div class="space-y-4">
					<div class="flex justify-center mb-4">
						<div class="w-24 h-24 rounded-full inline-flex items-center justify-center bg-surface-container-high text-on-surface-variant text-3xl font-bold cursor-pointer" onclick="pickAvatar()">
							<i id="edit-avatar-placeholder" class="fa-solid fa-camera"></i>
							<img id="edit-avatar-img" class="hidden w-full h-full rounded-full object-cover">
						</div>
					</div>
					<input id="edit-first" placeholder="First name" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
					<input id="edit-last" placeholder="Last name" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
					<div class="grid grid-cols-2 gap-2">
						<input id="edit-title" placeholder="Title" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
						<input id="edit-org" placeholder="Company" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
					</div>
					<div>
						<div class="flex items-center justify-between mb-1">
							<span class="text-xs font-bold uppercase tracking-wide text-on-surface-variant">Email</span>
							<button type="button" onclick="addTypedRow('emails')" class="text-xs text-primary border-0 bg-transparent cursor-pointer">+ Add</button>
						</div>
						<div id="edit-emails" class="space-y-2"></div>
					</div>
					<div>
						<div class="flex items-center justify-between mb-1">
							<span class="text-xs font-bold uppercase tracking-wide text-on-surface-variant">Phone</span>
							<button type="button" onclick="addTypedRow('phones')" class="text-xs text-primary border-0 bg-transparent cursor-pointer">+ Add</button>
						</div>
						<div id="edit-phones" class="space-y-2"></div>
					</div>
					<div>
						<span class="text-xs font-bold uppercase tracking-wide text-on-surface-variant">Address</span>
						<div class="space-y-2 mt-1">
							<input id="edit-street" placeholder="Street" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
							<div class="grid grid-cols-2 gap-2">
								<input id="edit-city" placeholder="City" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
								<input id="edit-region" placeholder="State / province" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
							</div>
							<div class="grid grid-cols-2 gap-2">
								<input id="edit-postal" placeholder="Postal code" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
								<input id="edit-country" placeholder="Country" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
							</div>
						</div>
					</div>
					<input id="edit-url" placeholder="Website" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
					<input id="edit-birthday" placeholder="Birthday (YYYY-MM-DD)" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">
					<textarea id="edit-notes" placeholder="Notes" rows="3" class="w-full px-4 py-2 rounded-2xl bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 resize-y"></textarea>
					<div class="relative">
					<div id="groups-dropdown" class="w-full min-h-[40px] px-4 py-1.5 rounded-full bg-surface-container text-sm border-0 outline-none focus-within:ring-2 focus-within:ring-primary/30 cursor-pointer flex flex-wrap gap-1 items-center" onclick="toggleGroupsDropdown(event)">
						<span id="groups-placeholder" class="text-on-surface-variant text-sm">Select groups...</span>
						<div id="groups-tags" class="flex flex-wrap gap-1"></div>
						<i class="fa-solid fa-chevron-down text-on-surface-variant text-xs ml-auto"></i>
					</div>
					<div id="groups-dropdown-menu" class="hidden absolute top-full left-0 right-0 mt-1 z-40 bg-surface-container rounded-2xl shadow-2 border border-outline-variant py-1 max-h-[200px] overflow-y-auto">
						<div id="groups-dropdown-list"></div>
						<div id="groups-dropdown-empty" class="hidden px-3 py-2 text-xs text-on-surface-variant">No groups yet. Create one from the sidebar.</div>
					</div>
				</div>
					<div class="flex gap-2 pt-4">
						<button onclick="cancelEdit()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
						<button onclick="saveContact()" class="h-10 px-4 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90">Save</button>
					</div>
				</div>
			</div>
		</section>
	</div>
</div>

<!-- Settings modal -->
<div id="settings-modal" class="hidden fixed inset-0 bg-scrim/30 z-40 flex items-center justify-center p-4" onclick="if(event.target===this)closeSettings()">
	<div class="bg-surface rounded-2xl w-full max-w-lg shadow-2">
		<div class="flex items-center justify-between p-6 pb-2">
			<h2 class="text-lg font-semibold">Contacts settings</h2>
			<button onclick="closeSettings()" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center">
				<i class="fa-solid fa-xmark"></i>
			</button>
		</div>
		<div class="flex border-b border-outline-variant px-6">
			<button id="tab-general" onclick="switchSettingsTab('general')" class="border-b-2 border-primary text-primary px-4 py-2 text-sm font-medium bg-transparent cursor-pointer">General</button>
			<button id="tab-books" onclick="switchSettingsTab('books')" class="border-b-2 border-transparent text-on-surface-variant px-4 py-2 text-sm font-medium bg-transparent cursor-pointer hover:text-on-surface">Address books</button>
		</div>
		<div id="panel-general" class="px-6 py-4 space-y-4">
			<label class="text-sm font-medium text-on-surface">Sort contacts by</label>
			<select id="contacts-sort-by" class="w-full h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none">
				<option value="display_name">Display name</option><option value="last_name">Last name</option><option value="first_name">First name</option>
			</select>
			<div>
				<div class="text-sm font-medium text-on-surface mb-1">CardDAV URL</div>
				<p class="text-xs text-on-surface-variant mb-2">Use your email as username and an app password from Settings → Security.</p>
				<div class="flex items-center gap-2">
					<input id="carddav-url" readonly class="flex-1 h-9 px-3 rounded-full bg-surface-container text-xs text-on-surface border-0 outline-none font-mono">
					<button type="button" onclick="copyCardDAV()" class="h-9 px-4 rounded-full bg-primary text-on-primary text-xs font-medium border-0 cursor-pointer hover:opacity-90 shrink-0">Copy</button>
				</div>
			</div>
			<button onclick="importContacts()" class="w-full h-10 rounded-full bg-surface-container-high text-on-surface text-sm font-medium border-0 cursor-pointer hover:bg-surface-container-highest inline-flex items-center justify-center gap-2"><i class="fa-solid fa-file-import"></i>Import contacts</button>
			<div class="flex justify-end gap-2 pt-2"><button type="button" onclick="closeSettings()" class="h-10 rounded-full px-5 hover:bg-surface-container-high">Cancel</button><button type="button" onclick="saveContactsSettings()" class="h-10 rounded-full bg-primary px-5 text-on-primary">Save</button></div>
		</div>
		<div id="panel-books" class="hidden px-6 py-4 space-y-4">
			<div id="settings-books-list" class="space-y-3"></div>
			<button onclick="newAddressBook()" class="w-full h-10 rounded-full bg-primary-container text-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90 inline-flex items-center justify-center gap-2"><i class="fa-solid fa-plus"></i>New address book</button>
		</div>
		<div class="px-6 py-3 border-t border-outline-variant text-xs text-on-surface-variant text-right">Contacts 1.0</div>
	</div>
</div>

<!-- Book actions dropdown -->
<div id="book-menu" class="hidden fixed z-50 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 min-w-[180px]">
	<button onclick="bookAction('copy-link')" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2">
		<i class="fa-solid fa-link w-4 text-xs"></i> Copy link
	</button>
	<button onclick="bookAction('download')" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2">
		<i class="fa-solid fa-download w-4 text-xs"></i> Download
	</button>
	<button id="book-menu-show" onclick="bookAction('show')" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2">
		<i class="fa-solid fa-square-check w-4 text-xs"></i> Show
	</button>
	<button onclick="bookAction('rename')" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2">
		<i class="fa-solid fa-pen w-4 text-xs"></i> Rename
	</button>
	<hr class="border-outline-variant my-1">
	<button onclick="bookAction('delete')" class="w-full text-left px-3 py-2 text-sm text-error hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2">
		<i class="fa-solid fa-trash w-4 text-xs"></i> Delete
	</button>
</div>

<script>
const apiBase = "/api/contacts";
let contacts = [];
let addressBooks = [];
let currentEdit = null;
let currentDetail = null;
let groups = [];
let activeGroup = null;
let activeBookMenu = null;
let selectedGroups = [];
let defaultBookId = "default";
let contactPreferences = {sort_by:"display_name"};

function esc(s) { const d = document.createElement("div"); d.textContent = s||""; return d.innerHTML; }
function colorRGB(value) {
	const match = String(value||"").trim().match(/^#([0-9a-f]{6})$/i);
	if (!match) return null;
	return [0,2,4].map(offset => parseInt(match[1].slice(offset,offset+2),16));
}
function colorLuminance(value) {
	const rgb = colorRGB(value);
	if (!rgb) return null;
	return rgb.map(channel => {
		const v = channel/255;
		return v <= 0.04045 ? v/12.92 : Math.pow((v+0.055)/1.055,2.4);
	}).reduce((sum,v,index) => sum + v*[0.2126,0.7152,0.0722][index],0);
}
function contrastForeground(background) {
	const styles = getComputedStyle(document.documentElement);
	const candidates = [styles.getPropertyValue("--dar-on-primary"), styles.getPropertyValue("--dar-text")];
	const bg = colorLuminance(background);
	if (bg === null) return candidates[1].trim();
	return candidates.map(color => {
		const lum = colorLuminance(color);
		return {color:color.trim(),ratio:(Math.max(bg,lum)+0.05)/(Math.min(bg,lum)+0.05)};
	}).sort((a,b) => b.ratio-a.ratio)[0].color;
}
function copyText(t) { navigator.clipboard.writeText(t).catch(() => {}); }

// Deep-link helpers: /contacts and /contacts/{id}
function contactPath(id) {
	if (!id) return "/contacts";
	return "/contacts/" + encodeURIComponent(id);
}
function idFromPath() {
	let p = location.pathname.replace(/\/+$/, "") || "/";
	if (p === "/contacts") return null;
	if (p.indexOf("/contacts/") !== 0) return null;
	let id = p.slice("/contacts/".length);
	if (!id || id.indexOf("/") >= 0) return null;
	try { return decodeURIComponent(id); } catch (e) { return id; }
}
function setContactURL(id, replace) {
	let url = contactPath(id);
	if (location.pathname.replace(/\/+$/, "") === url.replace(/\/+$/, "")) return;
	if (replace) history.replaceState({contactId: id || null}, "", url);
	else history.pushState({contactId: id || null}, "", url);
}
function clearDetailPane() {
	currentDetail = null;
	currentEdit = null;
	document.getElementById("detail-view").classList.add("hidden");
	document.getElementById("detail-edit").classList.add("hidden");
	document.getElementById("detail-empty").classList.remove("hidden");
	DarUI.shell.closeDetail(document.getElementById("detail-pane"));
	renderList(contacts);
}
function closeContactDetail(fromHistory) {
	if (!fromHistory && DarUI.shell.isCompact()) {
		if (history.state && history.state.contactId) {
			history.back();
			return;
		}
		history.replaceState({contactId:null}, "", "/contacts");
	}
	clearDetailPane();
}
function applyPathToDetail(replaceURL) {
	let id = idFromPath();
	if (!id) {
		clearDetailPane();
		if (replaceURL) setContactURL(null, true);
		return;
	}
	let c = contacts.find(x => x.id === id);
	if (!c) {
		clearDetailPane();
		setContactURL(null, true);
		return;
	}
	showDetail(id, true);
	if (replaceURL) setContactURL(id, true);
}

async function api(method, path, body) {
	let opts = {method};
	if (body) { opts.headers = {"Content-Type":"application/json"}; opts.body = JSON.stringify(body); }
	let r = await fetch(apiBase + path, opts);
	if (!r.ok) {
		let msg = "";
		try { msg = await r.text(); } catch(e) {}
		throw new Error(msg || ("HTTP " + r.status));
	}
	// export returns vcard text; callers of api expect JSON
	const ct = r.headers.get("Content-Type") || "";
	if (ct.indexOf("json") === -1) return r;
	return r.json();
}

function renderBooksList() {
	let list = document.getElementById("settings-books-list");
	let html = "";
	for (const b of addressBooks) {
		let iconClass = b.visible ? "text-primary" : "text-on-surface-variant opacity-40";
		html += '<div class="flex items-center gap-3 p-3 rounded-2xl bg-surface-container">';
		html += '<i class="fa-solid fa-book '+iconClass+' w-10 h-10 rounded-full bg-surface-container-high flex items-center justify-center"></i>';
		html += '<div class="flex-1 min-w-0"><div class="text-sm font-medium text-on-surface">'+esc(b.name)+'</div>';
		html += '<div class="text-xs text-on-surface-variant">'+(b.contact_count||0)+' contacts</div></div>';
		html += '<button type="button" aria-label="Manage '+esc(b.name)+' address book" onclick="openBookMenu(event,\''+esc(b.id)+'\')" class="w-10 h-10 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center">';
		html += '<i class="fa-solid fa-ellipsis-vertical text-xs"></i></button></div>';
	}
	if (!addressBooks.length) html = '<div class="text-xs text-on-surface-variant py-2">No address books</div>';
	list.innerHTML = html;
	let bc = document.getElementById("settings-books-count"); if (bc) bc.textContent = contacts.length;
	document.getElementById("all-count").textContent = contacts.length;
}

function loadBooks() {
	return api("GET", "/books").then(d => {
		addressBooks = d.books || [];
		renderBooksList();
	}).catch(err => {
		console.error("loadBooks", err);
		if (!addressBooks.length) {
			addressBooks = [{id:"default",name:"Default",visible:true,contact_count:contacts.length}];
		}
		renderBooksList();
	});
}

function loadContacts() {
	return Promise.all([api("GET", "/list"), api("GET", "/preferences")]).then(results => {
		let d = results[0];
		contactPreferences = results[1] || contactPreferences;
		contacts = d.contacts || [];
		extractGroups();
		renderList(contacts);
		// Open contact from deep link after list is ready.
		let pathId = idFromPath();
		if (pathId) {
			let c = contacts.find(x => x.id === pathId);
			if (c) showDetail(pathId, true);
			else { clearDetailPane(); setContactURL(null, true); }
		} else if (currentDetail && !contacts.find(x => x.id === currentDetail.id)) {
			// Contact was hidden with its address book (Show toggle).
			clearDetailPane();
		}
		return loadBooks();
	});
}

function renderList(list) {
	list = [...list].sort((a,b) => {
		let key = contactPreferences.sort_by;
		let av = key === "last_name" ? (a.last_name||a.first_name||displayName(a)) : key === "first_name" ? (a.first_name||a.last_name||displayName(a)) : displayName(a);
		let bv = key === "last_name" ? (b.last_name||b.first_name||displayName(b)) : key === "first_name" ? (b.first_name||b.last_name||displayName(b)) : displayName(b);
		return av.localeCompare(bv, undefined, {sensitivity:"base"});
	});
	let div = document.getElementById("contact-list");
	if (!list.length) {
		div.innerHTML = '<div class="text-center text-on-surface-variant py-12"><i class="fa-solid fa-address-book text-4xl mb-2 block"></i>No contacts yet</div>';
		return;
	}
	let groups = {};
	for (const c of list) {
		let sortValue = contactPreferences.sort_by === "last_name" ? (c.last_name||c.first_name||displayName(c)) : contactPreferences.sort_by === "first_name" ? (c.first_name||c.last_name||displayName(c)) : displayName(c);
		let key = (sortValue||"?")[0].toUpperCase();
		if (!groups[key]) groups[key] = [];
		groups[key].push(c);
	}
	let html = '';
	for (const key of Object.keys(groups).sort()) {
		html += '<div class="text-xs font-bold text-on-surface-variant px-4 py-2 mt-1">' + key + '</div>';
		let sorted = groups[key];
		sorted.sort((a, b) => (b.favorite ? 1 : 0) - (a.favorite ? 1 : 0));
		for (const c of sorted) {
			let name = displayName(c);
			let sub = primaryEmail(c);
			let init = (c.first_name||name)[0];
			let star = c.favorite ? "fa-solid fa-star text-warning" : "fa-regular fa-star text-on-surface-variant opacity-40";
			let sel = (currentDetail && c.id === currentDetail.id) ? " ring-2 ring-primary bg-primary-container/20" : "";
			html += '<div class="flex items-center gap-3 px-4 py-3 hover:bg-surface-container-high rounded-2xl cursor-pointer transition-colors' + sel + '" onclick="showDetail(\'' + c.id + '\')">';
			html += '<button onclick="event.stopPropagation();toggleFavorite(\'' + c.id + '\')" class="shrink-0 border-0 bg-transparent cursor-pointer text-sm px-1"><i class="' + star + '"></i></button>';
			const avatarTheme = DarTheme.tonalize(c.color || "#f5c2e7");
			html += '<div class="w-10 h-10 rounded-full inline-flex items-center justify-center text-sm font-bold shrink-0" style="background:' + avatarTheme.background + ';color:' + avatarTheme.foreground + '">' + esc(init) + '</div>';
			html += '<div class="flex-1 min-w-0"><div class="text-sm font-medium text-on-surface truncate">' + esc(name) + '</div>';
			if (sub) html += '<div class="text-xs text-on-surface-variant truncate">' + esc(sub) + '</div>';
			html += '</div></div>';
		}
	}
	div.innerHTML = html;
}

function refreshSidebar() {
	document.getElementById("all-count").textContent = contacts.length;
	let ungroupedCount = contacts.filter(function(c){ return !(c.groups) || c.groups.length === 0; }).length;
	let el = document.getElementById("sidebar-not-grouped");
	if (el) {
		el.querySelector("[id=ungrouped-count]").textContent = ungroupedCount;
		el.style.display = ungroupedCount === 0 ? "none" : "";
	}
	renderGroupList();
}

function extractGroups() {
	let groupSet = {};
	for (const c of contacts) {
		if (c.groups && c.groups.length) {
			for (const g of c.groups) {
				groupSet[g] = true;
			}
		}
	}
	groups = Object.keys(groupSet).sort();
	renderGroupList();
	// Update ungrouped count and visibility
	let ungroupedCount = contacts.filter(function(c){ return !(c.groups) || c.groups.length === 0; }).length;
	let el = document.getElementById("sidebar-not-grouped");
	if (el) {
		el.querySelector("[id=ungrouped-count]").textContent = ungroupedCount;
		el.style.display = ungroupedCount === 0 ? "none" : "";
	}
}

function renderGroupList() {
	let el = document.getElementById("group-list");
	if (!groups.length) {
		el.innerHTML = '<div class="text-xs text-on-surface-variant px-3 py-2">No groups yet</div>';
		return;
	}
	let html = '';
	for (let gi = 0; gi < groups.length; gi++) {
		let g = groups[gi];
		html += '<div class="flex items-center group relative">';
		html += '<a href="#" onclick="selectGroup(\''+esc(g)+'\');return false" class="flex-1 block px-4 py-2 text-sm text-on-surface-variant rounded-full hover:bg-surface-container-highest cursor-pointer no-underline">';
		html += '<i class="fa-solid fa-user-group text-xs mr-2"></i>'+esc(g)+'</a>';
		html += '<button type="button" aria-label="Manage '+esc(g)+' group" onclick="event.stopPropagation();toggleGroupItemMenu(event,\''+esc(g)+'\')" class="opacity-0 group-hover:opacity-100 w-6 h-6 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center mr-2">';
		html += '<i class="fa-solid fa-ellipsis-vertical text-xs"></i></button>';
		html += '</div>';
	}
	el.innerHTML = html;
}

function selectGroup(name) {
	activeGroup = name;
	let filtered = contacts.filter(c => (c.groups||[]).includes(name));
	renderList(filtered);
}

function selectFilter(f) {
	if (f === "all") { activeGroup = null; renderList(contacts); }
	else if (f === "ungrouped") {
		activeGroup = "ungrouped";
		let ungrouped = contacts.filter(function(c){ return !(c.groups) || c.groups.length === 0; });
		renderList(ungrouped);
	}
}

// Settings
function openSettings() { document.getElementById("contacts-sort-by").value=contactPreferences.sort_by; document.getElementById("settings-modal").classList.remove("hidden"); }
function closeSettings() { document.getElementById("settings-modal").classList.add("hidden"); document.getElementById("book-menu").classList.add("hidden"); }
async function saveContactsSettings() {
	try {
		contactPreferences = await api("PUT", "/preferences", {sort_by:document.getElementById("contacts-sort-by").value});
		renderList(activeGroup ? (activeGroup === "ungrouped" ? contacts.filter(c=>!(c.groups||[]).length) : contacts.filter(c=>(c.groups||[]).includes(activeGroup))) : contacts);
		closeSettings();
		DarUI.toast("Contacts settings saved");
	} catch (err) { DarUI.toast(err.message||"Could not save settings",{tone:"error"}); }
}

function switchSettingsTab(tab) {
	var tg = document.getElementById("tab-general");
	var tb = document.getElementById("tab-books");
	if (tab === "general") {
		tg.classList.add("border-primary", "text-primary");
		tg.classList.remove("border-transparent", "text-on-surface-variant");
		tb.classList.remove("border-primary", "text-primary");
		tb.classList.add("border-transparent", "text-on-surface-variant");
		document.getElementById("panel-general").classList.remove("hidden");
		document.getElementById("panel-books").classList.add("hidden");
	} else {
		tb.classList.add("border-primary", "text-primary");
		tb.classList.remove("border-transparent", "text-on-surface-variant");
		tg.classList.remove("border-primary", "text-primary");
		tg.classList.add("border-transparent", "text-on-surface-variant");
		document.getElementById("panel-books").classList.remove("hidden");
		document.getElementById("panel-general").classList.add("hidden");
	}
}

// Address books
function openBookMenu(e, id) {
	e.stopPropagation();
	activeBookMenu = id;
	let menu = document.getElementById("book-menu");
	let rect = e.target.closest("button").getBoundingClientRect();
	menu.style.top = rect.bottom + "px";
	menu.style.left = (rect.left - 150) + "px";
	menu.classList.remove("hidden");
	let book = addressBooks.find(b => b.id === id);
	let showBtn = document.getElementById("book-menu-show");
	// Checked = book is shown in the contact list; uncheck hides its contacts.
	if (book && book.visible) {
		showBtn.innerHTML = '<i class="fa-solid fa-square-check w-4 text-xs text-primary"></i> Show in list';
	} else {
		showBtn.innerHTML = '<i class="fa-regular fa-square w-4 text-xs"></i> Show in list';
	}
	// Disable delete affordance for default (still handled server-side).
	let delBtn = menu.querySelector("button[onclick*=\"delete\"]");
	if (delBtn) {
		if (id === "default") {
			delBtn.classList.add("opacity-40");
			delBtn.title = "Cannot delete the default address book";
		} else {
			delBtn.classList.remove("opacity-40");
			delBtn.title = "";
		}
	}
}

async function bookAction(action) {
	document.getElementById("book-menu").classList.add("hidden");
	let book = addressBooks.find(b => b.id === activeBookMenu);
	if (!book) return;
	try {
		switch(action) {
			case "copy-link":
				// Downloadable export URL until CardDAV is available.
				copyText(location.origin + apiBase + "/export?book=" + encodeURIComponent(book.id));
				break;
			case "download":
				window.open(apiBase + "/export?book=" + encodeURIComponent(book.id), "_blank");
				break;
			case "show":
				await api("POST", "/books/visibility", {id: book.id, visible: !book.visible});
				await loadBooks();
				await loadContacts();
				break;
			case "rename": {
				let n = await DarUI.inputDialog({title: "Rename address book", label: "Name", value: book.name, confirmLabel: "Rename"});
				if (!n) break;
				await api("POST", "/books/rename", {id: book.id, name: n});
				await loadBooks();
				break;
			}
			case "delete":
				if (book.id === "default") {
					DarUI.toast("The default address book cannot be deleted.", {tone: "error"});
					break;
				}
				if (!await DarUI.confirmDialog({title: "Delete address book?", message: '"' + book.name + '" and all contacts in it will be removed.', confirmLabel: "Delete", tone: "danger"})) break;
				await api("POST", "/books/delete", {id: book.id});
				await loadBooks();
				await loadContacts();
				break;
		}
	} catch (err) {
		DarUI.toast("Address book action failed: " + (err.message || err), {tone: "error"});
	}
}

async function newAddressBook() {
	let name = await DarUI.inputDialog({title: "New address book", label: "Name", confirmLabel: "Create"});
	if (!name) return;
	try {
		await api("POST", "/books", {name: name});
		await loadBooks();
	} catch (err) {
		DarUI.toast("Could not create address book: " + (err.message || err), {tone: "error"});
	}
}

// Groups
function toggleGroupMenu(e) {
	e.stopPropagation();
	let menu = document.getElementById("group-menu");
	let rect = e.target.closest("button").getBoundingClientRect();
	menu.style.position = "fixed";
	menu.style.top = rect.bottom + "px";
	menu.style.left = rect.left + "px";
	menu.classList.toggle("hidden");
	document.getElementById("new-group-name").focus();
}

function createGroup() {
	let input = document.getElementById("new-group-name");
	let name = input.value.trim();
	if (!name) return;
	if (groups.includes(name)) { input.value = ""; return; }
	groups.push(name);
	input.value = "";
	document.getElementById("group-menu").classList.add("hidden");
	renderGroupList();
}

// Groups dropdown
function toggleGroupsDropdown(e) {
	e.stopPropagation();
	var menu = document.getElementById("groups-dropdown-menu");
	menu.classList.toggle("hidden");
	if (!menu.classList.contains("hidden")) renderGroupsDropdown();
}

function closeGroupsDropdown() {
	document.getElementById("groups-dropdown-menu").classList.add("hidden");
}

function renderGroupsDropdown() {
	var list = document.getElementById("groups-dropdown-list");
	var empty = document.getElementById("groups-dropdown-empty");
	if (!groups.length) {
		list.innerHTML = "";
		empty.classList.remove("hidden");
		return;
	}
	empty.classList.add("hidden");
	var html = "";
	for (var i = 0; i < groups.length; i++) {
		var g = groups[i];
		var checked = selectedGroups.indexOf(g) !== -1 ? " checked" : "";
		html += '<label class="flex items-center gap-2 px-3 py-1.5 hover:bg-surface-container-high cursor-pointer">';
		html += '<input type="checkbox" aria-label="Member of '+esc(g)+'" value="'+esc(g)+'"'+checked+' onchange="toggleGroupCheckbox(this)" class="w-4 h-4 accent-primary">';
		html += '<span class="text-sm text-on-surface">'+esc(g)+'</span></label>';
	}
	list.innerHTML = html;
}

function toggleGroupCheckbox(cb) {
	var g = cb.value;
	if (cb.checked) {
		if (selectedGroups.indexOf(g) === -1) selectedGroups.push(g);
	} else {
		selectedGroups = selectedGroups.filter(function(x){ return x !== g; });
	}
	renderGroupsTags();
}

function renderGroupsTags() {
	var tags = document.getElementById("groups-tags");
	var placeholder = document.getElementById("groups-placeholder");
	var html = "";
	for (var i = 0; i < selectedGroups.length; i++) {
		var g = selectedGroups[i];
		html += '<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-primary-container text-primary text-xs font-medium">'+esc(g);
		html += '<button onclick="event.stopPropagation();removeGroupTag(&#39;'+esc(g)+'&#39;)" class="w-4 h-4 rounded-full border-0 bg-transparent text-primary cursor-pointer hover:bg-primary/20 flex items-center justify-center text-[10px]"><i class="fa-solid fa-xmark"></i></button>';
		html += '</span>';
	}
	tags.innerHTML = html;
	placeholder.style.display = selectedGroups.length ? "none" : "";
}

function removeGroupTag(name) {
	selectedGroups = selectedGroups.filter(function(x){ return x !== name; });
	renderGroupsTags();
	var menu = document.getElementById("groups-dropdown-menu");
	if (!menu.classList.contains("hidden")) renderGroupsDropdown();
}

function setSelectedGroups(arr) {
	selectedGroups = (arr || []).slice();
	renderGroupsTags();
}

// Contact helpers (rich multi-value model)
const TYPE_OPTS = ["home","work","cell","other"];
function typeSelectHtml(selected) {
	let s = (selected||"home").toLowerCase();
	return '<select class="typed-type h-10 px-3 rounded-full bg-surface-container text-xs text-on-surface border-0 outline-none">' +
		TYPE_OPTS.map(t => '<option value="'+t+'"'+(t===s?' selected':'')+'>'+t.charAt(0).toUpperCase()+t.slice(1)+'</option>').join('') +
		'</select>';
}
function addTypedRow(kind, value, type) {
	let wrap = document.getElementById(kind === "emails" ? "edit-emails" : "edit-phones");
	let row = document.createElement("div");
	row.className = "flex gap-2 items-center typed-row";
	row.innerHTML = typeSelectHtml(type) +
		'<input type="'+(kind==="emails"?"email":"tel")+'" value="'+esc(value||"")+'" placeholder="'+(kind==="emails"?"Email":"Phone")+'" class="typed-value flex-1 h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30">' +
		'<button type="button" class="w-10 h-10 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center" onclick="this.parentElement.remove()"><i class="fa-solid fa-trash text-xs"></i></button>';
	wrap.appendChild(row);
}
function collectTypedRows(kind) {
	let wrap = document.getElementById(kind === "emails" ? "edit-emails" : "edit-phones");
	let out = [];
	wrap.querySelectorAll(".typed-row").forEach(row => {
		let v = (row.querySelector(".typed-value")||{}).value || "";
		let t = (row.querySelector(".typed-type")||{}).value || "";
		if (v.trim()) out.push({value: v.trim(), types: t ? [t] : []});
	});
	return out;
}
function primaryEmail(c) {
	if (c.emails && c.emails.length) return c.emails[0].value || "";
	return c.email || "";
}
function primaryPhone(c) {
	if (c.phones && c.phones.length) return c.phones[0].value || "";
	return c.phone || "";
}
function displayName(c) {
	let n = ((c.first_name||"") + " " + (c.last_name||"")).trim();
	return n || c.org || primaryEmail(c) || "Unnamed";
}
function fieldBlock(label, typeLabel, value, href) {
	let type = typeLabel ? '<span class="text-xs text-on-surface-variant mr-2">'+esc(typeLabel)+'</span>' : '';
	let body = href
		? '<a class="text-sm text-primary no-underline hover:underline" href="'+esc(href)+'">'+esc(value)+'</a>'
		: '<span class="text-sm text-on-surface">'+esc(value)+'</span>';
	return '<div class="mt-3 p-3 rounded-2xl bg-surface-container"><div class="text-xs font-medium text-on-surface-variant mb-1">'+esc(label)+'</div><div class="flex items-center gap-2">'+type+body+
		'<button onclick="event.stopPropagation();copyText('+JSON.stringify(value)+')" class="shrink-0 w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer text-xs hover:bg-surface-container-highest flex items-center justify-center"><i class="fa-regular fa-copy"></i></button></div></div>';
}

// Contact CRUD
function clearEditForm() {
	["edit-first","edit-last","edit-title","edit-org","edit-street","edit-city","edit-region","edit-postal","edit-country","edit-url","edit-birthday","edit-notes"].forEach(id => {
		let el = document.getElementById(id); if (el) el.value = "";
	});
	document.getElementById("edit-emails").innerHTML = "";
	document.getElementById("edit-phones").innerHTML = "";
	addTypedRow("emails");
	addTypedRow("phones");
	document.getElementById("edit-avatar-img").classList.add("hidden");
	document.getElementById("edit-avatar-placeholder").classList.remove("hidden");
}

function showCreate() {
	currentEdit = null;
	currentDetail = null;
	let defaultGroups = (activeGroup && activeGroup !== "ungrouped" && activeGroup !== "all") ? activeGroup : "";
	clearEditForm();
	setSelectedGroups(defaultGroups ? [defaultGroups] : []);
	document.getElementById("detail-empty").classList.add("hidden");
	document.getElementById("detail-view").classList.add("hidden");
	document.getElementById("detail-edit").classList.remove("hidden");
	document.getElementById("detail-pane").classList.remove("hidden");
	DarUI.shell.openDetail(document.getElementById("detail-pane"), document.activeElement);
}

function showDetail(id, skipPush) {
	currentDetail = contacts.find(c => c.id === id);
	if (!currentDetail) return;
	if (!skipPush) setContactURL(id, false);
	renderList(contacts);
	document.getElementById("detail-empty").classList.add("hidden");
	document.getElementById("detail-edit").classList.add("hidden");
	document.getElementById("detail-view").classList.remove("hidden");
	document.getElementById("detail-pane").classList.remove("hidden");
	DarUI.shell.openDetail(document.getElementById("detail-pane"), document.activeElement);

	let c = currentDetail;
	let fn = displayName(c);
	let init = (c.first_name||fn)[0];
	document.getElementById("detail-avatar").textContent = init;
	const detailColor = DarTheme.tonalize(c.color||"#f5c2e7");
	document.getElementById("detail-avatar").style.background = detailColor.background;
	document.getElementById("detail-avatar").style.color = detailColor.foreground;
	document.getElementById("detail-name").textContent = fn;
	let sub = [];
	if (c.title) sub.push(c.title);
	if (c.org) sub.push(c.org);
	document.getElementById("detail-subtitle").textContent = sub.join(" · ");

	let h = "";
	let emails = (c.emails && c.emails.length) ? c.emails : (c.email ? [{value:c.email,types:[]}] : []);
	emails.forEach(e => { if (e.value) h += fieldBlock("Email", (e.types&&e.types[0])||"", e.value, "mailto:"+e.value); });
	let phones = (c.phones && c.phones.length) ? c.phones : (c.phone ? [{value:c.phone,types:[]}] : []);
	phones.forEach(p => { if (p.value) h += fieldBlock("Phone", (p.types&&p.types[0])||"", p.value, "tel:"+p.value); });
	if (c.addresses && c.addresses.length) {
		c.addresses.forEach(a => {
			let parts = [a.street, a.extended, a.locality, a.region, a.postal_code, a.country].filter(Boolean);
			if (parts.length) h += fieldBlock("Address", (a.types&&a.types[0])||"", parts.join(", "), null);
		});
	}
	if (c.title) h += fieldBlock("Title", "", c.title, null);
	if (c.org) h += fieldBlock("Company", "", c.org, null);
	if (c.url) h += fieldBlock("Website", "", c.url, (c.url.match(/^https?:/) ? c.url : "https://"+c.url));
	if (c.urls && c.urls.length) c.urls.forEach(u => { if (u.value && u.value !== c.url) h += fieldBlock("Website", (u.types&&u.types[0])||"", u.value, u.value.match(/^https?:/) ? u.value : "https://"+u.value); });
	if (c.birthday) h += fieldBlock("Birthday", "", c.birthday, null);
	if (c.notes) h += fieldBlock("Notes", "", c.notes, null);
	if (c.groups && c.groups.length) h += fieldBlock("Contact groups", "", c.groups.join(", "), null);
	if (c.updated_at) {
		let d = new Date(c.updated_at * 1000);
		h += '<div class="mt-6 text-xs text-on-surface-variant">Last modified '+esc(d.toLocaleString())+'</div>';
	}
	document.getElementById("detail-fields").innerHTML = h || '<div class="text-sm text-on-surface-variant">No details yet</div>';
}

function editCurrent() {
	if (!currentDetail) return;
	let c = currentDetail;
	currentEdit = c.id;
	document.getElementById("edit-first").value = c.first_name||"";
	document.getElementById("edit-last").value = c.last_name||"";
	document.getElementById("edit-title").value = c.title||"";
	document.getElementById("edit-org").value = c.org||"";
	document.getElementById("edit-url").value = c.url||"";
	document.getElementById("edit-birthday").value = c.birthday||"";
	document.getElementById("edit-notes").value = c.notes||"";
	let a = (c.addresses && c.addresses[0]) || {};
	document.getElementById("edit-street").value = a.street||"";
	document.getElementById("edit-city").value = a.locality||"";
	document.getElementById("edit-region").value = a.region||"";
	document.getElementById("edit-postal").value = a.postal_code||"";
	document.getElementById("edit-country").value = a.country||"";
	document.getElementById("edit-emails").innerHTML = "";
	document.getElementById("edit-phones").innerHTML = "";
	let emails = (c.emails && c.emails.length) ? c.emails : (c.email ? [{value:c.email,types:["home"]}] : [{value:"",types:["home"]}]);
	emails.forEach(e => addTypedRow("emails", e.value, (e.types&&e.types[0])||"home"));
	let phones = (c.phones && c.phones.length) ? c.phones : (c.phone ? [{value:c.phone,types:["cell"]}] : [{value:"",types:["cell"]}]);
	phones.forEach(p => addTypedRow("phones", p.value, (p.types&&p.types[0])||"cell"));
	setSelectedGroups(c.groups || []);
	document.getElementById("detail-view").classList.add("hidden");
	document.getElementById("detail-edit").classList.remove("hidden");
}

function cancelEdit() {
	if (currentDetail) showDetail(currentDetail.id);
	else {
		document.getElementById("detail-edit").classList.add("hidden");
		document.getElementById("detail-empty").classList.remove("hidden");
	}
}

function saveContact() {
	let emails = collectTypedRows("emails");
	let phones = collectTypedRows("phones");
	let street = document.getElementById("edit-street").value.trim();
	let city = document.getElementById("edit-city").value.trim();
	let region = document.getElementById("edit-region").value.trim();
	let postal = document.getElementById("edit-postal").value.trim();
	let country = document.getElementById("edit-country").value.trim();
	let addresses = [];
	if (street || city || region || postal || country) {
		addresses.push({types:["home"], street: street, locality: city, region: region, postal_code: postal, country: country});
	}
	let data = {
		id: currentEdit || "",
		first_name: document.getElementById("edit-first").value,
		last_name: document.getElementById("edit-last").value,
		title: document.getElementById("edit-title").value,
		org: document.getElementById("edit-org").value,
		emails: emails,
		phones: phones,
		addresses: addresses,
		url: document.getElementById("edit-url").value,
		birthday: document.getElementById("edit-birthday").value,
		notes: document.getElementById("edit-notes").value,
		groups: selectedGroups.slice(),
	};
	if (currentEdit && currentDetail) data.favorite = !!currentDetail.favorite;
	let path = currentEdit ? "/update" : "/create";
	api("POST", path, data).then(r => {
		return loadContacts().then(() => {
			if (r && r.id) showDetail(r.id);
			else cancelEdit();
		});
	});
}

async function deleteCurrent() {
	if (!currentDetail) return;
	if (!await DarUI.confirmDialog({title: "Delete contact?", message: "This contact will be permanently removed.", confirmLabel: "Delete", tone: "danger"})) return;
	api("POST", "/delete?id=" + currentDetail.id).then(() => {
		clearDetailPane();
		setContactURL(null, false);
		loadContacts();
	});
}

async function cloneCurrent() {
	if (!currentDetail) return;
	let c = currentDetail;
	let r = await api("POST", "/create", {
		first_name: c.first_name||"", last_name: c.last_name||"",
		title: c.title||"", org: c.org||"", notes: c.notes||"",
		url: c.url||"", birthday: c.birthday||"",
		emails: c.emails||(c.email?[{value:c.email,types:[]}]:[]),
		phones: c.phones||(c.phone?[{value:c.phone,types:[]}]:[]),
		addresses: c.addresses||[],
		groups: c.groups||[]
	});
	if (r.id) { await loadContacts(); showDetail(r.id); }
}

async function toggleFavorite(id) {
	let c = contacts.find(x => x.id === id);
	if (!c) return;
	c.favorite = !c.favorite;
	await api("POST", "/update", {id: id, favorite: c.favorite});
	renderList(contacts);
	if (currentDetail && currentDetail.id === id) showDetail(id);
}

function downloadCurrent() {
	if (currentDetail) window.open(apiBase + "/export?id=" + currentDetail.id, "_blank");
}

function searchContacts() {
	let q = document.getElementById("search-input").value.toLowerCase();
	if (!q) { renderList(contacts); return; }
	let filtered = contacts.filter(c => {
		let parts = [c.first_name, c.last_name, c.org, c.title, c.notes, c.url, c.birthday, primaryEmail(c), primaryPhone(c)];
		(c.emails||[]).forEach(e => parts.push(e.value));
		(c.phones||[]).forEach(p => parts.push(p.value));
		(c.groups||[]).forEach(g => parts.push(g));
		return parts.join(" ").toLowerCase().includes(q);
	});
	renderList(filtered);
}

function importContacts() {
	let i = document.createElement("input"); i.type = "file"; i.accept = ".vcf,.vcard";
	i.onchange = async function() {
		if (!i.files || !i.files[0]) return;
		let f = new FormData(); f.append("file", i.files[0]);
		try {
			let r = await fetch(apiBase + "/import", {method: "POST", body: f});
			let d = await r.json();
			if (d.imported > 0) { loadContacts(); DarUI.toast("Imported " + d.imported + " contacts"); }
			else DarUI.toast("No contacts found in file", {tone: "error"});
		} catch(e) { DarUI.toast("Import failed: " + e.message, {tone: "error"}); }
	};
	i.click();
}

function copyCardDAV() {
	let u = document.getElementById("carddav-url");
	if (!u) return;
	u.select();
	navigator.clipboard.writeText(u.value).catch(function() {
		try { document.execCommand("copy"); } catch (e) {}
	});
}

function pickAvatar() {
	let i = document.createElement("input"); i.type = "file"; i.accept = "image/*";
	i.onchange = function() {
		if (!i.files || !i.files[0]) return;
		let r = new FileReader();
		r.onload = function(e) {
			document.getElementById("edit-avatar-img").src = e.target.result;
			document.getElementById("edit-avatar-img").classList.remove("hidden");
			document.getElementById("edit-avatar-placeholder").classList.add("hidden");
		};
		r.readAsDataURL(i.files[0]);
	};
	i.click();
}

let activeGroupItem = null;

function toggleGroupItemMenu(e, name) {
	e.stopPropagation();
	closeGroupItemMenu();
	if (activeGroupItem === name) { activeGroupItem = null; return; }
	activeGroupItem = name;
	let menu = document.getElementById("group-item-menu");
	let rect = e.target.closest("button").getBoundingClientRect();
	menu.style.top = rect.bottom + "px";
	menu.style.left = rect.left + "px";
	menu.style.position = "fixed";
	menu.classList.remove("hidden");
}

function closeGroupItemMenu() {
	activeGroupItem = null;
	document.getElementById("group-item-menu").classList.add("hidden");
}

async function renameGroup() {
	let oldName = activeGroupItem;
	if (!oldName) return;
	closeGroupItemMenu();
	let newName = await DarUI.inputDialog({title: "Rename group", label: "Name", value: oldName, confirmLabel: "Rename"});
	if (!newName || newName === oldName) return;
	// Update groups array
	let idx = groups.indexOf(oldName);
	if (idx !== -1) groups[idx] = newName;
	// Update all contacts' groups
	for (let c of contacts) {
		if (c.groups && c.groups.includes(oldName)) {
			c.groups = c.groups.map(function(g){ return g === oldName ? newName : g; });
			api("POST", "/update", {id: c.id, groups: c.groups});
		}
	}
	renderGroupList();
	if (currentDetail) showDetail(currentDetail.id);
}

async function deleteGroup() {
	let name = activeGroupItem;
	if (!name) return;
	closeGroupItemMenu();
	if (!await DarUI.confirmDialog({title: "Delete group?", message: '"' + name + '" will be removed from every contact.', confirmLabel: "Delete", tone: "danger"})) return;
	// Remove group from groups array
	groups = groups.filter(function(g){ return g !== name; });
	// Remove group from all contacts
	for (let c of contacts) {
		if (c.groups && c.groups.includes(name)) {
			c.groups = c.groups.filter(function(g){ return g !== name; });
			api("POST", "/update", {id: c.id, groups: c.groups});
		}
	}
	renderGroupList();
	if (activeGroup === name) { activeGroup = null; renderList(contacts); }
}

let addMembersGroup = null;
let addMembersChecked = {};

function addMembersToGroup() {
	addMembersGroup = activeGroupItem;
	closeGroupItemMenu();
	if (!addMembersGroup) return;

	document.getElementById("add-members-title").textContent = "Add members to " + addMembersGroup;
	document.getElementById("add-members-modal").classList.remove("hidden");
	document.getElementById("add-members-search").value = "";
	addMembersChecked = {};
	renderAddMembersList(contacts);
}

function closeAddMembersModal() {
	document.getElementById("add-members-modal").classList.add("hidden");
	addMembersGroup = null;
}

function renderAddMembersList(list) {
	var html = "";
	for (var i = 0; i < list.length; i++) {
		var c = list[i];
		// Skip contacts already in this group
		if (c.groups && c.groups.indexOf(addMembersGroup) !== -1) continue;
		var name = (c.first_name||"") + " " + (c.last_name||"");
		name = name.trim() || c.org || c.email || "Unnamed";
		var init = (c.first_name||name)[0];
		var checked = addMembersChecked[c.id] ? " checked" : "";
		html += '<label class="flex items-center gap-3 p-2 rounded-full hover:bg-surface-container-highest cursor-pointer">';
		html += '<input type="checkbox" aria-label="Select '+esc(c.fn||c.full_name||"contact")+'" value="'+c.id+'"'+checked+' onchange="addMembersChecked[&#39;'+c.id+'&#39;]=this.checked" class="w-4 h-4 accent-primary">';
		var memberColor = DarTheme.tonalize(c.color||"#f5c2e7");
		html += '<div class="w-8 h-8 rounded-full inline-flex items-center justify-center text-xs font-bold shrink-0" style="background:'+memberColor.background+';color:'+memberColor.foreground+'">'+init+'</div>';
		html += '<div class="flex-1 min-w-0"><div class="text-sm text-on-surface truncate">'+esc(name)+'</div>';
		if (c.email) html += '<div class="text-xs text-on-surface-variant truncate">'+esc(c.email)+'</div>';
		html += '</div></label>';
	}
	if (!html) html = '<div class="text-center text-on-surface-variant py-8 text-sm">All contacts are already in this group.</div>';
	document.getElementById("add-members-list").innerHTML = html;
}

function filterAddMemberList() {
	var q = document.getElementById("add-members-search").value.toLowerCase();
	if (!q) { renderAddMembersList(contacts); return; }
	var filtered = contacts.filter(function(c) {
		return (c.first_name+" "+c.last_name+" "+c.email).toLowerCase().indexOf(q) !== -1;
	});
	renderAddMembersList(filtered);
}

function saveGroupMembers() {
	if (!addMembersGroup) return;
	var promises = [];
	for (var id in addMembersChecked) {
		if (!addMembersChecked[id]) continue;
		var c = contacts.find(function(x){ return x.id === id; });
		if (!c) continue;
		if (!c.groups) c.groups = [];
		if (c.groups.indexOf(addMembersGroup) === -1) {
			c.groups.push(addMembersGroup);
		}
		promises.push(api("POST", "/update", {id: c.id, groups: c.groups.slice()}));
	}
	Promise.all(promises).then(function() {
		closeAddMembersModal();
		loadContacts();
	});
}

// Close menus on click outside
document.addEventListener("click", function(e) {
	if (!e.target.closest("#book-menu")) document.getElementById("book-menu").classList.add("hidden");
	if (!e.target.closest("#group-menu") && !e.target.closest("[onclick*='toggleGroupMenu']")) document.getElementById("group-menu").classList.add("hidden");
	if (!e.target.closest("#group-item-menu")) closeGroupItemMenu();
	if (!e.target.closest("#groups-dropdown")) closeGroupsDropdown();
});

window.addEventListener("popstate", function() {
	applyPathToDetail(false);
});

// Contact list column width (Nextcloud-sized default, user-resizable)
const LIST_WIDTH_KEY = "dar-contacts-list-width";
const LIST_WIDTH_DEFAULT = 300;
const LIST_WIDTH_MIN = 220;
const LIST_WIDTH_MAX = 520;

function applyListWidth(px) {
	let pane = document.getElementById("contact-list-pane");
	if (!pane) return;
	let w = Math.round(px);
	if (w < LIST_WIDTH_MIN) w = LIST_WIDTH_MIN;
	if (w > LIST_WIDTH_MAX) w = LIST_WIDTH_MAX;
	// On small screens list is full width; only constrain on lg+.
	if (window.matchMedia("(min-width: 1024px)").matches) {
		pane.style.width = w + "px";
		pane.style.flex = "0 0 " + w + "px";
	} else {
		pane.style.width = "";
		pane.style.flex = "";
	}
	return w;
}

function initListResizer() {
	let stored = parseInt(localStorage.getItem(LIST_WIDTH_KEY) || "", 10);
	applyListWidth(isFinite(stored) ? stored : LIST_WIDTH_DEFAULT);

	let handle = document.getElementById("list-resizer");
	let pane = document.getElementById("contact-list-pane");
	if (!handle || !pane) return;

	let dragging = false;
	let startX = 0;
	let startW = 0;

	function onMove(e) {
		if (!dragging) return;
		let clientX = e.touches ? e.touches[0].clientX : e.clientX;
		let next = startW + (clientX - startX);
		applyListWidth(next);
		e.preventDefault();
	}
	function onUp() {
		if (!dragging) return;
		dragging = false;
		document.body.classList.remove("select-none");
		document.body.style.cursor = "";
		let w = pane.getBoundingClientRect().width;
		localStorage.setItem(LIST_WIDTH_KEY, String(Math.round(w)));
		handle.classList.remove("bg-primary/50");
	}

	handle.addEventListener("mousedown", function(e) {
		if (!window.matchMedia("(min-width: 1024px)").matches) return;
		dragging = true;
		startX = e.clientX;
		startW = pane.getBoundingClientRect().width;
		document.body.classList.add("select-none");
		document.body.style.cursor = "col-resize";
		handle.classList.add("bg-primary/50");
		e.preventDefault();
	});
	handle.addEventListener("touchstart", function(e) {
		if (!window.matchMedia("(min-width: 1024px)").matches) return;
		dragging = true;
		startX = e.touches[0].clientX;
		startW = pane.getBoundingClientRect().width;
		document.body.classList.add("select-none");
		e.preventDefault();
	}, {passive: false});

	document.addEventListener("mousemove", onMove);
	document.addEventListener("mouseup", onUp);
	document.addEventListener("touchmove", onMove, {passive: false});
	document.addEventListener("touchend", onUp);
	window.addEventListener("resize", function() {
		let stored = parseInt(localStorage.getItem(LIST_WIDTH_KEY) || "", 10);
		applyListWidth(isFinite(stored) ? stored : LIST_WIDTH_DEFAULT);
	});
}

initListResizer();
(function() {
	var el = document.getElementById("carddav-url");
	if (el) el.value = window.location.origin + "/dav/";
})();
loadContacts();
</script>`
}
