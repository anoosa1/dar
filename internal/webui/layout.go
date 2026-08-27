// Package webui provides HTML layout rendering and reusable components for Dar.
package webui

import (
	"bytes"
	"context"
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"html/template"
	"io"
	"net/http"
	"path"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/theme"
)

//go:embed templates/* assets/*
var templateFS embed.FS

var templates = template.Must(template.New("").ParseFS(templateFS, "templates/*.html"))
var textEditorScript = mustAsset("assets/text-editor.js")
var dashboardScript = mustAsset("assets/dashboard.js")
var darUIScript = mustAsset("assets/dar-ui.js")
var notificationsScript = mustAsset("assets/notifications.js")
var codexPanelScript = mustAsset("assets/codex-panel.js")
var searchScript = mustAsset("assets/search.js")
var shellScript = mustAsset("assets/shell.js")
var publicAssetPaths = map[string]string{
	"/assets/app.v1.css":                                          "assets/app.v1.css",
	"/assets/favicon.svg":                                         "assets/favicon.svg",
	"/assets/fontawesome/6.5.1/css/all.min.css":                   "assets/fontawesome/6.5.1/css/all.min.css",
	"/assets/fontawesome/6.5.1/webfonts/fa-brands-400.woff2":      "assets/fontawesome/6.5.1/webfonts/fa-brands-400.woff2",
	"/assets/fontawesome/6.5.1/webfonts/fa-regular-400.woff2":     "assets/fontawesome/6.5.1/webfonts/fa-regular-400.woff2",
	"/assets/fontawesome/6.5.1/webfonts/fa-solid-900.woff2":       "assets/fontawesome/6.5.1/webfonts/fa-solid-900.woff2",
	"/assets/fontawesome/6.5.1/webfonts/fa-v4compatibility.woff2": "assets/fontawesome/6.5.1/webfonts/fa-v4compatibility.woff2",
}

func mustAsset(name string) []byte {
	data, err := templateFS.ReadFile(name)
	if err != nil {
		panic(err)
	}
	return data
}

// HandlePublicAsset serves pinned, immutable CSS and font assets required by
// both the login screen and authenticated pages.
func HandlePublicAsset(w http.ResponseWriter, r *http.Request) {
	name, ok := publicAssetPaths[path.Clean(r.URL.Path)]
	if !ok {
		http.NotFound(w, r)
		return
	}
	data, err := templateFS.ReadFile(name)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	sum := sha256.Sum256(data)
	etag := `"` + hex.EncodeToString(sum[:8]) + `"`
	w.Header().Set("ETag", etag)
	w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
	if r.Header.Get("If-None-Match") == etag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	switch path.Ext(name) {
	case ".css":
		w.Header().Set("Content-Type", "text/css; charset=utf-8")
	case ".svg":
		w.Header().Set("Content-Type", "image/svg+xml")
	case ".woff2":
		w.Header().Set("Content-Type", "font/woff2")
	}
	http.ServeContent(w, r, path.Base(name), time.Time{}, bytes.NewReader(data))
}

// HandleTextEditorAsset serves the dependency-free editor shared by Notes and Files.
func HandleTextEditorAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "text-editor.js", textEditorScript)
}

// HandleDashboardAsset serves the dependency-free dashboard controller.
func HandleDashboardAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "dashboard.js", dashboardScript)
}

// HandleDarUIAsset serves accessible dialogs and toast notifications shared by
// every Dar app.
func HandleDarUIAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "dar-ui.js", darUIScript)
}

// HandleNotificationsAsset serves the global notification-center controller.
func HandleNotificationsAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "notifications.js", notificationsScript)
}

// HandleCodexPanelAsset serves the shared Focus assistant panel controller.
func HandleCodexPanelAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "codex-panel.js", codexPanelScript)
}

// HandleSearchAsset serves the centralized Focus search controller.
func HandleSearchAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "search.js", searchScript)
}

// HandleShellAsset serves app switching and global search behavior.
func HandleShellAsset(w http.ResponseWriter, r *http.Request) {
	serveJavaScript(w, r, "shell.js", shellScript)
}

func serveJavaScript(w http.ResponseWriter, r *http.Request, name string, data []byte) {
	sum := sha256.Sum256(data)
	etag := `"` + hex.EncodeToString(sum[:8]) + `"`
	w.Header().Set("Cache-Control", "public, max-age=3600, must-revalidate")
	w.Header().Set("ETag", etag)
	if r.Header.Get("If-None-Match") == etag {
		w.WriteHeader(http.StatusNotModified)
		return
	}
	w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
	http.ServeContent(w, r, name, time.Time{}, bytes.NewReader(data))
}

// ExternalizeLastScript replaces the final inline controller in a legacy app
// document with a self-hosted script reference. It lets app behavior remain
// app-owned while all browser-visible page markup stays in the Focus shell.
func ExternalizeLastScript(document, assetPath string) (template.HTML, []byte) {
	start := strings.LastIndex(document, "<script>")
	end := strings.LastIndex(document, "</script>")
	if start < 0 || end <= start {
		return template.HTML(document), nil
	}
	scriptStart := start + len("<script>")
	script := []byte(document[scriptStart:end])
	reference := `<script src="` + template.HTMLEscapeString(assetPath) + `" defer></script>`
	return template.HTML(document[:start] + reference + document[end+len("</script>"):]), script
}

// ServeAppJavaScript serves a request-scoped app controller without caching.
func ServeAppJavaScript(w http.ResponseWriter, r *http.Request, name string, data []byte) {
	w.Header().Set("Cache-Control", "private, no-store")
	w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
	http.ServeContent(w, r, name, time.Time{}, bytes.NewReader(data))
}

// PageData is passed to the layout template for every page.
type PageData struct {
	Title           string
	CurrentApp      string
	AppBar          template.HTML
	UserMenu        template.HTML
	Sidebar         template.HTML
	Content         template.HTML
	HeadExtra       template.HTML
	ContentTemplate string
	ContentData     any
}

// Render renders a page using the base layout template.
func Render(w io.Writer, data PageData) error {
	return RenderResolved(w, data, theme.FromContext(context.Background()))
}

// RenderRequest renders a page with the appearance resolved by authenticated
// page middleware.
func RenderRequest(w io.Writer, r *http.Request, data PageData) error {
	if data.ContentTemplate != "" {
		content, err := RenderContent(data.ContentTemplate, data.ContentData)
		if err != nil {
			return err
		}
		data.Content = content
	}
	return RenderResolved(w, data, theme.FromContext(r.Context()))
}

// RenderContent renders one named template from the centralized UI set.
func RenderContent(name string, data any) (template.HTML, error) {
	var content bytes.Buffer
	if err := templates.ExecuteTemplate(&content, name, data); err != nil {
		return "", err
	}
	return template.HTML(content.String()), nil
}

// RenderContentWithScript renders centralized markup and externalizes its last
// inline controller for an app-owned asset route.
func RenderContentWithScript(name string, data any, assetPath string) (template.HTML, []byte, error) {
	content, err := RenderContent(name, data)
	if err != nil {
		return "", nil, err
	}
	markup, script := ExternalizeLastScript(string(content), assetPath)
	return markup, script, nil
}

// RenderPage renders a named centralized content template in the Focus shell.
func RenderPage(w io.Writer, r *http.Request, data PageData) error {
	return RenderRequest(w, r, data)
}

func RenderResolved(w io.Writer, data PageData, appearance theme.Resolved) error {
	if response, ok := w.(http.ResponseWriter); ok {
		response.Header().Set("Content-Type", "text/html; charset=utf-8")
	}
	return templates.ExecuteTemplate(w, "layout.html", struct {
		PageData
		Theme      theme.Palette
		ThemeCSS   template.CSS
		ThemeMode  string
		Appearance theme.Resolved
	}{
		PageData: data, Theme: appearance.Active, ThemeCSS: template.CSS(theme.CSS(appearance, appearance.PublicWallpaperURL)),
		ThemeMode: appearance.Mode, Appearance: appearance,
	})
}

// RenderLogin renders the login page.
func RenderLogin(w io.Writer, providers []LoginProvider) error {
	if response, ok := w.(http.ResponseWriter); ok {
		response.Header().Set("Content-Type", "text/html; charset=utf-8")
	}
	appearance := theme.FromContext(context.Background())
	return templates.ExecuteTemplate(w, "login.html", map[string]any{
		"Title":     "Login — Dar",
		"Providers": providers,
		"Theme":     appearance.Active,
		"ThemeCSS":  template.CSS(theme.CSS(appearance, "")),
		"ThemeMode": appearance.Mode,
	})
}

// RenderHome renders the home/app launcher page.
func RenderHome(w io.Writer, r *http.Request, userName string, appCards, appBar template.HTML) error {
	return RenderPage(w, r, PageData{
		Title:           "Home — Dar",
		CurrentApp:      "home",
		AppBar:          appBar,
		UserMenu:        buildUserMenu(userName),
		ContentTemplate: "dashboard.html",
	})
}

func DashboardHTML() template.HTML {
	var content bytes.Buffer
	_ = templates.ExecuteTemplate(&content, "dashboard.html", nil)
	return template.HTML(content.String())
}

// LoginProvider represents an OIDC provider shown on the login page.
type LoginProvider struct {
	Name string
	Icon string
	URL  string
}

// SidebarItem is a navigation item in the app sidebar.
type SidebarItem struct {
	ID     string
	Label  string
	Icon   string
	Href   string
	Active bool
}

// SidebarAction is a button rendered immediately after a sidebar section's items.
type SidebarAction struct {
	ID    string
	Label string
	Icon  string
}

// SidebarSection reserves a labeled navigation area for client-rendered items.
type SidebarSection struct {
	Label   string
	NavID   string
	Actions []SidebarAction
}

// SidebarConfig configures a Nextcloud-style app sidebar.
type SidebarConfig struct {
	Items         []SidebarItem
	Sections      []SidebarSection
	SettingsHref  string
	SettingsLabel string
	StateKey      string
	AriaLabel     string
}

// BuildSidebar renders a Nextcloud-style app sidebar with optional dynamic sections and a settings footer.
func BuildSidebar(config SidebarConfig) template.HTML {
	var rows string
	for _, item := range config.Items {
		activeCls := ""
		if item.Active {
			activeCls = " bg-primary-container text-primary font-medium"
		}
		idAttr := ""
		if item.ID != "" {
			idAttr = ` id="` + template.HTMLEscapeString(item.ID) + `"`
		}
		rows += `<a` + idAttr + ` href="` + template.HTMLEscapeString(item.Href) + `" class="flex items-center gap-3 rounded-full px-4 h-10 text-sm no-underline transition-colors hover:bg-surface-container-highest` + activeCls + `">
			<i class="` + item.Icon + ` w-5 text-center"></i>
			<span>` + template.HTMLEscapeString(item.Label) + `</span>
		</a>`
	}
	for _, section := range config.Sections {
		rows += `<section class="mt-4" aria-labelledby="` + template.HTMLEscapeString(section.NavID) + `-label">
			<h2 id="` + template.HTMLEscapeString(section.NavID) + `-label" class="px-4 py-2 text-xs font-semibold uppercase tracking-wider text-on-surface-variant">` + template.HTMLEscapeString(section.Label) + `</h2>
			<nav id="` + template.HTMLEscapeString(section.NavID) + `" class="flex flex-col gap-1" aria-label="` + template.HTMLEscapeString(section.Label) + `"></nav>`
		for _, action := range section.Actions {
			rows += `<button type="button" id="` + template.HTMLEscapeString(action.ID) + `" class="mt-1 flex w-full items-center gap-3 rounded-full px-4 h-10 text-sm text-primary hover:bg-primary-container">
				<i class="` + action.Icon + ` w-5 text-center"></i><span>` + template.HTMLEscapeString(action.Label) + `</span>
			</button>`
		}
		rows += `</section>`
	}
	settingsLabel := config.SettingsLabel
	if settingsLabel == "" {
		settingsLabel = "Settings"
	}
	stateKey := config.StateKey
	if stateKey == "" {
		stateKey = "app"
	}
	ariaLabel := config.AriaLabel
	if ariaLabel == "" {
		ariaLabel = "App navigation"
	}
	return template.HTML(`<aside id="sidebar" data-dar-sidebar data-dar-sidebar-key="` + template.HTMLEscapeString(stateKey) + `" aria-label="` + template.HTMLEscapeString(ariaLabel) + `" class="flex h-full min-h-0 shrink-0 bg-transparent flex-col">
		<header data-dar-sidebar-header><span data-dar-sidebar-title>` + template.HTMLEscapeString(strings.TrimSuffix(ariaLabel, " navigation")) + `</span></header>
		<nav class="p-3 flex flex-col gap-1 flex-1 overflow-y-auto">` + rows + `</nav>
		<div class="p-3">
			<a href="` + template.HTMLEscapeString(config.SettingsHref) + `" onclick="event.preventDefault();openSettings()" class="flex items-center gap-3 rounded-full px-4 h-10 text-sm text-on-surface-variant no-underline hover:bg-surface-container-highest">
				<i class="fa-solid fa-gear w-5 text-center"></i><span>` + template.HTMLEscapeString(settingsLabel) + `</span>
			</a>
		</div>
	</aside>
	<button type="button" data-dar-sidebar-toggle class="hover:bg-surface-container-highest" aria-controls="sidebar" aria-expanded="true" aria-label="Close ` + template.HTMLEscapeString(ariaLabel) + `"><i class="fa-solid fa-chevron-left"></i></button>
	<div data-dar-sidebar-backdrop hidden></div>`)
}

// UserMenuHTML builds the user avatar + dropdown menu.
func UserMenuHTML(displayName string) template.HTML {
	return buildUserMenu(displayName)
}

func buildUserMenu(displayName string) template.HTML {
	initial := "U"
	if len(displayName) > 0 {
		initial = displayName[:1]
	}
	initial = template.HTMLEscapeString(initial)
	displayName = template.HTMLEscapeString(displayName)
	return template.HTML(`<div class="relative">
		<button id="user-menu-button" type="button" onclick="toggleUserMenu()"
			class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer"
			aria-label="User menu">
			<span aria-hidden="true">` + initial + `</span>
		</button>
		<div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
			<div class="px-4 py-2 text-sm font-medium text-on-surface">` + displayName + `</div>
			<a href="/settings" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Settings</a>
			<button type="button" onclick="darLogout()" class="block w-full text-left px-4 py-2 text-sm text-error hover:bg-error-container">Logout</button>
		</div>
	</div>
	<script>
	function toggleUserMenu(){let m=document.getElementById('user-menu');if(m)m.classList.toggle('hidden')}
	async function darLogout(){const r=await fetch('/logout',{method:'POST'});if(r.ok||r.redirected)location.assign('/login')}
	document.addEventListener('click',function(e){let b=document.getElementById('user-menu-button');let m=document.getElementById('user-menu');if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add('hidden')});
	</script>`)
}
