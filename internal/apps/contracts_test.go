package apps_test

import (
	"context"
	"net/http"
	"sort"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/apps/activity"
	"github.com/anoosa/dar/internal/apps/calendar"
	"github.com/anoosa/dar/internal/apps/codex"
	"github.com/anoosa/dar/internal/apps/contacts"
	"github.com/anoosa/dar/internal/apps/email"
	"github.com/anoosa/dar/internal/apps/files"
	"github.com/anoosa/dar/internal/apps/news"
	"github.com/anoosa/dar/internal/apps/notes"
	"github.com/anoosa/dar/internal/apps/prayer"
	"github.com/anoosa/dar/internal/apps/todo"
)

type appContract struct {
	app         apps.App
	displayName string
	description string
	order       int
	paths       string
}

func TestEveryAppMetadataAndRouteManifest(t *testing.T) {
	contracts := map[string]appContract{
		"activity": {
			app: activity.New(activity.Config{}), displayName: "Activity", description: "Actions across Dar", order: 2,
			paths: `/activity/rss/{token} /activity/assets/app.js /activity /activity/* /api/activity /api/activity/settings /api/activity/rss/regenerate`,
		},
		"calendar": {
			app: calendar.New(calendar.Config{}), displayName: "Calendar", description: "Manage your schedule", order: 5,
			paths: `/calendar /calendar/assets/app.js /caldav /caldav/* /api/calendar/calendars /api/calendar/calendars/update /api/calendar/calendars/delete
				/api/calendar/list /api/calendar/get /api/calendar/create /api/calendar/update /api/calendar/delete /api/calendar/search
				/api/calendar/import /api/calendar/export /api/calendar/subscriptions /api/calendar/subscriptions/refresh`,
		},
		"codex": {
			app: codex.New(codex.Config{}), displayName: "Codex", description: "AI assistant for your Dar data", order: 1,
			paths: `/codex /codex/assets/app.js /api/codex /api/codex/*`,
		},
		"contacts": {
			app: contacts.New(contacts.Config{}), displayName: "Contacts", description: "Manage your contacts", order: 4,
			paths: `/contacts /contacts/assets/app.js /contacts/* /dav /dav/* /api/contacts/list /api/contacts/get /api/contacts/create
				/api/contacts/update /api/contacts/delete /api/contacts/search /api/contacts/export /api/contacts/import
				/api/contacts/books /api/contacts/books/rename /api/contacts/books/delete /api/contacts/books/visibility
				/api/contacts/preferences`,
		},
		"email": {
			app: email.New(email.Config{}), displayName: "Email", description: "Multi-account IMAP and SMTP mail", order: 10,
			paths: `/email /email/* /api/email /api/email/*`,
		},
		"files": {
			app: files.New(files.Config{}), displayName: "Files", description: "File browser", order: 3,
			paths: `/files/assets/app.js /files/assets/picker.js /files /files/* /s/* /remote.php/dav/files /remote.php/dav/files/*
				/api/files/list /api/files/picker /api/files/picker/validate /api/files/upload /api/files/download /api/files/editor
				/api/files/mkdir /api/files/touch /api/files/symlink /api/files/rename /api/files/move /api/files/copy
				/api/files/delete /api/files/restore /api/files/delete-permanent /api/files/favorite /api/files/search
				/api/files/tags /api/files/tag /api/files/thumb /api/files/preview /api/files/versions /api/files/version
				/api/files/diff /api/files/stats /api/files/share /api/files/shared /api/files/comments /api/files/lock
				/api/files/share-user /api/files/shared-with-me`,
		},
		"news": {
			app: news.New(news.Config{}), displayName: "News", description: "RSS and Atom reader", order: 9,
			paths: `/news /news/assets/app.js /news/api/* /news/* /index.php/apps/news/api/v1-3 /index.php/apps/news/api/v1-3/*`,
		},
		"notes": {
			app: notes.New(notes.Config{}), displayName: "Notes", description: "Markdown notes", order: 8,
			paths: `/notes/assets/app.js /notes /notes/new /notes/note/{id} /api/notes/notes /api/notes/notes/{id}
				/api/notes/categories /api/notes/undo/{id} /api/notes/versions/{id} /api/notes/settings /api/notes/attachment/{id}
				/index.php/apps/notes/api/v1/notes /index.php/apps/notes/api/v1/notes/{id}
				/index.php/apps/notes/api/v1/settings /index.php/apps/notes/api/v1.4/attachment/{id}`,
		},
		"prayer": {
			app: prayer.New(prayer.Config{}), displayName: "Prayer Times", description: "Daily prayer times and tracking", order: 6,
			paths: `/prayer /prayer/analytics /prayer/assets/app.js /api/prayer/times /api/prayer/settings /api/prayer/settings /api/prayer/analytics`,
		},
		"todo": {
			app: todo.New(todo.Config{}), displayName: "Todo", description: "Task lists and to-dos", order: 7,
			paths: `/todo /todo/assets/app.js /api/todo/lists /api/todo/lists/* /api/todo/tasks /api/todo/tasks/* /api/todo/preferences`,
		},
	}

	for name, contract := range contracts {
		t.Run(name, func(t *testing.T) {
			if contract.app.Name() != name ||
				contract.app.DisplayName() != contract.displayName ||
				contract.app.Description() != contract.description ||
				contract.app.AppBarOrder() != contract.order ||
				strings.TrimSpace(contract.app.Icon()) == "" {
				t.Fatalf("metadata name=%q display=%q description=%q order=%d icon=%q",
					contract.app.Name(), contract.app.DisplayName(), contract.app.Description(),
					contract.app.AppBarOrder(), contract.app.Icon())
			}

			expected := strings.Fields(contract.paths)
			actual := make([]string, 0, len(contract.app.Routes()))
			seen := map[string]bool{}
			for _, route := range contract.app.Routes() {
				if route.Handler == nil || route.Pattern == "" {
					t.Fatalf("invalid route %#v", route)
				}
				key := route.Method + " " + route.Pattern
				if seen[key] {
					t.Fatalf("duplicate route %s", key)
				}
				seen[key] = true
				actual = append(actual, route.Pattern)
				validateRouteClassification(t, name, route)
			}
			sort.Strings(expected)
			sort.Strings(actual)
			if strings.Join(actual, "\n") != strings.Join(expected, "\n") {
				t.Fatalf("route manifest changed\nactual:\n%s\nexpected:\n%s", strings.Join(actual, "\n"), strings.Join(expected, "\n"))
			}
		})
	}
}

func validateRouteClassification(t *testing.T, appName string, route apps.Route) {
	t.Helper()
	switch route.Auth {
	case apps.AuthPublic:
		if !strings.HasPrefix(route.Pattern, "/s/") && !strings.HasPrefix(route.Pattern, "/activity/rss/") {
			t.Errorf("unexpected public route %s", route.Pattern)
		}
	case apps.AuthBrowser:
		if strings.HasPrefix(route.Pattern, "/api/") || strings.HasPrefix(route.Pattern, "/dav") || strings.HasPrefix(route.Pattern, "/caldav") {
			t.Errorf("API-like route classified as browser: %s", route.Pattern)
		}
	case apps.AuthSessionAPI:
		if appName != "activity" && appName != "codex" && appName != "prayer" {
			t.Errorf("unexpected session-only API for %s: %s", appName, route.Pattern)
		}
	case apps.AuthAPI:
	default:
		t.Errorf("invalid auth mode %d for %s", route.Auth, route.Pattern)
	}
	if route.Method == "" {
		t.Errorf("empty method for %s", route.Pattern)
	}
	if route.Method != apps.MethodAny {
		switch route.Method {
		case http.MethodGet, http.MethodPost, http.MethodPut, http.MethodPatch, http.MethodDelete, http.MethodHead, http.MethodOptions:
		default:
			t.Errorf("unexpected method %q for %s", route.Method, route.Pattern)
		}
	}
}

func TestNilDatabaseLifecyclesAreSafe(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	for name, lifecycle := range map[string]apps.Lifecycle{
		"activity": activity.New(activity.Config{}),
		"calendar": calendar.New(calendar.Config{}),
	} {
		t.Run(name, func(t *testing.T) {
			if err := lifecycle.Start(ctx); err != nil {
				t.Fatalf("start: %v", err)
			}
			if err := lifecycle.Shutdown(context.Background()); err != nil {
				t.Fatalf("shutdown: %v", err)
			}
		})
	}
}
