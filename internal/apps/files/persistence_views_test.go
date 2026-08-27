package files

import (
	"context"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestFilesCompatibilityPersistenceAndVirtualViews(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.MkdirAll(ctx, "alice", "/Documents/Folder"); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/favorite.txt", []byte("favorite")); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Documents/tagged.txt", []byte("tagged")); err != nil {
		t.Fatal(err)
	}

	trashDir := app.trashStoreDir("alice", "/custom-trash")
	if info, err := os.Stat(trashDir); err != nil || !info.IsDir() {
		t.Fatalf("trash store info=%v err=%v", info, err)
	}

	app.saveFavorites("alice", map[string]bool{"/Documents/favorite.txt": true, "/ignored": false})
	favorites := app.loadFavorites("alice")
	if !favorites["/Documents/favorite.txt"] || favorites["/ignored"] {
		t.Fatalf("favorites=%v", favorites)
	}
	app.saveTags("alice", map[string]Tag{"important": {ID: "important", Name: "Important", Color: "#ff00ff"}})
	if tags := app.loadTags("alice"); tags["important"].Name != "Important" {
		t.Fatalf("tags=%v", tags)
	}
	app.saveFileTags("alice", map[string][]string{"/Documents/tagged.txt": {"important"}})
	if fileTags := app.loadFileTags("alice"); len(fileTags["/Documents/tagged.txt"]) != 1 {
		t.Fatalf("file tags=%v", fileTags)
	}

	for _, target := range []string{
		"/api/files/list?filter=favorites",
		"/api/files/list?filter=tag&tag=important",
	} {
		rec := filesCall(t, app, app.handleList, http.MethodGet, target, nil, "alice")
		if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), ".txt") {
			t.Fatalf("%s status=%d body=%s", target, rec.Code, rec.Body.String())
		}
	}

	shares := []UserShare{{
		ID: "share-one", FromUser: "alice", ToUser: "bob", Path: "/Documents/favorite.txt",
		Permission: "read", Created: time.Now().Unix(),
	}}
	app.saveUserShares("alice", shares)
	if got := app.loadUserShares("alice"); len(got) != 1 || got[0].ID != "share-one" {
		t.Fatalf("user shares=%v", got)
	}
	if got := app.loadIncomingShares("bob"); len(got) != 1 || got[0].FromUser != "alice" {
		t.Fatalf("incoming shares=%v", got)
	}
	rec := filesCall(t, app, app.handleList, http.MethodGet, "/api/files/list?path=/Shared", nil, "bob")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "favorite.txt") {
		t.Fatalf("shared view status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleList, http.MethodGet, "/api/files/list?path=/", nil, "bob")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"Shared"`) {
		t.Fatalf("root shared status=%d body=%s", rec.Code, rec.Body.String())
	}

	rec = filesCall(t, app, app.handleDelete, http.MethodPost, "/api/files/delete?path=/Documents/tagged.txt", nil, "alice")
	if rec.Code != http.StatusOK {
		t.Fatalf("trash status=%d body=%s", rec.Code, rec.Body.String())
	}
	rec = filesCall(t, app, app.handleList, http.MethodGet, "/api/files/list?path=/.local/share/Trash", nil, "alice")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "tagged.txt") {
		t.Fatalf("trash view status=%d body=%s", rec.Code, rec.Body.String())
	}

	var tagged []FileEntry
	app.addTaggedFiles("alice", app.userHome("alice"), "", app.loadFileTags("alice"), "important", app.loadFavorites("alice"), &tagged)
	if len(tagged) != 0 {
		t.Fatalf("deleted tagged file still listed: %v", tagged)
	}
	health := filesCall(t, app, app.handleHealth, http.MethodGet, "/api/files/health", nil, "alice")
	if health.Code != http.StatusOK || !strings.Contains(health.Body.String(), `"status":"ok"`) {
		t.Fatalf("health status=%d body=%s", health.Code, health.Body.String())
	}

	app.saveFavorites("alice", map[string]bool{})
	app.saveTags("alice", map[string]Tag{})
	app.saveFileTags("alice", map[string][]string{})
	app.saveUserShares("alice", []UserShare{})
	if len(app.loadFavorites("alice")) != 0 || len(app.loadTags("alice")) != 0 || len(app.loadFileTags("alice")) != 0 || len(app.loadUserShares("alice")) != 0 {
		t.Fatal("compatibility stores did not clear")
	}
	if rel, err := filepath.Rel(app.userHome("alice"), app.trashStoreDir("alice", "/../safe")); err != nil || strings.HasPrefix(rel, "..") {
		t.Fatalf("trash path escaped: rel=%q err=%v", rel, err)
	}
}
