package notes

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/server"
)

func noteRequest(method, target, body, uid string) *http.Request {
	r := httptest.NewRequest(method, target, strings.NewReader(body))
	if body != "" {
		r.Header.Set("Content-Type", "application/json")
	}
	return r.WithContext(context.WithValue(r.Context(), server.UserContextKey, &auth.User{ID: uid}))
}

func TestV1ContractHeadersConditionalAndChunking(t *testing.T) {
	app, _ := testNotesApp(t)
	for i := 0; i < 3; i++ {
		req := noteRequest(http.MethodPost, "/index.php/apps/notes/api/v1/notes", fmt.Sprintf(`{"title":"N%d","content":"body"}`, i), "alice")
		rec := httptest.NewRecorder()
		app.handleNotesCollection(rec, req)
		if rec.Code != 200 {
			t.Fatalf("create %d: %d %s", i, rec.Code, rec.Body.String())
		}
		if rec.Header().Get("X-Notes-API-Versions") != "1.4" {
			t.Fatal("missing API versions header")
		}
	}
	req := noteRequest(http.MethodGet, "/index.php/apps/notes/api/v1/notes?chunkSize=2&exclude=content", "", "alice")
	rec := httptest.NewRecorder()
	app.handleNotesCollection(rec, req)
	if rec.Code != 200 {
		t.Fatalf("list: %d %s", rec.Code, rec.Body.String())
	}
	if rec.Header().Get("X-Notes-Chunk-Cursor") == "" || rec.Header().Get("X-Notes-Chunk-Pending") != "1" {
		t.Fatalf("chunk headers = %#v", rec.Header())
	}
	var out []map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &out); err != nil {
		t.Fatal(err)
	}
	if len(out) != 2 {
		t.Fatalf("chunk length = %d", len(out))
	}
	if _, ok := out[0]["content"]; ok {
		t.Fatal("excluded content was returned")
	}
	tag := rec.Header().Get("ETag")
	req = noteRequest(http.MethodGet, "/index.php/apps/notes/api/v1/notes", "", "alice")
	req.Header.Set("If-None-Match", tag)
	rec = httptest.NewRecorder()
	app.handleNotesCollection(rec, req)
	if rec.Code != http.StatusNotModified {
		t.Fatalf("conditional status = %d", rec.Code)
	}
}

func TestV1EmptyPatchAndChunkCursorBoundaries(t *testing.T) {
	patch, err := decodePatch(httptest.NewRequest(http.MethodPost, "/", http.NoBody))
	if err != nil || patch != (notePatch{}) {
		t.Fatalf("empty patch=%#v err=%v", patch, err)
	}

	app, _ := testNotesApp(t)
	for i := 0; i < 2; i++ {
		rec := httptest.NewRecorder()
		app.handleNotesCollection(rec, noteRequest(
			http.MethodPost,
			"/index.php/apps/notes/api/v1/notes",
			fmt.Sprintf(`{"title":"Boundary %d","content":"body"}`, i),
			"alice",
		))
		if rec.Code != http.StatusOK {
			t.Fatalf("create %d: %d %s", i, rec.Code, rec.Body.String())
		}
	}
	for _, cursor := range []string{"-2", "2"} {
		rec := httptest.NewRecorder()
		app.handleNotesCollection(rec, noteRequest(
			http.MethodGet,
			"/index.php/apps/notes/api/v1/notes?chunkSize=1&chunkCursor="+cursor,
			"",
			"alice",
		))
		if rec.Code != http.StatusOK {
			t.Fatalf("cursor %s: %d %s", cursor, rec.Code, rec.Body.String())
		}
	}
}

func TestV1EmptyUpdateConflictSettingsAndModified(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	n, err := app.create(ctx, "alice", notePatch{Title: str("Contract"), Content: str("initial")})
	if err != nil {
		t.Fatal(err)
	}
	modified := time.Now().Add(-time.Hour).Unix()
	req := noteRequest(http.MethodPut, "/index.php/apps/notes/api/v1/notes/1?id="+fmt.Sprint(n.ID), fmt.Sprintf(`{"content":"","modified":%d}`, modified), "alice")
	req.Header.Set("If-Match", n.ETag)
	rec := httptest.NewRecorder()
	app.handleNote(rec, req)
	if rec.Code != 200 {
		t.Fatalf("update: %d %s", rec.Code, rec.Body.String())
	}
	var updated Note
	if err = json.Unmarshal(rec.Body.Bytes(), &updated); err != nil {
		t.Fatal(err)
	}
	if updated.Content != "" || updated.Modified != modified {
		t.Fatalf("updated = %#v", updated)
	}
	req = noteRequest(http.MethodPut, "/index.php/apps/notes/api/v1/notes/1?id="+fmt.Sprint(n.ID), `{"content":"stale"}`, "alice")
	req.Header.Set("If-Match", n.ETag)
	rec = httptest.NewRecorder()
	app.handleNote(rec, req)
	if rec.Code != 412 {
		t.Fatalf("conflict status = %d body=%s", rec.Code, rec.Body.String())
	}
	req = noteRequest(http.MethodPut, "/index.php/apps/notes/api/v1/settings", `{"notesPath":"Journal","fileSuffix":".markdown","editorMode":"source"}`, "alice")
	rec = httptest.NewRecorder()
	app.handleSettings(rec, req)
	if rec.Code != 200 {
		t.Fatalf("settings: %d %s", rec.Code, rec.Body.String())
	}
	var settings Settings
	_ = json.Unmarshal(rec.Body.Bytes(), &settings)
	if settings.NotesPath != "/Journal" || settings.FileSuffix != ".markdown" || settings.EditorMode != "source" {
		t.Fatalf("settings = %#v", settings)
	}
}

func TestV14AttachmentValidationAndDownload(t *testing.T) {
	app, _ := testNotesApp(t)
	n, err := app.create(context.Background(), "alice", notePatch{Title: str("Images")})
	if err != nil {
		t.Fatal(err)
	}
	png := []byte{0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0, 0, 0, 0, 0}
	var body bytes.Buffer
	writer := multipart.NewWriter(&body)
	part, err := writer.CreateFormFile("file", "image.png")
	if err != nil {
		t.Fatal(err)
	}
	_, _ = part.Write(png)
	_ = writer.Close()
	req := httptest.NewRequest(http.MethodPost, "/index.php/apps/notes/api/v1.4/attachment/1?id="+fmt.Sprint(n.ID), &body)
	req.Header.Set("Content-Type", writer.FormDataContentType())
	req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, &auth.User{ID: "alice"}))
	rec := httptest.NewRecorder()
	app.handleAttachment(rec, req)
	if rec.Code != 200 {
		t.Fatalf("upload: %d %s", rec.Code, rec.Body.String())
	}
	var uploaded map[string]string
	_ = json.Unmarshal(rec.Body.Bytes(), &uploaded)
	if !strings.HasSuffix(uploaded["filename"], ".png") {
		t.Fatalf("filename = %q", uploaded["filename"])
	}
	req = noteRequest(http.MethodGet, "/index.php/apps/notes/api/v1.4/attachment/1?id="+fmt.Sprint(n.ID)+"&path="+uploaded["filename"], "", "alice")
	rec = httptest.NewRecorder()
	app.handleAttachment(rec, req)
	if rec.Code != 200 || !bytes.Equal(rec.Body.Bytes(), png) {
		t.Fatalf("download: %d %x", rec.Code, rec.Body.Bytes())
	}
	req = noteRequest(http.MethodGet, "/index.php/apps/notes/api/v1.4/attachment/1?id="+fmt.Sprint(n.ID)+"&path=../Images.md", "", "alice")
	rec = httptest.NewRecorder()
	app.handleAttachment(rec, req)
	if rec.Code == 200 {
		t.Fatal("path traversal was accepted")
	}
}

func TestNotesClosedDatabaseHandlerErrorMatrix(t *testing.T) {
	app, _ := testNotesApp(t)
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	cases := []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    string
	}{
		{app.handleNotesCollection, http.MethodGet, "/api/notes/notes", ""},
		{app.handleNotesCollection, http.MethodPost, "/api/notes/notes", `{"title":"Closed"}`},
		{app.handleNote, http.MethodGet, "/api/notes/notes/1?id=1", ""},
		{app.handleNote, http.MethodPut, "/api/notes/notes/1?id=1", `{"title":"Closed"}`},
		{app.handleNote, http.MethodDelete, "/api/notes/notes/1?id=1", ""},
		{app.handleSettings, http.MethodGet, "/api/notes/settings", ""},
		{app.handleSettings, http.MethodPut, "/api/notes/settings", `{}`},
		{app.handleCategories, http.MethodGet, "/api/notes/categories", ""},
		{app.handleCategories, http.MethodPost, "/api/notes/categories", `{"name":"Closed"}`},
		{app.handleCategories, http.MethodPut, "/api/notes/categories", `{"from":"A","to":"B"}`},
		{app.handleCategories, http.MethodDelete, "/api/notes/categories?name=A", ""},
		{app.handleUndo, http.MethodPost, "/api/notes/undo/1?id=1", ""},
		{app.handleVersions, http.MethodGet, "/api/notes/versions/1?id=1", ""},
		{app.handleVersions, http.MethodPost, "/api/notes/versions/1?id=1&version=v1", ""},
		{app.handleAttachment, http.MethodGet, "/api/notes/attachment/1?id=1&path=image.png", ""},
		{app.handleAttachment, http.MethodPost, "/api/notes/attachment/1?id=1", ""},
	}
	for _, tc := range cases {
		rec := httptest.NewRecorder()
		tc.handler(rec, noteRequest(tc.method, tc.target, tc.body, "alice"))
		if rec.Code < 400 {
			t.Errorf("%s %s unexpectedly returned %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	for _, tc := range []struct {
		err  error
		code int
	}{
		{os.ErrNotExist, http.StatusNotFound},
		{errors.New("resource locked"), http.StatusLocked},
		{errors.New("storage failed"), http.StatusInternalServerError},
	} {
		rec := httptest.NewRecorder()
		app.writeError(rec, tc.err)
		if rec.Code != tc.code {
			t.Errorf("writeError(%v)=%d, want %d", tc.err, rec.Code, tc.code)
		}
	}
	if got := noteExcerpt(strings.Repeat("word ", 100)); !strings.HasSuffix(got, "…") {
		t.Fatalf("long excerpt=%q", got)
	}
}

func TestSummaryListProvidesExcerptAndFullTextSearch(t *testing.T) {
	app, _ := testNotesApp(t)
	if _, err := app.create(context.Background(), "alice", notePatch{Title: str("Roadmap"), Content: str("# Heading\n\nA hidden needle in **markdown**.")}); err != nil {
		t.Fatal(err)
	}
	if _, err := app.create(context.Background(), "alice", notePatch{Title: str("Other"), Content: str("unrelated")}); err != nil {
		t.Fatal(err)
	}
	deleted, err := app.create(context.Background(), "alice", notePatch{Title: str("Deleted"), Content: str("removed")})
	if err != nil {
		t.Fatal(err)
	}
	if err = app.delete(context.Background(), "alice", deleted.ID); err != nil {
		t.Fatal(err)
	}
	req := noteRequest(http.MethodGet, "/api/notes/notes?summary=1&q=needle", "", "alice")
	rec := httptest.NewRecorder()
	app.handleNotesCollection(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("summary search: %d %s", rec.Code, rec.Body.String())
	}
	var summaries []map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &summaries); err != nil {
		t.Fatal(err)
	}
	if len(summaries) != 1 || summaries[0]["title"] != "Roadmap" {
		t.Fatalf("summaries = %#v", summaries)
	}
	if _, ok := summaries[0]["content"]; ok {
		t.Fatal("summary included full content")
	}
	if excerpt, _ := summaries[0]["excerpt"].(string); excerpt == "" || strings.ContainsAny(excerpt, "#*") {
		t.Fatalf("excerpt = %q", excerpt)
	}
	summaryETag := rec.Header().Get("ETag")
	req = noteRequest(http.MethodGet, "/api/notes/notes?summary=1", "", "alice")
	rec = httptest.NewRecorder()
	app.handleNotesCollection(rec, req)
	summaryETag = rec.Header().Get("ETag")
	if err := json.Unmarshal(rec.Body.Bytes(), &summaries); err != nil {
		t.Fatal(err)
	}
	if len(summaries) != 2 {
		t.Fatalf("summary returned tombstones: %#v", summaries)
	}
	req = noteRequest(http.MethodGet, "/api/notes/notes", "", "alice")
	rec = httptest.NewRecorder()
	app.handleNotesCollection(rec, req)
	var full []map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &full); err != nil {
		t.Fatal(err)
	}
	if len(full) != 3 {
		t.Fatalf("full notes = %#v", full)
	}
	if rec.Header().Get("ETag") == summaryETag {
		t.Fatal("summary and compatibility responses shared an ETag")
	}
	contentCount := 0
	for _, note := range full {
		if _, ok := note["content"]; ok {
			contentCount++
		}
	}
	if contentCount != 2 {
		t.Fatalf("legacy full response changed: %#v", full)
	}
}

func TestNotesPageLoadsSharedEditorBeforeAdapter(t *testing.T) {
	app, _ := testNotesApp(t)
	req := noteRequest(http.MethodGet, "/notes", "", "alice")
	rec := httptest.NewRecorder()
	app.handlePage(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status=%d body=%s", rec.Code, rec.Body.String())
	}
	html := rec.Body.String()
	shared := strings.Index(html, `/assets/text-editor.js`)
	adapter := strings.Index(html, `/notes/assets/app.js`)
	if shared < 0 || adapter < shared {
		t.Fatalf("shared editor must load before Notes adapter: shared=%d adapter=%d", shared, adapter)
	}
	for _, want := range []string{`data-editor-mode="rich"`, `data-editor-format="bold"`} {
		if !strings.Contains(html, want) {
			t.Fatalf("notes page does not contain %q", want)
		}
	}
	if !strings.Contains(string(notesScript), `dar.notes.drafts.v1:`) {
		t.Fatal("Notes adapter no longer preserves its draft storage key")
	}
}

func TestNotesHTTPValidationConditionalAndMethodMatrix(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	note, err := app.create(ctx, "alice", notePatch{Title: str("Matrix"), Content: str("body")})
	if err != nil {
		t.Fatal(err)
	}

	for name, handler := range map[string]http.HandlerFunc{
		"collection": app.handleNotesCollection,
		"note":       app.handleNote,
		"settings":   app.handleSettings,
		"categories": app.handleCategories,
		"undo":       app.handleUndo,
		"versions":   app.handleVersions,
		"attachment": app.handleAttachment,
	} {
		t.Run("anonymous "+name, func(t *testing.T) {
			recorder := httptest.NewRecorder()
			handler(recorder, httptest.NewRequest(http.MethodGet, "/?id=1", nil))
			if recorder.Code != http.StatusUnauthorized {
				t.Fatalf("status=%d body=%s", recorder.Code, recorder.Body.String())
			}
		})
	}

	cases := []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    string
		want    int
	}{
		{"collection malformed", app.handleNotesCollection, http.MethodPost, "/api/notes/notes", `{`, http.StatusBadRequest},
		{"collection method", app.handleNotesCollection, http.MethodPatch, "/api/notes/notes", ``, http.StatusMethodNotAllowed},
		{"note invalid id", app.handleNote, http.MethodGet, "/api/notes/notes/nope?id=nope", ``, http.StatusBadRequest},
		{"note missing", app.handleNote, http.MethodGet, "/api/notes/notes/999?id=999", ``, http.StatusNotFound},
		{"note malformed", app.handleNote, http.MethodPut, fmt.Sprintf("/api/notes/notes/%d?id=%d", note.ID, note.ID), `{`, http.StatusBadRequest},
		{"note method", app.handleNote, http.MethodPatch, fmt.Sprintf("/api/notes/notes/%d?id=%d", note.ID, note.ID), ``, http.StatusMethodNotAllowed},
		{"settings malformed", app.handleSettings, http.MethodPut, "/api/notes/settings", `{`, http.StatusBadRequest},
		{"settings method", app.handleSettings, http.MethodDelete, "/api/notes/settings", ``, http.StatusMethodNotAllowed},
		{"category delete invalid", app.handleCategories, http.MethodDelete, "/api/notes/categories?name=../escape", ``, http.StatusBadRequest},
		{"attachment invalid id", app.handleAttachment, http.MethodGet, "/api/notes/attachment/nope?id=nope", ``, http.StatusBadRequest},
		{"attachment missing note", app.handleAttachment, http.MethodGet, "/api/notes/attachment/999?id=999&path=x.png", ``, http.StatusNotFound},
		{"attachment missing path", app.handleAttachment, http.MethodGet, fmt.Sprintf("/api/notes/attachment/%d?id=%d", note.ID, note.ID), ``, http.StatusBadRequest},
		{"attachment nested path", app.handleAttachment, http.MethodGet, fmt.Sprintf("/api/notes/attachment/%d?id=%d&path=dir/x.png", note.ID, note.ID), ``, http.StatusBadRequest},
		{"attachment missing file", app.handleAttachment, http.MethodGet, fmt.Sprintf("/api/notes/attachment/%d?id=%d&path=missing.png", note.ID, note.ID), ``, http.StatusNotFound},
		{"attachment no multipart", app.handleAttachment, http.MethodPost, fmt.Sprintf("/api/notes/attachment/%d?id=%d", note.ID, note.ID), ``, http.StatusRequestEntityTooLarge},
		{"attachment method", app.handleAttachment, http.MethodDelete, fmt.Sprintf("/api/notes/attachment/%d?id=%d", note.ID, note.ID), ``, http.StatusMethodNotAllowed},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			recorder := callNotesHandler(t, tc.handler, tc.method, tc.target, tc.body)
			if recorder.Code != tc.want {
				t.Fatalf("status=%d want=%d body=%s", recorder.Code, tc.want, recorder.Body.String())
			}
		})
	}

	request := noteRequest(http.MethodGet, fmt.Sprintf("/api/notes/notes/%d?id=%d", note.ID, note.ID), "", "alice")
	request.Header.Set("If-None-Match", `"`+note.ETag+`"`)
	recorder := httptest.NewRecorder()
	app.handleNote(recorder, request)
	if recorder.Code != http.StatusNotModified {
		t.Fatalf("conditional note status=%d", recorder.Code)
	}

	recorder = httptest.NewRecorder()
	app.handleAsset(recorder, httptest.NewRequest(http.MethodGet, "/notes/assets/app.js", nil))
	if recorder.Code != http.StatusOK || recorder.Header().Get("Content-Type") != "text/javascript; charset=utf-8" || recorder.Body.Len() == 0 {
		t.Fatalf("asset status=%d headers=%v bytes=%d", recorder.Code, recorder.Header(), recorder.Body.Len())
	}

	if !validCategoryName("parent/child") {
		t.Fatal("nested valid category rejected")
	}
	for _, invalid := range []string{"", "/absolute", `back\\slash`, ".", "parent//child"} {
		if validCategoryName(invalid) {
			t.Errorf("invalid category %q accepted", invalid)
		}
	}

	recorder = callNotesHandler(t, app.handleNote, http.MethodDelete,
		fmt.Sprintf("/api/notes/notes/%d?id=%d", note.ID, note.ID), "")
	if recorder.Code != http.StatusOK {
		t.Fatalf("delete status=%d body=%s", recorder.Code, recorder.Body.String())
	}
}
