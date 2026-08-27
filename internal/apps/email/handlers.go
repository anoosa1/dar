package email

import (
	"context"
	"crypto/tls"
	"embed"
	"errors"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

//go:embed assets/app.js
var emailFS embed.FS

type Config struct {
	DataRoot        string
	DB              *db.DB
	MBSyncPath      string
	TunnelPath      string
	SyncMinutes     int
	SyncConcurrency int
	CredentialKeys  []CredentialKey
	Recipients      apps.RecipientProvider
	OutboundPolicy  outbound.Policy
	Search          apps.SearchIndexWriter
	Registry        *apps.Registry
}

type App struct {
	dataRoot       string
	db             *db.DB
	mbsyncPath     string
	tunnelPath     string
	syncMinutes    int
	recipients     apps.RecipientProvider
	keys           keyRing
	disabledReason string
	ctx            context.Context
	cancel         context.CancelFunc
	wg             sync.WaitGroup
	workers        chan struct{}
	mu             sync.Mutex
	active         map[string]bool
	status         map[string]SyncStatus
	accountLocks   map[string]*sync.Mutex
	outbound       outbound.Policy
	search         apps.SearchIndexWriter
	registry       *apps.Registry
	smtpTLSConfig  func(string) *tls.Config
}

func New(config Config) *App {
	if config.DataRoot == "" {
		config.DataRoot = "data"
	}
	if config.MBSyncPath == "" {
		config.MBSyncPath = "mbsync"
	}
	if config.SyncMinutes <= 0 {
		config.SyncMinutes = 5
	}
	if config.SyncConcurrency <= 0 {
		config.SyncConcurrency = 2
	}
	a := &App{dataRoot: config.DataRoot, db: config.DB, mbsyncPath: config.MBSyncPath, tunnelPath: config.TunnelPath, syncMinutes: config.SyncMinutes, recipients: config.Recipients, workers: make(chan struct{}, config.SyncConcurrency), active: map[string]bool{}, status: map[string]SyncStatus{}, accountLocks: map[string]*sync.Mutex{}, outbound: config.OutboundPolicy, search: config.Search, registry: config.Registry}
	if a.tunnelPath == "" {
		a.tunnelPath, _ = os.Executable()
	}
	a.keys = newKeyRing(config.CredentialKeys)
	if a.keys.err != nil {
		a.disabledReason = a.keys.err.Error()
	}
	return a
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
		ActorID: actorID, ActorName: actorName, ActorKind: actorKind, Source: "email",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/email", Origin: origin, RecipientIDs: []string{userID},
	}); err != nil {
		log.Printf("record email activity: %v", err)
	}
}

func (a *App) Name() string        { return "email" }
func (a *App) DisplayName() string { return "Email" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-envelope"></i>` }
func (a *App) Description() string { return "Multi-account IMAP and SMTP mail" }
func (a *App) AppBarOrder() int    { return 10 }
func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/email", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: http.MethodGet, Pattern: "/email/*", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(a.handlePage)},
		{Method: apps.MethodAny, Pattern: "/api/email", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleAPI)},
		{Method: apps.MethodAny, Pattern: "/api/email/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleAPI)},
	}
}

func (a *App) handlePage(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/email/assets/app.js" {
		data, err := emailFS.ReadFile("assets/app.js")
		if err != nil {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "text/javascript; charset=utf-8")
		w.Header().Set("Cache-Control", "no-cache")
		_, _ = w.Write(data)
		return
	}
	u := server.GetUser(r)
	if u == nil {
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	_ = webui.RenderRequest(w, r, webui.PageData{Title: "Email — Dar", CurrentApp: "email", AppBar: server.AppBarHTML(r), UserMenu: webui.UserMenuHTML(u.DisplayName), ContentTemplate: "email.html", ContentData: map[string]any{"DisabledReason": a.disabledReason}})
}

func (a *App) Start(ctx context.Context) error {
	if a.db == nil {
		return errors.New("email database is required")
	}
	root := filepath.Join(a.dataRoot, "email")
	if err := os.MkdirAll(root, 0700); err != nil {
		return err
	}
	if a.disabledReason == "" {
		if err := a.validateStoredCredentials(ctx); err != nil {
			a.disabledReason = err.Error()
		}
	}
	if _, err := exec.LookPath(a.mbsyncPath); err != nil {
		if a.disabledReason != "" {
			a.disabledReason += "; "
		}
		a.disabledReason += "mbsync is not installed or DAR_EMAIL_MBSYNC_PATH is invalid"
	}
	a.ctx, a.cancel = context.WithCancel(ctx)
	a.wg.Add(1)
	go a.pollLoop()
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

func (a *App) pollLoop() {
	defer a.wg.Done()
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for {
		select {
		case <-a.ctx.Done():
			return
		case <-ticker.C:
		}
		ids, _ := a.dueAccountIDs(a.ctx, nowUnix())
		for _, item := range ids {
			a.queueSync(item.UserID, item.AccountID, "scheduled")
		}
	}
}

func (a *App) lockFor(accountID string) *sync.Mutex {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.accountLocks[accountID] == nil {
		a.accountLocks[accountID] = &sync.Mutex{}
	}
	return a.accountLocks[accountID]
}

func (a *App) available() error {
	if a.disabledReason != "" {
		return errors.New(a.disabledReason)
	}
	return nil
}
