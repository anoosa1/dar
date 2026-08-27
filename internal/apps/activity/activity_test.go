package activity

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
	"github.com/go-chi/chi/v5"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

type sourceApp struct {
	name  string
	order int
}

func (a sourceApp) Name() string        { return a.name }
func (a sourceApp) DisplayName() string { return strings.ToUpper(a.name[:1]) + a.name[1:] }
func (a sourceApp) Icon() string        { return "" }
func (a sourceApp) Description() string { return "" }
func (a sourceApp) AppBarOrder() int    { return a.order }
func (a sourceApp) Routes() []apps.Route {
	return nil
}

func openActivityTestApp(t *testing.T) (*App, *apps.Registry, *db.DB, *auth.User, *auth.User) {
	t.Helper()
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, `CREATE TABLE files_activity(id TEXT PRIMARY KEY);`)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "activity.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	alice, _, err := auth.FindOrCreateUser(ctx, conn, "alice", "alice@example.test", "Alice", "", "https://id.test", "alice")
	if err != nil {
		release()
		t.Fatal(err)
	}
	bob, _, err := auth.FindOrCreateUser(ctx, conn, "bob", "bob@example.test", "Bob", "", "https://id.test", "bob")
	release()
	if err != nil {
		t.Fatal(err)
	}
	registry := apps.NewRegistry()
	app := New(Config{DB: database, Registry: registry, BaseURL: "https://dar.example"})
	registry.Register(app)
	registry.Register(sourceApp{name: "files", order: 3})
	return app, registry, database, alice, bob
}

func requestAs(request *http.Request, user *auth.User) *http.Request {
	return request.WithContext(context.WithValue(request.Context(), server.UserContextKey, user))
}

func tableExists(t *testing.T, database *db.DB, name string) bool {
	t.Helper()
	found := false
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	if err := sqlitex.Execute(conn, `SELECT 1 FROM sqlite_master WHERE type='table' AND name=?`, &sqlitex.ExecOptions{
		Args: []any{name}, ResultFunc: func(*sqlite.Stmt) error { found = true; return nil },
	}); err != nil {
		t.Fatal(err)
	}
	return found
}

func TestMigrationsDropLegacyFilesActivity(t *testing.T) {
	_, _, database, _, _ := openActivityTestApp(t)
	for _, table := range []string{"activity_events", "activity_recipients", "activity_outbox", "activity_feed_settings"} {
		if !tableExists(t, database, table) {
			t.Fatalf("%s was not created", table)
		}
	}
	if tableExists(t, database, "files_activity") {
		t.Fatal("legacy files_activity table still exists")
	}
}

func TestOutboxPersistsOnceAndIsolatesRecipients(t *testing.T) {
	app, registry, _, alice, bob := openActivityTestApp(t)
	event := apps.ActivityEvent{
		ID: "stable-event", ActorID: alice.ID, ActorName: alice.DisplayName, ActorKind: "user",
		Source: "files", Action: "uploaded", Summary: "uploaded report & notes",
		ObjectType: "file", ObjectID: "/report.txt", ObjectName: "report.txt",
		ObjectURL: "/files?path=%2Freport.txt", Origin: "browser", HappenedAt: 100,
		RecipientIDs: []string{alice.ID},
	}
	if err := registry.EnqueueActivity(context.Background(), event); err != nil {
		t.Fatal(err)
	}
	app.drainOutbox(context.Background())
	// A retry with the same stable ID must not duplicate the event.
	if err := registry.EnqueueActivity(context.Background(), event); err != nil {
		t.Fatal(err)
	}
	app.drainOutbox(context.Background())

	aliceEvents, _, err := app.listEvents(context.Background(), listOptions{UserID: alice.ID, Limit: 50})
	if err != nil {
		t.Fatal(err)
	}
	if len(aliceEvents) != 1 || !aliceEvents[0].ActorIsYou || aliceEvents[0].ObjectID != "/report.txt" {
		t.Fatalf("alice events = %#v", aliceEvents)
	}
	bobEvents, _, err := app.listEvents(context.Background(), listOptions{UserID: bob.ID, Limit: 50})
	if err != nil {
		t.Fatal(err)
	}
	if len(bobEvents) != 0 {
		t.Fatalf("bob saw private events: %#v", bobEvents)
	}
	filtered, _, err := app.listEvents(context.Background(), listOptions{UserID: alice.ID, Source: "files", ObjectType: "file", ObjectID: "/report.txt"})
	if err != nil || len(filtered) != 1 {
		t.Fatalf("object filter = %#v, %v", filtered, err)
	}
}

func TestCursorIsStableNewestFirstAndRetainsOldEvents(t *testing.T) {
	app, _, _, alice, _ := openActivityTestApp(t)
	for _, event := range []apps.ActivityEvent{
		{ID: "a", ActorName: "Dar", ActorKind: "system", Source: "system", Action: "old", Summary: "old", Origin: "worker", HappenedAt: 1, RecipientIDs: []string{alice.ID}},
		{ID: "b", ActorName: "Alice", ActorKind: "user", ActorID: alice.ID, Source: "files", Action: "new", Summary: "new b", Origin: "api", HappenedAt: 20, RecipientIDs: []string{alice.ID}},
		{ID: "c", ActorName: "Alice", ActorKind: "user", ActorID: alice.ID, Source: "files", Action: "new", Summary: "new c", Origin: "api", HappenedAt: 20, RecipientIDs: []string{alice.ID}},
	} {
		if err := app.persistEvent(context.Background(), event); err != nil {
			t.Fatal(err)
		}
	}
	first, next, err := app.listEvents(context.Background(), listOptions{UserID: alice.ID, Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(first) != 2 || first[0].ID != "c" || first[1].ID != "b" || next == "" {
		t.Fatalf("first page = %#v next=%q", first, next)
	}
	cur, err := decodeCursor(next)
	if err != nil {
		t.Fatal(err)
	}
	second, next, err := app.listEvents(context.Background(), listOptions{UserID: alice.ID, Limit: 2, Cursor: cur})
	if err != nil || len(second) != 1 || second[0].ID != "a" || next != "" {
		t.Fatalf("second page = %#v next=%q err=%v", second, next, err)
	}
}

func TestWorkerWakesAfterEnqueue(t *testing.T) {
	app, registry, _, alice, _ := openActivityTestApp(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := app.Start(ctx); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		shutdown, stop := context.WithTimeout(context.Background(), time.Second)
		defer stop()
		_ = app.Shutdown(shutdown)
	})
	if err := registry.EnqueueActivity(context.Background(), apps.ActivityEvent{
		ID: "wake", ActorName: "Alice", ActorKind: "user", Source: "files",
		Action: "created", Summary: "created a folder", Origin: "api", RecipientIDs: []string{alice.ID},
	}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		events, _, err := app.listEvents(context.Background(), listOptions{UserID: alice.ID})
		if err == nil && len(events) == 1 {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("worker did not drain the outbox after wake-up")
}

func TestFailedOutboxPayloadIsRetainedForRetry(t *testing.T) {
	app, _, database, _, _ := openActivityTestApp(t)
	if err := app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `INSERT INTO activity_outbox(id,payload_json,next_attempt_at,created_at) VALUES('broken','{',0,1)`, nil)
	}); err != nil {
		t.Fatal(err)
	}
	app.drainOutbox(context.Background())
	var attempts, next int64
	var lastError string
	if err := app.withConn(context.Background(), func(conn *sqlite.Conn) error {
		return sqlitex.Execute(conn, `SELECT attempt_count,next_attempt_at,last_error FROM activity_outbox WHERE id='broken'`, &sqlitex.ExecOptions{ResultFunc: func(stmt *sqlite.Stmt) error {
			attempts, next, lastError = stmt.ColumnInt64(0), stmt.ColumnInt64(1), stmt.ColumnText(2)
			return nil
		}})
	}); err != nil {
		t.Fatal(err)
	}
	if attempts != 1 || next <= time.Now().Unix() || lastError == "" || !tableExists(t, database, "activity_outbox") {
		t.Fatalf("retry state attempts=%d next=%d error=%q", attempts, next, lastError)
	}
}

func TestSettingsRSSRotationFilteringAndDisable(t *testing.T) {
	app, _, _, alice, _ := openActivityTestApp(t)
	for _, event := range []apps.ActivityEvent{
		{ID: "rss-files", ActorName: "Alice", ActorKind: "user", ActorID: alice.ID, Source: "files", Action: "upload", Summary: "uploaded <report> & notes", Origin: "api", HappenedAt: 10, RecipientIDs: []string{alice.ID}},
		{ID: "rss-system", ActorName: "Dar", ActorKind: "system", Source: "system", Action: "sync", Summary: "system-only", Origin: "worker", HappenedAt: 9, RecipientIDs: []string{alice.ID}},
	} {
		if err := app.persistEvent(context.Background(), event); err != nil {
			t.Fatal(err)
		}
	}

	get := httptest.NewRecorder()
	app.handleSettings(get, requestAs(httptest.NewRequest(http.MethodGet, "/api/activity/settings", nil), alice))
	if get.Code != http.StatusOK || !strings.Contains(get.Body.String(), `"files"`) || !strings.Contains(get.Body.String(), `"system"`) {
		t.Fatalf("default settings: %d %s", get.Code, get.Body.String())
	}
	bad := httptest.NewRecorder()
	app.handleSettings(bad, requestAs(httptest.NewRequest(http.MethodPut, "/api/activity/settings", strings.NewReader(`{"rss_enabled":true,"sources":["files"],"extra":1}`)), alice))
	if bad.Code != http.StatusBadRequest {
		t.Fatalf("unknown settings field status = %d", bad.Code)
	}

	enable := httptest.NewRecorder()
	app.handleSettings(enable, requestAs(httptest.NewRequest(http.MethodPut, "/api/activity/settings", strings.NewReader(`{"rss_enabled":true,"sources":["files"]}`)), alice))
	if enable.Code != http.StatusOK {
		t.Fatalf("enable: %d %s", enable.Code, enable.Body.String())
	}
	var enabled map[string]any
	if err := json.Unmarshal(enable.Body.Bytes(), &enabled); err != nil {
		t.Fatal(err)
	}
	firstURL, _ := enabled["feed_url"].(string)
	if !strings.HasPrefix(firstURL, "https://dar.example/activity/rss/") {
		t.Fatalf("feed URL = %q", firstURL)
	}
	token := strings.TrimPrefix(firstURL, "https://dar.example/activity/rss/")
	settings, _, err := app.loadFeedSettings(context.Background(), alice.ID)
	if err != nil || settings.TokenHash == token || settings.TokenHash != tokenHash(token) {
		t.Fatalf("token was not hash-only: %#v %v", settings, err)
	}

	feed := serveRSS(app, token)
	if feed.Code != http.StatusOK || !strings.Contains(feed.Body.String(), "rss-files") || strings.Contains(feed.Body.String(), "system-only") {
		t.Fatalf("filtered RSS: %d %s", feed.Code, feed.Body.String())
	}
	if !bytes.Contains(feed.Body.Bytes(), []byte("&lt;report&gt; &amp; notes")) {
		t.Fatalf("RSS summary was not escaped: %s", feed.Body.String())
	}

	rotate := httptest.NewRecorder()
	app.handleRegenerate(rotate, requestAs(httptest.NewRequest(http.MethodPost, "/api/activity/rss/regenerate", nil), alice))
	if rotate.Code != http.StatusOK {
		t.Fatalf("rotate: %d %s", rotate.Code, rotate.Body.String())
	}
	if serveRSS(app, token).Code != http.StatusNotFound {
		t.Fatal("old RSS token still works after rotation")
	}
	var rotated map[string]any
	_ = json.Unmarshal(rotate.Body.Bytes(), &rotated)
	newToken := strings.TrimPrefix(rotated["feed_url"].(string), "https://dar.example/activity/rss/")
	if serveRSS(app, newToken).Code != http.StatusOK {
		t.Fatal("new RSS token does not work")
	}

	disable := httptest.NewRecorder()
	app.handleSettings(disable, requestAs(httptest.NewRequest(http.MethodPut, "/api/activity/settings", strings.NewReader(`{"rss_enabled":false,"sources":["files"]}`)), alice))
	if disable.Code != http.StatusOK || serveRSS(app, newToken).Code != http.StatusNotFound {
		t.Fatal("disabling did not revoke RSS token")
	}
}

func TestListRejectsUnknownQueryAndInvalidCursor(t *testing.T) {
	app, _, _, alice, _ := openActivityTestApp(t)
	for _, target := range []string{
		"/api/activity?unexpected=true",
		"/api/activity?cursor=not-a-cursor",
		"/api/activity?limit=101",
		"/api/activity?source=unknown",
	} {
		response := httptest.NewRecorder()
		app.handleList(response, requestAs(httptest.NewRequest(http.MethodGet, target, nil), alice))
		if response.Code != http.StatusBadRequest {
			t.Fatalf("%s returned %d: %s", target, response.Code, response.Body.String())
		}
	}
}

func serveRSS(app *App, token string) *httptest.ResponseRecorder {
	router := chi.NewRouter()
	router.Get("/activity/rss/{token}", app.handleRSS)
	response := httptest.NewRecorder()
	router.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/activity/rss/"+token, nil))
	return response
}

func TestActivityAssetsAndPageReference(t *testing.T) {
	app, _, _, alice, _ := openActivityTestApp(t)
	anonymous := httptest.NewRecorder()
	app.handlePage(anonymous, httptest.NewRequest(http.MethodGet, "/activity", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", anonymous.Code)
	}
	asset := httptest.NewRecorder()
	app.handleAsset(asset, httptest.NewRequest(http.MethodGet, "/activity/assets/app.js", nil))
	if asset.Code != http.StatusOK || !strings.Contains(asset.Body.String(), "/api/activity") {
		t.Fatalf("asset: %d %s", asset.Code, asset.Body.String())
	}
	page := httptest.NewRecorder()
	app.handlePage(page, requestAs(httptest.NewRequest(http.MethodGet, "/activity", nil), alice))
	if page.Code != http.StatusOK || !strings.Contains(page.Body.String(), "/activity/assets/app.js") || !strings.Contains(page.Body.String(), "Activity settings") {
		t.Fatalf("page: %d %s", page.Code, page.Body.String())
	}
}

func TestActivityStrictUnauthorizedAndClosedDatabaseMatrix(t *testing.T) {
	app, _, database, alice, _ := openActivityTestApp(t)
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleList, http.MethodGet, "/api/activity", ""},
		{app.handleSettings, http.MethodGet, "/api/activity/settings", ""},
		{app.handleRegenerate, http.MethodPost, "/api/activity/settings/regenerate", ""},
	} {
		rec := httptest.NewRecorder()
		tc.handler(rec, httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)))
		if rec.Code != http.StatusUnauthorized {
			t.Errorf("anonymous %s=%d", tc.target, rec.Code)
		}
	}
	for _, body := range []string{
		`{"rss_enabled":true,"sources":["system"]} {}`,
		`{"rss_enabled":true,"sources":["missing"]}`,
		`{"rss_enabled":true,"sources":[]}`,
		`{"rss_enabled":true,"sources":["system"],"unknown":true}`,
	} {
		rec := httptest.NewRecorder()
		app.handleSettings(rec, requestAs(httptest.NewRequest(http.MethodPut, "/api/activity/settings", strings.NewReader(body)), alice))
		if rec.Code != http.StatusBadRequest {
			t.Errorf("strict settings body=%q status=%d", body, rec.Code)
		}
	}
	app.recordSystem(alice.ID, "", "tested", "tested activity recording")
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleList, http.MethodGet, "/api/activity", ""},
		{app.handleSettings, http.MethodGet, "/api/activity/settings", ""},
		{app.handleSettings, http.MethodPut, "/api/activity/settings", `{"rss_enabled":true,"sources":["system"]}`},
		{app.handleRegenerate, http.MethodPost, "/api/activity/settings/regenerate", ""},
	} {
		rec := httptest.NewRecorder()
		tc.handler(rec, requestAs(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), alice))
		if rec.Code < 400 {
			t.Errorf("closed DB %s=%d %s", tc.target, rec.Code, rec.Body.String())
		}
	}
	rssRequest := httptest.NewRequest(http.MethodGet, "/activity/rss/token", nil)
	route := chi.NewRouteContext()
	route.URLParams.Add("token", "token")
	rssRequest = rssRequest.WithContext(context.WithValue(rssRequest.Context(), chi.RouteCtxKey, route))
	rec := httptest.NewRecorder()
	app.handleRSS(rec, rssRequest)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("closed RSS=%d", rec.Code)
	}
}
