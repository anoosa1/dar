package search

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"zombiezen.com/go/sqlite"
)

type testProvider struct {
	name       string
	order      int
	docs       map[string][]apps.SearchDocument
	err        error
	pauseAfter int
	staged     chan struct{}
	resume     <-chan struct{}
}

func (p *testProvider) Name() string        { return p.name }
func (p *testProvider) DisplayName() string { return strings.ToUpper(p.name) }
func (p *testProvider) Icon() string        { return `<i class="fa-solid fa-box"></i>` }
func (p *testProvider) Description() string { return "" }
func (p *testProvider) AppBarOrder() int    { return p.order }
func (p *testProvider) Routes() []apps.Route {
	return nil
}
func (p *testProvider) IndexSearch(ctx context.Context, userID string, sink apps.SearchDocumentSink) error {
	for i, doc := range p.docs[userID] {
		if err := sink.AddSearchDocument(ctx, doc); err != nil {
			return err
		}
		if p.pauseAfter > 0 && i+1 == p.pauseAfter {
			close(p.staged)
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-p.resume:
			}
		}
	}
	return p.err
}

func testService(t *testing.T, providers ...*testProvider) (*Service, string, string) {
	t.Helper()
	migrations := append([]string{}, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "search.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	first, _, err := auth.FindOrCreateUser(context.Background(), conn, "first", "first@example.test", "First", "", "https://issuer.test", "first")
	if err != nil {
		release()
		t.Fatal(err)
	}
	second, _, err := auth.FindOrCreateUser(context.Background(), conn, "second", "second@example.test", "Second", "", "https://issuer.test", "second")
	release()
	if err != nil {
		t.Fatal(err)
	}
	registry := apps.NewRegistry()
	for _, provider := range providers {
		registry.Register(provider)
	}
	return New(database, registry, time.Hour), first.ID, second.ID
}

func doc(object, chunk, title, body string, modified int64) apps.SearchDocument {
	return apps.SearchDocument{
		ObjectID: object, ChunkID: chunk, Title: title, Body: body,
		URL: "/files", Anchor: "hit-" + chunk, ModifiedAt: modified,
	}
}

func TestQueryIsUserScopedAndMatchesUnicodePrefixes(t *testing.T) {
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}}
	service, first, second := testService(t, provider)
	provider.docs[first] = []apps.SearchDocument{doc("one", "a", "Café résumé", "private alpha", 10)}
	provider.docs[second] = []apps.SearchDocument{doc("two", "a", "Other", "private beta", 20)}
	if err := service.reconcileProvider(context.Background(), first, provider.name, provider); err != nil {
		t.Fatal(err)
	}
	if err := service.reconcileProvider(context.Background(), second, provider.name, provider); err != nil {
		t.Fatal(err)
	}
	got, err := service.Query(context.Background(), first, "cafe res", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Recent) != 1 || got.Recent[0].ObjectID != "one" {
		t.Fatalf("unexpected first-user results: %#v", got)
	}
	leak, err := service.Query(context.Background(), first, "beta", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(leak.Recent) != 0 {
		t.Fatalf("cross-user result leaked: %#v", leak.Recent)
	}
}

func TestReconcileFailureKeepsPreviousIndexAndMarksStale(t *testing.T) {
	provider := &testProvider{name: "notes", docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, provider)
	provider.docs[userID] = []apps.SearchDocument{doc("old", "one", "Durable result", "original", 1)}
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
		t.Fatal(err)
	}
	provider.docs[userID] = []apps.SearchDocument{doc("new", "one", "Replacement", "new body", 2)}
	provider.err = errors.New("provider unavailable")
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err == nil {
		t.Fatal("failed provider reconciliation unexpectedly succeeded")
	}
	got, err := service.Query(context.Background(), userID, "durable", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Recent) != 1 || got.Recent[0].ObjectID != "old" {
		t.Fatalf("old index was not retained: %#v", got)
	}
	if len(got.StaleProviders) != 1 || got.StaleProviders[0] != provider.name {
		t.Fatalf("stale providers = %#v", got.StaleProviders)
	}
}

func TestReconcileBuildsInactiveGenerationWithoutBlockingAccountWrites(t *testing.T) {
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, provider)
	provider.docs[userID] = []apps.SearchDocument{doc("old", "one", "Durable result", "old body", 1)}
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
		t.Fatal(err)
	}

	next := make([]apps.SearchDocument, indexBatchSize*2)
	for i := range next {
		next[i] = doc(fmt.Sprintf("new-%d", i), "one", fmt.Sprintf("Replacement %d", i), "new body", int64(i+2))
	}
	staged := make(chan struct{})
	resume := make(chan struct{})
	provider.docs[userID] = next
	provider.pauseAfter = indexBatchSize
	provider.staged = staged
	provider.resume = resume
	reconciled := make(chan error, 1)
	go func() {
		reconciled <- service.reconcileProvider(context.Background(), userID, provider.name, provider)
	}()
	select {
	case <-staged:
	case <-time.After(5 * time.Second):
		close(resume)
		t.Fatal("new generation was not staged")
	}

	oldResults, err := service.Query(context.Background(), userID, "durable", "results", "", "")
	if err != nil {
		close(resume)
		t.Fatal(err)
	}
	if len(oldResults.Recent) != 1 || oldResults.Recent[0].ObjectID != "old" {
		close(resume)
		t.Fatalf("active generation changed before reconciliation completed: %#v", oldResults.Recent)
	}
	newResults, err := service.Query(context.Background(), userID, "replacement", "results", "", "")
	if err != nil {
		close(resume)
		t.Fatal(err)
	}
	if len(newResults.Recent) != 0 {
		close(resume)
		t.Fatalf("inactive generation leaked into results: %#v", newResults.Recent)
	}

	conn, release, err := service.db.Conn(context.Background())
	if err != nil {
		close(resume)
		t.Fatal(err)
	}
	started := time.Now()
	_, _, err = auth.FindOrCreateUser(context.Background(), conn,
		"first", "first@example.test", "First", "", "https://issuer.test", "first")
	release()
	if err != nil {
		close(resume)
		t.Fatalf("account write failed during generation build: %v", err)
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		close(resume)
		t.Fatalf("account write waited %v during generation build", elapsed)
	}

	close(resume)
	if err := <-reconciled; err != nil {
		t.Fatal(err)
	}
	oldResults, err = service.Query(context.Background(), userID, "durable", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(oldResults.Recent) != 0 {
		t.Fatalf("old generation remained visible: %#v", oldResults.Recent)
	}
	newResults, err = service.Query(context.Background(), userID, "replacement", "results", "", "")
	if err != nil || len(newResults.Recent) == 0 {
		t.Fatalf("replacement generation was not activated: %#v, %v", newResults.Recent, err)
	}
}

func TestRecentCollapsesChunksAndPreferencesOrderGroups(t *testing.T) {
	files := &testProvider{name: "files", order: 1, docs: map[string][]apps.SearchDocument{}}
	notes := &testProvider{name: "notes", order: 2, docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, files, notes)
	files.docs[userID] = []apps.SearchDocument{
		doc("shared", "one", "Project Alpha", "alpha first", 30),
		doc("shared", "two", "Project Alpha", "alpha second", 30),
	}
	notes.docs[userID] = []apps.SearchDocument{doc("note", "one", "Alpha note", "alpha", 20)}
	for _, provider := range []*testProvider{files, notes} {
		if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
			t.Fatal(err)
		}
	}
	got, err := service.Query(context.Background(), userID, "alpha", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Recent) != 2 || got.Recent[0].ObjectID != "shared" || got.Recent[0].MatchCount != 2 {
		t.Fatalf("recent results were not collapsed: %#v", got.Recent)
	}
	if len(got.Groups) != 2 || len(got.Groups[0].Results) != 1 || got.Groups[0].Results[0].ObjectID != "shared" {
		t.Fatalf("recent result was not repeated in its provider group: %#v", got.Groups)
	}
	saved, err := service.SavePreferences(context.Background(), userID, []Provider{
		{Name: "notes", Enabled: true},
		{Name: "files", Enabled: false},
	})
	if err != nil {
		t.Fatal(err)
	}
	if saved.Providers[0].Name != "notes" || saved.Providers[1].Enabled {
		t.Fatalf("preferences not saved: %#v", saved)
	}
	got, err = service.Query(context.Background(), userID, "alpha", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Recent) != 1 || got.Recent[0].Provider != "notes" {
		t.Fatalf("disabled provider remained visible: %#v", got)
	}
	if len(got.Groups) != 1 || got.Groups[0].Provider != "notes" {
		t.Fatalf("provider order/groups = %#v", got.Groups)
	}
}

func TestChunkTextOverlapsLongContent(t *testing.T) {
	text := strings.Repeat("paragraph words ", 1800)
	chunks := ChunkText(text)
	if len(chunks) < 2 {
		t.Fatalf("got %d chunks", len(chunks))
	}
	if len(chunks[0]) > (16<<10)+10 {
		t.Fatalf("first chunk too large: %d", len(chunks[0]))
	}
	tail := chunks[0][max(0, len(chunks[0])-(1<<10)):]
	if !strings.Contains(chunks[1], strings.TrimSpace(tail[:min(100, len(tail))])) {
		t.Fatal("second chunk does not overlap first")
	}
}

func TestScopedWriterAtomicallyReplacesAndDeletesParent(t *testing.T) {
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}}
	service, userID, otherID := testService(t, provider)
	writer := service.Writer("files")
	if err := writer.ReplaceSearchObject(context.Background(), userID, "object", []apps.SearchDocument{
		doc("object", "one", "Original", "first body", 1),
		doc("object", "two", "Original", "second body", 1),
	}); err != nil {
		t.Fatal(err)
	}
	if err := writer.ReplaceSearchObject(context.Background(), userID, "object", []apps.SearchDocument{
		doc("object", "replacement", "Replacement", "new body", 2),
	}); err != nil {
		t.Fatal(err)
	}
	got, err := service.Query(context.Background(), userID, "original", "results", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if len(got.Recent) != 0 {
		t.Fatalf("old chunks survived replacement: %#v", got.Recent)
	}
	got, err = service.Query(context.Background(), userID, "replacement", "results", "", "")
	if err != nil || len(got.Recent) != 1 {
		t.Fatalf("replacement missing: %#v, %v", got, err)
	}
	other, err := service.Query(context.Background(), otherID, "replacement", "results", "", "")
	if err != nil || len(other.Recent) != 0 {
		t.Fatalf("writer escaped user scope: %#v, %v", other, err)
	}
	if err := writer.DeleteSearchObject(context.Background(), userID, "object"); err != nil {
		t.Fatal(err)
	}
	got, err = service.Query(context.Background(), userID, "replacement", "results", "", "")
	if err != nil || len(got.Recent) != 0 {
		t.Fatalf("delete failed: %#v, %v", got, err)
	}
}

func TestSearchHTTPValidationAndJSONShape(t *testing.T) {
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, provider)
	provider.docs[userID] = []apps.SearchDocument{doc("one", "one", "Needle", "body", 1)}
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
		t.Fatal(err)
	}
	request := httptest.NewRequest(http.MethodPost, "/api/search/query", bytes.NewBufferString(`{"query":"needle","view":"results"}`))
	request = request.WithContext(context.WithValue(request.Context(), server.UserContextKey, &auth.User{ID: userID}))
	response := httptest.NewRecorder()
	service.HandleQuery(response, request)
	if response.Code != http.StatusOK {
		t.Fatalf("query status=%d body=%s", response.Code, response.Body.String())
	}
	var payload map[string]any
	if err := json.Unmarshal(response.Body.Bytes(), &payload); err != nil {
		t.Fatal(err)
	}
	if payload["recent"] == nil || payload["groups"] == nil {
		t.Fatalf("query response has null collections: %s", response.Body.String())
	}

	bad := httptest.NewRequest(http.MethodPost, "/api/search/query", bytes.NewBufferString(`{"query":"x","view":"results","unexpected":true}`))
	bad = bad.WithContext(context.WithValue(bad.Context(), server.UserContextKey, &auth.User{ID: userID}))
	badResponse := httptest.NewRecorder()
	service.HandleQuery(badResponse, bad)
	if badResponse.Code != http.StatusBadRequest {
		t.Fatalf("unknown field status=%d", badResponse.Code)
	}

	if err := service.Writer("files").ReplaceSearchObject(context.Background(), userID, "external", []apps.SearchDocument{
		{ObjectID: "external", ChunkID: "one", Title: "Bad", URL: "https://example.test"},
	}); err == nil {
		t.Fatal("external result URL was accepted")
	}
}

func TestMatchesExcludeDisabledProvider(t *testing.T) {
	provider := &testProvider{name: "notes", docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, provider)
	provider.docs[userID] = []apps.SearchDocument{doc("one", "one", "Needle", "needle body", 1)}
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
		t.Fatal(err)
	}
	if _, err := service.SavePreferences(context.Background(), userID, []Provider{{Name: "notes", Enabled: false}}); err != nil {
		t.Fatal(err)
	}
	matches, err := service.Matches(context.Background(), userID, "notes", "one", "needle")
	if err != nil {
		t.Fatal(err)
	}
	if len(matches) != 0 {
		t.Fatalf("disabled provider returned matches: %#v", matches)
	}
}

func TestLifecycleHTTPAndUtilityRegressionMatrix(t *testing.T) {
	staged := make(chan struct{})
	resume := make(chan struct{})
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}, pauseAfter: 1, staged: staged, resume: resume}
	service, first, _ := testService(t, provider)
	provider.docs[first] = []apps.SearchDocument{doc("one", "chunk", "Lifecycle", "searchable body", 1)}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if err := service.Start(ctx); err != nil {
		t.Fatal(err)
	}
	select {
	case <-staged:
		close(resume)
	case <-time.After(5 * time.Second):
		t.Fatal("search lifecycle did not begin reconciliation")
	}
	service.Writer("files").MarkSearchDirty("missing-provider")
	if err := service.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := New(nil, nil, 0).Start(context.Background()); err == nil {
		t.Fatal("search started without dependencies")
	}

	request := func(handler http.HandlerFunc, method, body string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(method, "/", strings.NewReader(body))
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: first}))
		recorder := httptest.NewRecorder()
		handler(recorder, req)
		return recorder
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		method  string
		body    string
		want    int
	}{
		{service.HandleQuery, http.MethodGet, "", http.StatusMethodNotAllowed},
		{service.HandleQuery, http.MethodPost, `{`, http.StatusBadRequest},
		{service.HandleQuery, http.MethodPost, `{"query":"lifecycle","view":"results"}`, http.StatusOK},
		{service.HandleQuery, http.MethodPost, `{"query":"` + strings.Repeat("x", 501) + `"}`, http.StatusBadRequest},
		{service.HandleMatches, http.MethodGet, "", http.StatusMethodNotAllowed},
		{service.HandleMatches, http.MethodPost, `{`, http.StatusBadRequest},
		{service.HandleMatches, http.MethodPost, `{"query":"body","provider":"files","object_id":"one"}`, http.StatusOK},
		{service.HandlePreferences, http.MethodGet, "", http.StatusOK},
		{service.HandlePreferences, http.MethodPut, `{"providers":[{"name":"files","enabled":true}]}`, http.StatusOK},
		{service.HandlePreferences, http.MethodPut, `{"providers":[{"name":"unknown","enabled":true}]}`, http.StatusBadRequest},
		{service.HandlePreferences, http.MethodDelete, "", http.StatusMethodNotAllowed},
	} {
		recorder := request(tc.handler, tc.method, tc.body)
		if recorder.Code != tc.want {
			t.Errorf("%s body=%q status=%d want=%d response=%s", tc.method, tc.body, recorder.Code, tc.want, recorder.Body.String())
		}
	}
	providers := []Provider{{Name: "z"}, {Name: "a"}, {Name: "m"}}
	SortProviders(providers)
	if providers[0].Name != "a" || providers[2].Name != "z" {
		t.Fatalf("sorted providers = %+v", providers)
	}
	if decodeCursor("-1") != 0 || decodeCursor("bad") != 0 || truncate("abcdef", 3) != "abc" {
		t.Fatal("cursor or truncation boundary regression")
	}
}

func TestWriterLifecycleAndClosedDatabaseFailureMatrix(t *testing.T) {
	service, first, _ := testService(t, &testProvider{name: "docs", docs: map[string][]apps.SearchDocument{}})
	writer := service.Writer("docs")
	valid := apps.SearchDocument{ChunkID: "one", Title: "Document", URL: "/docs"}
	if err := writer.ReplaceSearchObject(context.Background(), "", "object", []apps.SearchDocument{valid}); err == nil {
		t.Fatal("empty search user accepted")
	}
	if err := writer.ReplaceSearchObject(context.Background(), first, "", []apps.SearchDocument{valid}); err == nil {
		t.Fatal("empty search object accepted")
	}
	mismatch := valid
	mismatch.ObjectID = "different"
	if err := writer.ReplaceSearchObject(context.Background(), first, "object", []apps.SearchDocument{mismatch}); err == nil {
		t.Fatal("mismatched search object accepted")
	}
	invalid := valid
	invalid.Title = ""
	if err := writer.ReplaceSearchObject(context.Background(), first, "object", []apps.SearchDocument{invalid}); err == nil {
		t.Fatal("invalid search document accepted")
	}
	if err := writer.ReplaceSearchObject(context.Background(), first, "object", []apps.SearchDocument{valid}); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < cap(service.dirty)+2; i++ {
		writer.MarkSearchDirty("")
	}
	sink := &generationSink{service: service}
	if err := sink.flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	canceled, cancel := context.WithCancel(context.Background())
	cancel()
	if err := service.cleanupInactiveGenerations(canceled); !errors.Is(err, context.Canceled) {
		t.Fatalf("cleanup cancellation=%v", err)
	}

	service.wg.Add(1)
	shutdownCtx, shutdownCancel := context.WithCancel(context.Background())
	shutdownCancel()
	if err := service.Shutdown(shutdownCtx); !errors.Is(err, context.Canceled) {
		t.Fatalf("shutdown cancellation=%v", err)
	}
	service.wg.Done()

	if err := service.db.Close(); err != nil {
		t.Fatal(err)
	}
	for name, call := range map[string]func() error{
		"replace": func() error {
			return writer.ReplaceSearchObject(context.Background(), first, "closed", []apps.SearchDocument{valid})
		},
		"delete": func() error { return writer.DeleteSearchObject(context.Background(), first, "object") },
		"preferences": func() error {
			_, err := service.Preferences(context.Background(), first)
			return err
		},
		"query": func() error {
			_, err := service.Query(context.Background(), first, "doc", "results", "", "")
			return err
		},
		"matches": func() error {
			_, err := service.Matches(context.Background(), first, "docs", "object", "doc")
			return err
		},
	} {
		if err := call(); err == nil {
			t.Errorf("%s unexpectedly succeeded with closed database", name)
		}
	}
}

func TestSearchBatchPreferencePaginationAndErrorBranchMatrix(t *testing.T) {
	provider := &testProvider{name: "files", docs: map[string][]apps.SearchDocument{}}
	service, userID, _ := testService(t, provider)
	for i := 0; i < pageLimit+3; i++ {
		provider.docs[userID] = append(provider.docs[userID], doc(
			fmt.Sprintf("object-%02d", i), "body", fmt.Sprintf("Document %02d", i), "pagination needle", int64(i+1),
		))
	}
	sink := &generationSink{service: service, generation: "manual", userID: userID, provider: provider.name}
	if err := sink.AddSearchDocument(context.Background(), apps.SearchDocument{}); err == nil {
		t.Fatal("generation sink accepted invalid document")
	}
	for i := 0; i < indexBatchSize; i++ {
		item := doc(fmt.Sprintf("batch-%02d", i), "one", "Batch", "body", int64(i))
		if err := sink.AddSearchDocument(context.Background(), item); err != nil {
			t.Fatal(err)
		}
	}
	if len(sink.pending) != 0 {
		t.Fatalf("batch did not auto-flush: %d pending", len(sink.pending))
	}
	if err := sink.flush(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := service.reconcileProvider(context.Background(), userID, provider.name, provider); err != nil {
		t.Fatal(err)
	}

	results, err := service.Query(context.Background(), userID, "needle", "results", "", "")
	if err != nil || len(results.Groups) != 1 || results.Groups[0].NextCursor == "" || len(results.Groups[0].Results) != pageLimit {
		t.Fatalf("first page=%+v err=%v", results, err)
	}
	next, err := service.Query(context.Background(), userID, "needle", "results", "files", results.Groups[0].NextCursor)
	if err != nil || len(next.Groups) != 1 || len(next.Groups[0].Results) != 3 {
		t.Fatalf("second page=%+v err=%v", next, err)
	}
	suggestions, err := service.Query(context.Background(), userID, "needle", "suggestions", "", "")
	if err != nil || len(suggestions.Groups) != 1 || len(suggestions.Groups[0].Results) != suggestLimit {
		t.Fatalf("suggestions=%+v err=%v", suggestions, err)
	}
	if _, err := service.Query(context.Background(), userID, "", "invalid", "", ""); err == nil {
		t.Fatal("invalid search view accepted")
	}
	if matches, err := service.Matches(context.Background(), userID, "files", "object-00", " "); err != nil || len(matches) != 0 {
		t.Fatalf("blank matches=%+v err=%v", matches, err)
	}
	excluded := map[string]bool{"files\x00object-00": true}
	if rows, err := service.queryResults(context.Background(), userID, "files", ftsQuery("needle"), 50, 0, false, excluded, nil); err != nil {
		t.Fatal(err)
	} else {
		for _, row := range rows {
			if row.ObjectID == "object-00" {
				t.Fatal("excluded result was returned")
			}
		}
	}

	if err := service.withConn(context.Background(), func(conn *sqlite.Conn) error {
		return preferences.SetJSON(conn, userID, "search", "providers", map[string]any{
			"providers": []map[string]any{
				{"name": "unknown", "enabled": false},
				{"name": "files", "enabled": false},
				{"name": "files", "enabled": true},
			},
		})
	}); err != nil {
		t.Fatal(err)
	}
	prefs, err := service.Preferences(context.Background(), userID)
	if err != nil || len(prefs.Providers) != 1 || prefs.Providers[0].Enabled {
		t.Fatalf("normalized preferences=%+v err=%v", prefs, err)
	}
	if got, err := service.Query(context.Background(), userID, "needle", "results", "", ""); err != nil || len(got.Recent) != 0 || len(got.Groups) != 0 {
		t.Fatalf("disabled query=%+v err=%v", got, err)
	}
	if _, err := service.SavePreferences(context.Background(), userID, nil); err == nil {
		t.Fatal("incomplete preferences accepted")
	}
	if _, err := service.SavePreferences(context.Background(), userID, []Provider{{Name: "unknown"}}); err == nil {
		t.Fatal("unknown preferences accepted")
	}

	if err := service.db.Close(); err != nil {
		t.Fatal(err)
	}
	request := func(handler http.HandlerFunc, body string) *httptest.ResponseRecorder {
		req := httptest.NewRequest(http.MethodPost, "/", strings.NewReader(body))
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: userID}))
		rec := httptest.NewRecorder()
		handler(rec, req)
		return rec
	}
	if rec := request(service.HandleQuery, `{"query":"x","view":"results"}`); rec.Code != http.StatusInternalServerError {
		t.Fatalf("closed query status=%d body=%s", rec.Code, rec.Body.String())
	}
	if rec := request(service.HandleMatches, `{"query":"x","provider":"files","object_id":"one"}`); rec.Code != http.StatusInternalServerError {
		t.Fatalf("closed matches status=%d body=%s", rec.Code, rec.Body.String())
	}
}
