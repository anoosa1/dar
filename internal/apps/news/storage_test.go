package news

import (
	"bytes"
	"context"
	"path/filepath"
	"testing"

	"github.com/anoosa/dar/internal/credentials"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/preferences"
)

func testApp(t *testing.T) *App {
	t.Helper()
	policy, err := outbound.ParsePolicy("127.0.0.1,localhost", "")
	if err != nil {
		t.Fatal(err)
	}
	m := append(preferences.Migrations(), Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "news.sqlite"), m)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	return New(Config{
		DB: database, DataRoot: t.TempDir(), RefreshMinutes: 30,
		CredentialKeys: []credentials.Key{{ID: "test", Material: bytes.Repeat([]byte{0x42}, 32)}},
		OutboundPolicy: policy,
	})
}

func TestSQLiteStorageIsUserScoped(t *testing.T) {
	a := testApp(t)
	ctx := context.Background()
	folder, err := a.createFolder(ctx, "alice", "Tech")
	if err != nil {
		t.Fatal(err)
	}
	if _, err = a.insertFeed(ctx, "alice", Feed{URL: "https://example.com/feed", Title: "Example", FolderID: folder.ID, BasicAuthUser: "reader", BasicAuthPassword: "secret"}); err != nil {
		t.Fatal(err)
	}
	alice, err := a.feeds(ctx, "alice")
	if err != nil || len(alice) != 1 {
		t.Fatalf("alice feeds=%v err=%v", alice, err)
	}
	if alice[0].BasicAuthPassword != "secret" {
		t.Fatal("credential not persisted")
	}
	bob, err := a.feeds(ctx, "bob")
	if err != nil || len(bob) != 0 {
		t.Fatalf("bob feeds=%v err=%v", bob, err)
	}
}

func TestFolderDeleteCascadesFeeds(t *testing.T) {
	a := testApp(t)
	ctx := context.Background()
	folder, _ := a.createFolder(ctx, "alice", "Delete")
	feed, _ := a.insertFeed(ctx, "alice", Feed{URL: "https://example.com/delete", Title: "Delete", FolderID: folder.ID})
	if err := a.deleteFolder(ctx, "alice", folder.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := a.feed(ctx, "alice", feed.ID); err != errNotFound {
		t.Fatalf("feed error=%v", err)
	}
}

func TestPreferencesRoundTrip(t *testing.T) {
	a := testApp(t)
	ctx := context.Background()
	p := defaultPreferences()
	p.OldestFirst = true
	p.Images = "never"
	if err := a.savePreferences(ctx, "alice", p); err != nil {
		t.Fatal(err)
	}
	got, err := a.preferences(ctx, "alice")
	if err != nil {
		t.Fatal(err)
	}
	if got != p {
		t.Fatalf("got %+v want %+v", got, p)
	}
}

func TestPrivateIPBlocked(t *testing.T) {
	if _, err := validateRemoteURL(context.Background(), "http://127.0.0.1/feed"); err == nil {
		t.Fatal("loopback URL allowed")
	}
}
