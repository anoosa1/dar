package files

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"
)

func editorTestRequest(t *testing.T, method, vpath, body, etag, uid string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, "/api/files/editor?path="+url.QueryEscape(vpath), strings.NewReader(body))
	req = withTestUser(req, uid)
	if body != "" {
		req.Header.Set("Content-Type", "application/json")
	}
	if etag != "" {
		req.Header.Set("If-Match", etag)
	}
	rec := httptest.NewRecorder()
	testApp.handleEditor(rec, req)
	return rec
}

func decodeEditorDocument(t *testing.T, rec *httptest.ResponseRecorder) editorDocument {
	t.Helper()
	var document editorDocument
	if err := json.Unmarshal(rec.Body.Bytes(), &document); err != nil {
		t.Fatalf("decode editor response: %v body=%q", err, rec.Body.String())
	}
	return document
}

func TestEditorLoadsUTF8MarkdownAndRejectsUnsafeContent(t *testing.T) {
	withDataRoot(t)
	if err := os.MkdirAll(testApp.realPath("alice", "/docs"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(testApp.realPath("bob", "/docs"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/docs/readme.md"), []byte("# Hello\n"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/docs/LICENSE"), []byte("plain text"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/docs/binary.dat"), []byte{'a', 0, 'b'}, 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/docs/large.txt"), bytes.Repeat([]byte("x"), editorFileLimit+1), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("bob", "/docs/readme.md"), []byte("bob"), 0600); err != nil {
		t.Fatal(err)
	}

	rec := editorTestRequest(t, http.MethodGet, "/docs/readme.md", "", "", "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("markdown status=%d body=%s", rec.Code, rec.Body.String())
	}
	document := decodeEditorDocument(t, rec)
	if !document.Markdown || document.Content != "# Hello\n" || document.ETag == "" {
		t.Fatalf("markdown document=%+v", document)
	}
	if rec.Header().Get("ETag") != `"`+document.ETag+`"` || rec.Header().Get("Cache-Control") != "no-store" {
		t.Fatalf("editor headers=%v", rec.Header())
	}

	rec = editorTestRequest(t, http.MethodGet, "/docs/LICENSE", "", "", "alice")
	if rec.Code != http.StatusOK || decodeEditorDocument(t, rec).Markdown {
		t.Fatalf("extensionless text status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = editorTestRequest(t, http.MethodGet, "/docs/binary.dat", "", "", "alice")
	if rec.Code != http.StatusUnsupportedMediaType {
		t.Fatalf("binary status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = editorTestRequest(t, http.MethodGet, "/docs/large.txt", "", "", "alice")
	if rec.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("large status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = editorTestRequest(t, http.MethodGet, "/docs/readme.md", "", "", "bob")
	if rec.Code != http.StatusOK || decodeEditorDocument(t, rec).Content != "bob" {
		t.Fatalf("user isolation status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = editorTestRequest(t, http.MethodGet, "/docs/../readme.md", "", "", "alice")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("unclean path status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestEditorGuardedSaveRenameVersionsAndMetadata(t *testing.T) {
	withDataRoot(t)
	if err := testApp.WriteFile(context.Background(), "alice", "/draft.md", []byte("old")); err != nil {
		t.Fatal(err)
	}
	if err := testApp.SetFavorite(context.Background(), "alice", "/draft.md", true); err != nil {
		t.Fatal(err)
	}
	current := decodeEditorDocument(t, editorTestRequest(t, http.MethodGet, "/draft.md", "", "", "alice"))

	rec := editorTestRequest(t, http.MethodPut, "/draft.md", `{"content":"new"}`, "", "alice")
	if rec.Code != http.StatusPreconditionRequired {
		t.Fatalf("missing precondition status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = editorTestRequest(t, http.MethodPut, "/draft.md", `{"content":"stale"}`, strings.Repeat("0", 64), "alice")
	if rec.Code != http.StatusPreconditionFailed {
		t.Fatalf("stale status=%d body=%s", rec.Code, rec.Body.String())
	}
	server := decodeEditorDocument(t, rec)
	if server.Content != "old" || server.ETag != current.ETag {
		t.Fatalf("conflict document=%+v", server)
	}

	rec = editorTestRequest(t, http.MethodPut, "/draft.md", `{"content":"# Updated\n","name":"renamed.markdown"}`, current.ETag, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("save status=%d body=%s", rec.Code, rec.Body.String())
	}
	saved := decodeEditorDocument(t, rec)
	if saved.Path != "/renamed.markdown" || !saved.Markdown || saved.Content != "# Updated\n" {
		t.Fatalf("saved document=%+v", saved)
	}
	if _, err := os.Stat(testApp.realPath("alice", "/draft.md")); !os.IsNotExist(err) {
		t.Fatalf("old path still exists: %v", err)
	}
	if data, err := os.ReadFile(testApp.realPath("alice", "/renamed.markdown")); err != nil || string(data) != "# Updated\n" {
		t.Fatalf("renamed data=%q err=%v", data, err)
	}
	if favorite, err := testApp.Favorite(context.Background(), "alice", "/renamed.markdown"); err != nil || !favorite {
		t.Fatalf("favorite moved=%v err=%v", favorite, err)
	}
	versions, err := testApp.Versions(context.Background(), "alice", "/renamed.markdown")
	if err != nil || len(versions) != 1 {
		t.Fatalf("versions=%+v err=%v", versions, err)
	}
	version, err := testApp.ReadVersion(context.Background(), "alice", "/renamed.markdown", versions[0].ID)
	if err != nil || string(version) != "old" {
		t.Fatalf("version=%q err=%v", version, err)
	}
}

func TestEditorRenameCollisionLockAndTouchCollision(t *testing.T) {
	withDataRoot(t)
	if err := testApp.WriteFile(context.Background(), "alice", "/one.txt", []byte("one")); err != nil {
		t.Fatal(err)
	}
	if err := testApp.WriteFile(context.Background(), "alice", "/two.txt", []byte("two")); err != nil {
		t.Fatal(err)
	}
	current := decodeEditorDocument(t, editorTestRequest(t, http.MethodGet, "/one.txt", "", "", "alice"))
	rec := editorTestRequest(t, http.MethodPut, "/one.txt", `{"content":"one","name":"two.txt"}`, current.ETag, "alice")
	if rec.Code != http.StatusConflict {
		t.Fatalf("collision status=%d body=%s", rec.Code, rec.Body.String())
	}

	token, err := testApp.acquireLeases(context.Background(), "alice", []string{"/one.txt"}, time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	rec = editorTestRequest(t, http.MethodPut, "/one.txt", `{"content":"locked"}`, current.ETag, "alice")
	testApp.releaseLeases(context.Background(), token)
	if rec.Code != http.StatusLocked {
		t.Fatalf("locked status=%d body=%s", rec.Code, rec.Body.String())
	}

	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/touch?path=/&name=one.txt", nil), "alice")
	rec = httptest.NewRecorder()
	testApp.handleTouch(rec, req)
	if rec.Code != http.StatusConflict {
		t.Fatalf("touch collision status=%d body=%s", rec.Code, rec.Body.String())
	}
	data, err := os.ReadFile(testApp.realPath("alice", "/one.txt"))
	if err != nil || string(data) != "one" {
		t.Fatalf("touch changed existing file=%q err=%v", data, err)
	}
}

func TestPickerMIMEPrefixesFilterAndValidate(t *testing.T) {
	withDataRoot(t)
	if err := os.MkdirAll(testApp.realPath("alice", "/media/sub"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/media/photo.png"), []byte{0x89, 'P', 'N', 'G'}, 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/media/readme.txt"), []byte("text"), 0600); err != nil {
		t.Fatal(err)
	}
	req := withTestUser(httptest.NewRequest(http.MethodGet, "/api/files/picker?path=/media&selection=files&mime_prefix=image/", nil), "alice")
	rec := httptest.NewRecorder()
	testApp.handlePicker(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("picker status=%d body=%s", rec.Code, rec.Body.String())
	}
	var listed FileResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &listed); err != nil {
		t.Fatal(err)
	}
	if len(listed.Files) != 2 || listed.Files[0].Name != "sub" || listed.Files[1].Name != "photo.png" {
		t.Fatalf("filtered files=%+v", listed.Files)
	}

	body := `{"paths":["/media/readme.txt"],"selection":"files","multiple":false,"mime_prefixes":["image/"]}`
	req = withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/picker/validate", strings.NewReader(body)), "alice")
	rec = httptest.NewRecorder()
	testApp.handlePickerValidate(rec, req)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("validation status=%d body=%s", rec.Code, rec.Body.String())
	}
}

func TestPickerAllowsRootFolderDestinationAndUsesSharedTokens(t *testing.T) {
	withDataRoot(t)
	body := `{"paths":["/"],"selection":"folders","multiple":false,"mime_prefixes":[]}`
	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/picker/validate", strings.NewReader(body)), "alice")
	rec := httptest.NewRecorder()
	testApp.handlePickerValidate(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("root validation status=%d body=%s", rec.Code, rec.Body.String())
	}
	var result struct {
		Files []FileEntry `json:"files"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil || len(result.Files) != 1 || result.Files[0].Path != "/" || !result.Files[0].IsDir {
		t.Fatalf("root result=%+v err=%v", result, err)
	}

	asset := httptest.NewRecorder()
	handleAsset(asset, httptest.NewRequest(http.MethodGet, "/files/assets/picker.js", nil))
	if asset.Code != http.StatusOK {
		t.Fatalf("picker asset status=%d", asset.Code)
	}
	script := asset.Body.String()
	for _, want := range []string{"showModal", "allowCurrent", "excludePaths", "Search this folder", "Choose this folder"} {
		if !strings.Contains(script, want) {
			t.Fatalf("picker asset missing %q", want)
		}
	}
	for _, forbidden := range []string{"createElement('style')", `style="`} {
		if strings.Contains(script, forbidden) {
			t.Fatalf("picker asset still uses injected styling: %q", forbidden)
		}
	}
}

func TestFilesPageLoadsSharedEditorBeforeAppAndRendersOverlay(t *testing.T) {
	var page bytes.Buffer
	req := httptest.NewRequest(http.MethodGet, "/files", nil)
	if err := renderPage(&page, req, "Alice", "alice"); err != nil {
		t.Fatal(err)
	}
	html := page.String()
	for _, want := range []string{
		`data-user-id="alice"`,
		`id="files-editor-overlay"`,
		`id="files-create-text-dialog"`,
		`id="files-editor-conflict"`,
		`value="Text document.md"`,
	} {
		if !strings.Contains(html, want) {
			t.Fatalf("files page does not contain %q", want)
		}
	}
	shared := strings.Index(html, `/assets/text-editor.js`)
	picker := strings.Index(html, `/files/assets/picker.js`)
	appScript := strings.Index(html, `/files/assets/app.js`)
	if shared < 0 || picker < shared || appScript < picker {
		t.Fatalf("editor assets are in the wrong order: shared=%d picker=%d app=%d", shared, picker, appScript)
	}
}
