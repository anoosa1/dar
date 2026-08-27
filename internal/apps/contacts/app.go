package contacts

import (
	"context"
	"log"
	"net/http"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/carddav"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
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

func (a *App) recordActivity(ctx context.Context, userID, action, summary, objectType, objectID, objectName, origin string) {
	if a.registry == nil || userID == "" {
		return
	}
	if err := a.registry.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: userID, ActorName: "User", ActorKind: "user", Source: "contacts",
		Action: action, Summary: summary, ObjectType: objectType, ObjectID: objectID,
		ObjectName: objectName, ObjectURL: "/contacts", Origin: origin, RecipientIDs: []string{userID},
	}); err != nil {
		log.Printf("record contacts activity: %v", err)
	}
}

func (a *App) Name() string        { return "contacts" }
func (a *App) DisplayName() string { return "Contacts" }
func (a *App) Icon() string        { return `<i class="fa-solid fa-address-book"></i>` }
func (a *App) Description() string { return "Manage your contacts" }
func (a *App) AppBarOrder() int    { return 4 }
func (a *App) Routes() []apps.Route {
	dav := &carddav.Handler{
		Backend: fsCardDAV{app: a},
		UserID: func(r *http.Request) string {
			if user := server.GetUser(r); user != nil {
				return user.ID
			}
			return ""
		},
	}
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/contacts/assets/app.js", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handleContactsAsset)},
		{Method: http.MethodGet, Pattern: "/contacts", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: http.MethodGet, Pattern: "/contacts/*", Auth: apps.AuthBrowser, Handler: http.HandlerFunc(handlePage)},
		{Method: apps.MethodAny, Pattern: "/dav", Auth: apps.AuthAPI, Handler: dav},
		{Method: apps.MethodAny, Pattern: "/dav/*", Auth: apps.AuthAPI, Handler: dav},
		{Method: apps.MethodAny, Pattern: "/api/contacts/list", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleList)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/get", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleGet)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/create", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleCreate)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/update", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleUpdate)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/delete", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleDelete)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/search", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleSearch)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/export", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleExport)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/import", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleImport)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/books", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleBooks)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/books/rename", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleBooksRename)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/books/delete", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleBooksDelete)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/books/visibility", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handleBooksVisibility)},
		{Method: apps.MethodAny, Pattern: "/api/contacts/preferences", Auth: apps.AuthAPI, Handler: http.HandlerFunc(a.handlePreferences)},
	}
}
