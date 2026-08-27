package main

import (
	"context"
	"encoding/json"
	"fmt"
	"html/template"
	"net/http"
	"path"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"github.com/anoosa/dar/internal/webui"
	"github.com/go-chi/chi/v5"
)

func handleAppearanceSettings(service *theme.Service) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		switch r.Method {
		case http.MethodGet:
			response, err := service.Response(r.Context(), user.ID)
			if err != nil {
				http.Error(w, "appearance unavailable", http.StatusInternalServerError)
				return
			}
			writeJSON(w, response)
		case http.MethodPut:
			var draft theme.Draft
			if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&draft); err != nil {
				http.Error(w, `{"error":"invalid appearance"}`, http.StatusBadRequest)
				return
			}
			resolved, err := service.Save(r.Context(), user.ID, draft)
			if err != nil {
				w.Header().Set("Content-Type", "application/json")
				w.WriteHeader(http.StatusBadRequest)
				_ = json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
				return
			}
			writeJSON(w, theme.Response{Settings: resolved})
		default:
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		}
	}
}

func handleAppearancePreview(service *theme.Service) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		var draft theme.Draft
		if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&draft); err != nil {
			http.Error(w, `{"error":"invalid appearance"}`, http.StatusBadRequest)
			return
		}
		resolved, err := service.Preview(r.Context(), user.ID, draft)
		if err != nil {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			_ = json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
			return
		}
		writeJSON(w, resolved)
	}
}

func handleAppearanceWallpaper(service *theme.Service) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.NotFound(w, r)
			return
		}
		service.ServeWallpaper(w, r, user.ID)
	}
}

func handleSettings(s *server.Server, reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Redirect(w, r, "/login", http.StatusSeeOther)
			return
		}
		section := settingsSection(r.URL.Path)
		content, _ := webui.ExternalizeLastScript(string(settingsPageHTML(user, section)), "/assets/settings.js")
		if err := webui.RenderRequest(w, r, webui.PageData{
			Title:      "Settings — Dar",
			CurrentApp: "settings",
			AppBar:     reg.AppBarHTML(),
			UserMenu:   webui.UserMenuHTML(user.DisplayName),
			Content:    content,
		}); err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
		}
	}
}

func handleSettingsAsset(w http.ResponseWriter, r *http.Request) {
	_, script := webui.ExternalizeLastScript(string(settingsScripts()), "/assets/settings.js")
	webui.ServeAppJavaScript(w, r, "settings.js", script)
}

func settingsSection(path string) string {
	path = strings.TrimPrefix(path, "/settings")
	path = strings.Trim(path, "/")
	if path == "" || path == "personal" {
		return "personal"
	}
	if path == "security" {
		return "security"
	}
	if path == "appearance" {
		return "appearance"
	}
	return "personal"
}

func settingsNavItem(href, icon, label string, active bool) string {
	cls := "flex items-center gap-3 rounded-full px-4 h-10 text-sm no-underline transition-colors hover:bg-surface-container-highest text-on-surface-variant"
	if active {
		cls = "flex items-center gap-3 rounded-full px-4 h-10 text-sm no-underline bg-primary-container text-primary font-medium"
	}
	return fmt.Sprintf(`<a href="%s" class="%s"><i class="%s w-5 text-center"></i><span>%s</span></a>`,
		href, cls, icon, label)
}

func settingsPageHTML(user *auth.User, section string) template.HTML {
	personalActive := section == "personal"
	securityActive := section == "security"
	appearanceActive := section == "appearance"

	nav := settingsNavItem("/settings", "fa-solid fa-user", "Personal info", personalActive) +
		settingsNavItem("/settings/appearance", "fa-solid fa-palette", "Appearance", appearanceActive) +
		settingsNavItem("/settings/security", "fa-solid fa-shield-halved", "Security", securityActive)

	var main string
	if section == "appearance" {
		main = appearancePaneHTML()
	} else if section == "security" {
		main = securityPaneHTML()
	} else {
		main = personalPaneHTML(user)
	}

	return template.HTML(`
<div id="settings-app" data-dar-shell class="flex h-full min-h-0 bg-surface">
	<aside data-dar-sidebar data-dar-sidebar-key="settings" aria-label="Settings navigation" class="flex flex-col w-[280px] shrink-0 border-r border-outline-variant bg-surface-container-low h-full">
		<div class="px-4 pt-5 pb-2">
			<div class="text-xs font-bold uppercase tracking-wider text-on-surface-variant">Personal</div>
		</div>
		<nav class="p-3 flex flex-col gap-1 flex-1">
			` + nav + `
		</nav>
	</aside>
	<div data-dar-master-pane class="flex-1 min-w-0 flex flex-col overflow-hidden bg-surface">
		<!-- Mobile section switcher -->
		<div class="sm:hidden flex gap-1 p-3 border-b border-outline-variant overflow-x-auto">
			` + nav + `
		</div>
		<div class="flex-1 min-h-0 overflow-y-auto overscroll-contain p-5 [scrollbar-gutter:stable] md:p-10">
			` + main + `
		</div>
	</div>
</div>
` + settingsScripts())
}

func appearancePaneHTML() string {
	return `
<div id="appearance-settings" class="max-w-4xl space-y-7">
	<div data-dar-sidebar-toggle-host class="flex items-start gap-3">
		<div class="min-w-0"><h1 class="text-2xl font-semibold text-on-surface">Appearance</h1>
		<p class="text-sm text-on-surface-variant mt-1">Dar uses Catppuccin Mocha with a calm optional backdrop.</p></div>
	</div>
	<section class="space-y-4" aria-labelledby="appearance-wallpaper-title">
		<div><h2 id="appearance-wallpaper-title" class="text-base font-semibold">Wallpaper</h2><p class="text-sm text-on-surface-variant">Use an existing image from Files or upload one to /Wallpapers.</p></div>
		<div id="appearance-wallpaper-preview" class="dar-glass relative flex min-h-52 items-center justify-center overflow-hidden rounded-2xl">
			<img id="appearance-wallpaper-image" class="hidden absolute inset-0 h-full w-full object-cover" alt="Selected wallpaper preview">
			<p id="appearance-wallpaper-empty" class="relative z-10 text-sm text-on-surface-variant">No wallpaper selected</p>
		</div>
		<div class="flex flex-wrap gap-2">
			<button id="appearance-choose-wallpaper" type="button" class="h-10 rounded-full bg-primary px-5 text-sm font-medium text-on-primary"><i class="fa-solid fa-folder-open mr-2"></i>Choose from Files</button>
			<button id="appearance-upload-wallpaper" type="button" class="h-10 rounded-full bg-secondary-container px-5 text-sm font-medium text-on-secondary-container"><i class="fa-solid fa-upload mr-2"></i>Upload</button>
			<button id="appearance-remove-wallpaper" type="button" class="h-10 rounded-full px-4 text-error hover:bg-error-container">Remove</button>
			<input id="appearance-upload-input" type="file" accept="image/jpeg,image/png,image/gif" class="hidden">
		</div>
		<label class="block rounded-2xl bg-surface-container-low p-4">
			<span class="flex justify-between text-sm"><span>Wallpaper visibility</span><output id="appearance-visibility-output"></output></span>
			<input id="appearance-visibility" type="range" min="0" max="100" class="mt-3 w-full accent-primary">
		</label>
	</section>
	<div id="appearance-message" class="hidden rounded-2xl px-4 py-3 text-sm" role="status" aria-live="polite"></div>
	<button id="appearance-save" type="button" class="h-11 rounded-full bg-primary px-6 font-medium text-on-primary">Save appearance</button>
</div>
<script src="/files/assets/picker.js"></script>`
}

func personalField(id, label, typ, value, placeholder, hint string, multiline bool) string {
	labelHTML := fmt.Sprintf(`<label for="%s" class="block text-sm font-medium text-on-surface mb-1.5">%s</label>`, id, label)
	hintHTML := ""
	if hint != "" {
		hintHTML = fmt.Sprintf(`<p class="text-xs text-on-surface-variant mt-1.5">%s</p>`, hint)
	}
	cls := "w-full px-4 rounded-2xl bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant"
	if multiline {
		return fmt.Sprintf(`
		<div>
			%s
			<textarea id="%s" name="%s" rows="3" maxlength="2000" placeholder="%s" class="%s py-3 resize-y">%s</textarea>
			%s
		</div>`, labelHTML, id, id, template.HTMLEscapeString(placeholder), cls, value, hintHTML)
	}
	inputCls := cls + " h-12 rounded-full"
	return fmt.Sprintf(`
		<div>
			%s
			<input id="%s" name="%s" type="%s" maxlength="500" value="%s" placeholder="%s" class="%s">
			%s
		</div>`, labelHTML, id, id, typ, value, template.HTMLEscapeString(placeholder), inputCls, hintHTML)
}

func personalPaneHTML(user *auth.User) string {
	name := template.HTMLEscapeString(user.DisplayName)
	email := template.HTMLEscapeString(user.Email)
	initial := profileInitial(user.DisplayName)
	p := user.Profile
	esc := template.HTMLEscapeString
	adminStatus := "Standard user"
	if user.IsAdmin {
		adminStatus = "Administrator (managed by your identity provider)"
	}

	return `
<div class="max-w-xl space-y-6">
	<div data-dar-sidebar-toggle-host class="flex items-start gap-3">
		<div class="min-w-0"><h1 class="text-2xl font-semibold text-on-surface">Personal info</h1>
		<p class="text-sm text-on-surface-variant mt-1">This information is used on your profile and creates your <strong class="font-medium text-on-surface">(Me)</strong> contact in Contacts.</p></div>
	</div>

	<form id="personal-form" class="space-y-5" onsubmit="savePersonal(event)">
		<div class="flex items-center gap-4">
			<div class="w-16 h-16 rounded-full bg-primary-container text-primary inline-flex items-center justify-center text-2xl font-medium shrink-0">` + initial + `</div>
			<div class="text-sm text-on-surface-variant">Avatar comes from your login provider when available.</div>
		</div>

		<div>
			<label for="personal-name" class="block text-sm font-medium text-on-surface mb-1.5">Full name</label>
			<input id="personal-name" name="display_name" type="text" required maxlength="120" value="` + name + `"
				class="w-full h-12 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant"
				placeholder="Your name">
			<p class="text-xs text-on-surface-variant mt-1.5">Saved as a contact named “Your name (Me)”.</p>
		</div>
		<div>
			<label class="block text-sm font-medium text-on-surface mb-1.5">Account access</label>
			<div class="w-full min-h-12 px-4 py-3 rounded-2xl bg-surface-container-high text-sm text-on-surface-variant">` + template.HTMLEscapeString(adminStatus) + `</div>
		</div>

		<div>
			<label for="personal-email" class="block text-sm font-medium text-on-surface mb-1.5">Email</label>
			<input id="personal-email" type="email" readonly value="` + email + `"
				class="w-full h-12 px-4 rounded-full bg-surface-container-high text-sm text-on-surface-variant border-0 outline-none cursor-not-allowed">
			<p class="text-xs text-on-surface-variant mt-1.5">From your identity provider. Your immutable username or this verified email can be used with app passwords.</p>
		</div>

		` + personalField("personal-phone", "Phone number", "tel", esc(p.Phone), "+1 555 0100", "", false) + `
		` + personalField("personal-address", "Address", "text", esc(p.Address), "Street, city, country", "", false) + `
		` + personalField("personal-website", "Website", "url", esc(p.Website), "https://example.com", "", false) + `
		` + personalField("personal-twitter", "Twitter", "text", esc(p.Twitter), "@username", "", false) + `
		` + personalField("personal-fediverse", "Fediverse", "text", esc(p.Fediverse), "@you@example.social", "", false) + `
		` + personalField("personal-organisation", "Organisation", "text", esc(p.Organisation), "Company or group", "", false) + `
		` + personalField("personal-role", "Role", "text", esc(p.Role), "Your job title", "", false) + `
		` + personalField("personal-headline", "Headline", "text", esc(p.Headline), "A short tagline", "", false) + `
		` + personalField("personal-biography", "Biography", "text", esc(p.Biography), "A few words about yourself", "", true) + `
		` + personalField("personal-location", "Location", "text", esc(p.Location), "City, country", "", false) + `
		` + personalField("personal-birthday", "Date of birth", "date", esc(p.Birthday), "", "", false) + `
		` + personalField("personal-language", "Language", "text", esc(p.Language), "en", "BCP 47 language tag (e.g. en, de, fr).", false) + `
		` + personalField("personal-locale", "Locale", "text", esc(p.Locale), "en_US", "Locale for number and date formats (e.g. en_US, de_DE).", false) + `
		<label class="flex items-center justify-between gap-4 rounded-2xl bg-surface-container-low px-4 py-3 text-sm cursor-pointer">
			<span><span class="block font-medium text-on-surface">Use 24-hour time</span><span class="block text-xs text-on-surface-variant mt-1">Display times as 13:00 instead of 1:00 PM across Dar.</span></span>
			<input id="personal-time-format-24h" type="checkbox" class="w-5 h-5 accent-primary">
		</label>

		<div id="personal-msg" class="hidden text-sm"></div>

		<button type="submit" class="h-10 px-6 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90">
			Save
		</button>
	</form>
</div>
`
}

func securityPaneHTML() string {
	return `
<div class="max-w-xl space-y-6">
	<div data-dar-sidebar-toggle-host class="flex items-start gap-3">
		<div class="min-w-0"><h1 class="text-2xl font-semibold text-on-surface">Security</h1>
		<p class="text-sm text-on-surface-variant mt-1">App passwords for CardDAV, CalDAV, WebDAV, and other clients.</p></div>
	</div>

	<section class="space-y-4">
		<div>
			<h2 class="text-base font-semibold text-on-surface">App passwords</h2>
			<p class="text-sm text-on-surface-variant mt-1">
				Create a named password for each device or app (DAVx5, Thunderbird, …).
				<strong class="font-medium text-on-surface">Username</strong> = your immutable Dar username
				or verified account email.
				<strong class="font-medium text-on-surface">Password</strong> = the full
				<code class="font-mono text-xs">dar_…</code> secret shown <em>once</em> when created
				(not the truncated prefix in the list below).
			</p>
			<p class="text-sm text-on-surface-variant">
				CardDAV base URL: <code class="font-mono text-xs">/dav/</code>
				(or server root — discovery redirects via <code class="font-mono text-xs">/.well-known/carddav</code>).
			</p>
		</div>

		<div id="app-pass-reveal" class="hidden rounded-2xl bg-primary-container/40 border border-outline-variant p-4 space-y-2">
			<div class="text-sm font-medium text-on-surface">Copy your new app password now</div>
			<p class="text-xs text-on-surface-variant">It will not be shown again. Store it in your client immediately.</p>
			<div class="flex gap-2 items-center">
				<input id="app-pass-token" readonly class="flex-1 h-10 px-4 rounded-full bg-surface text-sm font-mono text-on-surface border-0 outline-none">
				<button type="button" onclick="copyAppToken()" class="h-10 px-4 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90 shrink-0">Copy</button>
			</div>
		</div>

		<form id="app-pass-form" class="space-y-3" onsubmit="createAppPassword(event)">
			<div class="flex flex-col sm:flex-row gap-2"><input id="app-pass-name" type="text" required maxlength="80" placeholder="App name (e.g. DAVx5 phone)"
				class="flex-1 h-10 px-4 rounded-full bg-surface-container text-sm text-on-surface border-0 outline-none focus:ring-2 focus:ring-primary/30 placeholder:text-on-surface-variant">
			<button type="submit" class="h-10 px-5 rounded-full bg-primary text-on-primary text-sm font-medium border-0 cursor-pointer hover:opacity-90 shrink-0">
				Create new app password
			</button></div>
			<fieldset><legend class="text-sm font-medium mb-2">Access scopes</legend><div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="files" class="accent-primary"> Files</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="calendar" class="accent-primary"> Calendar</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="contacts" class="accent-primary"> Contacts</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="todo" class="accent-primary"> Todo</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="notes" class="accent-primary"> Notes</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="news" class="accent-primary"> News</label>
				<label class="text-sm"><input type="checkbox" name="app-pass-scope" value="email" class="accent-primary"> Email</label>
			</div></fieldset>
		</form>

		<div id="app-pass-error" class="hidden text-sm text-error"></div>
		<div id="app-pass-list" class="space-y-2">
			<div class="text-sm text-on-surface-variant py-2">Loading…</div>
		</div>
	</section>
</div>
`
}

func settingsScripts() string {
	return `
<script>
function settingsNode(tag, classes, text) {
	const node = document.createElement(tag);
	if (classes) node.className = classes;
	if (text !== undefined) node.textContent = text;
	return node;
}
function fmtDate(iso) {
	if (!iso || iso.indexOf("0001") === 0) return "—";
	try {
		const t = new Date(iso);
		if (isNaN(t.getTime()) || t.getTime() === 0) return "—";
		return t.toLocaleString();
	} catch (e) { return "—"; }
}

function personalVal(id) {
	const el = document.getElementById(id);
	return el ? (el.value || "").trim() : "";
}
async function savePersonal(e) {
	e.preventDefault();
	const msg = document.getElementById("personal-msg");
	msg.classList.add("hidden");
	const display_name = personalVal("personal-name");
	if (!display_name) return;
	const body = {
		display_name: display_name,
		phone: personalVal("personal-phone"),
		address: personalVal("personal-address"),
		website: personalVal("personal-website"),
		twitter: personalVal("personal-twitter"),
		fediverse: personalVal("personal-fediverse"),
		organisation: personalVal("personal-organisation"),
		role: personalVal("personal-role"),
		headline: personalVal("personal-headline"),
		biography: personalVal("personal-biography"),
		location: personalVal("personal-location"),
		birthday: personalVal("personal-birthday"),
		language: personalVal("personal-language"),
		locale: personalVal("personal-locale"),
	};
	const timeFormat24h = !!document.getElementById("personal-time-format-24h")?.checked;
	try {
	const r = await fetch("/api/settings/personal", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify(body),
		});
		const d = await r.json().catch(() => ({}));
		if (!r.ok) throw new Error(d.error || "Save failed");
		await fetch("/api/settings/time-format", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({time_format_24h:timeFormat24h})});
		msg.textContent = "Saved. Your contact “" + (d.contact_name || (display_name + " (Me)")) + "” is up to date in Contacts.";
		msg.className = "text-sm text-primary";
		msg.classList.remove("hidden");
	} catch (err) {
		msg.textContent = err.message || String(err);
		msg.className = "text-sm text-error";
		msg.classList.remove("hidden");
	}
}

async function loadAppPasswords() {
	const el = document.getElementById("app-pass-list");
	if (!el) return;
	try {
		const r = await fetch("/api/settings/app-passwords");
		if (!r.ok) throw new Error("Failed to load");
		const d = await r.json();
		const list = d.app_passwords || [];
		el.replaceChildren();
		if (!list.length) {
			el.appendChild(settingsNode("div", "text-sm text-on-surface-variant py-3", "No app passwords yet."));
			return;
		}
		const container = settingsNode("div", "rounded-2xl overflow-hidden border border-outline-variant divide-y divide-outline-variant");
		for (const p of list) {
			const row = settingsNode("div", "flex flex-wrap items-center gap-3 px-4 py-3 bg-surface-container-low");
			const details = settingsNode("div", "flex-1 min-w-0");
			details.appendChild(settingsNode("div", "text-sm font-medium text-on-surface truncate", p.name || ""));
			details.appendChild(settingsNode("div", "text-xs text-on-surface-variant font-mono", p.token_prefix || ""));
			const used = p.last_used_at && String(p.last_used_at).indexOf("0001") !== 0 ? " · Last used " + fmtDate(p.last_used_at) : " · Never used";
			details.appendChild(settingsNode("div", "text-xs text-on-surface-variant mt-0.5", "Created " + fmtDate(p.created_at) + used));
			const revoke = settingsNode("button", "h-9 px-4 rounded-full bg-error-container text-error text-xs font-medium border-0 cursor-pointer hover:opacity-90", "Revoke");
			revoke.type = "button";
			revoke.dataset.id = String(p.id || "");
			revoke.dataset.name = String(p.name || "");
			revoke.addEventListener("click", () => revokeAppPassword(revoke));
			row.append(details, revoke);
			container.appendChild(row);
		}
		el.appendChild(container);
	} catch (e) {
		el.replaceChildren(settingsNode("div", "text-sm text-error", "Could not load app passwords."));
	}
}
async function createAppPassword(e) {
	e.preventDefault();
	const nameEl = document.getElementById("app-pass-name");
	const errEl = document.getElementById("app-pass-error");
	errEl.classList.add("hidden");
	const name = (nameEl.value || "").trim();
	const scopes = [...document.querySelectorAll('input[name="app-pass-scope"]:checked')].map(el=>el.value);
	if (!name || !scopes.length) { errEl.textContent = "Choose at least one access scope."; errEl.classList.remove("hidden"); return; }
	try {
		const r = await fetch("/api/settings/app-passwords", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({name: name, scopes: scopes}),
		});
		const d = await r.json().catch(() => ({}));
		if (!r.ok) throw new Error(d.error || "Create failed");
		nameEl.value = "";
		document.getElementById("app-pass-token").value = d.token || "";
		document.getElementById("app-pass-reveal").classList.remove("hidden");
		await loadAppPasswords();
	} catch (err) {
		errEl.textContent = err.message || String(err);
		errEl.classList.remove("hidden");
	}
}
function copyAppToken() {
	const t = document.getElementById("app-pass-token");
	t.select();
	navigator.clipboard.writeText(t.value).catch(() => {
		try { document.execCommand("copy"); } catch (e) {}
	});
}
async function revokeAppPassword(btn) {
	const id = btn.getAttribute("data-id");
	const name = btn.getAttribute("data-name") || "this password";
	if (!await DarUI.confirmDialog({
		title: "Revoke app password?",
		message: '"' + name + '" will stop working on every client using it.',
		confirmLabel: "Revoke",
		tone: "danger",
	})) return;
	const r = await fetch("/api/settings/app-passwords/" + encodeURIComponent(id), {method: "DELETE"});
	if (!r.ok) {
		DarUI.toast("Could not revoke the app password.", {tone: "error"});
		return;
	}
	await loadAppPasswords();
	DarUI.toast("App password revoked.");
}
let appearanceState=null,appearancePreviewTimer;
function appearanceDraft(){return {wallpaper_path:appearanceState?.wallpaper_path||"",wallpaper_visibility:Number(document.getElementById("appearance-visibility")?.value||50)}}
function appearanceApply(resolved){
	const draft=appearanceDraft(),wallpaper=draft.wallpaper_path?"/api/files/download?path="+encodeURIComponent(draft.wallpaper_path):"";
	let style=document.getElementById("appearance-live-preview");if(!style){style=document.createElement("style");style.id="appearance-live-preview";document.head.append(style)}
	style.textContent=":root{--dar-wallpaper-image:"+(wallpaper?"url("+JSON.stringify(wallpaper)+")":"none")+";--dar-wallpaper-visibility:"+(draft.wallpaper_visibility/100).toFixed(2)+";}";appearanceUpdateWallpaperPreview()
}
function appearanceApplyWallpaper(){document.getElementById("appearance-visibility-output").textContent=appearanceDraft().wallpaper_visibility+"%";appearanceApply({})}
function appearanceMessage(text,error=false){
	const box=document.getElementById("appearance-message");if(!box)return;box.textContent=text;box.className="rounded-2xl px-4 py-3 text-sm "+(error?"bg-error-container text-on-error-container":"bg-primary-container text-on-primary-container")
}
function appearanceUpdateWallpaperPreview(){
	const img=document.getElementById("appearance-wallpaper-image"),empty=document.getElementById("appearance-wallpaper-empty"),remove=document.getElementById("appearance-remove-wallpaper");
	const path=appearanceState?.wallpaper_path||"";remove.disabled=!path;remove.classList.toggle("opacity-40",!path);
	if(!path){img.removeAttribute("src");img.classList.add("hidden");empty.classList.remove("hidden");return}
	img.src="/api/files/download?path="+encodeURIComponent(path);img.classList.remove("hidden");empty.classList.add("hidden")
}
function appearanceRender(){
	if(!appearanceState)return;
	document.getElementById("appearance-visibility").value=appearanceState.wallpaper_visibility;
	appearanceApplyWallpaper();appearanceUpdateWallpaperPreview()
}
async function appearancePreview(){
	const draft=appearanceDraft();appearanceState=Object.assign(appearanceState,draft);
	const response=await fetch("/api/settings/appearance/preview",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(draft)});
	const data=await response.json().catch(()=>({}));if(!response.ok)throw new Error(data.error||"Preview failed");appearanceApply(data)
}
function appearanceQueuePreview(){clearTimeout(appearancePreviewTimer);appearancePreviewTimer=setTimeout(()=>appearancePreview().catch(error=>appearanceMessage(error.message,true)),180)}
async function appearanceLoad(){
	const response=await fetch("/api/settings/appearance"),data=await response.json();if(!response.ok)throw new Error("Appearance could not be loaded");
	appearanceState={wallpaper_path:data.settings.wallpaper_path||"",wallpaper_visibility:data.settings.wallpaper_visibility??50};appearanceRender();appearanceApply(data.settings)
}
async function appearanceChoose(){
	try{const files=await DarFilePicker.open({selection:"files",multiple:false,title:"Choose a wallpaper",confirmLabel:"Use wallpaper",mimePrefixes:["image/"]});
		if(files.length){appearanceState.wallpaper_path=files[0].path;appearanceRender();await appearancePreview()}}catch(error){appearanceMessage(error.message,true)}
}
async function appearanceUpload(file){
	if(!file)return;if(file.size>20*1024*1024){appearanceMessage("Wallpaper must be 20 MiB or smaller.",true);return}
	if(!["image/jpeg","image/png","image/gif"].includes(file.type)){appearanceMessage("Choose a JPEG, PNG, or GIF image.",true);return}
	try{
		const folder=await fetch("/api/files/mkdir?path=%2F&name=Wallpapers",{method:"POST"});if(!folder.ok)throw new Error("Could not create the Wallpapers folder");
		const safe=file.name.replace(/[^A-Za-z0-9._-]+/g,"-"),name=Date.now()+"-"+safe,form=new FormData();form.append("file",file,name);
		const upload=await fetch("/api/files/upload?path=%2FWallpapers",{method:"POST",body:form});if(!upload.ok)throw new Error((await upload.text())||"Upload failed");
		appearanceState.wallpaper_path="/Wallpapers/"+name;appearanceRender();await appearancePreview();appearanceMessage("Wallpaper uploaded. Save appearance to keep it.")
	}catch(error){appearanceMessage(error.message||String(error),true)}
}
async function appearanceSave(){
	const button=document.getElementById("appearance-save");button.disabled=true;
	try{const response=await fetch("/api/settings/appearance",{method:"PUT",headers:{"Content-Type":"application/json"},body:JSON.stringify(appearanceDraft())}),data=await response.json().catch(()=>({}));
		if(!response.ok)throw new Error(data.error||"Save failed");appearanceApply(data.settings);appearanceMessage("Appearance saved.");DarUI.toast("Appearance saved.")}
	catch(error){appearanceMessage(error.message||String(error),true);await appearanceLoad().catch(()=>{})}finally{button.disabled=false}
}
function appearanceBind(){
	document.getElementById("appearance-visibility").oninput=appearanceApplyWallpaper;
	document.getElementById("appearance-visibility").onchange=appearanceQueuePreview;
	document.getElementById("appearance-choose-wallpaper").onclick=appearanceChoose;
	document.getElementById("appearance-upload-wallpaper").onclick=()=>document.getElementById("appearance-upload-input").click();
	document.getElementById("appearance-upload-input").onchange=e=>appearanceUpload(e.target.files[0]);
	document.getElementById("appearance-remove-wallpaper").onclick=()=>{appearanceState.wallpaper_path="";appearanceRender();appearanceQueuePreview()};
	document.getElementById("appearance-save").onclick=appearanceSave;
}
if (document.getElementById("app-pass-list")) loadAppPasswords();
if (document.getElementById("personal-time-format-24h")) fetch("/api/settings/time-format").then(r=>r.ok?r.json():null).then(v=>{if(v)document.getElementById("personal-time-format-24h").checked=!!v.time_format_24h});
if(document.getElementById("appearance-settings")){appearanceBind();appearanceLoad().catch(error=>appearanceMessage(error.message,true))}
</script>
`
}

func profileInitial(displayName string) string {
	if displayName == "" {
		return "U"
	}
	r := []rune(displayName)
	return template.HTMLEscapeString(string(r[0]))
}

// personalSaveBody is the JSON body for POST /api/settings/personal (Nextcloud personal info).
type personalSaveBody struct {
	DisplayName  string `json:"display_name"`
	Phone        string `json:"phone"`
	Address      string `json:"address"`
	Website      string `json:"website"`
	Twitter      string `json:"twitter"`
	Fediverse    string `json:"fediverse"`
	Organisation string `json:"organisation"`
	Role         string `json:"role"`
	Headline     string `json:"headline"`
	Biography    string `json:"biography"`
	Location     string `json:"location"`
	Language     string `json:"language"`
	Locale       string `json:"locale"`
	Birthday     string `json:"birthday"`
}

const filesSettingsKey = "settings"
const globalTimeFormatKey = "time_format"

type timeFormatSettings struct {
	TimeFormat24h bool `json:"time_format_24h"`
}

func handleGetTimeFormat(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		value := timeFormatSettings{}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, "database", 500)
			return
		}
		defer release()
		if _, err := preferences.GetJSON(conn, user.ID, "global", globalTimeFormatKey, &value); err != nil {
			http.Error(w, "settings failed", 500)
			return
		}
		writeJSON(w, value)
	}
}

func handleSaveTimeFormat(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		var value timeFormatSettings
		if err := json.NewDecoder(r.Body).Decode(&value); err != nil {
			http.Error(w, "invalid json", 400)
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, "database", 500)
			return
		}
		defer release()
		if err := preferences.SetJSON(conn, user.ID, "global", globalTimeFormatKey, value); err != nil {
			http.Error(w, "save failed", 500)
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "settings_updated", "updated time display settings", "settings", "time-format", "Time format", "/settings/personal")
		writeJSON(w, value)
	}
}

type filesSettings struct {
	PersonalFolder            string `json:"personal_folder"`
	TrashFolder               string `json:"trash_folder"`
	SortFavoritesFirst        bool   `json:"sort_favorites_first"`
	SortFoldersBeforeFiles    bool   `json:"sort_folders_before_files"`
	FolderTree                bool   `json:"folder_tree"`
	ShowHidden                bool   `json:"show_hidden"`
	ShowFileTypeColumn        bool   `json:"show_file_type_column"`
	ShowFileExtensions        bool   `json:"show_file_extensions"`
	CropImagePreviews         bool   `json:"crop_image_previews"`
	ShowRecommendations       bool   `json:"show_recommendations"`
	WarnBeforeExtensionChange bool   `json:"warn_before_extension_change"`
	WarnBeforeDelete          bool   `json:"warn_before_delete"`
	DefaultView               string `json:"default_view"`
}

func defaultFilesSettings() filesSettings {
	return filesSettings{
		PersonalFolder:            "/docs",
		TrashFolder:               "/.local/share/Trash",
		SortFavoritesFirst:        true,
		SortFoldersBeforeFiles:    true,
		FolderTree:                false,
		ShowHidden:                false,
		ShowFileTypeColumn:        false,
		ShowFileExtensions:        true,
		CropImagePreviews:         true,
		ShowRecommendations:       true,
		WarnBeforeExtensionChange: true,
		WarnBeforeDelete:          false,
		DefaultView:               "list",
	}
}

func normalizeFilesFolder(p string) (string, error) {
	p = strings.TrimSpace(p)
	if p == "" || !strings.HasPrefix(p, "/") {
		return "", fmt.Errorf("folder must be an absolute path")
	}
	cleaned := path.Clean(p)
	if cleaned == "." {
		cleaned = "/"
	}
	if cleaned == "/" {
		return "", fmt.Errorf("folder cannot be /")
	}
	return cleaned, nil
}

func filesPathOverlaps(a, b string) bool {
	a = path.Clean(a)
	b = path.Clean(b)
	return a == b || strings.HasPrefix(a, b+"/") || strings.HasPrefix(b, a+"/")
}

func validateFilesSettings(in filesSettings) (filesSettings, error) {
	out := in
	var err error
	out.PersonalFolder, err = normalizeFilesFolder(in.PersonalFolder)
	if err != nil {
		return out, fmt.Errorf("personal folder: %w", err)
	}
	out.TrashFolder, err = normalizeFilesFolder(in.TrashFolder)
	if err != nil {
		return out, fmt.Errorf("trash folder: %w", err)
	}
	if filesPathOverlaps(out.PersonalFolder, out.TrashFolder) {
		return out, fmt.Errorf("personal and trash folders cannot overlap")
	}
	switch out.DefaultView {
	case "", "list", "grid":
		if out.DefaultView == "" {
			out.DefaultView = "list"
		}
	default:
		return out, fmt.Errorf("default view must be list or grid")
	}
	out.FolderTree = false
	return out, nil
}

func loadFilesSettings(r *http.Request, s *server.Server, userID string) (filesSettings, error) {
	settings := defaultFilesSettings()
	conn, release, err := s.DB.Conn(r.Context())
	if err != nil {
		return settings, err
	}
	defer release()
	if _, err := preferences.GetJSON(conn, userID, "files", filesSettingsKey, &settings); err != nil {
		return settings, err
	}
	return validateFilesSettings(settings)
}

func handleGetFilesSettings(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		settings, err := loadFilesSettings(r, s, user.ID)
		if err != nil {
			http.Error(w, `{"error":"settings failed"}`, http.StatusInternalServerError)
			return
		}
		writeJSON(w, settings)
	}
}

func handleSaveFilesSettings(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		settings := defaultFilesSettings()
		if err := json.NewDecoder(r.Body).Decode(&settings); err != nil {
			http.Error(w, `{"error":"invalid json"}`, http.StatusBadRequest)
			return
		}
		settings, err := validateFilesSettings(settings)
		if err != nil {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()
		if err := preferences.SetJSON(conn, user.ID, "files", filesSettingsKey, settings); err != nil {
			http.Error(w, `{"error":"save failed"}`, http.StatusInternalServerError)
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "files", "preferences_updated", "updated Files preferences", "settings", "files", "Files preferences", "/settings/files")
		writeJSON(w, settings)
	}
}

const calendarSettingsKey = "settings"

type calendarSettings struct {
	Timezone              string `json:"timezone"`
	BirthdayCalendar      bool   `json:"birthday_calendar"`
	ShowWeekends          bool   `json:"show_weekends"`
	ShowWeekNumbers       bool   `json:"show_week_numbers"`
	LimitMonthEvents      bool   `json:"limit_month_events"`
	SlotMinutes           int    `json:"slot_minutes"`
	DefaultTimedReminder  *int   `json:"default_timed_reminder"`
	DefaultAllDayReminder *int   `json:"default_all_day_reminder"`
	SimpleEditor          bool   `json:"simple_editor"`
}

func defaultCalendarSettings() calendarSettings {
	return calendarSettings{
		Timezone:         "automatic",
		BirthdayCalendar: true,
		ShowWeekends:     true,
		LimitMonthEvents: true,
		SlotMinutes:      30,
		SimpleEditor:     true,
	}
}

func validCalendarReminder(value *int) bool {
	if value == nil {
		return true
	}
	for _, allowed := range []int{0, 5, 10, 15, 30, 60, 120, 1440, 10080} {
		if *value == allowed {
			return true
		}
	}
	return false
}

func validateCalendarSettings(in calendarSettings) (calendarSettings, error) {
	out := in
	out.Timezone = strings.TrimSpace(out.Timezone)
	if out.Timezone == "" {
		out.Timezone = "automatic"
	}
	if out.Timezone != "automatic" {
		if _, err := time.LoadLocation(out.Timezone); err != nil {
			return out, fmt.Errorf("timezone must be automatic or a valid IANA timezone")
		}
	}
	switch out.SlotMinutes {
	case 5, 10, 15, 20, 30, 60:
	default:
		return out, fmt.Errorf("slot_minutes must be 5, 10, 15, 20, 30, or 60")
	}
	if !validCalendarReminder(out.DefaultTimedReminder) || !validCalendarReminder(out.DefaultAllDayReminder) {
		return out, fmt.Errorf("unsupported default reminder")
	}
	return out, nil
}

func loadCalendarSettings(r *http.Request, s *server.Server, userID string) (calendarSettings, error) {
	settings := defaultCalendarSettings()
	conn, release, err := s.DB.Conn(r.Context())
	if err != nil {
		return settings, err
	}
	defer release()
	if _, err := preferences.GetJSON(conn, userID, "calendar", calendarSettingsKey, &settings); err != nil {
		return settings, err
	}
	return validateCalendarSettings(settings)
}

func handleGetCalendarSettings(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		settings, err := loadCalendarSettings(r, s, user.ID)
		if err != nil {
			http.Error(w, `{"error":"settings failed"}`, http.StatusInternalServerError)
			return
		}
		writeJSON(w, settings)
	}
}

func handleSaveCalendarSettings(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		settings := defaultCalendarSettings()
		decoder := json.NewDecoder(r.Body)
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&settings); err != nil {
			http.Error(w, `{"error":"invalid json"}`, http.StatusBadRequest)
			return
		}
		settings, err := validateCalendarSettings(settings)
		if err != nil {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusBadRequest)
			_ = json.NewEncoder(w).Encode(map[string]string{"error": err.Error()})
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()
		if err := preferences.SetJSON(conn, user.ID, "calendar", calendarSettingsKey, settings); err != nil {
			http.Error(w, `{"error":"save failed"}`, http.StatusInternalServerError)
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "calendar", "preferences_updated", "updated Calendar preferences", "settings", "calendar", "Calendar preferences", "/settings/calendar")
		writeJSON(w, settings)
	}
}

func (b personalSaveBody) profile() auth.UserProfile {
	return auth.UserProfile{
		Phone:        strings.TrimSpace(b.Phone),
		Address:      strings.TrimSpace(b.Address),
		Website:      strings.TrimSpace(b.Website),
		Twitter:      strings.TrimSpace(b.Twitter),
		Fediverse:    strings.TrimSpace(b.Fediverse),
		Organisation: strings.TrimSpace(b.Organisation),
		Role:         strings.TrimSpace(b.Role),
		Headline:     strings.TrimSpace(b.Headline),
		Biography:    strings.TrimSpace(b.Biography),
		Location:     strings.TrimSpace(b.Location),
		Language:     strings.TrimSpace(b.Language),
		Locale:       strings.TrimSpace(b.Locale),
		Birthday:     strings.TrimSpace(b.Birthday),
	}
}

type meContactUpdater interface {
	UpsertMe(context.Context, string, string, string, auth.UserProfile) (string, error)
}

type weatherLocationPreference struct {
	Unit           string  `json:"unit"`
	LocationSource string  `json:"location_source,omitempty"`
	LocationLabel  string  `json:"location_label,omitempty"`
	Latitude       float64 `json:"latitude,omitempty"`
	Longitude      float64 `json:"longitude,omitempty"`
	Timezone       string  `json:"timezone,omitempty"`
}

func handleSavePersonal(s *server.Server, contacts meContactUpdater, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		var body personalSaveBody
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil || strings.TrimSpace(body.DisplayName) == "" {
			http.Error(w, `{"error":"display_name required"}`, http.StatusBadRequest)
			return
		}
		name := strings.TrimSpace(body.DisplayName)
		profile := body.profile()
		oldLocation := user.Profile.Location

		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()

		if err := auth.UpdateUserProfile(conn, user.ID, name, profile); err != nil {
			http.Error(w, `{"error":"update failed"}`, http.StatusInternalServerError)
			return
		}
		if oldLocation != profile.Location {
			var weather weatherLocationPreference
			if ok, _ := preferences.GetJSON(conn, user.ID, "weather", "settings", &weather); ok {
				weather.LocationSource = ""
				weather.LocationLabel = ""
				weather.Latitude = 0
				weather.Longitude = 0
				weather.Timezone = ""
				_ = preferences.SetJSON(conn, user.ID, "weather", "settings", weather)
			}
		}
		// Refresh in-memory user for this request (session middleware already loaded old values).
		user.DisplayName = name
		user.Profile = profile

		// Upsert the Me contact through the in-process Contacts capability.
		contactName, meErr := contacts.UpsertMe(r.Context(), user.ID, name, user.Email, profile)
		if meErr != nil {
			recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "profile_updated", "updated their profile", "profile", user.ID, name, "/settings/personal")
			// Profile saved; contact sync is best-effort but report error.
			writeJSON(w, map[string]any{
				"display_name": name,
				"profile":      profile,
				"contact_name": name + " (Me)",
				"warning":      "profile saved but contact sync failed: " + meErr.Error(),
			})
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "profile_updated", "updated their profile", "profile", user.ID, name, "/settings/personal")
		writeJSON(w, map[string]any{
			"display_name": name,
			"profile":      profile,
			"contact_name": contactName,
		})
	}
}

func handleListAppPasswords(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()
		list, err := auth.ListAppPasswords(conn, user.ID)
		if err != nil {
			http.Error(w, `{"error":"list failed"}`, http.StatusInternalServerError)
			return
		}
		type row struct {
			ID          string     `json:"id"`
			Name        string     `json:"name"`
			TokenPrefix string     `json:"token_prefix"`
			Scopes      []string   `json:"scopes"`
			CreatedAt   time.Time  `json:"created_at"`
			LastUsedAt  *time.Time `json:"last_used_at"`
		}
		out := make([]row, 0, len(list))
		for _, p := range list {
			rr := row{
				ID: p.ID, Name: p.Name, TokenPrefix: p.TokenPrefix, Scopes: p.Scopes, CreatedAt: p.CreatedAt,
			}
			if !p.LastUsedAt.IsZero() {
				t := p.LastUsedAt
				rr.LastUsedAt = &t
			}
			out = append(out, rr)
		}
		writeJSON(w, map[string]any{"app_passwords": out})
	}
}

func handleCreateAppPassword(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		var body struct {
			Name   string   `json:"name"`
			Scopes []string `json:"scopes"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil || strings.TrimSpace(body.Name) == "" {
			http.Error(w, `{"error":"name required"}`, http.StatusBadRequest)
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()
		ap, err := auth.CreateAppPassword(conn, user.ID, body.Name, body.Scopes)
		if err != nil {
			http.Error(w, `{"error":"invalid name or scopes"}`, http.StatusBadRequest)
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "app_password_created", "created app password "+ap.Name, "app_password", ap.ID, ap.Name, "/settings/security")
		writeJSON(w, map[string]any{
			"id":           ap.ID,
			"name":         ap.Name,
			"token_prefix": ap.TokenPrefix,
			"scopes":       ap.Scopes,
			"created_at":   ap.CreatedAt,
			"token":        ap.Token,
		})
	}
}

func handleDeleteAppPassword(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
			return
		}
		id := chi.URLParam(r, "id")
		if id == "" {
			http.Error(w, `{"error":"missing id"}`, http.StatusBadRequest)
			return
		}
		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, `{"error":"database"}`, http.StatusInternalServerError)
			return
		}
		defer release()
		ok, err := auth.DeleteAppPassword(conn, user.ID, id)
		if err != nil {
			http.Error(w, `{"error":"delete failed"}`, http.StatusInternalServerError)
			return
		}
		if !ok {
			http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
			return
		}
		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "app_password_revoked", "revoked an app password", "app_password", id, "App password", "/settings/security")
		writeJSON(w, map[string]string{"status": "ok"})
	}
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
