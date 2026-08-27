package notes

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"testing/fstest"
	"time"

	filesapp "github.com/anoosa/dar/internal/apps/files"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"zombiezen.com/go/sqlite"
)

func testNotesApp(t *testing.T) (*App, *filesapp.App) {
	t.Helper()
	root := t.TempDir()
	m := append(preferences.Migrations(), filesapp.Migrations()...)
	m = append(m, Migrations()...)
	database, err := db.Open(filepath.Join(root, "dar.sqlite"), m)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { database.Close() })
	files := filesapp.New(filesapp.Config{DataRoot: filepath.Join(root, "data"), DB: database})
	return New(Config{DB: database, Files: files}), files
}

func str(v string) *string { return &v }
func boolean(v bool) *bool { return &v }

type observedFileStore struct {
	FileStore
	mu          sync.Mutex
	walks       map[string]int
	walkStarted chan string
	walkRelease chan struct{}
	failStat    bool
}

func (s *observedFileStore) Walk(ctx context.Context, uid, root string, fn filepath.WalkFunc) error {
	s.mu.Lock()
	if s.walks == nil {
		s.walks = make(map[string]int)
	}
	s.walks[uid]++
	started, release := s.walkStarted, s.walkRelease
	s.mu.Unlock()
	if started != nil {
		select {
		case started <- uid:
		default:
		}
	}
	if release != nil {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-release:
		}
	}
	return s.FileStore.Walk(ctx, uid, root, fn)
}

func (s *observedFileStore) Stat(ctx context.Context, uid, vpath string) (os.FileInfo, error) {
	s.mu.Lock()
	fail := s.failStat
	s.mu.Unlock()
	if fail {
		return nil, errors.New("injected stat failure")
	}
	return s.FileStore.Stat(ctx, uid, vpath)
}

func (s *observedFileStore) walkCount(uid string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.walks[uid]
}

func TestNoteStorageIsUserScopedAndAllowsEmptyContent(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	alice, err := app.create(ctx, "alice", notePatch{Title: str("Alice"), Content: str("hello")})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = app.create(ctx, "bob", notePatch{Title: str("Bob")}); err != nil {
		t.Fatal(err)
	}
	notes, err := app.indexedNotes(ctx, "alice", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 1 || notes[0].ID != alice.ID {
		t.Fatalf("alice notes = %#v", notes)
	}
	empty := ""
	updated, err := app.update(ctx, "alice", alice.ID, notePatch{Content: &empty}, alice.ETag, false)
	if err != nil {
		t.Fatal(err)
	}
	if updated.Content != "" {
		t.Fatalf("content = %q, want empty", updated.Content)
	}
}

func TestOptimisticConflictReturnsServerNote(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	n, err := app.create(ctx, "alice", notePatch{Title: str("Conflict"), Content: str("one")})
	if err != nil {
		t.Fatal(err)
	}
	changed, err := app.update(ctx, "alice", n.ID, notePatch{Content: str("server")}, n.ETag, false)
	if err != nil {
		t.Fatal(err)
	}
	server, err := app.update(ctx, "alice", n.ID, notePatch{Content: str("local")}, n.ETag, false)
	if !errors.Is(err, errConflict) {
		t.Fatalf("error = %v", err)
	}
	if server.ETag != changed.ETag || server.Content != "server" {
		t.Fatalf("server note = %#v", server)
	}
}

func TestReconcilePreservesIDAcrossExternalRenameAndFingerprints(t *testing.T) {
	app, files := testNotesApp(t)
	ctx := context.Background()
	n, err := app.create(ctx, "alice", notePatch{Title: str("Before"), Content: str("same content")})
	if err != nil {
		t.Fatal(err)
	}
	if err = files.Rename(ctx, "alice", "/Notes/Before.md", "/Notes/After.md"); err != nil {
		t.Fatal(err)
	}
	app.invalidateScan("alice")
	notes, err := app.indexedNotes(ctx, "alice", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 1 || notes[0].ID != n.ID || notes[0].Title != "After" {
		t.Fatalf("renamed notes = %#v", notes)
	}
	if err = files.WriteFile(ctx, "alice", "/Notes/copy.md", []byte("same content")); err != nil {
		t.Fatal(err)
	}
	app.invalidateScan("alice")
	notes, err = app.indexedNotes(ctx, "alice", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 2 {
		t.Fatalf("notes after duplicate = %#v", notes)
	}
}

func TestScanWindowCoalescesConcurrentReaders(t *testing.T) {
	app, files := testNotesApp(t)
	store := &observedFileStore{FileStore: files, walkStarted: make(chan string, 2), walkRelease: make(chan struct{})}
	app.files = store
	results := make(chan error, 2)
	go func() { _, err := app.indexedNotes(context.Background(), "alice", false); results <- err }()
	select {
	case <-store.walkStarted:
	case <-time.After(2 * time.Second):
		t.Fatal("first scan did not start")
	}
	go func() { _, err := app.indexedNotes(context.Background(), "alice", false); results <- err }()
	close(store.walkRelease)
	for range 2 {
		if err := <-results; err != nil {
			t.Fatal(err)
		}
	}
	if got := store.walkCount("alice"); got != 1 {
		t.Fatalf("walks = %d, want 1", got)
	}
	if _, err := app.indexedNotes(context.Background(), "alice", false); err != nil {
		t.Fatal(err)
	}
	if got := store.walkCount("alice"); got != 1 {
		t.Fatalf("fresh-window walks = %d, want 1", got)
	}
	app.scans.mu.Lock()
	app.scans.users["alice"].lastScan = time.Now().Add(-scanFreshFor)
	app.scans.mu.Unlock()
	store.mu.Lock()
	store.walkRelease = nil
	store.walkStarted = nil
	store.mu.Unlock()
	if _, err := app.indexedNotes(context.Background(), "alice", false); err != nil {
		t.Fatal(err)
	}
	if got := store.walkCount("alice"); got != 2 {
		t.Fatalf("stale-window walks = %d, want 2", got)
	}
	app.scans.mu.Lock()
	app.scans.users["completed"] = &userScanState{completedAt: time.Now().Add(-scanKeepFor - time.Second)}
	app.scans.mu.Unlock()
	if _, err := app.indexedNotes(context.Background(), "alice", false); err != nil {
		t.Fatal(err)
	}
	app.scans.mu.Lock()
	_, retained := app.scans.users["completed"]
	app.scans.mu.Unlock()
	if retained {
		t.Fatal("completed scan state was not pruned")
	}
}

func TestScanWaitHonorsContextAndUsersDoNotShareLocks(t *testing.T) {
	app, files := testNotesApp(t)
	store := &observedFileStore{FileStore: files, walkStarted: make(chan string, 4), walkRelease: make(chan struct{})}
	app.files = store
	aliceDone := make(chan error, 1)
	go func() { _, err := app.indexedNotes(context.Background(), "alice", false); aliceDone <- err }()
	select {
	case <-store.walkStarted:
	case <-time.After(2 * time.Second):
		t.Fatal("alice scan did not start")
	}
	bobStarted := make(chan error, 1)
	go func() { _, err := app.indexedNotes(context.Background(), "bob", false); bobStarted <- err }()
	select {
	case uid := <-store.walkStarted:
		if uid != "bob" {
			t.Fatalf("second scan user = %q", uid)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("bob was blocked by alice")
	}
	waitCtx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	if _, err := app.indexedNotes(waitCtx, "alice", false); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("waiting scan error = %v", err)
	}
	close(store.walkRelease)
	if err := <-aliceDone; err != nil {
		t.Fatal(err)
	}
	if err := <-bobStarted; err != nil {
		t.Fatal(err)
	}
}

func TestDirectMutationsAvoidRescanAndRepairPartialFailure(t *testing.T) {
	app, files := testNotesApp(t)
	store := &observedFileStore{FileStore: files}
	app.files = store
	if _, err := app.indexedNotes(context.Background(), "alice", false); err != nil {
		t.Fatal(err)
	}
	n, err := app.create(context.Background(), "alice", notePatch{Title: str("Direct"), Content: str("one")})
	if err != nil {
		t.Fatal(err)
	}
	if _, err = app.update(context.Background(), "alice", n.ID, notePatch{Content: str("two")}, n.ETag, false); err != nil {
		t.Fatal(err)
	}
	if got := store.walkCount("alice"); got != 1 {
		t.Fatalf("walks after direct mutations = %d, want 1", got)
	}
	store.mu.Lock()
	store.failStat = true
	store.mu.Unlock()
	if _, err = app.create(context.Background(), "alice", notePatch{Title: str("Repair")}); err == nil {
		t.Fatal("create succeeded despite injected stat failure")
	}
	app.scans.mu.Lock()
	lastScan := app.scans.users["alice"].lastScan
	app.scans.mu.Unlock()
	if !lastScan.IsZero() {
		t.Fatalf("failed mutation did not invalidate scan: %v", lastScan)
	}
	store.mu.Lock()
	store.failStat = false
	store.mu.Unlock()
	notes, err := app.indexedNotes(context.Background(), "alice", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 2 {
		t.Fatalf("repaired notes = %#v", notes)
	}
}

func TestConcurrentListAndUpdateDoNotExhaustPool(t *testing.T) {
	app, _ := testNotesApp(t)
	ctx := context.Background()
	var target Note
	for i := 0; i < 100; i++ {
		n, err := app.create(ctx, "alice", notePatch{Title: str("Concurrent " + strconv.Itoa(i)), Content: str("body")})
		if err != nil {
			t.Fatal(err)
		}
		if i == 0 {
			target = n
		}
	}
	app.invalidateScan("alice")
	start := make(chan struct{})
	errs := make(chan error, 6)
	for i := 0; i < 5; i++ {
		go func() {
			<-start
			_, err := app.indexedNotes(ctx, "alice", true)
			errs <- err
		}()
	}
	go func() {
		<-start
		_, err := app.update(ctx, "alice", target.ID, notePatch{Content: str("updated")}, target.ETag, true)
		errs <- err
	}()
	close(start)
	deadline := time.After(5 * time.Second)
	for range 6 {
		select {
		case err := <-errs:
			if err != nil {
				t.Fatal(err)
			}
		case <-deadline:
			t.Fatal("concurrent list/update timed out")
		}
	}
}

func TestLegacyJSONIsIgnored(t *testing.T) {
	app, files := testNotesApp(t)
	ctx := context.Background()
	if err := files.WriteFile(ctx, "alice", "/Notes/notes.json", []byte(`[{"title":"legacy"}]`)); err != nil {
		t.Fatal(err)
	}
	notes, err := app.indexedNotes(ctx, "alice", false)
	if err != nil {
		t.Fatal(err)
	}
	if len(notes) != 0 {
		t.Fatalf("legacy JSON was indexed: %#v", notes)
	}
}

func TestNotesSettingsIdentityAndValidationHelperMatrix(t *testing.T) {
	defaults := defaultSettings()
	for _, tc := range []struct {
		in   Settings
		path string
		ext  string
		mode string
	}{
		{Settings{}, defaults.NotesPath, defaults.FileSuffix, defaults.EditorMode},
		{Settings{NotesPath: "/", FileSuffix: ".", EditorMode: "unknown"}, defaults.NotesPath, defaults.FileSuffix, defaults.EditorMode},
		{Settings{NotesPath: "../escape", FileSuffix: "/bad", EditorMode: "source"}, "/escape", defaults.FileSuffix, "source"},
		{Settings{NotesPath: "Custom Notes", FileSuffix: "txt", EditorMode: "preview"}, "/Custom Notes", ".txt", "preview"},
		{Settings{NotesPath: "/Notes", FileSuffix: strings.Repeat("x", 17), EditorMode: "rich"}, "/Notes", defaults.FileSuffix, "rich"},
	} {
		got := sanitizeSettings(tc.in)
		if got.NotesPath != tc.path || got.FileSuffix != tc.ext || got.EditorMode != tc.mode {
			t.Errorf("sanitizeSettings(%+v)=%+v", tc.in, got)
		}
	}
	if boolInt(true) != 1 || boolInt(false) != 0 {
		t.Fatal("boolean integer conversion regression")
	}
	info, err := fstest.MapFS{"note": &fstest.MapFile{Data: []byte("note")}}.Stat("note")
	if err != nil {
		t.Fatal(err)
	}
	if dev, ino := identity(info); dev != 0 || ino != 0 {
		t.Fatalf("portable identity=%d/%d", dev, ino)
	}
	if titleFrom("/Notes/topic.md", "ignored", ".md") != "topic" ||
		categoryFor("/Notes", "/Notes/topic.md") != "" ||
		categoryFor("/Notes", "/Notes/work/topic.md") != "work" {
		t.Fatal("note title/category helper regression")
	}
	for _, raw := range []string{"", "0", "-1", "nope"} {
		if _, err := parseID(raw); err == nil {
			t.Errorf("invalid ID accepted: %q", raw)
		}
	}
	if id, err := parseID("42"); err != nil || id != 42 {
		t.Fatalf("valid ID=%d err=%v", id, err)
	}
	for _, tc := range []struct {
		data []byte
		name string
		ok   bool
	}{
		{[]byte("\x89PNG\r\n\x1a\n"), "image.png", true},
		{[]byte("plain text"), "image.txt", false},
		{[]byte("GIF89a"), "image.exe", false},
	} {
		_, _, ok := validImageType(tc.data, tc.name)
		if ok != tc.ok {
			t.Errorf("validImageType(%q,%q)=%v", tc.data, tc.name, ok)
		}
	}
	if name := randomName(".png"); len(name) != 36 || filepath.Ext(name) != ".png" {
		t.Fatalf("random image name=%q", name)
	}
	if err := (&App{}).conn(context.Background(), func(*sqlite.Conn) error { return nil }); err == nil {
		t.Fatal("unconfigured notes database accepted a connection")
	}
}
