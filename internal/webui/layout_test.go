package webui

import (
	"bytes"
	"html/template"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

func TestRenderUsesSharedCatppuccinMochaTheme(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Theme test"}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`name="theme-color" content="#1e1e2e"`,
		`name="color-scheme" content="dark"`,
		`--dar-primary:245 194 231`,
		`href="/assets/app.v1.css?v=4"`,
		`selection:bg-primary/30`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("rendered page does not contain %q", want)
		}
	}
	for _, legacy := range []string{"#fef7ff", "#6750a4", "#1b1b1f", "#cac4d0"} {
		if strings.Contains(html, legacy) {
			t.Errorf("rendered page still contains legacy color %s", legacy)
		}
	}
}

func TestRenderIncludesPersistentTransientGlobalSearch(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Search shell"}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`id="global-search-input"`,
		`placeholder="Search Dar"`,
		`id="global-search-mobile"`,
		`src="/assets/shell.js"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("global search shell missing %q", want)
		}
	}
	if strings.Contains(html, `/api/search?q=`) {
		t.Fatal("global search query should not be placed in the URL")
	}
	rec := httptest.NewRecorder()
	HandleShellAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/shell.js", nil))
	if rec.Code != http.StatusOK || !strings.HasPrefix(rec.Header().Get("Content-Type"), "text/javascript") {
		t.Fatalf("shell asset status=%d type=%q", rec.Code, rec.Header().Get("Content-Type"))
	}
	for _, want := range []string{`/api/search/query`, `method: "POST"`, `sessionStorage.setItem("dar.search.query"`, `location.assign("/search")`} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("global shell controller missing %q", want)
		}
	}
}

func TestRenderIncludesGlobalNotificationCenter(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Notification shell", UserMenu: UserMenuHTML("Alice")}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`src="/assets/notifications.js"`,
		`id="dar-notifications-button"`,
		`aria-label="Notifications"`,
		`id="dar-notifications-badge"`,
		`id="dar-notifications-panel"`,
		`id="dar-notifications-desktop"`,
		`id="dar-notifications-dismiss-all"`,
		`id="user-menu-button"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("notification shell missing %q", want)
		}
	}
	if strings.Index(html, `id="dar-notifications-button"`) > strings.Index(html, `id="user-menu-button"`) {
		t.Fatal("notification button is not immediately before the user menu")
	}

	script := string(notificationsScript)
	for _, want := range []string{
		`new EventSource("/api/notifications/stream")`,
		`new Notification(title`,
		`location.pathname === "/email"`,
		`"New mail"`,
		`Notification.requestPermission()`,
		`method: "DELETE"`,
	} {
		if !strings.Contains(script, want) {
			t.Fatalf("notification controller missing %q", want)
		}
	}
}

func TestAuthenticatedShellUsesFixedViewportAndOverlaySearch(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Fixed shell", Sidebar: template.HTML(`<aside id="fixture-sidebar"></aside>`)}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`<body class="focus-body text-on-surface h-dvh overflow-hidden flex flex-col`,
		`<header class="focus-topbar h-16 shrink-0`,
		`id="global-search-mobile-row" class="hidden md:hidden fixed inset-x-3 bottom-20`,
		`class="dar-app-frame focus-frame flex flex-1 min-h-0 bg-surface"`,
		`<main class="flex-1 min-w-0 min-h-0 overflow-hidden">`,
		`class="focus-mobile-dock md:hidden fixed`,
		`html, body { height: 100%; }`,
		`scrollbar-gutter: stable`,
		`overscroll-behavior: contain`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("fixed viewport shell does not contain %q", want)
		}
	}
	for _, legacy := range []string{
		`min-h-screen`,
		`min-h-[calc(100vh-56px)]`,
		`<main class="flex-1 p-4`,
	} {
		if strings.Contains(html, legacy) {
			t.Errorf("fixed viewport shell still contains %q", legacy)
		}
	}
}

func TestSearchPageHasResultsMatchesAndCustomization(t *testing.T) {
	html := string(SearchPageHTML())
	for _, want := range []string{
		`id="search-page-scroll"`,
		`h-full min-h-0 overflow-y-auto overscroll-contain`,
		`[scrollbar-gutter:stable]`,
		`id="search-page-input"`,
		`id="search-customize"`,
		`class="focus-field`,
		`src="/assets/search.js"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("search page missing %q", want)
		}
	}
	rec := httptest.NewRecorder()
	HandleSearchAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/search.js", nil))
	if rec.Code != http.StatusOK || !strings.HasPrefix(rec.Header().Get("Content-Type"), "text/javascript") {
		t.Fatalf("search asset status=%d type=%q", rec.Code, rec.Header().Get("Content-Type"))
	}
	for _, want := range []string{`/api/search/matches`, `/api/search/preferences`, `view: "results"`, `match_count`} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("search controller missing %q", want)
		}
	}
}

func TestRenderLoginUsesSharedCatppuccinMochaTheme(t *testing.T) {
	var page bytes.Buffer
	if err := RenderLogin(&page, []LoginProvider{{Name: "Test", URL: "/auth"}}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`--dar-surface:30 30 46`,
		`href="/assets/app.v1.css?v=4"`,
		`<body class="focus-body bg-surface text-on-surface h-dvh overflow-hidden`,
		`max-h-full overflow-y-auto overscroll-contain rounded-3xl`,
		`quietly organized`,
		`Continue with Test`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("rendered login does not contain %q", want)
		}
	}
}

func TestRenderersDeclareHTMLForNoSniffBrowsers(t *testing.T) {
	page := httptest.NewRecorder()
	if err := Render(page, PageData{Title: "HTML"}); err != nil {
		t.Fatal(err)
	}
	if got := page.Header().Get("Content-Type"); got != "text/html; charset=utf-8" {
		t.Fatalf("page content type=%q", got)
	}
	login := httptest.NewRecorder()
	if err := RenderLogin(login, nil); err != nil {
		t.Fatal(err)
	}
	if got := login.Header().Get("Content-Type"); got != "text/html; charset=utf-8" {
		t.Fatalf("login content type=%q", got)
	}
}

func TestBuildSidebarConfigRendersItemsSectionsAndFooter(t *testing.T) {
	html := string(BuildSidebar(SidebarConfig{
		StateKey:  "projects",
		AriaLabel: "Project navigation",
		Items:     []SidebarItem{{ID: "all-items", Label: "All items", Icon: "fa-solid fa-list", Href: "/items", Active: true}},
		Sections: []SidebarSection{{
			Label:   "Projects",
			NavID:   "project-list",
			Actions: []SidebarAction{{ID: "new-project", Label: "New project", Icon: "fa-solid fa-plus"}},
		}},
		SettingsHref:  "#project-settings",
		SettingsLabel: "Project settings",
	}))
	for _, want := range []string{
		`id="all-items"`,
		`bg-primary-container text-primary font-medium`,
		`id="project-list-label"`,
		`id="project-list"`,
		`aria-label="Projects"`,
		`id="new-project"`,
		`>New project</span>`,
		`href="#project-settings"`,
		`<span>Project settings</span>`,
		`data-dar-sidebar-key="projects"`,
		`aria-label="Project navigation"`,
		`data-dar-sidebar-header`,
		`data-dar-sidebar-title>Project</span>`,
		`data-dar-sidebar-toggle`,
		`aria-controls="sidebar"`,
		`aria-expanded="true"`,
		`aria-label="Close Project navigation"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("sidebar HTML does not contain %q", want)
		}
	}
	if strings.Count(html, `data-dar-sidebar-toggle`) != 1 {
		t.Fatal("sidebar HTML must render exactly one shared sidebar toggle")
	}
	if strings.Contains(html, `data-dar-sidebar-close`) || strings.Contains(html, `data-dar-sidebar-open`) {
		t.Fatal("sidebar HTML must not render separate open and close controls")
	}
	if strings.Index(html, `id="new-project"`) < strings.Index(html, `id="project-list"`) {
		t.Fatal("section action should render after the section navigation container")
	}
}

func TestSharedShellAssetsExposeResponsiveContract(t *testing.T) {
	css, err := templateFS.ReadFile("templates/theme.html")
	if err != nil {
		t.Fatal(err)
	}
	script, err := templateFS.ReadFile("assets/dar-ui.js")
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{`width: 280px`, `max-width: 1023px`, `[data-dar-detail-pane]`, `--dar-glass-alpha`, `.focus-frame`} {
		if !strings.Contains(string(css), want) {
			t.Fatalf("shared shell CSS does not contain %q", want)
		}
	}
	if strings.Contains(string(css), `padding-inline-start: 4rem !important`) {
		t.Fatal("sidebar toggle must not shift the complete master pane")
	}
	if !strings.Contains(string(css), `[data-dar-sidebar-toggle-host] > [data-dar-sidebar-toggle]`) {
		t.Fatal("single sidebar toggle must participate in the middle-pane title row")
	}
	if strings.Contains(string(css), `[data-dar-sidebar-close]`) || strings.Contains(string(css), `
        [data-dar-sidebar-open]`) {
		t.Fatal("shared shell CSS must not define separate open and close controls")
	}
	for _, want := range []string{`dar.sidebar.`, `openDetail`, `closeDetail`, `aria-modal`} {
		if !strings.Contains(string(script), want) {
			t.Fatalf("shared shell script does not contain %q", want)
		}
	}
	for _, want := range []string{`toggle.getAttribute("aria-expanded") !== "true"`, `toggle.setAttribute("aria-label", (open ? "Close " : "Open ") + label)`, `fa-solid fa-bars`, `fa-chevron-right`, `fa-chevron-left`} {
		if !strings.Contains(string(script), want) {
			t.Fatalf("single sidebar toggle script does not contain %q", want)
		}
	}
	if strings.Contains(string(script), `[data-dar-sidebar-close]`) || strings.Contains(string(script), `querySelector("[data-dar-sidebar-open]")`) {
		t.Fatal("shared shell script must not manage separate open and close controls")
	}
}

func TestBuildSidebarDefaultsSettingsLabel(t *testing.T) {
	html := string(BuildSidebar(SidebarConfig{
		Items:        []SidebarItem{{Label: "Home", Icon: "fa-solid fa-house", Href: "/home"}},
		SettingsHref: "/settings",
	}))
	if !strings.Contains(html, `<span>Settings</span>`) {
		t.Fatal("sidebar should retain the default Settings label")
	}
	if strings.Contains(html, `<section`) {
		t.Fatal("sidebar without configured sections should not render a section")
	}
}

func TestTextEditorAssetIsServedAsJavaScript(t *testing.T) {
	rec := httptest.NewRecorder()
	HandleTextEditorAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/text-editor.js", nil))
	if rec.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", rec.Code, rec.Body.String())
	}
	if got := rec.Header().Get("Content-Type"); !strings.HasPrefix(got, "text/javascript") {
		t.Fatalf("content type=%q", got)
	}
	if rec.Header().Get("ETag") == "" || !strings.Contains(rec.Header().Get("Cache-Control"), "must-revalidate") {
		t.Fatalf("editor cache headers=%v", rec.Header())
	}
	for _, want := range []string{"window.DarTextEditor", "create: create", "Saved locally — retrying"} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("shared editor asset does not contain %q", want)
		}
	}
}

func TestDashboardUsesEmbeddedControllerAsset(t *testing.T) {
	html := string(DashboardHTML())
	for _, want := range []string{
		`id="dashboard-scroll"`,
		`h-full min-h-0 overflow-y-auto overscroll-contain`,
		`[scrollbar-gutter:stable]`,
		`id="dashboard-grid"`,
		`id="dashboard-settings-size-options"`,
		`id="dashboard-status"`,
		`src="/assets/dashboard.js?v=2"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("dashboard HTML does not contain %q", want)
		}
	}
	if strings.Contains(html, `function loadWidget`) {
		t.Fatal("dashboard controller should not remain inline")
	}

	rec := httptest.NewRecorder()
	HandleDashboardAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/dashboard.js", nil))
	if rec.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", rec.Code, rec.Body.String())
	}
	if got := rec.Header().Get("Content-Type"); !strings.HasPrefix(got, "text/javascript") {
		t.Fatalf("content type=%q", got)
	}
	for _, want := range []string{"function reflow", "function openSettings", "activeId", "widget_size"} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("dashboard asset does not contain %q", want)
		}
	}
}

func TestNewsGridReleasesCollapsedSidebarTrack(t *testing.T) {
	html, err := RenderContent("news.html", struct{ View string }{View: "unread"})
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{
		`lg:grid-cols-[auto_360px_minmax(0,1fr)]`,
		`lg:grid-cols-[auto_minmax(0,1fr)]`,
	} {
		if !strings.Contains(string(html), want) {
			t.Fatalf("News grid does not contain collapsible track %q", want)
		}
	}
	if strings.Contains(string(html), `lg:grid-cols-[300px`) {
		t.Fatal("News grid still reserves a fixed desktop sidebar track")
	}
}

func TestAppRootsDoNotReintroduceViewportCompensation(t *testing.T) {
	roots := map[string]string{
		"templates/activity.html":      "activity-app",
		"../apps/calendar/handlers.go": "calendar-app",
		"../apps/codex/handlers.go":    "codex-app",
		"../apps/contacts/handlers.go": "contacts-app",
		"templates/email.html":         "email-app",
		"templates/files.html":         "files-app",
		"templates/news.html":          "news-app",
		"templates/notes.html":         "notes-app",
		"../apps/todo/handlers.go":     "todo-app",
		"../../cmd/server/settings.go": "settings-app",
	}
	legacy := regexp.MustCompile(`(?:h|min-h)-\[calc\(100vh|(?:^|[\s"])md:-m-[46](?:[\s"]|$)|class="[^"]*(?:^|[\s])-m-[46](?:[\s"]|$)`)
	for path, rootID := range roots {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Contains(data, []byte(`id="`+rootID+`"`)) || !bytes.Contains(data, []byte(`h-full min-h-0`)) {
			t.Errorf("%s does not define the %s full-height workspace contract", path, rootID)
		}
		if !bytes.Contains(data, []byte(`data-dar-sidebar-toggle-host`)) {
			t.Errorf("%s does not define a sidebar toggle title host", path)
		}
		if match := legacy.Find(data); match != nil {
			t.Errorf("%s reintroduced shell compensation %q", path, match)
		}
	}
}

func TestDarUIAssetAndLayoutReference(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Dialog shell"}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(page.String(), `src="/assets/dar-ui.js"`) {
		t.Fatal("layout does not load the shared Dar UI asset")
	}
	rec := httptest.NewRecorder()
	HandleDarUIAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/dar-ui.js", nil))
	if rec.Code != http.StatusOK || !strings.HasPrefix(rec.Header().Get("Content-Type"), "text/javascript") {
		t.Fatalf("asset response: status=%d content-type=%q", rec.Code, rec.Header().Get("Content-Type"))
	}
	for _, want := range []string{"window.DarUI", "confirmDialog", "inputDialog", "formDialog", "toast", "showModal", "window.DarWallpaperContrast", "data-wallpaper-tone"} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("shared Dar UI asset does not contain %q", want)
		}
	}
}

func TestPinnedPublicAssetsAreImmutableAndConditional(t *testing.T) {
	for _, asset := range []string{"/assets/app.v1.css", "/assets/fontawesome/6.5.1/css/all.min.css", "/assets/fontawesome/6.5.1/webfonts/fa-solid-900.woff2"} {
		rec := httptest.NewRecorder()
		HandlePublicAsset(rec, httptest.NewRequest(http.MethodGet, asset, nil))
		if rec.Code != http.StatusOK {
			t.Fatalf("%s status=%d", asset, rec.Code)
		}
		if rec.Header().Get("ETag") == "" || rec.Header().Get("Cache-Control") != "public, max-age=31536000, immutable" {
			t.Fatalf("%s cache headers=%v", asset, rec.Header())
		}
		conditional := httptest.NewRequest(http.MethodGet, asset, nil)
		conditional.Header.Set("If-None-Match", rec.Header().Get("ETag"))
		notModified := httptest.NewRecorder()
		HandlePublicAsset(notModified, conditional)
		if notModified.Code != http.StatusNotModified {
			t.Fatalf("%s conditional status=%d", asset, notModified.Code)
		}
	}

	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Local assets"}); err != nil {
		t.Fatal(err)
	}
	for _, external := range []string{"cdn.tailwindcss.com", "cdnjs.cloudflare.com", "unpkg.com"} {
		if strings.Contains(page.String(), external) {
			t.Fatalf("page still depends on external frontend asset %q", external)
		}
	}
}

func TestAssetHandlersRejectUnknownFilesAndHonorEditorETag(t *testing.T) {
	missing := httptest.NewRecorder()
	HandlePublicAsset(missing, httptest.NewRequest(http.MethodGet, "/assets/missing.css", nil))
	if missing.Code != http.StatusNotFound {
		t.Fatalf("unknown asset status=%d", missing.Code)
	}

	first := httptest.NewRecorder()
	HandleTextEditorAsset(first, httptest.NewRequest(http.MethodGet, "/assets/text-editor.js", nil))
	if first.Code != http.StatusOK || first.Header().Get("ETag") == "" {
		t.Fatalf("editor asset status=%d headers=%v", first.Code, first.Header())
	}
	conditional := httptest.NewRequest(http.MethodGet, "/assets/text-editor.js", nil)
	conditional.Header.Set("If-None-Match", first.Header().Get("ETag"))
	notModified := httptest.NewRecorder()
	HandleTextEditorAsset(notModified, conditional)
	if notModified.Code != http.StatusNotModified {
		t.Fatalf("conditional editor asset status=%d", notModified.Code)
	}
}

func TestWallpaperContrastTargetsTransparentShellChrome(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Wallpaper contrast"}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`href="/home" class="focus-wordmark`,
		`id="global-search-mobile"`,
		`[data-wallpaper-contrast][data-wallpaper-tone="light"]`,
		`[data-wallpaper-contrast][data-wallpaper-tone="dark"]`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("wallpaper-aware shell missing %q", want)
		}
	}
	dashboard := string(DashboardHTML())
	for _, want := range []string{`id="dashboard-title"`, `id="dashboard-edit"`, `class="dashboard-composition`} {
		if !strings.Contains(dashboard, want) {
			t.Fatalf("opaque Focus dashboard missing %q", want)
		}
	}
}

func TestFocusShellIncludesGlobalCodexPanel(t *testing.T) {
	var page bytes.Buffer
	if err := Render(&page, PageData{Title: "Codex panel"}); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`src="/assets/codex-panel.js"`,
		`id="dar-codex-panel"`,
		`data-dar-codex-open`,
		`id="dar-codex-sessions"`,
		`id="dar-codex-messages"`,
		`id="dar-codex-input"`,
		`href="/codex"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("Focus Codex panel missing %q", want)
		}
	}
	rec := httptest.NewRecorder()
	HandleCodexPanelAsset(rec, httptest.NewRequest(http.MethodGet, "/assets/codex-panel.js", nil))
	if rec.Code != http.StatusOK || !strings.HasPrefix(rec.Header().Get("Content-Type"), "text/javascript") {
		t.Fatalf("Codex panel asset status=%d type=%q", rec.Code, rec.Header().Get("Content-Type"))
	}
	for _, want := range []string{`fetch("/api/codex" + path`, `api("/status")`, `api("/sessions")`, "EventSource", "call_id"} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("Codex panel controller missing %q", want)
		}
	}
}

func TestProductionSourcesDoNotUseBrowserPopups(t *testing.T) {
	popup := regexp.MustCompile(`\b(?:alert|confirm|prompt)\s*\(|beforeunload`)
	for _, root := range []string{"../../cmd", "../../internal/apps"} {
		err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
			if err != nil {
				return err
			}
			if info.IsDir() || strings.HasSuffix(path, "_test.go") {
				return nil
			}
			switch filepath.Ext(path) {
			case ".go", ".js", ".html":
			default:
				return nil
			}
			data, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			if match := popup.Find(data); match != nil {
				t.Errorf("%s contains executable browser popup %q", path, match)
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}
}
