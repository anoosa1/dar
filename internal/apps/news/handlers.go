package news

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/credentials"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type Config struct {
	DataRoot       string
	DB             *db.DB
	RefreshMinutes int
	ExploreURL     string
	CredentialKeys []credentials.Key
	OutboundPolicy outbound.Policy
	Search         apps.SearchIndexWriter
	Registry       *apps.Registry
}

type App struct {
	dataRoot       string
	db             *db.DB
	refreshMinutes int
	exploreURL     string
	ctx            context.Context
	cancel         context.CancelFunc
	wg             sync.WaitGroup
	mu             sync.Mutex
	active         map[string]bool
	refresh        map[string]refreshState
	workers        chan struct{}
	keys           credentials.Ring
	keyErr         error
	outbound       outbound.Policy
	search         apps.SearchIndexWriter
	registry       *apps.Registry
}

func New(config Config) *App {
	if config.DataRoot == "" {
		config.DataRoot = "data"
	}
	if config.RefreshMinutes <= 0 {
		config.RefreshMinutes = 30
	}
	ring, keyErr := credentials.New(config.CredentialKeys)
	return &App{dataRoot: config.DataRoot, db: config.DB, refreshMinutes: config.RefreshMinutes, exploreURL: config.ExploreURL, active: map[string]bool{}, refresh: map[string]refreshState{}, workers: make(chan struct{}, 4), keys: ring, keyErr: keyErr, outbound: config.OutboundPolicy, search: config.Search, registry: config.Registry}
}

func (a *App) recordActivity(ctx context.Context, userID, action, summary, objectType, objectID, objectName, origin string) {
	if a.registry == nil || userID == "" {
		return
	}
	actorID, actorName, actorKind := userID, "User", "user"
	if origin == "worker" || origin == "background" {
		actorID, actorName, actorKind = "", "Dar", "system"
	} else {
		_ = a.withConn(ctx, func(conn *sqlite.Conn) error {
			return sqlitex.Execute(conn, `SELECT display_name FROM users WHERE id=?`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
				if strings.TrimSpace(stmt.ColumnText(0)) != "" {
					actorName = stmt.ColumnText(0)
				}
				return nil
			}})
		})
	}
	if err := a.registry.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: actorID, ActorName: actorName, ActorKind: actorKind, Source: "news",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/news", Origin: origin, RecipientIDs: []string{userID},
	}); err != nil {
		log.Printf("record news activity: %v", err)
	}
}
func (a *App) Name() string        { return "news" }
func (a *App) DisplayName() string { return "News" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-newspaper"></i>` }
func (a *App) Description() string { return "RSS and Atom reader" }
func (a *App) AppBarOrder() int    { return 9 }

func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/news/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handleAsset)},
		{Method: apps.MethodAny, Pattern: "/news", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: apps.MethodAny, Pattern: "/news/api/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleAux)},
		{Method: apps.MethodAny, Pattern: "/news/*", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/news/api/v1-3", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleV13)},
		{Method: apps.MethodAny, Pattern: "/index.php/apps/news/api/v1-3/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleV13)},
	}
}

func (a *App) Start(ctx context.Context) error {
	if a.db == nil {
		return errors.New("news database is required")
	}
	if err := os.MkdirAll(filepath.Join(a.dataRoot, "news"), 0700); err != nil {
		return err
	}
	a.ctx, a.cancel = context.WithCancel(ctx)
	a.wg.Add(1)
	go func() {
		defer a.wg.Done()
		ticker := time.NewTicker(time.Duration(a.refreshMinutes) * time.Minute)
		defer ticker.Stop()
		for {
			select {
			case <-a.ctx.Done():
				return
			case <-ticker.C:
				a.refreshAll(a.ctx)
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

func (a *App) refreshAll(ctx context.Context) {
	var users []string
	_ = a.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `SELECT DISTINCT user_id FROM news_feeds WHERE update_enabled=1`, &sqlitex.ExecOptions{ResultFunc: func(s *sqlite.Stmt) error { users = append(users, s.ColumnText(0)); return nil }})
	})
	for _, uid := range users {
		select {
		case <-ctx.Done():
			return
		default:
			a.refreshUserAsync(uid)
		}
	}
}

// updateFeed de-duplicates concurrent scheduler, UI, and updater requests.
func (a *App) updateFeed(ctx context.Context, uid string, id int64) error {
	select {
	case a.workers <- struct{}{}:
		defer func() { <-a.workers }()
	case <-ctx.Done():
		return ctx.Err()
	}
	key := uid + ":" + strconv.FormatInt(id, 10)
	a.mu.Lock()
	if a.active[key] {
		a.mu.Unlock()
		return nil
	}
	a.active[key] = true
	a.mu.Unlock()
	defer func() { a.mu.Lock(); delete(a.active, key); a.mu.Unlock() }()
	return a.updateFeedNow(ctx, uid, id)
}

func (a *App) refreshUserAsync(uid string) {
	a.mu.Lock()
	if a.refresh[uid].Running {
		a.mu.Unlock()
		return
	}
	a.refresh[uid] = refreshState{Running: true, StartedAt: time.Now()}
	a.mu.Unlock()
	ctx := a.ctx
	if ctx == nil {
		ctx = context.Background()
	}
	a.wg.Add(1)
	go func() {
		defer a.wg.Done()
		feeds, err := a.feeds(ctx, uid)
		if err == nil {
			ids := make([]int64, 0, len(feeds))
			for _, f := range feeds {
				if f.UpdateEnabled {
					ids = append(ids, f.ID)
				}
			}
			for _, id := range ids {
				// One in-flight feed per user preserves fairness while the shared
				// worker semaphore bounds aggregate outbound work.
				_ = a.updateFeed(ctx, uid, id)
			}
		}
		summary := fmt.Sprintf("refreshed %d news feeds", len(feeds))
		if err != nil {
			summary = "news refresh failed"
		}
		a.recordActivity(ctx, uid, "feeds_refreshed", summary, "sync", uid, "News refresh", "background")
		a.mu.Lock()
		a.refresh[uid] = refreshState{}
		a.mu.Unlock()
	}()
}

func (a *App) handlePage(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	view := "unread"
	if r.URL.Path != "/news" {
		view = strings.Trim(strings.TrimPrefix(r.URL.Path, "/news/"), "/")
	}
	content, _, err := webui.RenderContentWithScript("news.html", map[string]any{"View": view}, "/news/assets/app.js")
	if err != nil {
		http.Error(w, "template error", http.StatusInternalServerError)
		return
	}
	webui.RenderRequest(w, r, webui.PageData{Title: "News — Dar", CurrentApp: "news", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(user.DisplayName), Content: content})
}

func (a *App) handleAsset(w http.ResponseWriter, r *http.Request) {
	_, script, err := webui.RenderContentWithScript("news.html", map[string]any{"View": "unread"}, "/news/assets/app.js")
	if err != nil {
		http.NotFound(w, r)
		return
	}
	webui.ServeAppJavaScript(w, r, "app.js", script)
}

func userID(r *http.Request) string {
	if u := server.GetUser(r); u != nil {
		return u.ID
	}
	return ""
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
func readJSON(r *http.Request, dst any) error {
	dec := json.NewDecoder(io.LimitReader(r.Body, 2<<20))
	return dec.Decode(dst)
}
func idPart(s string) (int64, error) { return strconv.ParseInt(s, 10, 64) }
