package files

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"image"
	"image/color"
	"image/png"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
)

type filesSearchSink struct {
	docs []apps.SearchDocument
	err  error
}

func (s *filesSearchSink) AddSearchDocument(_ context.Context, doc apps.SearchDocument) error {
	if s.err != nil {
		return s.err
	}
	s.docs = append(s.docs, doc)
	return nil
}

func filesCall(t *testing.T, app *App, handler http.HandlerFunc, method, target string, body []byte, uid string) *httptest.ResponseRecorder {
	t.Helper()
	req := withTestUser(httptest.NewRequest(method, target, bytes.NewReader(body)), uid)
	rec := httptest.NewRecorder()
	handler(rec, req)
	return rec
}

func TestFilesMutationMetadataVersionTrashSearchAndDAVWorkflow(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()

	rec := filesCall(t, app, app.handleMkdir, http.MethodPost, "/api/files/mkdir?path=/&name=Documents", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("mkdir: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleTouch, http.MethodPost, "/api/files/touch?path=/Documents&name=plan.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("touch: %d %s", rec.Code, rec.Body.String())
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/plan.md", []byte("# Original\nsearchable plan")); err != nil {
		t.Fatal(err)
	}

	var upload bytes.Buffer
	writer := multipart.NewWriter(&upload)
	part, err := writer.CreateFormFile("file", "upload.txt")
	if err != nil {
		t.Fatal(err)
	}
	_, _ = part.Write([]byte("uploaded content"))
	_ = writer.Close()
	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/upload?path=/Documents", &upload), "alice")
	req.Header.Set("Content-Type", writer.FormDataContentType())
	rec = httptest.NewRecorder()
	app.handleUpload(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("upload: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleDownload, http.MethodGet, "/api/files/download?path=/Documents/upload.txt", nil, "alice")
	if rec.Code != http.StatusOK || rec.Body.String() != "uploaded content" {
		t.Fatalf("download: %d %q", rec.Code, rec.Body.String())
	}

	for range 2 {
		rec = filesCall(t, app, app.handleFavorite, http.MethodPost, "/api/files/favorite?path=/Documents/plan.md", nil, "alice")
		if rec.Code != http.StatusOK {
			t.Fatalf("favorite: %d %s", rec.Code, rec.Body.String())
		}
	}
	rec = filesCall(t, app, app.handleTags, http.MethodPost, "/api/files/tags", []byte(`{"name":"Important"}`), "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("create tag: %d %s", rec.Code, rec.Body.String())
	}
	var tag Tag
	if err = json.Unmarshal(rec.Body.Bytes(), &tag); err != nil {
		t.Fatal(err)
	}
	for _, add := range []bool{true, false, true} {
		body, _ := json.Marshal(map[string]any{"path": "/Documents/plan.md", "tag": tag.ID, "add": add})
		rec = filesCall(t, app, app.handleTag, http.MethodPost, "/api/files/tag", body, "alice")
		if rec.Code != http.StatusOK {
			t.Fatalf("tag %v: %d %s", add, rec.Code, rec.Body.String())
		}
	}
	rec = filesCall(t, app, app.handleTags, http.MethodGet, "/api/files/tags", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Important") {
		t.Fatalf("list tags: %d %s", rec.Code, rec.Body.String())
	}

	v1, err := app.CreateVersion(ctx, "alice", "/Documents/plan.md")
	if err != nil {
		t.Fatal(err)
	}
	if err = app.WriteFile(ctx, "alice", "/Documents/plan.md", []byte("# Revised\nsearchable plan")); err != nil {
		t.Fatal(err)
	}
	v2, err := app.CreateVersion(ctx, "alice", "/Documents/plan.md")
	if err != nil {
		t.Fatal(err)
	}
	rec = filesCall(t, app, app.handleVersions, http.MethodGet, "/api/files/versions?path=/Documents/plan.md", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), v1.ID) {
		t.Fatalf("versions: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleDiff, http.MethodGet, "/api/files/diff?path=/Documents/plan.md&v1="+url.QueryEscape(v1.ID)+"&v2="+url.QueryEscape(v2.ID), nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Original") {
		t.Fatalf("diff: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleVersion, http.MethodGet, "/api/files/version?path=/Documents/plan.md&id="+url.QueryEscape(v1.ID), nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Original") {
		t.Fatalf("read version: %d %s", rec.Code, rec.Body.String())
	}

	rec = filesCall(t, app, app.handleCopy, http.MethodPost, "/api/files/copy?src=/Documents/plan.md&dst=/Documents/copy.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("copy: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleMove, http.MethodPost, "/api/files/move?src=/Documents/copy.md&dst=/Documents/moved.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("move: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleRename, http.MethodPost, "/api/files/rename?src=/Documents/moved.md&dst=/Documents/final.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("rename: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleSymlink, http.MethodPost, "/api/files/symlink", []byte(`{"path":"/Documents/link.md","target":"plan.md"}`), "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("symlink: %d %s", rec.Code, rec.Body.String())
	}

	rec = filesCall(t, app, app.handleComments, http.MethodPost, "/api/files/comments?path=/Documents/plan.md", []byte(`{"body":"Looks good"}`), "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("comment: %d %s", rec.Code, rec.Body.String())
	}
	var comment Comment
	if err = json.Unmarshal(rec.Body.Bytes(), &comment); err != nil {
		t.Fatal(err)
	}
	rec = filesCall(t, app, app.handleComments, http.MethodGet, "/api/files/comments?path=/Documents/plan.md", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Looks good") {
		t.Fatalf("list comments: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleComments, http.MethodDelete, "/api/files/comments?path=/Documents/plan.md&id="+comment.ID, nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete comment: %d %s", rec.Code, rec.Body.String())
	}
	for _, method := range []string{http.MethodPost, http.MethodGet, http.MethodDelete} {
		rec = filesCall(t, app, app.handleLock, method, "/api/files/lock?path=/Documents/plan.md", nil, "alice")
		if rec.Code != http.StatusOK {
			t.Fatalf("%s lock: %d %s", method, rec.Code, rec.Body.String())
		}
	}

	rec = filesCall(t, app, app.handleSearch, http.MethodGet, "/api/files/search?q=plan&type=document&folder=/Documents&limit=10", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "plan.md") {
		t.Fatalf("search: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handlePreview, http.MethodGet, "/api/files/preview?path=/Documents/plan.md", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "Revised") {
		t.Fatalf("preview: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleStats, http.MethodGet, "/api/files/stats", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"used":`) {
		t.Fatalf("stats: %d %s", rec.Code, rec.Body.String())
	}

	img := image.NewRGBA(image.Rect(0, 0, 4, 3))
	for y := 0; y < 3; y++ {
		for x := 0; x < 4; x++ {
			img.Set(x, y, color.RGBA{R: 255, A: 255})
		}
	}
	var pngData bytes.Buffer
	if err = png.Encode(&pngData, img); err != nil {
		t.Fatal(err)
	}
	if err = app.WriteFile(ctx, "alice", "/Documents/image.png", pngData.Bytes()); err != nil {
		t.Fatal(err)
	}
	rec = filesCall(t, app, app.handleThumbnail, http.MethodGet, "/api/files/thumb?path=/Documents/image.png", nil, "alice")
	if rec.Code != http.StatusOK || rec.Header().Get("Content-Type") != "image/jpeg" {
		t.Fatalf("thumbnail: %d headers=%v body=%s", rec.Code, rec.Header(), rec.Body.String())
	}

	sink := &filesSearchSink{}
	if err = app.IndexSearch(ctx, "alice", sink); err != nil {
		t.Fatal(err)
	}
	if len(sink.docs) == 0 {
		t.Fatal("files search index was empty")
	}
	sink.err = context.Canceled
	if err = app.IndexSearch(ctx, "alice", sink); !errors.Is(err, context.Canceled) {
		t.Fatalf("search sink error = %v", err)
	}

	rec = filesCall(t, app, app.handleDelete, http.MethodPost, "/api/files/delete?path=/Documents/final.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("trash file: %d %s", rec.Code, rec.Body.String())
	}
	trash, err := app.trashRecords(ctx, "alice")
	if err != nil || len(trash) != 1 {
		t.Fatalf("trash = %#v, %v", trash, err)
	}
	rec = filesCall(t, app, app.handleRestore, http.MethodPost, "/api/files/restore?id="+trash[0].ID, nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("restore: %d %s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleDelete, http.MethodPost, "/api/files/delete?path=/Documents/final.md", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("trash again: %d %s", rec.Code, rec.Body.String())
	}
	trash, _ = app.trashRecords(ctx, "alice")
	rec = filesCall(t, app, app.handleDeletePermanent, http.MethodPost, "/api/files/delete-permanent?id="+trash[0].ID, nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("delete permanently: %d %s", rec.Code, rec.Body.String())
	}

	if err = app.PruneVersions(ctx, "alice", "/Documents/plan.md", 1); err != nil {
		t.Fatal(err)
	}
	if ids, err := app.VersionIDs(ctx, "alice", "/Documents/plan.md"); err != nil || len(ids) != 1 {
		t.Fatalf("pruned versions = %#v, %v", ids, err)
	}
	if err = app.SetModTime(ctx, "alice", "/Documents/plan.md", time.Unix(1700000000, 0)); err != nil {
		t.Fatal(err)
	}
	seen := 0
	if err = app.Walk(ctx, "alice", "/Documents", func(vpath string, _ os.FileInfo, walkErr error) error {
		seen++
		return walkErr
	}); err != nil || seen == 0 {
		t.Fatalf("walk: seen=%d err=%v", seen, err)
	}
}

func TestFilesWebDAVCompatibilityWorkflow(t *testing.T) {
	app := sqliteTestApp(t)
	call := func(method, target, body string) *httptest.ResponseRecorder {
		return filesCall(t, app, app.handleWebDAV, method, target, []byte(body), "alice")
	}
	for _, tc := range []struct {
		method string
		target string
		body   string
		status int
	}{
		{"MKCOL", "/webdav/DAV", "", 201},
		{http.MethodPut, "/webdav/DAV/file.txt", "dav content", 201},
		{"PROPFIND", "/webdav/DAV", "", 207},
		{http.MethodGet, "/webdav/DAV/file.txt", "", 200},
		{http.MethodGet, "/webdav/DAV", "", 200},
		{http.MethodPatch, "/webdav/DAV/file.txt", "", 405},
		{http.MethodDelete, "/webdav/DAV/file.txt", "", 204},
		{http.MethodGet, "/webdav/DAV/file.txt", "", 404},
	} {
		rec := call(tc.method, tc.target, tc.body)
		if rec.Code != tc.status {
			t.Fatalf("%s %s: %d, want %d; body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}
}

func TestFilesUserShareReadWriteAndSearchIsolation(t *testing.T) {
	root := t.TempDir()
	migrations := append(auth.Migrations(), preferences.Migrations()...)
	migrations = append(migrations, Migrations()...)
	database, err := db.Open(filepath.Join(root, "shares.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	app := New(Config{DataRoot: filepath.Join(root, "data"), DB: database, SharePasswordKey: bytes.Repeat([]byte{0x42}, 32)})
	createUser := func(name string) *auth.User {
		var user *auth.User
		conn, release, err := database.Conn(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		defer release()
		user, _, err = auth.FindOrCreateUser(context.Background(), conn, name, name+"@example.test", name, "", "https://issuer.test", name)
		if err != nil {
			t.Fatal(err)
		}
		return user
	}
	alice, bob := createUser("alice"), createUser("bob")
	if err = app.WriteFile(context.Background(), alice.ID, "/shared.txt", []byte("shared searchable body")); err != nil {
		t.Fatal(err)
	}
	body, _ := json.Marshal(map[string]any{"path": "/shared.txt", "to_user": bob.Email, "permission": "read"})
	rec := filesCall(t, app, app.handleUserShare, http.MethodPost, "/api/files/share-user", body, alice.ID)
	if rec.Code != http.StatusOK {
		t.Fatalf("share user: %d %s", rec.Code, rec.Body.String())
	}
	var share UserShare
	if err = json.Unmarshal(rec.Body.Bytes(), &share); err != nil {
		t.Fatal(err)
	}
	rec = filesCall(t, app, app.handleSharedWithMe, http.MethodGet, "/api/files/shared-with-me", nil, bob.ID)
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), share.ID) {
		t.Fatalf("shared with me: %d %s", rec.Code, rec.Body.String())
	}
	data, err := app.ReadFile(context.Background(), bob.ID, "/Shared/"+share.ID)
	if err != nil || string(data) != "shared searchable body" {
		t.Fatalf("read shared file = %q, %v", data, err)
	}
	if err = app.WriteFile(context.Background(), bob.ID, "/Shared/"+share.ID, []byte("blocked")); !errors.Is(err, ErrShareReadOnly) {
		t.Fatalf("read-only shared write error = %v", err)
	}
	sink := &filesSearchSink{}
	if err = app.IndexSearch(context.Background(), bob.ID, sink); err != nil {
		t.Fatal(err)
	}
	found := false
	for _, doc := range sink.docs {
		if strings.HasPrefix(doc.ObjectID, "share:"+share.ID+":") && strings.Contains(doc.Body, "shared searchable") {
			found = true
		}
	}
	if !found {
		t.Fatalf("shared search docs = %#v", sink.docs)
	}
	if err = app.WriteFile(context.Background(), alice.ID, "/editable/nested.txt", []byte("editable")); err != nil {
		t.Fatal(err)
	}
	body, _ = json.Marshal(map[string]any{"path": "/editable", "to_user": bob.Email, "permission": "write"})
	rec = filesCall(t, app, app.handleUserShare, http.MethodPost, "/api/files/share-user", body, alice.ID)
	if rec.Code != http.StatusOK {
		t.Fatalf("write share: %d %s", rec.Code, rec.Body.String())
	}
	var editable UserShare
	if err = json.Unmarshal(rec.Body.Bytes(), &editable); err != nil {
		t.Fatal(err)
	}
	rec = filesCall(t, app, app.handleDelete, http.MethodPost, "/api/files/delete?path=/Shared/"+editable.ID+"/nested.txt", nil, bob.ID)
	if rec.Code != http.StatusOK {
		t.Fatalf("delete writable shared item: %d %s", rec.Code, rec.Body.String())
	}
	if _, err = app.ReadFile(context.Background(), alice.ID, "/editable/nested.txt"); !os.IsNotExist(err) {
		t.Fatalf("shared delete did not remove owner file: %v", err)
	}
	rec = filesCall(t, app, app.handleUserShare, http.MethodDelete, "/api/files/share-user?id="+share.ID, nil, bob.ID)
	if rec.Code != http.StatusOK {
		t.Fatalf("remove share: %d %s", rec.Code, rec.Body.String())
	}
	if _, err = app.ReadFile(context.Background(), bob.ID, "/Shared/"+share.ID); err == nil {
		t.Fatal("removed share remained readable")
	}
}
