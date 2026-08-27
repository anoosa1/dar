package auth

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

// userMigrations contains the SQL migrations for users and sessions tables.
var userMigrations = []string{
	`CREATE TABLE IF NOT EXISTS users (
		id TEXT PRIMARY KEY,
		username TEXT NOT NULL UNIQUE COLLATE NOCASE,
		email TEXT NOT NULL UNIQUE COLLATE NOCASE,
		display_name TEXT NOT NULL DEFAULT '',
		avatar_url TEXT NOT NULL DEFAULT '',
		oidc_issuer TEXT NOT NULL,
		oidc_subject TEXT NOT NULL,
		profile_json TEXT NOT NULL DEFAULT '{}',
		is_admin INTEGER NOT NULL DEFAULT 0,
		created_at INTEGER NOT NULL DEFAULT 0,
		last_login_at INTEGER NOT NULL DEFAULT 0,
		UNIQUE(oidc_issuer, oidc_subject)
	);`,
	`CREATE TABLE IF NOT EXISTS sessions (
		id TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		created_at INTEGER NOT NULL DEFAULT 0,
		expires_at INTEGER NOT NULL DEFAULT 0
	);`,
	`CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);`,
	// App passwords: high-entropy tokens for CardDAV/CalDAV/WebDAV clients (not login passwords).
	`CREATE TABLE IF NOT EXISTS app_passwords (
		id TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		name TEXT NOT NULL,
		token_hash TEXT NOT NULL UNIQUE,
		token_prefix TEXT NOT NULL,
		scopes_json TEXT NOT NULL,
		created_at INTEGER NOT NULL DEFAULT 0,
		last_used_at INTEGER NOT NULL DEFAULT 0
	);
	CREATE INDEX IF NOT EXISTS idx_app_passwords_user ON app_passwords(user_id);`,
	// Alpha security reset: invalidate legacy sessions whose bearer tokens were
	// stored in plaintext and replace the table with token hashes.
	`DROP TABLE IF EXISTS sessions;
	CREATE TABLE sessions (
		token_hash TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
		created_at INTEGER NOT NULL DEFAULT 0,
		expires_at INTEGER NOT NULL DEFAULT 0
	);
	CREATE INDEX idx_sessions_user_id ON sessions(user_id);`,
}

// Migrations returns the SQL migrations for the auth subsystem.
func Migrations() []string {
	return userMigrations
}

// FindOrCreateUser finds an existing user by the immutable OIDC issuer and
// subject pair, or creates one with an immutable username. Email addresses are
// deliberately not used for account linking.
func FindOrCreateUser(ctx context.Context, conn *sqlite.Conn, username, email, displayName, avatarURL, issuer, subject string) (*User, bool, error) {
	username = strings.TrimSpace(username)
	email = strings.TrimSpace(email)
	issuer = strings.TrimRight(strings.TrimSpace(issuer), "/")
	subject = strings.TrimSpace(subject)
	if username == "" || email == "" || issuer == "" || subject == "" {
		return nil, false, fmt.Errorf("username, email, issuer, and subject are required")
	}
	existing, err := findUserByOIDCIdentity(conn, issuer, subject)
	if err != nil {
		return nil, false, err
	}
	if existing != nil {
		now := time.Now().Unix()
		err := sqlitex.Execute(conn,
			`UPDATE users SET email = ?, display_name = ?, avatar_url = ?, last_login_at = ? WHERE id = ?`,
			&sqlitex.ExecOptions{Args: []any{email, displayName, avatarURL, now, existing.ID}},
		)
		if err != nil {
			return nil, false, fmt.Errorf("update user: %w", err)
		}
		existing.Email = email
		existing.DisplayName = displayName
		existing.AvatarURL = avatarURL
		existing.LastLoginAt = time.Unix(now, 0)
		return existing, false, nil
	}

	// Never link an OIDC identity by mutable profile attributes.
	byUsername, err := findUserByUsername(conn, username)
	if err != nil {
		return nil, false, err
	}
	if byUsername != nil {
		return nil, false, fmt.Errorf("username %q is already in use", username)
	}
	byEmail, err := findUserByEmail(conn, email)
	if err != nil {
		return nil, false, err
	}
	if byEmail != nil {
		return nil, false, fmt.Errorf("email address is already associated with another identity")
	}

	// Create new user.
	id := newID()
	now := time.Now().Unix()
	err = sqlitex.Execute(conn,
		`INSERT INTO users (id, username, email, display_name, avatar_url, oidc_issuer, oidc_subject, created_at, last_login_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		&sqlitex.ExecOptions{Args: []any{id, username, email, displayName, avatarURL, issuer, subject, now, now}},
	)
	if err != nil {
		return nil, false, fmt.Errorf("insert user: %w", err)
	}

	return &User{
		ID:          id,
		Username:    username,
		Email:       email,
		DisplayName: displayName,
		AvatarURL:   avatarURL,
		OIDCIssuer:  issuer,
		OIDCSubject: subject,
		CreatedAt:   time.Unix(now, 0),
		LastLoginAt: time.Unix(now, 0),
	}, true, nil
}

const userSelectCols = `id, username, email, display_name, avatar_url, oidc_issuer, oidc_subject, created_at, last_login_at, profile_json, is_admin`

func scanUser(stmt *sqlite.Stmt) *User {
	u := &User{
		ID:          stmt.ColumnText(0),
		Username:    stmt.ColumnText(1),
		Email:       stmt.ColumnText(2),
		DisplayName: stmt.ColumnText(3),
		AvatarURL:   stmt.ColumnText(4),
		OIDCIssuer:  stmt.ColumnText(5),
		OIDCSubject: stmt.ColumnText(6),
		CreatedAt:   time.Unix(stmt.ColumnInt64(7), 0),
		LastLoginAt: time.Unix(stmt.ColumnInt64(8), 0),
	}
	u.Profile = parseProfileJSON(stmt.ColumnText(9))
	u.IsAdmin = stmt.ColumnInt64(10) != 0
	return u
}

// SetUserAdmin synchronizes the database-backed administrator flag with the
// authoritative OIDC groups claim.
func SetUserAdmin(conn *sqlite.Conn, userID string, admin bool) error {
	if strings.TrimSpace(userID) == "" {
		return fmt.Errorf("user id required")
	}
	value := 0
	if admin {
		value = 1
	}
	if err := sqlitex.Execute(conn, `UPDATE users SET is_admin = ? WHERE id = ?`, &sqlitex.ExecOptions{Args: []any{value, userID}}); err != nil {
		return fmt.Errorf("set user admin: %w", err)
	}
	return nil
}

func parseProfileJSON(s string) UserProfile {
	var p UserProfile
	if s == "" || s == "{}" {
		return p
	}
	_ = json.Unmarshal([]byte(s), &p)
	return p
}

func encodeProfileJSON(p UserProfile) string {
	b, err := json.Marshal(p)
	if err != nil {
		return "{}"
	}
	return string(b)
}

// GetUserByID returns a user by their ID, or nil if not found.
func GetUserByID(conn *sqlite.Conn, id string) (*User, error) {
	var u *User
	err := sqlitex.Execute(conn,
		"SELECT "+userSelectCols+" FROM users WHERE id = ?",
		&sqlitex.ExecOptions{
			Args: []any{id},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = scanUser(stmt)
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("get user: %w", err)
	}
	return u, nil
}

// GetUserByEmail returns a user by their login email. It is used by in-process
// apps when granting a resource to another registered Dar user.
func GetUserByEmail(conn *sqlite.Conn, email string) (*User, error) {
	return findUserByEmail(conn, email)
}

func findUserByOIDCIdentity(conn *sqlite.Conn, issuer, subject string) (*User, error) {
	var u *User
	err := sqlitex.Execute(conn,
		"SELECT "+userSelectCols+" FROM users WHERE oidc_issuer = ? AND oidc_subject = ?",
		&sqlitex.ExecOptions{
			Args: []any{issuer, subject},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = scanUser(stmt)
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("find user: %w", err)
	}
	return u, nil
}

func findUserByEmail(conn *sqlite.Conn, email string) (*User, error) {
	email = strings.TrimSpace(email)
	if email == "" {
		return nil, nil
	}
	var u *User
	// Case-insensitive: DAVx5 / phones may alter email casing.
	err := sqlitex.Execute(conn,
		"SELECT "+userSelectCols+" FROM users WHERE lower(email) = lower(?)",
		&sqlitex.ExecOptions{
			Args: []any{email},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = scanUser(stmt)
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("find user by email: %w", err)
	}
	return u, nil
}

// UpdateUserProfile updates display name and Nextcloud-style personal info fields.
func UpdateUserProfile(conn *sqlite.Conn, userID, displayName string, profile UserProfile) error {
	displayName = strings.TrimSpace(displayName)
	if userID == "" {
		return fmt.Errorf("user id required")
	}
	if displayName == "" {
		return fmt.Errorf("display name required")
	}
	err := sqlitex.Execute(conn,
		`UPDATE users SET display_name = ?, profile_json = ? WHERE id = ?`,
		&sqlitex.ExecOptions{Args: []any{displayName, encodeProfileJSON(profile), userID}},
	)
	if err != nil {
		return fmt.Errorf("update profile: %w", err)
	}
	return nil
}

// UpdateUserDisplayName updates only the display name, leaving profile_json unchanged.
func UpdateUserDisplayName(conn *sqlite.Conn, userID, displayName string) error {
	displayName = strings.TrimSpace(displayName)
	if userID == "" {
		return fmt.Errorf("user id required")
	}
	if displayName == "" {
		return fmt.Errorf("display name required")
	}
	err := sqlitex.Execute(conn,
		`UPDATE users SET display_name = ? WHERE id = ?`,
		&sqlitex.ExecOptions{Args: []any{displayName, userID}},
	)
	if err != nil {
		return fmt.Errorf("update display name: %w", err)
	}
	return nil
}

// CreateSession creates a new session for a user (30-day expiry).
func CreateSession(conn *sqlite.Conn, userID string) (*Session, error) {
	id := newSessionID()
	tokenHash := hashSessionToken(id)
	now := time.Now()
	expires := now.Add(30 * 24 * time.Hour)

	err := sqlitex.Execute(conn,
		`INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)`,
		&sqlitex.ExecOptions{Args: []any{tokenHash, userID, now.Unix(), expires.Unix()}},
	)
	if err != nil {
		return nil, fmt.Errorf("insert session: %w", err)
	}

	return &Session{
		ID:        id,
		UserID:    userID,
		CreatedAt: now,
		ExpiresAt: expires,
	}, nil
}

// ValidateSession checks if a session ID is valid (exists and not expired).
// Returns the session if valid, nil if invalid/expired.
func ValidateSession(conn *sqlite.Conn, sessionID string) (*Session, error) {
	if strings.TrimSpace(sessionID) == "" {
		return nil, nil
	}
	tokenHash := hashSessionToken(sessionID)
	var s *Session
	err := sqlitex.Execute(conn,
		"SELECT user_id, created_at, expires_at FROM sessions WHERE token_hash = ?",
		&sqlitex.ExecOptions{
			Args: []any{tokenHash},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				s = &Session{
					ID:        sessionID,
					UserID:    stmt.ColumnText(0),
					CreatedAt: time.Unix(stmt.ColumnInt64(1), 0),
					ExpiresAt: time.Unix(stmt.ColumnInt64(2), 0),
				}
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("validate session: %w", err)
	}
	if s == nil {
		return nil, nil
	}
	// Check expiry.
	if time.Now().After(s.ExpiresAt) {
		// Clean up expired session.
		_ = sqlitex.Execute(conn, "DELETE FROM sessions WHERE token_hash = ?",
			&sqlitex.ExecOptions{Args: []any{tokenHash}},
		)
		return nil, nil
	}
	return s, nil
}

// DeleteSession removes a session from the database.
func DeleteSession(conn *sqlite.Conn, sessionID string) error {
	if strings.TrimSpace(sessionID) == "" {
		return nil
	}
	return sqlitex.Execute(conn, "DELETE FROM sessions WHERE token_hash = ?",
		&sqlitex.ExecOptions{Args: []any{hashSessionToken(sessionID)}},
	)
}

func hashSessionToken(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

// newID generates a random UUIDv4 for user IDs.
func newID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6] = (b[6] & 0x0f) | 0x40 // version 4
	b[8] = (b[8] & 0x3f) | 0x80 // variant 10
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// newSessionID generates a 32-byte random hex string for session tokens.
func newSessionID() string {
	var b [32]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

func hashAppToken(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

func newAppToken() string {
	var b [32]byte
	_, _ = rand.Read(b[:])
	return "dar_" + hex.EncodeToString(b[:])
}

var validAppScopes = map[string]struct{}{
	"files": {}, "calendar": {}, "contacts": {}, "todo": {},
	"notes": {}, "news": {}, "email": {},
}

// ValidAppScopes returns the supported app-password scopes in display order.
func ValidAppScopes() []string {
	return []string{"files", "calendar", "contacts", "todo", "notes", "news", "email"}
}

func normalizeScopes(scopes []string) ([]string, error) {
	seen := make(map[string]struct{}, len(scopes))
	out := make([]string, 0, len(scopes))
	for _, scope := range scopes {
		scope = strings.ToLower(strings.TrimSpace(scope))
		if _, ok := validAppScopes[scope]; !ok {
			return nil, fmt.Errorf("invalid app password scope %q", scope)
		}
		if _, ok := seen[scope]; ok {
			continue
		}
		seen[scope] = struct{}{}
		out = append(out, scope)
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("at least one app password scope is required")
	}
	return out, nil
}

// CreateAppPassword creates a named app password for a user.
// The returned AppPassword.Token is the plaintext secret (shown once); it is not stored.
func CreateAppPassword(conn *sqlite.Conn, userID, name string, requestedScopes []string) (*AppPassword, error) {
	name = strings.TrimSpace(name)
	if userID == "" || name == "" {
		return nil, fmt.Errorf("user id and name required")
	}
	scopes, err := normalizeScopes(requestedScopes)
	if err != nil {
		return nil, err
	}
	scopesJSON, err := json.Marshal(scopes)
	if err != nil {
		return nil, fmt.Errorf("encode app password scopes: %w", err)
	}
	token := newAppToken()
	id := newID()
	now := time.Now()
	prefix := token
	if len(prefix) > 12 {
		prefix = prefix[:12] + "…"
	}
	err = sqlitex.Execute(conn,
		`INSERT INTO app_passwords (id, user_id, name, token_hash, token_prefix, scopes_json, created_at, last_used_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, 0)`,
		&sqlitex.ExecOptions{Args: []any{id, userID, name, hashAppToken(token), prefix, string(scopesJSON), now.Unix()}},
	)
	if err != nil {
		return nil, fmt.Errorf("insert app password: %w", err)
	}
	return &AppPassword{
		ID:          id,
		UserID:      userID,
		Name:        name,
		TokenPrefix: prefix,
		Scopes:      scopes,
		CreatedAt:   now,
		Token:       token,
	}, nil
}

// ListAppPasswords returns all app passwords for a user (no secrets).
func ListAppPasswords(conn *sqlite.Conn, userID string) ([]AppPassword, error) {
	var out []AppPassword
	err := sqlitex.Execute(conn,
		`SELECT id, user_id, name, token_prefix, scopes_json, created_at, last_used_at
		 FROM app_passwords WHERE user_id = ? ORDER BY created_at DESC`,
		&sqlitex.ExecOptions{
			Args: []any{userID},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				ap := AppPassword{
					ID:          stmt.ColumnText(0),
					UserID:      stmt.ColumnText(1),
					Name:        stmt.ColumnText(2),
					TokenPrefix: stmt.ColumnText(3),
				}
				if err := json.Unmarshal([]byte(stmt.ColumnText(4)), &ap.Scopes); err != nil {
					return fmt.Errorf("decode app password scopes: %w", err)
				}
				ap.CreatedAt = time.Unix(stmt.ColumnInt64(5), 0)
				if lu := stmt.ColumnInt64(6); lu > 0 {
					ap.LastUsedAt = time.Unix(lu, 0)
				}
				out = append(out, ap)
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("list app passwords: %w", err)
	}
	if out == nil {
		out = []AppPassword{}
	}
	return out, nil
}

// DeleteAppPassword revokes an app password. Returns false if not found for that user.
func DeleteAppPassword(conn *sqlite.Conn, userID, id string) (bool, error) {
	// Check ownership first.
	var found bool
	err := sqlitex.Execute(conn,
		`SELECT 1 FROM app_passwords WHERE id = ? AND user_id = ?`,
		&sqlitex.ExecOptions{
			Args: []any{id, userID},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				found = true
				return nil
			},
		},
	)
	if err != nil {
		return false, fmt.Errorf("lookup app password: %w", err)
	}
	if !found {
		return false, nil
	}
	err = sqlitex.Execute(conn,
		`DELETE FROM app_passwords WHERE id = ? AND user_id = ?`,
		&sqlitex.ExecOptions{Args: []any{id, userID}},
	)
	if err != nil {
		return false, fmt.Errorf("delete app password: %w", err)
	}
	return true, nil
}

// ValidateAppPassword authenticates an immutable username or verified email
// together with an app-password token. Both must belong to the same account.
func ValidateAppPassword(conn *sqlite.Conn, username, token string) (*User, *AppPassword, error) {
	username = strings.TrimSpace(username)
	// Strip common copy/paste junk from app password.
	token = strings.TrimSpace(token)
	token = strings.ReplaceAll(token, "\u00a0", "") // nbsp
	token = strings.ReplaceAll(token, "\r", "")
	token = strings.ReplaceAll(token, "\n", "")
	token = strings.ReplaceAll(token, " ", "")
	if token == "" {
		return nil, nil, nil
	}
	// App passwords are dar_ + 64 hex ≈ 68 chars. Reject obvious paste mistakes early.
	if len(token) > 512 {
		return nil, nil, nil
	}

	hash := hashAppToken(token)

	if username == "" {
		return nil, nil, nil
	}
	user, err := findUserByUsername(conn, username)
	if err != nil {
		return nil, nil, err
	}
	if user == nil {
		return nil, nil, nil
	}
	ap, err := matchAppPassword(conn, user.ID, hash)
	if err != nil {
		return nil, nil, err
	}
	if ap == nil {
		return nil, nil, nil
	}
	return user, ap, nil
}

func findUserByUsername(conn *sqlite.Conn, username string) (*User, error) {
	username = strings.TrimSpace(username)
	if username == "" {
		return nil, nil
	}
	var user *User
	err := sqlitex.Execute(conn,
		"SELECT "+userSelectCols+" FROM users WHERE username = ? COLLATE NOCASE OR email = ? COLLATE NOCASE LIMIT 1",
		&sqlitex.ExecOptions{
			Args: []any{username, username},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				user = scanUser(stmt)
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("find user by username: %w", err)
	}
	return user, nil
}

func matchAppPassword(conn *sqlite.Conn, userID, hash string) (*AppPassword, error) {
	var ap *AppPassword
	err := sqlitex.Execute(conn,
		`SELECT id, user_id, name, token_prefix, scopes_json, created_at, last_used_at
		 FROM app_passwords WHERE user_id = ? AND token_hash = ?`,
		&sqlitex.ExecOptions{
			Args: []any{userID, hash},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				var scopes []string
				if err := json.Unmarshal([]byte(stmt.ColumnText(4)), &scopes); err != nil {
					return err
				}
				ap = &AppPassword{ID: stmt.ColumnText(0), UserID: stmt.ColumnText(1), Name: stmt.ColumnText(2), TokenPrefix: stmt.ColumnText(3), Scopes: scopes, CreatedAt: time.Unix(stmt.ColumnInt64(5), 0)}
				if used := stmt.ColumnInt64(6); used > 0 {
					ap.LastUsedAt = time.Unix(used, 0)
				}
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("match app password: %w", err)
	}
	if ap == nil {
		return nil, nil
	}
	now := time.Now().Unix()
	_ = sqlitex.Execute(conn,
		`UPDATE app_passwords SET last_used_at = ? WHERE id = ?`,
		&sqlitex.ExecOptions{Args: []any{now, ap.ID}},
	)
	ap.LastUsedAt = time.Unix(now, 0)
	return ap, nil
}
