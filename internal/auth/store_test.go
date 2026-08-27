package auth

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/db"
	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
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
	user, created, err := FindOrCreateUser(ctx, conn, "test", "test@example.com", "Test User", "", "https://accounts.google.com", "google-123")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}
	if !created {
		t.Error("expected user to be created")
	}
	if user.Email != "test@example.com" {
		t.Errorf("email = %q, want %q", user.Email, "test@example.com")
	}
	if user.OIDCIssuer != "https://accounts.google.com" {
		t.Errorf("issuer = %q", user.OIDCIssuer)
	}

	// Find the same user again.
	user2, created2, err := FindOrCreateUser(ctx, conn, "ignored-on-existing", "test@example.com", "Test User Updated", "https://avatar", "https://accounts.google.com", "google-123")
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

	user, _, err := FindOrCreateUser(ctx, conn, "a", "a@b.com", "A", "", "https://github.example", "gh-1")
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

func TestAdminStatusRoundTrip(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_admin.sqlite")
	defer cleanup()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	user, _, err := FindOrCreateUser(context.Background(), conn, "admin", "admin@example.com", "Admin", "", "https://oidc.example", "admin-1")
	if err != nil {
		t.Fatal(err)
	}
	if err := SetUserAdmin(conn, user.ID, true); err != nil {
		t.Fatal(err)
	}
	got, err := GetUserByID(conn, user.ID)
	if err != nil || !got.IsAdmin {
		t.Fatalf("admin=%v err=%v", got, err)
	}
	if err := SetUserAdmin(conn, user.ID, false); err != nil {
		t.Fatal(err)
	}
	got, err = GetUserByID(conn, user.ID)
	if err != nil || got.IsAdmin {
		t.Fatalf("demoted=%v err=%v", got, err)
	}
}

func TestOIDCProfileInGroup(t *testing.T) {
	p := OIDCProfile{Groups: []string{"users", "dar-admins"}}
	if !p.InGroup("dar-admins") || p.InGroup("admins") || p.InGroup("") {
		t.Fatal("unexpected group matching")
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

	user, _, err := FindOrCreateUser(ctx, conn, "sess", "sess@test.com", "Session Tester", "", "https://oidc.example", "oidc-1")
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
	var storedHash string
	if err := sqlitex.Execute(conn, `SELECT token_hash FROM sessions WHERE user_id=?`, &sqlitex.ExecOptions{
		Args: []any{user.ID},
		ResultFunc: func(stmt *sqlite.Stmt) error {
			storedHash = strings.ToLower(stmt.ColumnText(0))
			return nil
		},
	}); err != nil {
		t.Fatalf("read stored session: %v", err)
	}
	if storedHash != hashSessionToken(sess.ID) {
		t.Fatalf("stored session is not the SHA-256 token hash")
	}
	if storedHash == sess.ID {
		t.Fatal("raw session token was persisted")
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

func TestOIDCIdentityDoesNotLinkByEmail(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_emailfallback.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	// First login with provider "ProviderA".
	_, created, err := FindOrCreateUser(ctx, conn, "same", "same@email.com", "Same User", "", "https://provider-a.example", "sub-123")
	if err != nil {
		t.Fatalf("first FindOrCreateUser: %v", err)
	}
	if !created {
		t.Error("expected first user to be created")
	}

	// Second login with different provider name, same email.
	_, _, err = FindOrCreateUser(ctx, conn, "same2", "same@email.com", "Same User Updated", "", "https://provider-b.example", "sub-123")
	if err == nil {
		t.Fatal("different OIDC identity must not link by email")
	}
}

func TestUsernameCollisionIsRejected(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_username_collision.sqlite")
	defer cleanup()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	if _, _, err := FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://issuer-a.example", "subject-a"); err != nil {
		t.Fatal(err)
	}
	if _, _, err := FindOrCreateUser(context.Background(), conn, "alice", "other@example.test", "Other", "", "https://issuer-b.example", "subject-b"); err == nil {
		t.Fatal("duplicate immutable username was accepted")
	}
}

func TestAppPasswordScopeValidation(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_scopes.sqlite")
	defer cleanup()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	user, _, err := FindOrCreateUser(context.Background(), conn, "scope", "scope@example.test", "Scope", "", "https://issuer.example", "subject")
	if err != nil {
		t.Fatal(err)
	}
	for _, scopes := range [][]string{nil, {}, {"unknown"}, {"files", "unknown"}} {
		if _, err := CreateAppPassword(conn, user.ID, "invalid", scopes); err == nil {
			t.Fatalf("invalid scopes accepted: %v", scopes)
		}
	}
	all := ValidAppScopes()
	created, err := CreateAppPassword(conn, user.ID, "all", append(all, "files"))
	if err != nil {
		t.Fatal(err)
	}
	if len(created.Scopes) != len(all) {
		t.Fatalf("normalized scopes=%v", created.Scopes)
	}
}

func TestAppPasswords(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_apppass.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	user, _, err := FindOrCreateUser(ctx, conn, "app", "app@test.com", "App User", "", "https://oidc.example", "oidc-app")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}
	other, _, err := FindOrCreateUser(ctx, conn, "other", "other@test.com", "Other", "", "https://oidc.example", "oidc-other")
	if err != nil {
		t.Fatalf("FindOrCreateUser other: %v", err)
	}

	created, err := CreateAppPassword(conn, user.ID, "DAVx5", []string{"contacts", "calendar"})
	if err != nil {
		t.Fatalf("CreateAppPassword: %v", err)
	}
	if created.Token == "" || !strings.HasPrefix(created.Token, "dar_") {
		t.Fatalf("token = %q, want dar_…", created.Token)
	}
	if created.Name != "DAVx5" {
		t.Fatalf("name = %q", created.Name)
	}
	plain := created.Token

	list, err := ListAppPasswords(conn, user.ID)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if len(list) != 1 || list[0].Token != "" {
		t.Fatalf("list = %+v (token must be empty)", list)
	}

	// Validate
	got, principal, err := ValidateAppPassword(conn, "app@test.com", plain)
	if err != nil {
		t.Fatalf("Validate: %v", err)
	}
	if got == nil || got.ID != user.ID {
		t.Fatalf("validate user = %+v", got)
	}
	if principal == nil || len(principal.Scopes) != 2 {
		t.Fatalf("principal scopes = %#v", principal)
	}

	// Bad token
	bad, _, err := ValidateAppPassword(conn, "app@test.com", "dar_wrong")
	if err != nil {
		t.Fatalf("Validate bad: %v", err)
	}
	if bad != nil {
		t.Fatal("expected nil for bad token")
	}

	// Wrong user cannot delete
	ok, err := DeleteAppPassword(conn, other.ID, created.ID)
	if err != nil {
		t.Fatalf("Delete other: %v", err)
	}
	if ok {
		t.Fatal("other user should not delete")
	}

	// Owner deletes
	ok, err = DeleteAppPassword(conn, user.ID, created.ID)
	if err != nil || !ok {
		t.Fatalf("Delete owner ok=%v err=%v", ok, err)
	}
	after, _, err := ValidateAppPassword(conn, "app@test.com", plain)
	if err != nil {
		t.Fatalf("Validate after delete: %v", err)
	}
	if after != nil {
		t.Fatal("expected invalid after revoke")
	}
}

func TestValidateAppPasswordRequiresMatchingUsername(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_localpart.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	user, _, err := FindOrCreateUser(ctx, conn, "ada", "ada@example.com", "Ada", "", "https://oidc.example", "oidc-ada")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}
	ap, err := CreateAppPassword(conn, user.ID, "phone", []string{"contacts"})
	if err != nil {
		t.Fatalf("CreateAppPassword: %v", err)
	}

	// Local-part username.
	got, _, err := ValidateAppPassword(conn, "ada", ap.Token)
	if err != nil || got == nil || got.ID != user.ID {
		t.Fatalf("local-part auth: user=%v err=%v", got, err)
	}

	// Token-only (wrong username but correct secret).
	got2, _, err := ValidateAppPassword(conn, "not-a-user", ap.Token)
	if err != nil || got2 != nil {
		t.Fatalf("wrong username auth: user=%v err=%v", got2, err)
	}

	// Huge paste must not match.
	huge := strings.Repeat("x", 20000)
	got3, _, err := ValidateAppPassword(conn, "ada", huge)
	if err != nil || got3 != nil {
		t.Fatalf("huge password should fail, got %v err %v", got3, err)
	}
}

func TestUpdateUserProfile(t *testing.T) {
	database, cleanup := openTestDB(t, "/tmp/dar_test_auth_profile.sqlite")
	defer cleanup()

	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("Conn: %v", err)
	}
	defer release()

	user, _, err := FindOrCreateUser(ctx, conn, "profile", "profile@test.com", "Profile User", "", "https://oidc.example", "oidc-prof")
	if err != nil {
		t.Fatalf("FindOrCreateUser: %v", err)
	}

	prof := UserProfile{
		Phone:        "+1 555 0100",
		Address:      "1 Infinite Loop",
		Website:      "https://example.com",
		Twitter:      "@ada",
		Fediverse:    "@ada@example.social",
		Organisation: "Acme",
		Role:         "Engineer",
		Headline:     "Building things",
		Biography:    "Hello world",
		Location:     "San Francisco",
		Language:     "en",
		Locale:       "en_US",
		Birthday:     "1815-12-10",
	}
	if err := UpdateUserProfile(conn, user.ID, "Ada Lovelace", prof); err != nil {
		t.Fatalf("UpdateUserProfile: %v", err)
	}

	got, err := GetUserByID(conn, user.ID)
	if err != nil || got == nil {
		t.Fatalf("GetUserByID: %v %#v", err, got)
	}
	if got.DisplayName != "Ada Lovelace" {
		t.Fatalf("display_name = %q", got.DisplayName)
	}
	if got.Profile.Phone != "+1 555 0100" || got.Profile.Organisation != "Acme" {
		t.Fatalf("profile = %+v", got.Profile)
	}
	if got.Profile.Birthday != "1815-12-10" || got.Profile.Fediverse != "@ada@example.social" {
		t.Fatalf("profile fields = %+v", got.Profile)
	}

	// Display-name-only update must not wipe profile.
	if err := UpdateUserDisplayName(conn, user.ID, "Ada L."); err != nil {
		t.Fatalf("UpdateUserDisplayName: %v", err)
	}
	got2, err := GetUserByID(conn, user.ID)
	if err != nil || got2 == nil {
		t.Fatalf("GetUserByID after name: %v", err)
	}
	if got2.DisplayName != "Ada L." {
		t.Fatalf("display_name after = %q", got2.DisplayName)
	}
	if got2.Profile.Phone != "+1 555 0100" || got2.Profile.Biography != "Hello world" {
		t.Fatalf("profile wiped: %+v", got2.Profile)
	}
}

func TestStoreValidationExpiryCorruptionAndDatabaseErrors(t *testing.T) {
	database, cleanup := openTestDB(t, filepath.Join(t.TempDir(), "auth-edges.sqlite"))
	defer cleanup()
	ctx := context.Background()
	conn, release, err := database.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer release()

	for _, fields := range [][4]string{
		{"", "a@example.test", "https://issuer.example", "subject"},
		{"alice", "", "https://issuer.example", "subject"},
		{"alice", "a@example.test", "", "subject"},
		{"alice", "a@example.test", "https://issuer.example", ""},
	} {
		if _, _, err := FindOrCreateUser(ctx, conn, fields[0], fields[1], "Alice", "", fields[2], fields[3]); err == nil {
			t.Fatalf("required identity fields accepted: %#v", fields)
		}
	}
	if err := SetUserAdmin(conn, "", true); err == nil {
		t.Fatal("empty admin user accepted")
	}
	if got, err := GetUserByEmail(conn, "  "); err != nil || got != nil {
		t.Fatalf("blank email result=%v err=%v", got, err)
	}
	for _, tc := range []struct {
		id, name string
	}{
		{"", "Name"}, {"user", " "},
	} {
		if err := UpdateUserProfile(conn, tc.id, tc.name, UserProfile{}); err == nil {
			t.Fatalf("invalid profile update accepted: %#v", tc)
		}
		if err := UpdateUserDisplayName(conn, tc.id, tc.name); err == nil {
			t.Fatalf("invalid display-name update accepted: %#v", tc)
		}
	}
	if got, err := ValidateSession(conn, " "); err != nil || got != nil {
		t.Fatalf("blank session result=%v err=%v", got, err)
	}
	if err := DeleteSession(conn, " "); err != nil {
		t.Fatalf("blank session delete=%v", err)
	}
	for _, tc := range []struct {
		id, name string
	}{
		{"", "phone"}, {"user", " "},
	} {
		if _, err := CreateAppPassword(conn, tc.id, tc.name, []string{"files"}); err == nil {
			t.Fatalf("invalid app password owner/name accepted: %#v", tc)
		}
	}
	for _, tc := range []struct {
		username, token string
	}{
		{"alice", ""}, {"alice", strings.Repeat("x", 513)}, {"", "dar_token"},
	} {
		user, password, err := ValidateAppPassword(conn, tc.username, tc.token)
		if err != nil || user != nil || password != nil {
			t.Fatalf("invalid app login (%q,%d) user=%v password=%v err=%v", tc.username, len(tc.token), user, password, err)
		}
	}
	if got := parseProfileJSON("{broken"); got != (UserProfile{}) {
		t.Fatalf("malformed stored profile=%+v", got)
	}

	user, _, err := FindOrCreateUser(ctx, conn, "alice", "alice@example.test", "Alice", "", "https://issuer.example/", "subject")
	if err != nil {
		t.Fatal(err)
	}
	empty, err := ListAppPasswords(conn, user.ID)
	if err != nil || empty == nil || len(empty) != 0 {
		t.Fatalf("empty password list=%#v err=%v", empty, err)
	}
	session, err := CreateSession(conn, user.ID)
	if err != nil {
		t.Fatal(err)
	}
	if err := sqlitex.Execute(conn, `UPDATE sessions SET expires_at = ? WHERE token_hash = ?`, &sqlitex.ExecOptions{
		Args: []any{time.Now().Add(-time.Hour).Unix(), hashSessionToken(session.ID)},
	}); err != nil {
		t.Fatal(err)
	}
	if got, err := ValidateSession(conn, session.ID); err != nil || got != nil {
		t.Fatalf("expired session result=%v err=%v", got, err)
	}

	password, err := CreateAppPassword(conn, user.ID, "phone", []string{" FILES ", "files"})
	if err != nil || len(password.Scopes) != 1 || password.Scopes[0] != "files" {
		t.Fatalf("normalized password=%+v err=%v", password, err)
	}
	spaced := strings.Replace(password.Token, "dar_", "dar_ \n\u00a0", 1) + "\r"
	gotUser, gotPassword, err := ValidateAppPassword(conn, "ALICE@EXAMPLE.TEST", spaced)
	if err != nil || gotUser == nil || gotPassword == nil || gotPassword.LastUsedAt.IsZero() {
		t.Fatalf("sanitized password user=%v password=%v err=%v", gotUser, gotPassword, err)
	}
	if err := sqlitex.Execute(conn, `UPDATE app_passwords SET scopes_json = 'not-json' WHERE id = ?`, &sqlitex.ExecOptions{
		Args: []any{password.ID},
	}); err != nil {
		t.Fatal(err)
	}
	if _, err := ListAppPasswords(conn, user.ID); err == nil {
		t.Fatal("malformed listed scopes accepted")
	}
	if _, _, err := ValidateAppPassword(conn, "alice", password.Token); err == nil {
		t.Fatal("malformed matched scopes accepted")
	}

	for _, table := range []string{"app_passwords", "sessions", "users"} {
		if err := sqlitex.Execute(conn, "DROP TABLE "+table, nil); err != nil {
			t.Fatal(err)
		}
	}
	errorChecks := []struct {
		name string
		run  func() error
	}{
		{"find user", func() error {
			_, _, err := FindOrCreateUser(ctx, conn, "bob", "bob@example.test", "Bob", "", "https://issuer.example", "bob")
			return err
		}},
		{"get user", func() error { _, err := GetUserByID(conn, user.ID); return err }},
		{"get email", func() error { _, err := GetUserByEmail(conn, user.Email); return err }},
		{"set admin", func() error { return SetUserAdmin(conn, user.ID, true) }},
		{"update profile", func() error { return UpdateUserProfile(conn, user.ID, "Alice", UserProfile{}) }},
		{"update display name", func() error { return UpdateUserDisplayName(conn, user.ID, "Alice") }},
		{"create session", func() error { _, err := CreateSession(conn, user.ID); return err }},
		{"validate session", func() error { _, err := ValidateSession(conn, "token"); return err }},
		{"delete session", func() error { return DeleteSession(conn, "token") }},
		{"create app password", func() error {
			_, err := CreateAppPassword(conn, user.ID, "phone", []string{"files"})
			return err
		}},
		{"list app passwords", func() error { _, err := ListAppPasswords(conn, user.ID); return err }},
		{"delete app password", func() error { _, err := DeleteAppPassword(conn, user.ID, password.ID); return err }},
		{"validate app password", func() error {
			_, _, err := ValidateAppPassword(conn, "alice", password.Token)
			return err
		}},
	}
	for _, tc := range errorChecks {
		t.Run(tc.name, func(t *testing.T) {
			if err := tc.run(); err == nil {
				t.Fatal("operation succeeded without its table")
			}
		})
	}
}
