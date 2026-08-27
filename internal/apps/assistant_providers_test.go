package apps_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/apps/activity"
	"github.com/anoosa/dar/internal/apps/calendar"
	"github.com/anoosa/dar/internal/apps/codex"
	"github.com/anoosa/dar/internal/apps/contacts"
	"github.com/anoosa/dar/internal/apps/coreassistant"
	"github.com/anoosa/dar/internal/apps/email"
	"github.com/anoosa/dar/internal/apps/files"
	"github.com/anoosa/dar/internal/apps/news"
	"github.com/anoosa/dar/internal/apps/notes"
	"github.com/anoosa/dar/internal/apps/todo"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/testutil"
)

func TestAssistantProviderSchemasAndEffects(t *testing.T) {
	providers := map[string]apps.AssistantProvider{
		"files": files.New(files.Config{}), "calendar": calendar.New(calendar.Config{}),
		"contacts": contacts.New(contacts.Config{}), "todo": todo.New(todo.Config{}),
		"notes": notes.New(notes.Config{}), "news": news.New(news.Config{}),
		"email": email.New(email.Config{}),
		"core":  coreassistant.New(nil),
	}
	for namespace, provider := range providers {
		t.Run(namespace, func(t *testing.T) {
			seen := map[string]bool{}
			for _, tool := range provider.AssistantTools() {
				if tool.Name == "" || seen[tool.Name] {
					t.Fatalf("invalid or duplicate tool name %q", tool.Name)
				}
				seen[tool.Name] = true
				var schema map[string]any
				if err := json.Unmarshal(tool.InputSchema, &schema); err != nil || schema["type"] != "object" {
					t.Fatalf("%s schema invalid: %s (%v)", tool.Name, tool.InputSchema, err)
				}
				switch tool.Effect {
				case apps.AssistantRead, apps.AssistantSafeWrite, apps.AssistantExternal, apps.AssistantDestructive:
				default:
					t.Fatalf("%s effect %q", tool.Name, tool.Effect)
				}
				lower := strings.ToLower(tool.Name)
				if (strings.Contains(lower, "delete") || strings.Contains(lower, "remove")) && tool.Effect != apps.AssistantDestructive {
					t.Fatalf("%s must be destructive, got %s", tool.Name, tool.Effect)
				}
				if strings.Contains(lower, "send") && tool.Effect != apps.AssistantExternal {
					t.Fatalf("%s must be external", tool.Name)
				}
			}
			if len(seen) == 0 {
				t.Fatal("provider has no tools")
			}
		})
	}
}

func TestEveryContentAppImplementsSearchIndexer(t *testing.T) {
	contentApps := map[string]apps.App{
		"files": files.New(files.Config{}), "calendar": calendar.New(calendar.Config{}),
		"contacts": contacts.New(contacts.Config{}), "todo": todo.New(todo.Config{}),
		"notes": notes.New(notes.Config{}), "news": news.New(news.Config{}),
		"email": email.New(email.Config{}), "codex": codex.New(codex.Config{}),
	}
	for name, app := range contentApps {
		if _, ok := app.(apps.SearchIndexer); !ok {
			t.Errorf("%s does not implement SearchIndexer", name)
		}
	}
}

func TestEveryAssistantToolHandlesEmptyAndSchemaDerivedInput(t *testing.T) {
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, calendar.Migrations()...)
	migrations = append(migrations, todo.Migrations()...)
	migrations = append(migrations, news.Migrations()...)
	migrations = append(migrations, files.Migrations()...)
	migrations = append(migrations, notes.Migrations()...)
	migrations = append(migrations, email.Migrations()...)
	database := testutil.OpenDB(t, migrations...)
	user := testutil.User(t, database, "assistant-matrix")
	root := t.TempDir()
	filesProvider := files.New(files.Config{DB: database, DataRoot: root})
	providers := map[string]apps.AssistantProvider{
		"files":    filesProvider,
		"calendar": calendar.New(calendar.Config{DB: database, DataRoot: root}),
		"contacts": contacts.New(contacts.Config{DB: database, DataRoot: root}),
		"todo":     todo.New(todo.Config{DB: database, DataRoot: root}),
		"notes":    notes.New(notes.Config{DB: database, Files: filesProvider}),
		"news":     news.New(news.Config{DB: database, DataRoot: root}),
		"email": email.New(email.Config{
			DB: database, DataRoot: root, MBSyncPath: "/bin/true",
			CredentialKeys: []email.CredentialKey{{ID: "test", Key: make([]byte, 32)}},
		}),
		"core": coreassistant.New(database),
	}
	for namespace, provider := range providers {
		for _, tool := range provider.AssistantTools() {
			t.Run(namespace+"/"+tool.Name, func(t *testing.T) {
				for _, arguments := range []json.RawMessage{
					json.RawMessage(`{}`),
					schemaArguments(t, tool.InputSchema, false),
					schemaArguments(t, tool.InputSchema, true),
				} {
					ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
					func() {
						defer func() {
							if recovered := recover(); recovered != nil {
								t.Fatalf("tool panicked for %s: %v", arguments, recovered)
							}
						}()
						_, _ = provider.CallAssistantTool(ctx, user.ID, tool.Name, arguments)
					}()
					cancel()
				}
			})
		}
	}
}

func schemaArguments(t *testing.T, raw json.RawMessage, includeOptional bool) json.RawMessage {
	t.Helper()
	var schema struct {
		Properties map[string]struct {
			Type string `json:"type"`
			Enum []any  `json:"enum"`
		} `json:"properties"`
		Required []string `json:"required"`
	}
	if err := json.Unmarshal(raw, &schema); err != nil {
		t.Fatal(err)
	}
	values := map[string]any{}
	names := append([]string(nil), schema.Required...)
	if includeOptional {
		for name := range schema.Properties {
			if !slices.Contains(names, name) {
				names = append(names, name)
			}
		}
	}
	for _, name := range names {
		property := schema.Properties[name]
		if len(property.Enum) > 0 {
			values[name] = property.Enum[0]
			continue
		}
		switch property.Type {
		case "integer", "number":
			values[name] = 1
		case "boolean":
			values[name] = true
		case "array":
			values[name] = []any{1}
		case "object":
			values[name] = map[string]any{}
		default:
			values[name] = "test"
		}
	}
	encoded, err := json.Marshal(values)
	if err != nil {
		t.Fatal(err)
	}
	return encoded
}

func TestEveryDeclaredAppRouteExecutesItsHandlerMatrix(t *testing.T) {
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, activity.Migrations()...)
	migrations = append(migrations, calendar.Migrations()...)
	migrations = append(migrations, todo.Migrations()...)
	migrations = append(migrations, news.Migrations()...)
	migrations = append(migrations, files.Migrations()...)
	migrations = append(migrations, notes.Migrations()...)
	migrations = append(migrations, email.Migrations()...)
	migrations = append(migrations, codex.Migrations()...)
	database := testutil.OpenDB(t, migrations...)
	user := testutil.User(t, database, "route-matrix")
	root := t.TempDir()
	filesApp := files.New(files.Config{DB: database, DataRoot: root})
	allApps := []apps.App{
		activity.New(activity.Config{DB: database}),
		filesApp,
		contacts.New(contacts.Config{DB: database, DataRoot: root}),
		calendar.New(calendar.Config{DB: database, DataRoot: root}),
		todo.New(todo.Config{DB: database, DataRoot: root}),
		notes.New(notes.Config{DB: database, Files: filesApp}),
		news.New(news.Config{DB: database, DataRoot: root}),
		email.New(email.Config{
			DB: database, DataRoot: root, MBSyncPath: "/bin/true",
			CredentialKeys: []email.CredentialKey{{ID: "test", Key: make([]byte, 32)}},
		}),
		codex.New(codex.Config{DB: database, CodexPath: "/bin/true", HomeRoot: root}),
	}
	for _, app := range allApps {
		for _, route := range app.Routes() {
			methods := []string{route.Method}
			if route.Method == apps.MethodAny {
				methods = []string{http.MethodGet, http.MethodPost, http.MethodPut, http.MethodDelete, http.MethodOptions, "PROPFIND", "REPORT"}
			}
			for _, method := range methods {
				name := app.Name() + "/" + method + "/" + route.Pattern
				t.Run(name, func(t *testing.T) {
					target := concreteRoutePath(route.Pattern)
					ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
					defer cancel()
					request := httptest.NewRequest(method, target, strings.NewReader(`{}`)).WithContext(ctx)
					request.Header.Set("Content-Type", "application/json")
					request.Header.Set("Depth", "1")
					request = request.WithContext(context.WithValue(request.Context(), server.UserContextKey, user))
					recorder := httptest.NewRecorder()
					defer func() {
						if recovered := recover(); recovered != nil {
							t.Fatalf("handler panicked: %v", recovered)
						}
					}()
					route.Handler.ServeHTTP(recorder, request)
					if recorder.Code < 100 || recorder.Code > 599 {
						t.Fatalf("invalid status %d", recorder.Code)
					}
				})
			}
		}
	}
}

func concreteRoutePath(pattern string) string {
	target := strings.ReplaceAll(pattern, "*", "test")
	for {
		start := strings.IndexByte(target, '{')
		if start < 0 {
			break
		}
		end := strings.IndexByte(target[start:], '}')
		if end < 0 {
			break
		}
		target = target[:start] + "1" + target[start+end+1:]
	}
	return target
}
