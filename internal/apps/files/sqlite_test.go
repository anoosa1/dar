package files

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/theme"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

func sqliteTestApp(t testing.TB) *App {
	t.Helper()
	root := t.TempDir()
	migrations := append(preferences.Migrations(), Migrations()...)
	database, err := db.Open(filepath.Join(root, "files.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { database.Close() })
	return New(Config{DataRoot: filepath.Join(root, "data"), DB: database, SharePasswordKey: bytes.Repeat([]byte{0x42}, 32), SecureCookies: true})
}

func createTestShare(t *testing.T, app *App, drop bool, password string, expires int64) Share {
	t.Helper()
	if err := app.MkdirAll(context.Background(), "alice", "/drop"); err != nil {
		t.Fatal(err)
	}
	body, _ := json.Marshal(map[string]any{"path": "/drop", "drop": drop, "password": password, "expires": expires})
	r := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/share", bytes.NewReader(body)), "alice")
	w := httptest.NewRecorder()
	app.handleShare(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("create share status=%d body=%s", w.Code, w.Body.String())
	}
	var share Share
	if err := json.NewDecoder(w.Body).Decode(&share); err != nil {
		t.Fatal(err)
	}
	return share
}

func TestPublicShareUnlockRateLimitExpiryAndNoURLPassword(t *testing.T) {
	app := sqliteTestApp(t)
	share := createTestShare(t, app, true, "correct horse", 0)
	if len(share.Token) != 64 {
		t.Fatalf("token length=%d", len(share.Token))
	}
	get := httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"?password=correct+horse", nil)
	w := httptest.NewRecorder()
	app.servePublicShare(w, get, share.Token)
	if !strings.Contains(w.Body.String(), "Unlock share") {
		t.Fatal("password in URL unlocked the share")
	}
	for _, want := range []string{
		`name="theme-color" content="#1e1e2e"`,
		`background:#1e1e2e`,
		`color:#cdd6f4`,
		`height:100dvh;overflow:hidden`,
		`max-height:100%;margin:0 auto;overflow-y:auto`,
		`overscroll-behavior:contain;scrollbar-gutter:stable`,
	} {
		if !strings.Contains(w.Body.String(), want) {
			t.Errorf("themed unlock page does not contain %q", want)
		}
	}
	for i := 0; i < 6; i++ {
		r := httptest.NewRequest(http.MethodPost, "/s/"+share.Token+"/unlock", strings.NewReader("password=wrong"))
		r.Header.Set("Content-Type", "application/x-www-form-urlencoded")
		r.RemoteAddr = "198.51.100.20:1234"
		w = httptest.NewRecorder()
		app.unlockPublicShare(w, r, share.Token)
		want := http.StatusUnauthorized
		if i == 5 {
			want = http.StatusTooManyRequests
		}
		if w.Code != want {
			t.Fatalf("attempt %d status=%d body=%s", i+1, w.Code, w.Body.String())
		}
	}
	unlock := httptest.NewRequest(http.MethodPost, "/s/"+share.Token+"/unlock", strings.NewReader("password=correct+horse"))
	unlock.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	unlock.RemoteAddr = "198.51.100.21:1234"
	w = httptest.NewRecorder()
	app.unlockPublicShare(w, unlock, share.Token)
	if w.Code != http.StatusSeeOther {
		t.Fatalf("correct unlock status=%d body=%s", w.Code, w.Body.String())
	}
	cookies := w.Result().Cookies()
	if len(cookies) != 1 || !cookies[0].HttpOnly || !cookies[0].Secure || cookies[0].SameSite != http.SameSiteStrictMode {
		t.Fatalf("unlock cookie=%+v", cookies)
	}
	unlockedPage := httptest.NewRequest(http.MethodGet, "/s/"+share.Token, nil)
	unlockedPage.AddCookie(cookies[0])
	w = httptest.NewRecorder()
	app.servePublicShare(w, unlockedPage, share.Token)
	if w.Code != http.StatusOK || !strings.Contains(w.Body.String(), "File Drop") {
		t.Fatalf("unlocked drop status=%d body=%s", w.Code, w.Body.String())
	}
	if !strings.Contains(w.Body.String(), `background:#f5c2e7`) {
		t.Fatal("unlocked drop page is missing the Catppuccin primary action")
	}
	if !strings.Contains(w.Body.String(), `height:100dvh;overflow:hidden`) {
		t.Fatal("file drop page is missing the fixed viewport contract")
	}

	expired := createTestShare(t, app, true, "", time.Now().Add(time.Minute).Unix())
	if err := app.withConn(context.Background(), func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE files_public_shares SET expires=? WHERE token=?`, &sqlitex.ExecOptions{Args: []any{time.Now().Add(-time.Second).Unix(), expired.Token}})
	}); err != nil {
		t.Fatal(err)
	}
	w = httptest.NewRecorder()
	app.servePublicShare(w, httptest.NewRequest(http.MethodGet, "/s/"+expired.Token, nil), expired.Token)
	if w.Code != http.StatusGone {
		t.Fatalf("expired share status=%d", w.Code)
	}
	w = httptest.NewRecorder()
	app.handlePublicShareUpload(w, httptest.NewRequest(http.MethodPost, "/s/"+expired.Token+"/upload", nil), expired.Token)
	if w.Code != http.StatusGone {
		t.Fatalf("expired upload status=%d", w.Code)
	}
	w = httptest.NewRecorder()
	app.servePublicShare(w, httptest.NewRequest(http.MethodGet, "/s/missing", nil), "missing")
	if w.Code != http.StatusNotFound {
		t.Fatalf("missing share status=%d", w.Code)
	}
}

func TestPublicShareWallpaperIsScopedToValidShareToken(t *testing.T) {
	app := sqliteTestApp(t)
	service := theme.NewService(app.db)
	service.BindFiles(app)
	app.appearance = service

	img := image.NewNRGBA(image.Rect(0, 0, 3, 3))
	for y := 0; y < 3; y++ {
		for x := 0; x < 3; x++ {
			img.SetNRGBA(x, y, color.NRGBA{R: 80, G: 130, B: 210, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, img); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(context.Background(), "alice", "/Wallpapers/share.png", encoded.Bytes()); err != nil {
		t.Fatal(err)
	}
	if _, err := service.Save(context.Background(), "alice", theme.Draft{
		WallpaperPath: "/Wallpapers/share.png", WallpaperVisibility: 60,
	}); err != nil {
		t.Fatal(err)
	}
	share := createTestShare(t, app, true, "", 0)

	page := httptest.NewRecorder()
	app.handlePublicShareRoute(page, httptest.NewRequest(http.MethodGet, "/s/"+share.Token, nil))
	if page.Code != http.StatusOK || !strings.Contains(page.Body.String(), "/s/"+share.Token+"/appearance/wallpaper") {
		t.Fatalf("public page status=%d body=%s", page.Code, page.Body.String())
	}
	wallpaper := httptest.NewRecorder()
	app.handlePublicShareRoute(wallpaper, httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"/appearance/wallpaper", nil))
	if wallpaper.Code != http.StatusOK || !bytes.Equal(wallpaper.Body.Bytes(), encoded.Bytes()) {
		t.Fatalf("wallpaper status=%d content-type=%s", wallpaper.Code, wallpaper.Header().Get("Content-Type"))
	}
	missing := httptest.NewRecorder()
	app.handlePublicShareRoute(missing, httptest.NewRequest(http.MethodGet, "/s/not-a-token/appearance/wallpaper", nil))
	if missing.Code != http.StatusNotFound {
		t.Fatalf("invalid token wallpaper status=%d", missing.Code)
	}
}

func TestPublicFolderShareUsesOwnerThemeAndKeepsFilesRaw(t *testing.T) {
	app := sqliteTestApp(t)
	if err := app.WriteFile(context.Background(), "alice", "/drop/read me.txt", []byte("raw file body")); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(context.Background(), "alice", "/drop/nested/deeper/item.txt", []byte("nested body")); err != nil {
		t.Fatal(err)
	}
	share := createTestShare(t, app, false, "", 0)

	page := httptest.NewRecorder()
	app.handlePublicShareRoute(page, httptest.NewRequest(http.MethodGet, "/s/"+share.Token, nil))
	if page.Code != http.StatusOK {
		t.Fatalf("folder page status=%d body=%s", page.Code, page.Body.String())
	}
	for _, want := range []string{
		"Shared files",
		"read me.txt",
		`href="/s/` + share.Token + `/read%20me.txt"`,
		`background:#1e1e2e`,
		`height:100dvh;overflow:hidden`,
		`max-height:100%;margin:0 auto;overflow-y:auto`,
	} {
		if !strings.Contains(page.Body.String(), want) {
			t.Errorf("folder page does not contain %q", want)
		}
	}

	file := httptest.NewRecorder()
	app.handlePublicShareRoute(file, httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"/read%20me.txt", nil))
	if file.Code != http.StatusOK || file.Body.String() != "raw file body" {
		t.Fatalf("raw file status=%d body=%q", file.Code, file.Body.String())
	}
	nested := httptest.NewRecorder()
	app.handlePublicShareRoute(nested, httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"/nested/deeper", nil))
	if nested.Code != http.StatusOK || !strings.Contains(nested.Body.String(), "Parent folder") ||
		!strings.Contains(nested.Body.String(), `/s/`+share.Token+`/nested`) ||
		!strings.Contains(nested.Body.String(), "item.txt") {
		t.Fatalf("nested folder status=%d body=%q", nested.Code, nested.Body.String())
	}

	traversal := httptest.NewRecorder()
	app.handlePublicShareRoute(traversal, httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"/../secret", nil))
	if traversal.Code != http.StatusNotFound {
		t.Fatalf("traversal status=%d body=%q", traversal.Code, traversal.Body.String())
	}
}

func TestPublicDropRejectsMaliciousFilenameAndAcceptsUnlockedUpload(t *testing.T) {
	app := sqliteTestApp(t)
	share := createTestShare(t, app, true, "", 0)
	for _, name := range []string{"../escape.txt", `..\\escape.txt`, "safe.txt"} {
		var body bytes.Buffer
		mw := multipart.NewWriter(&body)
		part, err := mw.CreateFormFile("file", name)
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write([]byte("payload"))
		_ = mw.Close()
		r := httptest.NewRequest(http.MethodPost, "/s/"+share.Token+"/upload", &body)
		r.Header.Set("Content-Type", mw.FormDataContentType())
		w := httptest.NewRecorder()
		app.handlePublicShareUpload(w, r, share.Token)
		if name == "safe.txt" {
			if w.Code != http.StatusOK {
				t.Fatalf("safe upload status=%d body=%s", w.Code, w.Body.String())
			}
			if !strings.Contains(w.Body.String(), `Files uploaded`) || !strings.Contains(w.Body.String(), `height:100dvh;overflow:hidden`) {
				t.Fatalf("completion page is missing the fixed viewport contract: %s", w.Body.String())
			}
			data, err := os.ReadFile(app.realPath("alice", "/drop/safe.txt"))
			if err != nil || string(data) != "payload" {
				t.Fatalf("safe upload=%q err=%v", data, err)
			}
		} else if w.Code != http.StatusBadRequest {
			t.Fatalf("malicious name %q status=%d", name, w.Code)
		}
	}
	if _, err := os.Stat(filepath.Join(app.dataRoot, "alice", "home", "escape.txt")); !os.IsNotExist(err) {
		t.Fatalf("malicious upload escaped target: %v", err)
	}
}

func TestPublicShareRoutingValidationAndDeletion(t *testing.T) {
	app := sqliteTestApp(t)
	for _, body := range []string{`{}`, `{"path":"/file.txt","expires":1}`} {
		r := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/share", strings.NewReader(body)), "alice")
		w := httptest.NewRecorder()
		app.handleShare(w, r)
		if w.Code != http.StatusBadRequest {
			t.Fatalf("invalid share %s status=%d", body, w.Code)
		}
	}
	if err := app.WriteFile(context.Background(), "alice", "/file.txt", []byte("shared content")); err != nil {
		t.Fatal(err)
	}
	body := `{"path":"/file.txt"}`
	r := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/share", strings.NewReader(body)), "alice")
	w := httptest.NewRecorder()
	app.handleShare(w, r)
	var share Share
	_ = json.NewDecoder(w.Body).Decode(&share)
	if share.Token == "" {
		t.Fatalf("create status=%d body=%s", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	app.handlePublicShareRoute(w, httptest.NewRequest(http.MethodGet, "/s/"+share.Token, nil))
	if w.Code != http.StatusOK || w.Body.String() != "shared content" {
		t.Fatalf("public download status=%d body=%q", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	app.handlePublicShareRoute(w, httptest.NewRequest(http.MethodGet, "/s/"+share.Token+"/unlock", nil))
	if w.Code != http.StatusNotFound {
		t.Fatalf("unsafe unlock method status=%d", w.Code)
	}
	deleteRequest := withTestUser(httptest.NewRequest(http.MethodDelete, "/api/files/share?token="+share.Token, nil), "alice")
	w = httptest.NewRecorder()
	app.handleShare(w, deleteRequest)
	if w.Code != http.StatusOK {
		t.Fatalf("delete status=%d", w.Code)
	}
	if _, err := app.shareByToken(context.Background(), share.Token); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("deleted share error=%v", err)
	}
	w = httptest.NewRecorder()
	app.handleShare(w, withTestUser(httptest.NewRequest(http.MethodGet, "/api/files/share", nil), "alice"))
	if w.Code != http.StatusMethodNotAllowed {
		t.Fatalf("share GET status=%d", w.Code)
	}
}

func TestFilesClosedDatabaseStorageBoundaryMatrix(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.WriteFile(ctx, "alice", "/file.txt", []byte("body")); err != nil {
		t.Fatal(err)
	}
	if err := app.db.Close(); err != nil {
		t.Fatal(err)
	}
	check := func(name string, err error) {
		t.Helper()
		if err == nil {
			t.Errorf("%s unexpectedly succeeded with closed database", name)
		}
	}
	_, err := app.ReadFile(ctx, "alice", "/Shared/missing")
	check("read shared", err)
	_, err = app.Stat(ctx, "alice", "/Shared/missing")
	check("stat shared", err)
	check("walk shared", app.Walk(ctx, "alice", "/Shared/missing", func(string, os.FileInfo, error) error { return nil }))
	check("mkdir", app.MkdirAll(ctx, "alice", "/folder"))
	check("create file", app.CreateFile(ctx, "alice", "/new.txt"))
	check("create symlink", app.CreateSymlink(ctx, "alice", "/link", "file.txt"))
	check("write stream", app.WriteStream(ctx, "alice", "/stream.txt", strings.NewReader("stream")))
	check("rename", app.Rename(ctx, "alice", "/file.txt", "/renamed.txt"))
	check("remove", app.Remove(ctx, "alice", "/file.txt"))
	check("set modtime", app.SetModTime(ctx, "alice", "/file.txt", time.Unix(1, 0)))
	_, err = app.CreateVersion(ctx, "alice", "/file.txt")
	check("create version", err)
	_, err = app.ReadVersion(ctx, "alice", "/file.txt", "missing")
	check("read version", err)
	check("prune versions", app.PruneVersions(ctx, "alice", "/file.txt", 1))
	check("snapshot", app.Snapshot(ctx, "alice", "/file.txt"))
}

func TestPublicShareDeniedAndLimiterBranches(t *testing.T) {
	app := sqliteTestApp(t)
	passwordShare := createTestShare(t, app, true, "secret", 0)
	w := httptest.NewRecorder()
	app.handlePublicShareUpload(w, httptest.NewRequest(http.MethodPost, "/s/"+passwordShare.Token+"/upload", nil), passwordShare.Token)
	if w.Code != http.StatusForbidden {
		t.Fatalf("locked drop upload status=%d", w.Code)
	}
	w = httptest.NewRecorder()
	badForm := httptest.NewRequest(http.MethodPost, "/s/"+passwordShare.Token+"/unlock", strings.NewReader("%"))
	badForm.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	app.unlockPublicShare(w, badForm, passwordShare.Token)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("invalid unlock form status=%d", w.Code)
	}
	nonDrop := createTestShare(t, app, false, "", 0)
	w = httptest.NewRecorder()
	app.handlePublicShareUpload(w, httptest.NewRequest(http.MethodPost, "/s/"+nonDrop.Token+"/upload", nil), nonDrop.Token)
	if w.Code != http.StatusForbidden {
		t.Fatalf("non-drop upload status=%d", w.Code)
	}
	for _, handler := range []func(http.ResponseWriter, *http.Request, string){app.unlockPublicShare, app.handlePublicShareUpload} {
		w = httptest.NewRecorder()
		handler(w, httptest.NewRequest(http.MethodPost, "/s/missing", nil), "missing")
		if w.Code != http.StatusNotFound {
			t.Fatalf("missing token status=%d", w.Code)
		}
	}
	noKey := New(Config{DataRoot: t.TempDir(), DB: app.db})
	if _, err := noKey.passwordDigest([]byte("salt"), "password"); err == nil {
		t.Fatal("password digest worked without operator key")
	}
	if err := noKey.MkdirAll(context.Background(), "alice", "/drop"); err != nil {
		t.Fatal(err)
	}
	r := withTestUser(httptest.NewRequest(http.MethodPost, "/api/files/share", strings.NewReader(`{"path":"/drop","password":"secret"}`)), "alice")
	w = httptest.NewRecorder()
	noKey.handleShare(w, r)
	if w.Code != http.StatusServiceUnavailable {
		t.Fatalf("password share without key status=%d", w.Code)
	}

	drop := createTestShare(t, app, true, "", 0)
	w = httptest.NewRecorder()
	app.handlePublicShareUpload(w, httptest.NewRequest(http.MethodPost, "/s/"+drop.Token+"/upload", strings.NewReader("not multipart")), drop.Token)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("invalid multipart status=%d", w.Code)
	}
	for i := 0; i < 21; i++ {
		var upload bytes.Buffer
		mw := multipart.NewWriter(&upload)
		part, _ := mw.CreateFormFile("file", fmt.Sprintf("file-%d.txt", i))
		_, _ = part.Write([]byte("x"))
		_ = mw.Close()
		r := httptest.NewRequest(http.MethodPost, "/s/"+drop.Token+"/upload", &upload)
		r.RemoteAddr = "198.51.100.30:1234"
		r.Header.Set("Content-Type", mw.FormDataContentType())
		w = httptest.NewRecorder()
		app.handlePublicShareUpload(w, r, drop.Token)
		if i < 20 && w.Code != http.StatusOK {
			t.Fatalf("upload %d status=%d", i+1, w.Code)
		}
		if i == 20 && w.Code != http.StatusTooManyRequests {
			t.Fatalf("rate-limited upload status=%d", w.Code)
		}
	}
}

func TestConfinedSymlinksAndCopyPreservesLinks(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.WriteFile(ctx, "alice", "/docs/note.txt", []byte("inside")); err != nil {
		t.Fatal(err)
	}
	if err := app.CreateSymlink(ctx, "alice", "/note-link", "/docs/note.txt"); err != nil {
		t.Fatal(err)
	}
	got, err := app.ReadFile(ctx, "alice", "/note-link")
	if err != nil || string(got) != "inside" {
		t.Fatalf("read confined link=%q err=%v", got, err)
	}
	if err := app.copyUserPath(ctx, "alice", "/note-link", "/copied-link"); err != nil {
		t.Fatal(err)
	}
	info, err := os.Lstat(app.realPath("alice", "/copied-link"))
	if err != nil || info.Mode()&os.ModeSymlink == 0 {
		t.Fatalf("copy did not preserve symbolic link: info=%v err=%v", info, err)
	}
	if err := app.CreateSymlink(ctx, "alice", "/bad-link", "/../../etc/passwd"); err == nil {
		t.Fatal("non-normalized absolute target was accepted")
	}

	outside := filepath.Join(t.TempDir(), "secret.txt")
	if err := os.WriteFile(outside, []byte("outside"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, app.realPath("alice", "/host-link")); err != nil {
		t.Fatal(err)
	}
	if _, err := app.ReadFile(ctx, "alice", "/host-link"); err == nil {
		t.Fatal("absolute symlink escaped the user Files root")
	}
}

func TestSharedMountResolvesOwnerAndPermissions(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.WriteFile(ctx, "alice", "/project/readme.txt", []byte("owner")); err != nil {
		t.Fatal(err)
	}
	if err := app.withConn(ctx, func(conn *sqlite.Conn) error {
		for _, share := range []UserShare{
			{ID: "read-share", Path: "/project", FromUser: "alice", ToUser: "bob", Permission: "read"},
			{ID: "write-share", Path: "/project", FromUser: "alice", ToUser: "carol", Permission: "write"},
		} {
			if err := sqlitex.Execute(conn, `INSERT INTO files_user_shares(id,from_user,to_user,path,permission,created_at) VALUES(?,?,?,?,?,?)`, &sqlitex.ExecOptions{
				Args: []any{share.ID, share.FromUser, share.ToUser, share.Path, share.Permission, time.Now().Unix()},
			}); err != nil {
				return err
			}
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	got, err := app.ReadFile(ctx, "bob", "/Shared/read-share/readme.txt")
	if err != nil || string(got) != "owner" {
		t.Fatalf("shared read=%q err=%v", got, err)
	}
	if err := app.WriteFile(ctx, "bob", "/Shared/read-share/blocked.txt", []byte("no")); !errors.Is(err, ErrShareReadOnly) {
		t.Fatalf("read-only share write error=%v", err)
	}
	if err := app.WriteFile(ctx, "carol", "/Shared/write-share/allowed.txt", []byte("yes")); err != nil {
		t.Fatal(err)
	}
	got, err = app.ReadFile(ctx, "alice", "/project/allowed.txt")
	if err != nil || string(got) != "yes" {
		t.Fatalf("owner did not receive shared write=%q err=%v", got, err)
	}
	if err := app.Remove(ctx, "carol", "/Shared/write-share"); !errors.Is(err, ErrSharedRootMutation) {
		t.Fatalf("shared mount root mutation error=%v", err)
	}
	if _, err := app.ReadFile(ctx, "dave", "/Shared/read-share/readme.txt"); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("unrelated user resolved another recipient's share: %v", err)
	}
}

func TestManagedMaildirIsReadOnlyThroughFiles(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	for _, operation := range []func() error{
		func() error {
			return app.WriteFile(ctx, "alice", "/.local/var/mail/alice@example.test/INBOX/new/message", []byte("mail"))
		},
		func() error { return app.MkdirAll(ctx, "alice", "/.local/var/mail/alice@example.test") },
		func() error { return app.Remove(ctx, "alice", "/.local/var/mail") },
	} {
		if err := operation(); !errors.Is(err, ErrManagedPathReadOnly) {
			t.Fatalf("managed Maildir mutation error=%v", err)
		}
	}
}

type failingReader struct{ sent bool }

func (r *failingReader) Read(p []byte) (int, error) {
	if !r.sent {
		r.sent = true
		copy(p, "partial")
		return len("partial"), nil
	}
	return 0, fmt.Errorf("source failed")
}

func TestStreamingWriteCleansPartialFileOnFailure(t *testing.T) {
	app := sqliteTestApp(t)
	err := app.WriteStream(context.Background(), "alice", "/large.bin", &failingReader{})
	if err == nil {
		t.Fatal("source failure was ignored")
	}
	if _, statErr := os.Stat(app.realPath("alice", "/large.bin")); !os.IsNotExist(statErr) {
		t.Fatalf("partial destination exists: %v", statErr)
	}
	temps, _ := filepath.Glob(filepath.Join(filepath.Dir(app.realPath("alice", "/large.bin")), ".dar-write-*"))
	if len(temps) != 0 {
		t.Fatalf("partial temporary files exist: %v", temps)
	}
}

func TestUserStorageQuotaCountsAggregateContent(t *testing.T) {
	base := sqliteTestApp(t)
	app := New(Config{DataRoot: base.dataRoot, DB: base.db, UserQuotaBytes: 8})
	if err := app.WriteFile(context.Background(), "alice", "/first.txt", []byte("12345")); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(context.Background(), "alice", "/second.txt", []byte("6789")); err == nil {
		t.Fatal("aggregate quota allowed excess content")
	}
	if _, err := os.Stat(app.realPath("alice", "/second.txt")); !os.IsNotExist(err) {
		t.Fatalf("quota failure left destination: %v", err)
	}
	if err := app.WriteFile(context.Background(), "alice", "/first.txt", []byte("12345678")); err != nil {
		t.Fatalf("replacement did not subtract old size: %v", err)
	}
}

func TestOperationLeasesOverlapAndRecoverWhenStale(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	token, err := app.acquireLeases(ctx, "alice", []string{"/Notes/work"}, time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = app.acquireLeases(ctx, "alice", []string{"/Notes/work/a.md"}, time.Minute); !errors.Is(err, ErrLocked) {
		t.Fatalf("nested lease error = %v", err)
	}
	if _, err = app.acquireLeases(ctx, "alice", []string{"/Other"}, time.Minute); err != nil {
		t.Fatalf("disjoint lease: %v", err)
	}
	app.releaseLeases(ctx, token)
	stale, err := app.acquireLeases(ctx, "bob", []string{"/Notes"}, time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	_ = app.withConn(ctx, func(c *sqlite.Conn) error {
		return sqlitex.Execute(c, `UPDATE files_operation_leases SET expires_at=0 WHERE token=?`, &sqlitex.ExecOptions{Args: []any{stale}})
	})
	if _, err = app.acquireLeases(ctx, "bob", []string{"/Notes/a.md"}, time.Minute); err != nil {
		t.Fatalf("stale lease was not recovered: %v", err)
	}
}

func TestAtomicWriteAndSQLiteMetadata(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.WriteFile(ctx, "alice", "/Notes/a.md", []byte("first")); err != nil {
		t.Fatal(err)
	}
	if err := app.WriteFile(ctx, "alice", "/Notes/a.md", []byte("second")); err != nil {
		t.Fatal(err)
	}
	data, err := app.ReadFile(ctx, "alice", "/Notes/a.md")
	if err != nil || string(data) != "second" {
		t.Fatalf("read = %q, %v", data, err)
	}
	if err = app.SetFavorite(ctx, "alice", "/Notes/a.md", true); err != nil {
		t.Fatal(err)
	}
	favorite, err := app.Favorite(ctx, "alice", "/Notes/a.md")
	if err != nil || !favorite {
		t.Fatalf("favorite = %v, %v", favorite, err)
	}
	entries, err := os.ReadDir(filepath.Dir(app.realPath("alice", "/Notes/a.md")))
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if len(entry.Name()) >= 11 && entry.Name()[:11] == ".dar-write-" {
			t.Fatalf("temporary file left behind: %s", entry.Name())
		}
	}
}

func TestFavoritesAreBulkLoadedAndUserScoped(t *testing.T) {
	app := sqliteTestApp(t)
	ctx := context.Background()
	if err := app.SetFavorite(ctx, "alice", "/Notes/a.md", true); err != nil {
		t.Fatal(err)
	}
	if err := app.SetFavorite(ctx, "alice", "/Notes/b.md", true); err != nil {
		t.Fatal(err)
	}
	if err := app.SetFavorite(ctx, "bob", "/Notes/private.md", true); err != nil {
		t.Fatal(err)
	}
	favorites, err := app.Favorites(ctx, "alice")
	if err != nil {
		t.Fatal(err)
	}
	if len(favorites) != 2 || !favorites["/Notes/a.md"] || !favorites["/Notes/b.md"] || favorites["/Notes/private.md"] {
		t.Fatalf("alice favorites = %#v", favorites)
	}
}

func TestConcurrentMetadataMutationsDoNotLoseUpdates(t *testing.T) {
	app := sqliteTestApp(t)
	var wg sync.WaitGroup
	errs := make(chan error, 100)
	for i := 0; i < 100; i++ {
		path := fmt.Sprintf("/file-%03d.txt", i)
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs <- app.SetFavorite(context.Background(), "alice", path, true)
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	favorites, err := app.Favorites(context.Background(), "alice")
	if err != nil || len(favorites) != 100 {
		t.Fatalf("favorites=%d err=%v", len(favorites), err)
	}
}
