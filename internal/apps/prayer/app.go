// Package prayer implements offline prayer-time calculation, Todo tracking,
// and completion analytics for Dar users.
package prayer

import (
	"context"
	"log"
	"net/http"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/apps/todo"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/geocode"
)

const (
	settingsKey = "settings"
	sourceName  = "prayer"
)

type TodoSync interface {
	ReconcileGeneratedTasks(context.Context, string, string, string, []todo.GeneratedTask) (map[string]todo.GeneratedTaskState, error)
	GeneratedTaskStates(context.Context, string, string) (map[string]todo.GeneratedTaskState, error)
	RemoveGeneratedTasks(context.Context, string, string, []string) ([]string, error)
}

type Config struct {
	DB           *db.DB
	Geocoder     *geocode.Client
	Todo         TodoSync
	Now          func() time.Time
	SyncInterval time.Duration
}

type App struct {
	db           *db.DB
	geocoder     *geocode.Client
	todo         TodoSync
	now          func() time.Time
	syncInterval time.Duration
	cancel       context.CancelFunc
	wg           sync.WaitGroup
	syncMu       sync.Mutex
}

func New(config Config) *App {
	if config.Now == nil {
		config.Now = time.Now
	}
	if config.SyncInterval <= 0 {
		config.SyncInterval = 6 * time.Hour
	}
	return &App{db: config.DB, geocoder: config.Geocoder, todo: config.Todo, now: config.Now, syncInterval: config.SyncInterval}
}

func (a *App) Name() string        { return "prayer" }
func (a *App) DisplayName() string { return "Prayer Times" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-mosque"></i>` }
func (a *App) Description() string { return "Daily prayer times and tracking" }
func (a *App) AppBarOrder() int    { return 6 }
func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/prayer/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePrayerAsset)},
		{Method: http.MethodGet, Pattern: "/prayer", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/prayer/analytics", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/api/prayer/times", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleTimes)},
		{Method: http.MethodGet, Pattern: "/api/prayer/settings", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleSettings)},
		{Method: http.MethodPut, Pattern: "/api/prayer/settings", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleSettings)},
		{Method: http.MethodGet, Pattern: "/api/prayer/analytics", Auth: apps.AuthSessionAPI, Handler: http.HandlerFunc(a.handleAnalytics)},
	}
}

func (a *App) Start(ctx context.Context) error {
	if a.db == nil || a.todo == nil {
		return nil
	}
	workerCtx, cancel := context.WithCancel(ctx)
	a.cancel = cancel
	a.wg.Add(1)
	go func() {
		defer a.wg.Done()
		a.syncAll(workerCtx)
		ticker := time.NewTicker(a.syncInterval)
		defer ticker.Stop()
		for {
			select {
			case <-workerCtx.Done():
				return
			case <-ticker.C:
				a.syncAll(workerCtx)
			}
		}
	}()
	return nil
}

func (a *App) Shutdown(ctx context.Context) error {
	if a.cancel != nil {
		a.cancel()
	}
	done := make(chan struct{})
	go func() { a.wg.Wait(); close(done) }()
	select {
	case <-done:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (a *App) syncAll(ctx context.Context) {
	for _, userID := range a.enabledUsers(ctx) {
		if err := a.syncUser(ctx, userID); err != nil {
			log.Printf("sync prayer tasks for %s: %v", userID, err)
			a.setSyncWarning(context.Background(), userID, err.Error())
		}
	}
}
