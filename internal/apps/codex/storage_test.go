package codex

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"testing"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
)

func testStore(t *testing.T) (store, string, string, func()) {
	t.Helper()
	migrations := append(auth.Migrations(), Migrations()...)
	database, err := db.Open(filepath.Join(t.TempDir(), "codex.sqlite"), migrations)
	if err != nil {
		t.Fatal(err)
	}
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	ids := make([]string, 0, 2)
	for _, email := range []string{"alice@example.test", "bob@example.test"} {
		username, _, _ := strings.Cut(email, "@")
		user, _, err := auth.FindOrCreateUser(context.Background(), conn, username, email, email, "", "https://test.example", email)
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, user.ID)
	}
	release()
	return store{db: database}, ids[0], ids[1], func() { _ = database.Close() }
}

func TestStoreSessionOwnershipArchiveAndDelete(t *testing.T) {
	s, alice, bob, closeStore := testStore(t)
	defer closeStore()
	ctx := context.Background()
	created, err := s.createSession(ctx, alice, "thread-1", "gpt-test", "medium")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.getSession(ctx, bob, created.ID); !errors.Is(err, errNotFound) {
		t.Fatalf("cross-user read error = %v", err)
	}
	archived := true
	if err := s.updateSession(ctx, alice, created.ID, "Trip", "", "", &archived); err != nil {
		t.Fatal(err)
	}
	active, err := s.listSessions(ctx, alice, false)
	if err != nil || len(active) != 0 {
		t.Fatalf("active sessions = %#v, %v", active, err)
	}
	items, err := s.listSessions(ctx, alice, true)
	if err != nil || len(items) != 1 || items[0].Title != "Trip" {
		t.Fatalf("archived sessions = %#v, %v", items, err)
	}
	if err := s.deleteSession(ctx, bob, created.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.getSession(ctx, alice, created.ID); err != nil {
		t.Fatalf("cross-user delete removed session: %v", err)
	}
	if err := s.deleteSession(ctx, alice, created.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.getSession(ctx, alice, created.ID); !errors.Is(err, errNotFound) {
		t.Fatalf("deleted session error = %v", err)
	}
}

func TestStorePreferencesDefaultsAndValidation(t *testing.T) {
	s, alice, _, closeStore := testStore(t)
	defer closeStore()
	ctx := context.Background()
	prefs, err := s.preferences(ctx, alice)
	if err != nil || prefs != (Preferences{ApprovalMode: "confirm", WebSearch: "disabled"}) {
		t.Fatalf("defaults = %#v, %v", prefs, err)
	}
	if err := s.savePreferences(ctx, alice, Preferences{ApprovalMode: "automatic", WebSearch: "live"}); err != nil {
		t.Fatal(err)
	}
	prefs, _ = s.preferences(ctx, alice)
	if prefs.ApprovalMode != "automatic" || prefs.WebSearch != "live" {
		t.Fatalf("saved = %#v", prefs)
	}
	if err := s.savePreferences(ctx, alice, Preferences{ApprovalMode: "always", WebSearch: "live"}); err == nil {
		t.Fatal("invalid approval mode accepted")
	}
	if err := s.savePreferences(ctx, alice, Preferences{ApprovalMode: "confirm", WebSearch: "internet"}); err == nil {
		t.Fatal("invalid web search mode accepted")
	}
}
