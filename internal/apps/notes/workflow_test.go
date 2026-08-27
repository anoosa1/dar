package notes

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/anoosa/dar/internal/apps"
)

type collectingSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *collectingSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func callNotesHandler(t *testing.T, handler http.HandlerFunc, method, target, body string) *httptest.ResponseRecorder {
	t.Helper()
	rec := httptest.NewRecorder()
	handler(rec, noteRequest(method, target, body, "alice"))
	return rec
}

func TestCategoryVersionUndoAndSearchWorkflow(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()

	rec := callNotesHandler(t, app.handleCategories, http.MethodPost, "/api/notes/categories", `{"name":"Projects"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("create category: %d %s", rec.Code, rec.Body.String())
	}

	note, err := app.create(ctx, "alice", notePatch{
		Title:    str("Plan"),
		Content:  str("original searchable content"),
		Category: str("Projects"),
	})
	if err != nil {
		t.Fatal(err)
	}
	updated, err := app.update(ctx, "alice", note.ID, notePatch{Content: str("revised searchable content")}, note.ETag, false)
	if err != nil {
		t.Fatal(err)
	}

	rec = callNotesHandler(t, app.handleCategories, http.MethodGet, "/api/notes/categories", "")
	if rec.Code != http.StatusOK {
		t.Fatalf("list categories: %d %s", rec.Code, rec.Body.String())
	}
	var listed struct {
		Categories []string `json:"categories"`
	}
	if err = json.Unmarshal(rec.Body.Bytes(), &listed); err != nil {
		t.Fatal(err)
	}
	if len(listed.Categories) != 1 || listed.Categories[0] != "Projects" {
		t.Fatalf("categories = %#v", listed.Categories)
	}

	rec = callNotesHandler(t, app.handleCategories, http.MethodPut, "/api/notes/categories", `{"from":"Projects","to":"Archive"}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("rename category: %d %s", rec.Code, rec.Body.String())
	}
	renamed, err := app.note(ctx, "alice", note.ID)
	if err != nil {
		t.Fatal(err)
	}
	if renamed.Category != "Archive" {
		t.Fatalf("renamed note category = %q", renamed.Category)
	}

	rec = callNotesHandler(t, app.handleVersions, http.MethodGet, "/api/notes/versions/1?id="+fmt.Sprint(note.ID), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("list versions: %d %s", rec.Code, rec.Body.String())
	}
	var versions struct {
		Versions []string `json:"versions"`
	}
	if err = json.Unmarshal(rec.Body.Bytes(), &versions); err != nil {
		t.Fatal(err)
	}
	if len(versions.Versions) == 0 {
		t.Fatal("update did not retain a restorable version")
	}
	rec = callNotesHandler(t, app.handleVersions, http.MethodPost, "/api/notes/versions/1?id="+fmt.Sprint(note.ID)+"&version="+versions.Versions[0], "")
	if rec.Code != http.StatusOK {
		t.Fatalf("restore version: %d %s", rec.Code, rec.Body.String())
	}
	var restored Note
	if err = json.Unmarshal(rec.Body.Bytes(), &restored); err != nil {
		t.Fatal(err)
	}
	if restored.Content != "original searchable content" {
		t.Fatalf("restored content = %q, previous update was %#v", restored.Content, updated)
	}

	rec = callNotesHandler(t, app.handleCategories, http.MethodDelete, "/api/notes/categories?name=Archive", "")
	if rec.Code != http.StatusConflict {
		t.Fatalf("delete non-empty category: %d %s", rec.Code, rec.Body.String())
	}

	if err = app.delete(ctx, "alice", note.ID); err != nil {
		t.Fatal(err)
	}
	rec = callNotesHandler(t, app.handleUndo, http.MethodPost, "/api/notes/undo/1?id="+fmt.Sprint(note.ID), "")
	if rec.Code != http.StatusOK {
		t.Fatalf("undo delete: %d %s", rec.Code, rec.Body.String())
	}
	if _, err = app.note(ctx, "bob", note.ID); err == nil {
		t.Fatal("restored note escaped its user scope")
	}

	sink := &collectingSearchSink{}
	if err = app.IndexSearch(ctx, "alice", sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) == 0 || sink.docs[0].ObjectID != fmt.Sprint(note.ID) {
		t.Fatalf("search documents = %#v", sink.docs)
	}
	sink.err = context.Canceled
	if err = app.IndexSearch(ctx, "alice", sink); err != context.Canceled {
		t.Fatalf("sink failure = %v", err)
	}

	rec = callNotesHandler(t, app.handleCategories, http.MethodDelete, "/api/notes/categories?name=Archive&force=1", "")
	if rec.Code != http.StatusOK {
		t.Fatalf("force delete category: %d %s", rec.Code, rec.Body.String())
	}
	if _, err = app.note(ctx, "alice", note.ID); err == nil {
		t.Fatal("force category deletion left the note live")
	}
}

func TestCategoryVersionUndoValidation(t *testing.T) {
	app, _ := testNotesApp(t)
	note, err := app.create(context.Background(), "alice", notePatch{Title: str("Validation")})
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    string
		status  int
	}{
		{"category bad json", app.handleCategories, http.MethodPost, "/api/notes/categories", `{`, 400},
		{"category traversal", app.handleCategories, http.MethodPost, "/api/notes/categories", `{"name":"../escape"}`, 400},
		{"category rename bad json", app.handleCategories, http.MethodPut, "/api/notes/categories", `{`, 400},
		{"category unsupported method", app.handleCategories, http.MethodPatch, "/api/notes/categories", ``, 405},
		{"undo invalid id", app.handleUndo, http.MethodPost, "/api/notes/undo/nope", ``, 400},
		{"undo missing note", app.handleUndo, http.MethodPost, "/api/notes/undo/1?id=999", ``, 404},
		{"versions invalid id", app.handleVersions, http.MethodGet, "/api/notes/versions/nope", ``, 400},
		{"versions requires version", app.handleVersions, http.MethodPost, "/api/notes/versions/1?id=" + fmt.Sprint(note.ID), ``, 400},
		{"versions unsupported method", app.handleVersions, http.MethodPatch, "/api/notes/versions/1?id=" + fmt.Sprint(note.ID), ``, 405},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			rec := callNotesHandler(t, tc.handler, tc.method, tc.target, tc.body)
			if rec.Code != tc.status {
				t.Fatalf("status = %d, want %d; body=%s", rec.Code, tc.status, rec.Body.String())
			}
		})
	}

	if id, err := routeIDFromString("42"); err != nil || id != 42 {
		t.Fatalf("route id = %d, %v", id, err)
	}
	if _, err := routeIDFromString("zero"); err == nil {
		t.Fatal("invalid route ID accepted")
	}
	if got := app.userID(noteRequest(http.MethodGet, "/", "", "alice")); got != "alice" {
		t.Fatalf("user ID = %q", got)
	}
	if got := app.userID(httptest.NewRequest(http.MethodGet, "/", nil)); got != "" {
		t.Fatalf("anonymous user ID = %q", got)
	}

	rec := httptest.NewRecorder()
	app.handlePage(rec, httptest.NewRequest(http.MethodGet, "/notes", nil))
	if rec.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page = %d", rec.Code)
	}
}
