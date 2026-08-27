package files

import (
	"bytes"
	"context"
	"errors"
	"image"
	"image/color"
	"image/png"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestFilesPageViewMIMEAndMutationErrorEdges(t *testing.T) {
	for ext, want := range map[string]string{
		".PNG": "fa-regular fa-image", ".md": "fa-regular fa-file-lines",
		".pdf": "fa-regular fa-file-pdf", ".bin": "fa-regular fa-file",
	} {
		if got := fileWidgetIcon(ext); got != want {
			t.Errorf("fileWidgetIcon(%q)=%q want=%q", ext, got, want)
		}
	}
	if !internalFavoritePath("/.versions/a") || !internalFavoritePath("/.trash/a") ||
		!internalFavoritePath("/.local/share/Trash/a") || internalFavoritePath("/Documents/a") ||
		len(mustReadFile("assets/app.js")) == 0 {
		t.Fatal("Files provider/asset helper regression")
	}
	for _, tc := range []struct {
		target string
		want   string
	}{
		{"/files?filter=favorites", "favorites"},
		{"/files?view=recent", "recent"},
		{"/files/docs", "personal"},
		{"/files/docs/nested", "personal"},
		{"/files/.local/share/Trash", "trash"},
		{"/files/.local/share/Trash/item", "trash"},
		{"/files", "all"},
		{"/files/other", "all"},
	} {
		if got := filesActiveView(httptest.NewRequest(http.MethodGet, tc.target, nil)); got != tc.want {
			t.Errorf("filesActiveView(%q)=%q want=%q", tc.target, got, tc.want)
		}
	}
	rec := httptest.NewRecorder()
	handlePage(rec, httptest.NewRequest(http.MethodGet, "/files", nil))
	if rec.Code != http.StatusSeeOther {
		t.Fatalf("anonymous page status=%d", rec.Code)
	}
	rec = filesCall(t, sqliteTestApp(t), handlePage, http.MethodGet, "/files?view=recent", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `id="files-app"`) {
		t.Fatalf("page status=%d body=%s", rec.Code, rec.Body.String())
	}

	if validMIMEPrefixes([]string{"Text/Plain"}) || validMIMEPrefixes([]string{"text"}) ||
		validMIMEPrefixes([]string{""}) || validMIMEPrefixes([]string{"text/pl@in"}) ||
		validMIMEPrefixes(make([]string, 9)) || !validMIMEPrefixes([]string{"text/plain", "image/png"}) {
		t.Fatal("MIME prefix validation regression")
	}

	rootDir := t.TempDir()
	if err := os.WriteFile(rootDir+"/known.txt", []byte("hello"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(rootDir+"/sniff", []byte("<html>hello</html>"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(rootDir+"/empty", nil, 0o600); err != nil {
		t.Fatal(err)
	}
	root, err := os.OpenRoot(rootDir)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	if got := detectedMIMEFromRoot(root, "known.txt", "known.txt"); !strings.HasPrefix(got, "text/plain") {
		t.Fatalf("known MIME=%q", got)
	}
	if got := detectedMIMEFromRoot(root, "sniff", "sniff"); !strings.HasPrefix(got, "text/html") {
		t.Fatalf("sniffed MIME=%q", got)
	}
	if got := detectedMIMEFromRoot(root, "empty", "empty"); got != "text/plain; charset=utf-8" {
		t.Fatalf("empty MIME=%q", got)
	}
	if got := detectedMIMEFromRoot(root, "missing", "missing"); got != "" {
		t.Fatalf("missing MIME=%q", got)
	}

	app := sqliteTestApp(t)
	for _, tc := range []struct {
		err    error
		status int
	}{
		{ErrManagedPathReadOnly, http.StatusForbidden},
		{ErrShareReadOnly, http.StatusForbidden},
		{ErrSharedRootMutation, http.StatusForbidden},
		{ErrLocked, http.StatusLocked},
		{errors.New("other"), http.StatusInternalServerError},
	} {
		rec = httptest.NewRecorder()
		app.writeMutationError(rec, tc.err)
		if rec.Code != tc.status {
			t.Errorf("writeMutationError(%v)=%d want=%d", tc.err, rec.Code, tc.status)
		}
	}
}

func TestFilesEditorAndPreviewFailureMatrix(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/Documents/folder"); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/text.txt", []byte("hello")); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/binary", []byte{0xff, 0x00}); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/large.txt", bytes.Repeat([]byte("x"), editorFileLimit+1)); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		method string
		target string
		body   string
		match  string
		status int
	}{
		{http.MethodGet, "/api/files/editor?path=relative", "", "", http.StatusBadRequest},
		{http.MethodPost, "/api/files/editor?path=/Documents/text.txt", "", "", http.StatusMethodNotAllowed},
		{http.MethodGet, "/api/files/editor?path=/Documents/missing.txt", "", "", http.StatusNotFound},
		{http.MethodGet, "/api/files/editor?path=/Documents/folder", "", "", http.StatusUnsupportedMediaType},
		{http.MethodGet, "/api/files/editor?path=/Documents/binary", "", "", http.StatusUnsupportedMediaType},
		{http.MethodGet, "/api/files/editor?path=/Documents/large.txt", "", "", http.StatusRequestEntityTooLarge},
		{http.MethodPut, "/api/files/editor?path=/Documents/text.txt", `{}`, "", http.StatusPreconditionRequired},
		{http.MethodPut, "/api/files/editor?path=/Documents/text.txt", `{`, "etag", http.StatusBadRequest},
		{http.MethodPut, "/api/files/editor?path=/Documents/text.txt", `{"content":"x","name":"../bad"}`, "etag", http.StatusBadRequest},
		{http.MethodPut, "/api/files/editor?path=/Documents/text.txt", `{"content":"x\u0000"}`, "etag", http.StatusUnsupportedMediaType},
		{http.MethodPut, "/api/files/editor?path=/Documents/text.txt", `{"content":"changed"}`, "wrong", http.StatusPreconditionFailed},
	} {
		req := withTestUser(httptest.NewRequest(tc.method, tc.target, strings.NewReader(tc.body)), "alice")
		if tc.match != "" {
			req.Header.Set("If-Match", tc.match)
		}
		rec := httptest.NewRecorder()
		app.handleEditor(rec, req)
		if rec.Code != tc.status {
			t.Errorf("%s %s status=%d want=%d body=%s", tc.method, tc.target, rec.Code, tc.status, rec.Body.String())
		}
	}

	rec := filesCall(t, app, app.handleEditor, http.MethodGet, "/api/files/editor?path=/Documents/text.txt", nil, "alice")
	if rec.Code != http.StatusOK || rec.Header().Get("ETag") == "" {
		t.Fatalf("editor read status=%d body=%s", rec.Code, rec.Body.String())
	}
	etag := rec.Header().Get("ETag")
	req := withTestUser(httptest.NewRequest(http.MethodPut, "/api/files/editor?path=/Documents/text.txt", strings.NewReader(`{"content":"hello"}`)), "alice")
	req.Header.Set("If-Match", etag)
	rec = httptest.NewRecorder()
	app.handleEditor(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("unchanged update status=%d body=%s", rec.Code, rec.Body.String())
	}

	for _, target := range []string{
		"/api/files/preview?path=/Documents/missing",
		"/api/files/preview?path=/Shared",
		"/api/files/preview?path=/Documents/text.txt",
		"/api/files/preview?path=/Documents/large.txt",
	} {
		rec = filesCall(t, app, app.handlePreview, http.MethodGet, target, nil, "alice")
		if strings.Contains(target, "missing") && rec.Code != http.StatusNotFound {
			t.Errorf("%s status=%d", target, rec.Code)
		} else if !strings.Contains(target, "missing") && rec.Code != http.StatusOK {
			t.Errorf("%s status=%d body=%s", target, rec.Code, rec.Body.String())
		}
	}
}

func TestFilesRecursiveCopyAndSymlinkValidation(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/source/nested"); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/source/nested/file.txt", []byte("copied")); err != nil {
		t.Fatal(err)
	}
	if err := app.CreateSymlink(ctx, "alice", "/source/link", "nested/file.txt"); err != nil {
		t.Fatal(err)
	}
	if err := app.copyUserPath(ctx, "alice", "/source", "/destination"); err != nil {
		t.Fatal(err)
	}
	if data, err := app.ReadFile(ctx, "alice", "/destination/nested/file.txt"); err != nil || string(data) != "copied" {
		t.Fatalf("copied data=%q err=%v", data, err)
	}
	if _, err := app.Stat(ctx, "alice", "/destination/link"); err != nil {
		t.Fatalf("copied symlink: %v", err)
	}
	if err := app.copyUserPath(ctx, "alice", "/missing", "/other"); !os.IsNotExist(err) {
		t.Fatalf("missing copy err=%v", err)
	}
	absoluteLink := app.realPath("alice", "/absolute-link")
	if err := os.Symlink("/etc/passwd", absoluteLink); err != nil {
		t.Fatal(err)
	}
	if err := app.copyUserPath(ctx, "alice", "/absolute-link", "/blocked-link"); err == nil {
		t.Fatal("absolute symlink was copied")
	}
}

func TestFilesUnavailableStorageAndClosedDatabaseMatrix(t *testing.T) {
	app := sqliteTestApp(t)
	blocker := filepath.Join(t.TempDir(), "not-a-directory")
	if err := os.WriteFile(blocker, []byte("blocked"), 0o600); err != nil {
		t.Fatal(err)
	}
	app.dataRoot = blocker
	cases := []struct {
		handler http.HandlerFunc
		method  string
		target  string
		body    []byte
		allowOK bool
	}{
		{app.handleList, http.MethodGet, "/api/files/list?path=/.local/share/Trash", nil, false},
		{app.handleList, http.MethodGet, "/api/files/list?path=/", nil, false},
		{app.handlePicker, http.MethodGet, "/api/files/picker?path=/", nil, false},
		{app.handlePickerValidate, http.MethodPost, "/api/files/picker/validate", []byte(`{"paths":["/file.txt"],"selection":"files"}`), false},
		{app.handleDownload, http.MethodGet, "/api/files/download?path=/file.txt", nil, false},
		{app.handleMkdir, http.MethodPost, "/api/files/mkdir?path=/folder", nil, false},
		{app.handleTouch, http.MethodPost, "/api/files/touch?path=/file.txt", nil, false},
		{app.handleSymlink, http.MethodPost, "/api/files/symlink", []byte(`{"path":"/link","target":"file.txt"}`), false},
		{app.handleCopy, http.MethodPost, "/api/files/copy?src=/file.txt&dst=/copy.txt", nil, false},
		{app.handleDelete, http.MethodPost, "/api/files/delete?path=/file.txt", nil, false},
		{app.handleDeletePermanent, http.MethodPost, "/api/files/delete-permanent?id=missing", nil, false},
		{app.handleVersions, http.MethodGet, "/api/files/versions?path=/file.txt", nil, true},
		{app.handleVersion, http.MethodGet, "/api/files/version?path=/file.txt&id=v1", nil, false},
		{app.handleDiff, http.MethodGet, "/api/files/diff?path=/file.txt&id=v1", nil, false},
		{app.handleStats, http.MethodGet, "/api/files/stats", nil, true},
		{app.handleWebDAV, http.MethodGet, "/webdav/file.txt", nil, false},
		{app.handleThumbnail, http.MethodGet, "/api/files/thumbnail?path=/file.png", nil, false},
		{app.handlePreview, http.MethodGet, "/api/files/preview?path=/file.png", nil, false},
	}
	for _, tc := range cases {
		rec := filesCall(t, app, tc.handler, tc.method, tc.target, tc.body, "alice")
		if rec.Code < 400 && !tc.allowOK {
			t.Errorf("%s %s unexpectedly returned %d: %s", tc.method, tc.target, rec.Code, rec.Body.String())
		}
	}
	health := filesCall(t, app, app.handleHealth, http.MethodGet, "/health", nil, "alice")
	if health.Code != http.StatusOK {
		t.Fatalf("unavailable health=%d %s", health.Code, health.Body.String())
	}

	closed := sqliteTestApp(t)
	if err := closed.WriteFile(context.Background(), "alice", "/file.txt", []byte("body")); err != nil {
		t.Fatal(err)
	}
	if err := closed.db.Close(); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		handler http.HandlerFunc
		target  string
		body    []byte
		allowOK bool
	}{
		{closed.handleRename, "/api/files/rename?src=/file.txt&dst=/renamed.txt", nil, false},
		{closed.handleCopy, "/api/files/copy?src=/file.txt&dst=/copy.txt", nil, false},
		{closed.handleDelete, "/api/files/delete?path=/file.txt", nil, false},
		{closed.handleDeletePermanent, "/api/files/delete-permanent?id=missing", nil, false},
		{closed.handleFavorite, "/api/files/favorite?path=/file.txt", nil, false},
		{closed.handleTag, "/api/files/tag", []byte(`{"path":"/file.txt","tag":"t1","add":true}`), true},
	} {
		rec := filesCall(t, closed, tc.handler, http.MethodPost, tc.target, tc.body, "alice")
		if rec.Code < 400 && !tc.allowOK {
			t.Errorf("closed DB %s returned %d: %s", tc.target, rec.Code, rec.Body.String())
		}
	}
}

func TestFilesHandlerMethodValidationSearchAndMediaMatrix(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/Documents/folder"); err != nil {
		t.Fatal(err)
	}
	for name, data := range map[string][]byte{
		"/Documents/readme.md":  []byte("document"),
		"/Documents/movie.mp4":  []byte("video"),
		"/Documents/sound.mp3":  []byte("audio"),
		"/Documents/plain.bin":  []byte("binary"),
		"/Documents/offset.txt": []byte("offset"),
	} {
		if err := app.WriteFile(ctx, "alice", name, data); err != nil {
			t.Fatal(err)
		}
	}
	img := image.NewNRGBA(image.Rect(0, 0, 3, 2))
	for y := 0; y < 2; y++ {
		for x := 0; x < 3; x++ {
			img.SetNRGBA(x, y, color.NRGBA{R: 30, G: 120, B: 220, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, img); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/picture.png", encoded.Bytes()); err != nil {
		t.Fatal(err)
	}

	for _, tc := range []struct {
		name    string
		handler http.HandlerFunc
		method  string
		target  string
		body    []byte
		want    int
	}{
		{"mkdir method", app.handleMkdir, http.MethodGet, "/api/files/mkdir", nil, http.StatusMethodNotAllowed},
		{"mkdir missing name", app.handleMkdir, http.MethodPost, "/api/files/mkdir?path=/", nil, http.StatusBadRequest},
		{"touch method", app.handleTouch, http.MethodGet, "/api/files/touch", nil, http.StatusMethodNotAllowed},
		{"touch invalid name", app.handleTouch, http.MethodPost, "/api/files/touch?path=/&name=../bad.txt", nil, http.StatusBadRequest},
		{"touch conflict", app.handleTouch, http.MethodPost, "/api/files/touch?path=/Documents&name=readme.md", nil, http.StatusConflict},
		{"symlink method", app.handleSymlink, http.MethodGet, "/api/files/symlink", nil, http.StatusMethodNotAllowed},
		{"symlink malformed", app.handleSymlink, http.MethodPost, "/api/files/symlink", []byte(`{`), http.StatusBadRequest},
		{"rename method", app.handleRename, http.MethodGet, "/api/files/rename", nil, http.StatusMethodNotAllowed},
		{"rename missing", app.handleRename, http.MethodPost, "/api/files/rename?src=/a", nil, http.StatusBadRequest},
		{"rename source absent", app.handleRename, http.MethodPost, "/api/files/rename?src=/missing&dst=/other", nil, http.StatusInternalServerError},
		{"copy method", app.handleCopy, http.MethodGet, "/api/files/copy", nil, http.StatusMethodNotAllowed},
		{"copy missing query", app.handleCopy, http.MethodPost, "/api/files/copy?src=/a", nil, http.StatusBadRequest},
		{"copy missing source", app.handleCopy, http.MethodPost, "/api/files/copy?src=/missing&dst=/copy", nil, http.StatusInternalServerError},
		{"delete method", app.handleDelete, http.MethodGet, "/api/files/delete", nil, http.StatusMethodNotAllowed},
		{"delete root", app.handleDelete, http.MethodPost, "/api/files/delete?path=/", nil, http.StatusBadRequest},
		{"delete shared trash", app.handleDelete, http.MethodPost, "/api/files/delete?path=/Documents/readme.md&trash_path=/Shared", nil, http.StatusBadRequest},
		{"restore method", app.handleRestore, http.MethodGet, "/api/files/restore", nil, http.StatusMethodNotAllowed},
		{"restore missing", app.handleRestore, http.MethodPost, "/api/files/restore?id=missing", nil, http.StatusNotFound},
		{"permanent method", app.handleDeletePermanent, http.MethodGet, "/api/files/delete-permanent", nil, http.StatusMethodNotAllowed},
		{"favorite method", app.handleFavorite, http.MethodGet, "/api/files/favorite", nil, http.StatusMethodNotAllowed},
		{"tags method", app.handleTags, http.MethodPatch, "/api/files/tags", nil, http.StatusMethodNotAllowed},
		{"tag method", app.handleTag, http.MethodGet, "/api/files/tag", nil, http.StatusMethodNotAllowed},
		{"version missing get", app.handleVersion, http.MethodGet, "/api/files/version?path=/Documents/readme.md&id=missing", nil, http.StatusNotFound},
		{"version missing restore", app.handleVersion, http.MethodPost, "/api/files/version?path=/Documents/readme.md&id=missing", nil, http.StatusNotFound},
		{"version method", app.handleVersion, http.MethodPatch, "/api/files/version", nil, http.StatusMethodNotAllowed},
		{"user share malformed", app.handleUserShare, http.MethodPost, "/api/files/share-user", []byte(`{`), http.StatusBadRequest},
		{"user share permission", app.handleUserShare, http.MethodPost, "/api/files/share-user", []byte(`{"path":"/Documents/readme.md","to_user":"bob","permission":"admin"}`), http.StatusBadRequest},
		{"user share root", app.handleUserShare, http.MethodPost, "/api/files/share-user", []byte(`{"path":"/","to_user":"bob"}`), http.StatusBadRequest},
		{"user share shared", app.handleUserShare, http.MethodPost, "/api/files/share-user", []byte(`{"path":"/Shared/item","to_user":"bob"}`), http.StatusBadRequest},
		{"user share missing", app.handleUserShare, http.MethodPost, "/api/files/share-user", []byte(`{"path":"/missing","to_user":"bob"}`), http.StatusNotFound},
		{"user share method", app.handleUserShare, http.MethodPatch, "/api/files/share-user", nil, http.StatusMethodNotAllowed},
		{"comments method", app.handleComments, http.MethodPatch, "/api/files/comments", nil, http.StatusMethodNotAllowed},
		{"lock method", app.handleLock, http.MethodPatch, "/api/files/lock", nil, http.StatusMethodNotAllowed},
		{"webdav method", app.handleWebDAV, http.MethodPatch, "/webdav/Documents/readme.md", nil, http.StatusMethodNotAllowed},
		{"public share malformed", app.handleShare, http.MethodPost, "/api/files/share", []byte(`{`), http.StatusBadRequest},
		{"public share empty", app.handleShare, http.MethodPost, "/api/files/share", []byte(`{"path":""}`), http.StatusBadRequest},
		{"public share expired", app.handleShare, http.MethodPost, "/api/files/share", []byte(`{"path":"/Documents/readme.md","expires":1}`), http.StatusBadRequest},
		{"public share shared", app.handleShare, http.MethodPost, "/api/files/share", []byte(`{"path":"/Shared/item"}`), http.StatusBadRequest},
		{"public share missing", app.handleShare, http.MethodPost, "/api/files/share", []byte(`{"path":"/missing"}`), http.StatusNotFound},
	} {
		t.Run(tc.name, func(t *testing.T) {
			recorder := filesCall(t, app, tc.handler, tc.method, tc.target, tc.body, "alice")
			if recorder.Code != tc.want {
				t.Fatalf("status=%d want=%d body=%s", recorder.Code, tc.want, recorder.Body.String())
			}
		})
	}

	for _, tc := range []struct {
		target string
		want   string
	}{
		{"/api/files/search", `"files":[]`},
		{"/api/files/search?type=folder", `"folder"`},
		{"/api/files/search?type=image", `"picture.png"`},
		{"/api/files/search?type=document&limit=500", `"readme.md"`},
		{"/api/files/search?type=video", `"movie.mp4"`},
		{"/api/files/search?type=audio", `"sound.mp3"`},
		{"/api/files/search?q=offset&offset=-3&folder=/Documents", `"offset.txt"`},
		{"/api/files/search?q=does-not-exist", `"files":[]`},
	} {
		recorder := filesCall(t, app, app.handleSearch, http.MethodGet, tc.target, nil, "alice")
		if recorder.Code != http.StatusOK || !strings.Contains(recorder.Body.String(), tc.want) {
			t.Errorf("%s status=%d body=%s want substring %q", tc.target, recorder.Code, recorder.Body.String(), tc.want)
		}
	}

	for _, handler := range []http.HandlerFunc{app.handleThumbnail, app.handlePreview} {
		recorder := filesCall(t, app, handler, http.MethodGet, "/api/files/media?path=/Shared", nil, "alice")
		if recorder.Code != http.StatusOK || !strings.Contains(recorder.Body.String(), `"files":[]`) {
			t.Fatalf("shared media status=%d body=%s", recorder.Code, recorder.Body.String())
		}
	}
	recorder := filesCall(t, app, app.handleThumbnail, http.MethodGet, "/api/files/thumbnail?path=/Documents/picture.png", nil, "alice")
	if recorder.Code != http.StatusOK || recorder.Header().Get("Content-Type") != "image/jpeg" || recorder.Body.Len() == 0 {
		t.Fatalf("thumbnail status=%d headers=%v bytes=%d", recorder.Code, recorder.Header(), recorder.Body.Len())
	}
	recorder = filesCall(t, app, app.handleThumbnail, http.MethodGet, "/api/files/thumbnail?path=/Documents/plain.bin", nil, "alice")
	if recorder.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("non-image thumbnail status=%d body=%s", recorder.Code, recorder.Body.String())
	}

	for _, method := range []string{http.MethodGet, http.MethodPost, http.MethodDelete} {
		recorder := filesCall(t, app, app.handleLock, method, "/api/files/lock?path=/Documents/readme.md", nil, "alice")
		if recorder.Code != http.StatusOK {
			t.Fatalf("lock %s status=%d body=%s", method, recorder.Code, recorder.Body.String())
		}
	}
}

func TestFilesPickerValidationAndMultipartUploadMatrix(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/media/folder"); err != nil {
		t.Fatal(err)
	}
	for path, data := range map[string][]byte{
		"/media/readme.txt": []byte("read me"),
		"/media/image.png":  []byte("\x89PNG\r\n\x1a\n"),
		"/media/.hidden":    []byte("hidden"),
	} {
		if err := app.WriteFile(ctx, "alice", path, data); err != nil {
			t.Fatal(err)
		}
	}
	for _, tc := range []struct {
		target string
		status int
		want   string
	}{
		{"/api/files/picker?path=/media", http.StatusOK, "readme.txt"},
		{"/api/files/picker?path=/media&selection=folders", http.StatusOK, "folder"},
		{"/api/files/picker?path=/media&selection=files&q=README", http.StatusOK, "readme.txt"},
		{"/api/files/picker?path=/media&selection=files&mime_prefix=image/", http.StatusOK, "image.png"},
		{"/api/files/picker?path=/media&selection=invalid", http.StatusBadRequest, ""},
		{"/api/files/picker?path=/media&mime_prefix=INVALID", http.StatusBadRequest, ""},
		{"/api/files/picker?path=/.local/share/Trash", http.StatusBadRequest, ""},
		{"/api/files/picker?path=/missing", http.StatusNotFound, ""},
	} {
		rec := filesCall(t, app, app.handlePicker, http.MethodGet, tc.target, nil, "alice")
		if rec.Code != tc.status || (tc.want != "" && !strings.Contains(rec.Body.String(), tc.want)) {
			t.Errorf("picker %s status=%d want=%d body=%s", tc.target, rec.Code, tc.status, rec.Body.String())
		}
		if strings.Contains(rec.Body.String(), ".hidden") {
			t.Errorf("picker exposed hidden file: %s", rec.Body.String())
		}
	}

	for _, tc := range []struct {
		method string
		body   string
		status int
	}{
		{http.MethodGet, `{}`, http.StatusMethodNotAllowed},
		{http.MethodPost, `{`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt"],"selection":"invalid"}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt"],"mime_prefixes":["INVALID"]}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":[]}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt","/media/image.png"]}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/../media/readme.txt"]}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/missing"]}`, http.StatusNotFound},
		{http.MethodPost, `{"paths":["/media/folder"],"selection":"files"}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt"],"selection":"folders"}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt"],"selection":"files","mime_prefixes":["image/"]}`, http.StatusBadRequest},
		{http.MethodPost, `{"paths":["/media/readme.txt","/media/image.png"],"selection":"files","multiple":true}`, http.StatusOK},
	} {
		rec := filesCall(t, app, app.handlePickerValidate, tc.method, "/api/files/picker/validate", []byte(tc.body), "alice")
		if rec.Code != tc.status {
			t.Errorf("validate %s body=%s status=%d want=%d response=%s", tc.method, tc.body, rec.Code, tc.status, rec.Body.String())
		}
	}

	rec := filesCall(t, app, app.handleUpload, http.MethodGet, "/api/files/upload?path=/media", nil, "alice")
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("upload method status=%d", rec.Code)
	}
	rec = filesCall(t, app, app.handleUpload, http.MethodPost, "/api/files/upload?path=/media", []byte("not multipart"), "alice")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid multipart status=%d", rec.Code)
	}

	upload := func(filename, content string) *httptest.ResponseRecorder {
		var body bytes.Buffer
		writer := multipart.NewWriter(&body)
		part, err := writer.CreateFormFile("file", filename)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := part.Write([]byte(content)); err != nil {
			t.Fatal(err)
		}
		if err := writer.Close(); err != nil {
			t.Fatal(err)
		}
		req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/upload?path=/media", &body), "alice")
		req.Header.Set("Content-Type", writer.FormDataContentType())
		response := httptest.NewRecorder()
		app.handleUpload(response, req)
		return response
	}
	if rec = upload("new.txt", "first"); rec.Code != http.StatusOK {
		t.Fatalf("new upload status=%d body=%s", rec.Code, rec.Body.String())
	}
	if rec = upload("new.txt", "second"); rec.Code != http.StatusOK {
		t.Fatalf("overwrite upload status=%d body=%s", rec.Code, rec.Body.String())
	}
	if data, err := app.ReadFile(ctx, "alice", "/media/new.txt"); err != nil || string(data) != "second" {
		t.Fatalf("uploaded data=%q err=%v", data, err)
	}
	if versions, err := app.VersionIDs(ctx, "alice", "/media/new.txt"); err != nil || len(versions) == 0 {
		t.Fatalf("overwrite versions=%v err=%v", versions, err)
	}
}
