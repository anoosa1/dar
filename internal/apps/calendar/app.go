package calendar

import (
	"context"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/caldav"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type Config struct {
	DataRoot        string
	DB              *db.DB
	CalDAV          *caldav.Service
	RefreshInterval time.Duration
	OutboundPolicy  outbound.Policy
	Search          apps.SearchIndexWriter
	Registry        *apps.Registry
}

type App struct {
	dataRoot        string
	db              *db.DB
	caldav          *caldav.Service
	refreshInterval time.Duration
	cancelRefresh   context.CancelFunc
	refreshWG       sync.WaitGroup
	outbound        outbound.Policy
	search          apps.SearchIndexWriter
	registry        *apps.Registry
}

func New(config Config) *App {
	if config.DataRoot == "" {
		config.DataRoot = "data"
	}
	if config.RefreshInterval <= 0 {
		config.RefreshInterval = 30 * time.Minute
	}
	a := &App{dataRoot: config.DataRoot, db: config.DB, caldav: config.CalDAV, refreshInterval: config.RefreshInterval, outbound: config.OutboundPolicy, search: config.Search, registry: config.Registry}
	if a.caldav != nil {
		a.caldav.Register(a)
	}
	return a
}

func (a *App) recordActivity(ctx context.Context, userID, action, summary, objectType, objectID, objectName, origin string) {
	if a.registry == nil || userID == "" {
		return
	}
	if strings.TrimSpace(origin) == "" {
		origin = "api"
	}
	actorID, actorName, actorKind := userID, "User", "user"
	if origin == "worker" || origin == "background" {
		actorID, actorName, actorKind = "", "Dar", "system"
	} else {
		_ = a.withConn(func(conn *sqlite.Conn) error {
			return sqlitex.Execute(conn, `SELECT display_name FROM users WHERE id=?`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				if strings.TrimSpace(stmt.ColumnText(0)) != "" {
					actorName = stmt.ColumnText(0)
				}
				return nil
			}})
		})
	}
	if err := a.registry.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: actorID, ActorName: actorName, ActorKind: actorKind, Source: "calendar",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/calendar", Origin: origin, RecipientIDs: []string{userID},
	}); err != nil {
		log.Printf("record calendar activity: %v", err)
	}
}

func (a *App) Name() string        { return "calendar" }
func (a *App) DisplayName() string { return "Calendar" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-calendar"></i>` }
func (a *App) Description() string { return "Manage your schedule" }
func (a *App) AppBarOrder() int    { return 5 }
func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/calendar/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handleCalendarAsset)},
		{Method: http.MethodGet, Pattern: "/calendar", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: apps.MethodAny, Pattern: "/caldav", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCalDAV)},
		{Method: apps.MethodAny, Pattern: "/caldav/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCalDAV)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/calendars", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCalendars)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/calendars/update", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCalendarUpdate)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/calendars/delete", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCalendarDelete)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/list", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleList)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/get", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleGet)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/create", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCreate)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/update", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleUpdate)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/delete", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDelete)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/search", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSearch)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/import", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleImport)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/export", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleExport)},
		{Method: apps.MethodAny, Pattern: "/api/calendar/subscriptions", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSubscriptions)},
		{Method: http.MethodPost, Pattern: "/api/calendar/subscriptions/refresh", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSubscriptionRefresh)},
	}
}

// Start refreshes subscribed calendars in the background. It is intentionally
// best-effort: a broken remote feed must never prevent Dar from starting.
func (a *App) Start(ctx context.Context) error {
	if a.db == nil {
		return nil
	}
	workerCtx, cancel := context.WithCancel(ctx)
	a.cancelRefresh = cancel
	a.refreshWG.Add(1)
	go func() {
		defer a.refreshWG.Done()
		a.refreshAllSubscriptions(workerCtx)
		ticker := time.NewTicker(a.refreshInterval)
		defer ticker.Stop()
		for {
			select {
			case <-workerCtx.Done():
				return
			case <-ticker.C:
				a.refreshAllSubscriptions(workerCtx)
			}
		}
	}()
	return nil
}

func (a *App) Shutdown(ctx context.Context) error {
	if a.cancelRefresh != nil {
		a.cancelRefresh()
	}
	done := make(chan struct{})
	go func() { a.refreshWG.Wait(); close(done) }()
	select {
	case <-done:
	case <-ctx.Done():
		return ctx.Err()
	}
	return nil
}
