// Package codex provides Dar's Codex CLI-backed assistant.
package codex

import (
	"context"
	"net/http"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
)

type Config struct {
	DB           *db.DB
	Registry     *apps.Registry
	CodexPath    string
	HomeRoot     string
	IdleTimeout  time.Duration
	MaxProcesses int
	CSRFToken    func(*http.Request) string
	Search       apps.SearchIndexWriter
}

type App struct{ service *Service }

func New(config Config) *App {
	return &App{service: newService(config)}
}

func (a *App) Name() string        { return "codex" }
func (a *App) DisplayName() string { return "Codex" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-terminal"></i>` }
func (a *App) Description() string { return "AI assistant for your Dar data" }
func (a *App) AppBarOrder() int    { return 1 }

func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/codex/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.service.handleAsset)},
		{Method: http.MethodGet, Pattern: "/codex", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.service.handlePage)},
		{Method: apps.MethodAny, Pattern: "/api/codex", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.service.handleAPI)},
		{Method: apps.MethodAny, Pattern: "/api/codex/*", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.service.handleAPI)},
	}
}

func (a *App) Start(ctx context.Context) error    { return a.service.Start(ctx) }
func (a *App) Shutdown(ctx context.Context) error { return a.service.Shutdown(ctx) }
