// Package activity provides Dar's cross-app activity stream and RSS feed.
package activity

import (
	"context"
	"embed"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
)

//go:embed assets/*
var activityFS embed.FS

var activityScript = mustAsset("assets/app.js")

func mustAsset(name string) []byte {
	data, err := activityFS.ReadFile(name)
	if err != nil {
		panic(err)
	}
	return data
}

type Config struct {
	DB       *db.DB
	Registry *apps.Registry
	BaseURL  string
}

type App struct {
	db       *db.DB
	registry *apps.Registry
	baseURL  string
	wake     chan struct{}
	cancel   context.CancelFunc
	wg       sync.WaitGroup
}

func New(config Config) *App {
	return &App{
		db:       config.DB,
		registry: config.Registry,
		baseURL:  strings.TrimRight(config.BaseURL, "/"),
		wake:     make(chan struct{}, 1),
	}
}

func (a *App) Name() string        { return "activity" }
func (a *App) DisplayName() string { return "Activity" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-bolt"></i>` }
func (a *App) Description() string { return "Actions across Dar" }
func (a *App) AppBarOrder() int    { return 2 }

func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/activity/rss/{token}", Auth: apps.AuthPublic, Handler: http.HandlerFunc(a.handleRSS)},
		{Method: http.MethodGet, Pattern: "/activity/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handleAsset)},
		{Method: http.MethodGet, Pattern: "/activity", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/activity/*", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/api/activity", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleList)},
		{Method: apps.MethodAny, Pattern: "/api/activity/settings", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleSettings)},
		{Method: http.MethodPost, Pattern: "/api/activity/rss/regenerate", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleRegenerate)},
	}
}

func (a *App) handleAsset(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
	http.ServeContent(w, r, "app.js", time.Time{}, strings.NewReader(string(activityScript)))
}

func (a *App) handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	if err := webui.RenderRequest(w, r, webui.PageData{
		Title:           "Activity — Dar",
		CurrentApp:      "activity",
		AppBar:          server.AppBarHTML(r),
		UserMenu:        webui.UserMenuHTML(user.DisplayName),
		ContentTemplate: "activity.html",
	}); err != nil {
		http.Error(w, "Internal server error", http.StatusInternalServerError)
	}
}

func (a *App) Start(ctx context.Context) error {
	if a.db == nil {
		return nil
	}
	workerCtx, cancel := context.WithCancel(ctx)
	a.cancel = cancel
	a.wg.Add(1)
	go a.runOutbox(workerCtx)
	return nil
}

func (a *App) Shutdown(ctx context.Context) error {
	if a.cancel != nil {
		a.cancel()
	}
	done := make(chan struct{})
	go func() {
		a.wg.Wait()
		close(done)
	}()
	select {
	case <-done:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}
