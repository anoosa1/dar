package todo

import (
	"context"
	"log"
	"net/http"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/db"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type Config struct {
	DataRoot string
	DB       *db.DB
	Search   apps.SearchIndexWriter
	Registry *apps.Registry
}

type App struct {
	dataRoot string
	db       *db.DB
	search   apps.SearchIndexWriter
	registry *apps.Registry
}

func New(config Config) *App {
	if config.DataRoot == "" {
		config.DataRoot = "data"
	}
	return &App{dataRoot: config.DataRoot, db: config.DB, search: config.Search, registry: config.Registry}
}

func (a *App) recordActivity(ctx context.Context, userID, listID, action, summary, objectType, objectID, objectName string) {
	a.recordActivityOrigin(ctx, userID, listID, action, summary, objectType, objectID, objectName, "api")
}

func (a *App) recordActivityOrigin(ctx context.Context, userID, listID, action, summary, objectType, objectID, objectName, origin string) {
	if a.registry == nil || userID == "" {
		return
	}
	recipients := []string{userID}
	actorName := "User"
	_ = a.withConn(func(conn *sqlite.Conn) error {
		_ = sqlitex.Execute(conn, `SELECT display_name FROM users WHERE id=?`, &sqlitex.ExecOptions{Args: []any{userID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			if strings.TrimSpace(stmt.ColumnText(0)) != "" {
				actorName = stmt.ColumnText(0)
			}
			return nil
		}})
		return sqlitex.Execute(conn, `SELECT owner_id FROM todo_lists WHERE id=? UNION SELECT user_id FROM todo_list_members WHERE list_id=?`, &sqlitex.ExecOptions{Args: []any{listID, listID}, ResultFunc: func(stmt *sqlite.Stmt) error {
			recipients = append(recipients, stmt.ColumnText(0))
			return nil
		}})
	})
	if err := a.registry.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: userID, ActorName: actorName, ActorKind: "user", Source: "todo",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/todo", Origin: origin, RecipientIDs: recipients,
	}); err != nil {
		log.Printf("record todo activity: %v", err)
	}
}

func (a *App) Name() string        { return "todo" }
func (a *App) DisplayName() string { return "Todo" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-check-double"></i>` }
func (a *App) Description() string { return "Task lists and to-dos" }
func (a *App) AppBarOrder() int    { return 7 }
func (a *App) Routes() []apps.Route {
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/todo/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handleTodoAsset)},
		{Method: http.MethodGet, Pattern: "/todo", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: apps.MethodAny, Pattern: "/api/todo/lists", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleLists)},
		{Method: apps.MethodAny, Pattern: "/api/todo/lists/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleList)},
		{Method: apps.MethodAny, Pattern: "/api/todo/tasks", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleTasks)},
		{Method: apps.MethodAny, Pattern: "/api/todo/tasks/*", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleTask)},
		{Method: apps.MethodAny, Pattern: "/api/todo/preferences", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handlePreferences)},
	}
}
