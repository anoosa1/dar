package files

import (
	"bytes"
	"embed"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/httpguard"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"github.com/anoosa/dar/internal/webui"
)

//go:embed assets/*
var filesFS embed.FS

var appScript = mustReadFile("assets/app.js")
var pickerScript = mustReadFile("assets/picker.js")
var appScriptModTime = time.Time{}

func mustReadFile(name string) []byte {
	data, err := filesFS.ReadFile(name)
	if err != nil {
		panic(err)
	}
	return data
}

type Config struct {
	DataRoot         string
	DB               *db.DB
	SharePasswordKey []byte
	SecureCookies    bool
	UserQuotaBytes   int64
	Search           apps.SearchIndexWriter
	Registry         *apps.Registry
	Appearance       *theme.Service
}

type App struct {
	dataRoot         string
	db               *db.DB
	sharePasswordKey []byte
	secureCookies    bool
	unlockLimiter    *httpguard.Limiter
	uploadLimiter    *httpguard.Limiter
	quotaBytes       int64
	quotaMu          sync.Mutex
	search           apps.SearchIndexWriter
	registry         *apps.Registry
	appearance       *theme.Service
}

func New(config Config) *App {
	if config.DataRoot == "" {
		config.DataRoot = "data"
	}
	return &App{dataRoot: config.DataRoot, db: config.DB, sharePasswordKey: append([]byte(nil), config.SharePasswordKey...), secureCookies: config.SecureCookies, unlockLimiter: httpguard.NewLimiter(5, time.Minute), uploadLimiter: httpguard.NewLimiter(20, time.Minute), quotaBytes: config.UserQuotaBytes, search: config.Search, registry: config.Registry, appearance: config.Appearance}
}

func (a *App) Name() string        { return "files" }
func (a *App) DisplayName() string { return "Files" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-folder"></i>` }
func (a *App) Description() string { return "File browser" }
func (a *App) AppBarOrder() int    { return 3 }
func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/files/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handleAsset)},
		{Method: http.MethodGet, Pattern: "/files/assets/picker.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handleAsset)},
		{Method: http.MethodGet, Pattern: "/files", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: http.MethodGet, Pattern: "/files/*", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: apps.MethodAny, Pattern: "/s/*", Auth: apps.AuthPublic, Handler: http.HandlerFunc(a.handlePublicShareRoute)},
		{Method: apps.MethodAny, Pattern: "/remote.php/dav/files", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNextcloudWebDAV)},
		{Method: apps.MethodAny, Pattern: "/remote.php/dav/files/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleNextcloudWebDAV)},
		{Method: apps.MethodAny, Pattern: "/api/files/list", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleList)},
		{Method: http.MethodGet, Pattern: "/api/files/picker", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handlePicker)},
		{Method: http.MethodPost, Pattern: "/api/files/picker/validate", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handlePickerValidate)},
		{Method: apps.MethodAny, Pattern: "/api/files/upload", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleUpload)},
		{Method: apps.MethodAny, Pattern: "/api/files/download", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDownload)},
		{Method: apps.MethodAny, Pattern: "/api/files/editor", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleEditor)},
		{Method: apps.MethodAny, Pattern: "/api/files/mkdir", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleMkdir)},
		{Method: apps.MethodAny, Pattern: "/api/files/touch", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleTouch)},
		{Method: http.MethodPost, Pattern: "/api/files/symlink", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSymlink)},
		{Method: apps.MethodAny, Pattern: "/api/files/rename", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleRename)},
		{Method: apps.MethodAny, Pattern: "/api/files/move", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleMove)},
		{Method: apps.MethodAny, Pattern: "/api/files/copy", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCopy)},
		{Method: apps.MethodAny, Pattern: "/api/files/delete", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDelete)},
		{Method: apps.MethodAny, Pattern: "/api/files/restore", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleRestore)},
		{Method: apps.MethodAny, Pattern: "/api/files/delete-permanent", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDeletePermanent)},
		{Method: apps.MethodAny, Pattern: "/api/files/favorite", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleFavorite)},
		{Method: apps.MethodAny, Pattern: "/api/files/search", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSearch)},
		{Method: apps.MethodAny, Pattern: "/api/files/tags", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleTags)},
		{Method: apps.MethodAny, Pattern: "/api/files/tag", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleTag)},
		{Method: apps.MethodAny, Pattern: "/api/files/thumb", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleThumbnail)},
		{Method: apps.MethodAny, Pattern: "/api/files/preview", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handlePreview)},
		{Method: apps.MethodAny, Pattern: "/api/files/versions", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleVersions)},
		{Method: apps.MethodAny, Pattern: "/api/files/version", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleVersion)},
		{Method: apps.MethodAny, Pattern: "/api/files/diff", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDiff)},
		{Method: apps.MethodAny, Pattern: "/api/files/stats", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleStats)},
		{Method: apps.MethodAny, Pattern: "/api/files/share", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleShare)},
		{Method: apps.MethodAny, Pattern: "/api/files/shared", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleShared)},
		{Method: apps.MethodAny, Pattern: "/api/files/comments", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleComments)},
		{Method: apps.MethodAny, Pattern: "/api/files/lock", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleLock)},
		{Method: apps.MethodAny, Pattern: "/api/files/share-user", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleUserShare)},
		{Method: apps.MethodAny, Pattern: "/api/files/shared-with-me", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSharedWithMe)},
	}
}

func (a *App) handlePublicShareRoute(w http.ResponseWriter, r *http.Request) {
	rest := strings.Trim(strings.TrimPrefix(r.URL.Path, "/s/"), "/")
	parts := strings.Split(rest, "/")
	token := parts[0]
	if token == "" {
		http.NotFound(w, r)
		return
	}
	if len(parts) == 2 && parts[1] == "unlock" && r.Method == http.MethodPost {
		a.unlockPublicShare(w, r, token)
		return
	}
	if len(parts) == 2 && parts[1] == "upload" && r.Method == http.MethodPost {
		a.handlePublicShareUpload(w, r, token)
		return
	}
	if len(parts) == 3 && parts[1] == "appearance" && parts[2] == "wallpaper" && r.Method == http.MethodGet {
		a.servePublicShareWallpaper(w, r, token)
		return
	}
	if r.Method == http.MethodGet {
		a.servePublicSharePath(w, r, token, strings.Join(parts[1:], "/"))
		return
	}
	http.NotFound(w, r)
}

func handleAsset(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
	switch r.URL.Path {
	case "/files/assets/app.js":
		http.ServeContent(w, r, "app.js", appScriptModTime, bytes.NewReader(appScript))
	case "/files/assets/picker.js":
		http.ServeContent(w, r, "picker.js", appScriptModTime, bytes.NewReader(pickerScript))
	default:
		http.NotFound(w, r)
	}
}

func handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	if err := renderPage(w, r, user.DisplayName, user.ID); err != nil {
		http.Error(w, "Internal server error", http.StatusInternalServerError)
	}
}

func renderPage(w io.Writer, r *http.Request, displayName, userID string) error {
	active := filesActiveView(r)
	return webui.RenderRequest(w, r, webui.PageData{
		Title:      "Files - Dar",
		CurrentApp: "files",
		AppBar:     server.AppBarHTML(r),
		UserMenu:   webui.UserMenuHTML(displayName),
		Sidebar: webui.BuildSidebar(webui.SidebarConfig{StateKey: "files", AriaLabel: "Files navigation", Items: []webui.SidebarItem{
			{Label: "Search", Icon: "fa-solid fa-magnifying-glass", Href: "/files?view=search", Active: active == "search"},
			{Label: "All files", Icon: "fa-solid fa-folder", Href: "/files", Active: active == "all"},
			{Label: "Personal files", Icon: "fa-solid fa-user", Href: "/files/docs", Active: active == "personal"},
			{Label: "Recent", Icon: "fa-solid fa-clock", Href: "/files?view=recent", Active: active == "recent"},
			{Label: "Favorites", Icon: "fa-solid fa-star", Href: "/files?filter=favorites", Active: active == "favorites"},
			{Label: "Shares", Icon: "fa-solid fa-share-nodes", Href: "/files?view=shares", Active: active == "shares"},
			{Label: "Tags", Icon: "fa-solid fa-tags", Href: "/files?view=tags", Active: active == "tags"},
			{Label: "Deleted files", Icon: "fa-solid fa-trash-can", Href: "/files/.local/share/Trash", Active: active == "trash"},
		}, SettingsHref: "#"}),
		ContentTemplate: "files.html",
		ContentData: map[string]any{
			"ActiveView": active,
			"UserID":     userID,
		},
	})
}

func filesActiveView(r *http.Request) string {
	filter := r.URL.Query().Get("filter")
	view := r.URL.Query().Get("view")
	switch {
	case filter == "favorites":
		return "favorites"
	case view != "":
		return view
	}

	urlPath := strings.TrimPrefix(r.URL.Path, "/files")
	switch {
	case urlPath == "/docs" || strings.HasPrefix(urlPath, "/docs/"):
		return "personal"
	case urlPath == "/.local/share/Trash" || strings.HasPrefix(urlPath, "/.local/share/Trash/"):
		return "trash"
	case urlPath == "" || urlPath == "/":
		return "all"
	default:
		return "all"
	}
}
