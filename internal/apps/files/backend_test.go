package files

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
)

var testApp = New(Config{})

func withTestUser(r *http.Request, id string) *http.Request {
	return r.WithContext(context.WithValue(r.Context(), server.UserContextKey, &auth.User{ID: id}))
}

func withDataRoot(t *testing.T) string {
	t.Helper()
	old := testApp.dataRoot
	root := t.TempDir()
	testApp.dataRoot = root
	database, err := db.Open(filepath.Join(root, "files.sqlite"), Migrations())
	if err != nil {
		t.Fatal(err)
	}
	oldDB := testApp.db
	testApp.db = database
	t.Cleanup(func() { testApp.dataRoot = old; testApp.db = oldDB; database.Close() })
	return root
}

func TestRealPathStaysUnderUserRoot(t *testing.T) {
	root := withDataRoot(t)
	got := testApp.realPath("alice", "/../../etc/passwd")
	wantRoot := filepath.Join(root, "alice", "home")
	rel, err := filepath.Rel(wantRoot, got)
	if err != nil {
		t.Fatal(err)
	}
	if rel == ".." || strings.HasPrefix(rel, "../") {
		t.Fatalf("realPath escaped user root: got %q want under %q", got, wantRoot)
	}
}

func TestListUsesAuthenticatedUserHeader(t *testing.T) {
	withDataRoot(t)
	if err := os.MkdirAll(testApp.realPath("alice", "/docs"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(testApp.realPath("bob", "/docs"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/docs/alice.txt"), []byte("alice"), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("bob", "/docs/bob.txt"), []byte("bob"), 0644); err != nil {
		t.Fatal(err)
	}

	req := httptest.NewRequest(http.MethodGet, "/list?path=/docs", nil)
	req = withTestUser(req, "alice")
	rec := httptest.NewRecorder()
	testApp.handleList(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", rec.Code, rec.Body.String())
	}
	var out FileResponse
	if err := json.Unmarshal(rec.Body.Bytes(), &out); err != nil {
		t.Fatal(err)
	}
	if len(out.Files) != 1 || out.Files[0].Name != "alice.txt" {
		t.Fatalf("unexpected files for alice: %+v", out.Files)
	}
}

func TestNewTagUsesCatppuccinPinkByDefault(t *testing.T) {
	withDataRoot(t)
	req := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/tags", strings.NewReader(`{"name":"Important"}`)), "alice")
	rec := httptest.NewRecorder()
	testApp.handleTags(rec, req)
	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d body = %s", rec.Code, rec.Body.String())
	}
	var tag Tag
	if err := json.Unmarshal(rec.Body.Bytes(), &tag); err != nil {
		t.Fatal(err)
	}
	if tag.Color != filesTheme.Pink {
		t.Fatalf("tag color = %q, want %q", tag.Color, filesTheme.Pink)
	}
}

func TestNextcloudWebDAVPathMapsToUserFiles(t *testing.T) {
	withDataRoot(t)
	req := httptest.NewRequest(http.MethodPut, "/remote.php/dav/files/alice/docs/note.txt", strings.NewReader("hello"))
	req = withTestUser(req, "alice")
	rec := httptest.NewRecorder()
	testApp.handleNextcloudWebDAV(rec, req)
	if rec.Code != http.StatusCreated {
		t.Fatalf("PUT status = %d body = %s", rec.Code, rec.Body.String())
	}
	data, err := os.ReadFile(testApp.realPath("alice", "/docs/note.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "hello" {
		t.Fatalf("file contents = %q", data)
	}
	if _, err := os.Stat(testApp.realPath("bob", "/docs/note.txt")); !os.IsNotExist(err) {
		t.Fatalf("bob should not see alice file, stat err = %v", err)
	}
}

func TestRealPathDocsMapsUnderHome(t *testing.T) {
	root := withDataRoot(t)
	got := testApp.realPath("alice", "/docs/a.txt")
	want := filepath.Join(root, "alice", "home", "docs", "a.txt")
	if got != want {
		t.Fatalf("realPath = %q want %q", got, want)
	}
}

func TestDeleteRestoreUsesConfiguredTrashPath(t *testing.T) {
	withDataRoot(t)
	if err := os.MkdirAll(testApp.realPath("alice", "/work"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(testApp.realPath("alice", "/work/a.txt"), []byte("hello"), 0644); err != nil {
		t.Fatal(err)
	}

	delReq := httptest.NewRequest(http.MethodPost, "/delete?path=/work/a.txt&trash_path=/.local/share/Trash", nil)
	delReq = withTestUser(delReq, "alice")
	delRec := httptest.NewRecorder()
	testApp.handleDelete(delRec, delReq)
	if delRec.Code != http.StatusOK {
		t.Fatalf("delete status = %d body = %s", delRec.Code, delRec.Body.String())
	}
	if _, err := os.Stat(testApp.realPath("alice", "/work/a.txt")); !os.IsNotExist(err) {
		t.Fatalf("source should be gone, err = %v", err)
	}
	trashEntries, err := os.ReadDir(testApp.realPath("alice", "/.local/share/Trash"))
	if err != nil {
		t.Fatal(err)
	}
	var trashID string
	for _, entry := range trashEntries {
		if !strings.HasSuffix(entry.Name(), ".orig") {
			trashID = entry.Name()
			break
		}
	}
	if trashID == "" {
		t.Fatal("deleted file was not moved into configured trash")
	}

	restoreReq := httptest.NewRequest(http.MethodPost, "/restore?id="+trashID+"&trash_path=/.local/share/Trash", nil)
	restoreReq = withTestUser(restoreReq, "alice")
	restoreRec := httptest.NewRecorder()
	testApp.handleRestore(restoreRec, restoreReq)
	if restoreRec.Code != http.StatusOK {
		t.Fatalf("restore status = %d body = %s", restoreRec.Code, restoreRec.Body.String())
	}
	data, err := os.ReadFile(testApp.realPath("alice", "/work/a.txt"))
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != "hello" {
		t.Fatalf("restored data = %q", data)
	}
}
