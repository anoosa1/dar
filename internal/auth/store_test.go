package auth

import (
	"context"
	"os"
	"testing"

	"github.com/anoosa/dar/internal/db"
)

func openTestDB(t *testing.T, path string) (*db.DB, func()) {
	t.Helper()
	_ = os.Remove(path)
	database, err := db.Open(path, Migrations())
	if err != nil {
		t.Fatalf("db.Open: %v", err)
	}
	cleanup := func() {
		database.Close()
		os.Remove(path)
	}
	return database, cleanup
}

func TestFindOrCreateUser(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_users.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	// Create a new user.
	user, created, err := FindOrCreateUser(ctx, conn, "test@example.com", "Test User", "", "google", "google-123")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}
	if !created {
		t.Error("expected user to be created")
	}
	if user.Email != "test@example.com" {
		t.Errorf("email = %q, want %q", user.Email, "test@example.com")
	}
	if user.Provider != "google" {
		t.Errorf("provider = %q, want %q", user.Provider, "google")
	}

	// Find the same user again.
	user2, created2, err := FindOrCreateUser(ctx, conn, "test@example.com", "Test User Updated", "https://avatar", "google", "google-123")
	if err != nil {
		t.Fatalf("FindOrCreateUser (2nd): %v", err)
	}
	if created2 {
		t.Error("expected user NOT to be created on second call")
	}
	if user2.ID != user.ID {
		t.Errorf("IDs differ: %q vs %q", user2.ID, user.ID)
	}
	if user2.DisplayName != "Test User Updated" {
		t.Errorf("display name not updated: %q", user2.DisplayName)
	}
}

func TestGetUserByID(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_getuser.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	user, _, err := FindOrCreateUser(ctx, conn, "a@b.com", "A", "", "github", "gh-1")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}

	found, err := GetUserByID(conn, user.ID)
	if err != nil {
		t.Fatalf("GetUserByID: %v", err)
	}
	if found == nil {
		t.Fatal("expected to find user")
	}
	if found.Email != "a@b.com" {
		t.Errorf("email = %q", found.Email)
	}

	// Non-existent user.
	notFound, err := GetUserByID(conn, "nonexistent")
	if err != nil {
		t.Fatalf("GetUserByID (nonexistent): %v", err)
	}
	if notFound != nil {
		t.Error("expected nil for nonexistent user")
	}
}

func TestSessionLifecycle(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_sessions.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	user, _, err := FindOrCreateUser(ctx, conn, "sess@test.com", "Session Tester", "", "oidc", "oidc-1")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}

	// Create session.
	sess, err := CreateSession(conn, user.ID)
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if sess.UserID != user.ID {
		t.Errorf("session user_id = %q, want %q", sess.UserID, user.ID)
	}

	// Validate session.
	valid, err := ValidateSession(conn, sess.ID)
	if err != nil {
		t.Fatalf("ValidateSession: %v", err)
	}
	if valid == nil {
		t.Fatal("expected session to be valid")
	}

	// Delete session.
	if err := DeleteSession(conn, sess.ID); err != nil {
		t.Fatalf("DeleteSession: %v", err)
	}

	// Validate after delete.
	afterDelete, err := ValidateSession(conn, sess.ID)
	if err != nil {
		t.Fatalf("ValidateSession after delete: %v", err)
	}
	if afterDelete != nil {
		t.Error("expected nil after delete")
	}
}

func TestEmailFallback(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_emailfallback.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	// First login with provider "ProviderA".
	u1, created, err := FindOrCreateUser(ctx, conn, "same@email.com", "Same User", "", "ProviderA", "sub-123")
	if err != nil {
		t.Fatalf("first FindOrCreateUser: %v", err)
	}
	if !created {
		t.Error("expected first user to be created")
	}

	// Second login with different provider name, same email.
	u2, created2, err := FindOrCreateUser(ctx, conn, "same@email.com", "Same User Updated", "", "ProviderB", "sub-123")
	if err != nil {
		t.Fatalf("second FindOrCreateUser: %v", err)
	}
	if created2 {
		t.Error("expected email fallback — user should NOT be re-created")
	}
	if u2.ID != u1.ID {
		t.Errorf("IDs differ: %q vs %q — fallback should return same user", u2.ID, u1.ID)
	}
	if u2.Provider != "ProviderB" {
		t.Errorf("provider should be updated to ProviderB, got %q", u2.Provider)
	}
}
