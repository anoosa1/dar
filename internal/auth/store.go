package auth

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"time"

	"zombiezen.com/go/sqlite"
	"zombiezen.com/go/sqlite/sqlitex"
)

// userMigrations contains the SQL migrations for users and sessions tables.
var userMigrations = []string{
	`CREATE TABLE IF NOT EXISTS users (
		id TEXT PRIMARY KEY,
		email TEXT NOT NULL UNIQUE,
		display_name TEXT NOT NULL DEFAULT '',
		avatar_url TEXT NOT NULL DEFAULT '',
		provider TEXT NOT NULL DEFAULT '',
		provider_id TEXT NOT NULL DEFAULT '',
		created_at INTEGER NOT NULL DEFAULT 0,
		last_login_at INTEGER NOT NULL DEFAULT 0
	);`,
	`CREATE TABLE IF NOT EXISTS sessions (
		id TEXT PRIMARY KEY,
		user_id TEXT NOT NULL REFERENCES users(id),
		created_at INTEGER NOT NULL DEFAULT 0,
		expires_at INTEGER NOT NULL DEFAULT 0
	);`,
	`CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);`,
}

// Migrations returns the SQL migrations for the auth subsystem.
func Migrations() []string {
	return userMigrations
}

// FindOrCreateUser finds an existing user by provider+provider_id, or creates a new one.
// Falls back to email lookup if the provider match fails (handles OIDC config changes).
func FindOrCreateUser(ctx context.Context, conn *sqlite.Conn, email, displayName, avatarURL, provider, providerID string) (*User, bool, error) {
	// Look up by provider + provider_id.
	existing, err := findUserByProvider(conn, provider, providerID)
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

	// Fallback: look up by email (handles same email logging in via different provider/config).
	byEmail, err := findUserByEmail(conn, email)
	if err != nil {
		return nil, false, err
	}
	if byEmail != nil {
		// Update provider info to match current config.
		now := time.Now().Unix()
		err := sqlitex.Execute(conn,
			`UPDATE users SET display_name = ?, avatar_url = ?, provider = ?, provider_id = ?, last_login_at = ? WHERE id = ?`,
			&sqlitex.ExecOptions{Args: []any{displayName, avatarURL, provider, providerID, now, byEmail.ID}},
		)
		if err != nil {
			return nil, false, fmt.Errorf("update user provider: %w", err)
		}
		byEmail.DisplayName = displayName
		byEmail.AvatarURL = avatarURL
		byEmail.Provider = provider
		byEmail.ProviderID = providerID
		byEmail.LastLoginAt = time.Unix(now, 0)
		return byEmail, false, nil
	}

	// Create new user.
	id := newID()
	now := time.Now().Unix()
	err = sqlitex.Execute(conn,
		`INSERT INTO users (id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
		&sqlitex.ExecOptions{Args: []any{id, email, displayName, avatarURL, provider, providerID, now, now}},
	)
	if err != nil {
		return nil, false, fmt.Errorf("insert user: %w", err)
	}

	return &User{
		ID:          id,
		Email:       email,
		DisplayName: displayName,
		AvatarURL:   avatarURL,
		Provider:    provider,
		ProviderID:  providerID,
		CreatedAt:   time.Unix(now, 0),
		LastLoginAt: time.Unix(now, 0),
	}, true, nil
}

// GetUserByID returns a user by their ID, or nil if not found.
func GetUserByID(conn *sqlite.Conn, id string) (*User, error) {
	var u *User
	err := sqlitex.Execute(conn,
		"SELECT id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at FROM users WHERE id = ?",
		&sqlitex.ExecOptions{
			Args: []any{id},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = &User{
					ID:          stmt.ColumnText(0),
					Email:       stmt.ColumnText(1),
					DisplayName: stmt.ColumnText(2),
					AvatarURL:   stmt.ColumnText(3),
					Provider:    stmt.ColumnText(4),
					ProviderID:  stmt.ColumnText(5),
					CreatedAt:   time.Unix(stmt.ColumnInt64(6), 0),
					LastLoginAt: time.Unix(stmt.ColumnInt64(7), 0),
				}
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("get user: %w", err)
	}
	return u, nil
}

func findUserByProvider(conn *sqlite.Conn, provider, providerID string) (*User, error) {
	var u *User
	err := sqlitex.Execute(conn,
		"SELECT id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at FROM users WHERE provider = ? AND provider_id = ?",
		&sqlitex.ExecOptions{
			Args: []any{provider, providerID},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = &User{
					ID:          stmt.ColumnText(0),
					Email:       stmt.ColumnText(1),
					DisplayName: stmt.ColumnText(2),
					AvatarURL:   stmt.ColumnText(3),
					Provider:    stmt.ColumnText(4),
					ProviderID:  stmt.ColumnText(5),
					CreatedAt:   time.Unix(stmt.ColumnInt64(6), 0),
					LastLoginAt: time.Unix(stmt.ColumnInt64(7), 0),
				}
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
	var u *User
	err := sqlitex.Execute(conn,
		"SELECT id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at FROM users WHERE email = ?",
		&sqlitex.ExecOptions{
			Args: []any{email},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				u = &User{
					ID:          stmt.ColumnText(0),
					Email:       stmt.ColumnText(1),
					DisplayName: stmt.ColumnText(2),
					AvatarURL:   stmt.ColumnText(3),
					Provider:    stmt.ColumnText(4),
					ProviderID:  stmt.ColumnText(5),
					CreatedAt:   time.Unix(stmt.ColumnInt64(6), 0),
					LastLoginAt: time.Unix(stmt.ColumnInt64(7), 0),
				}
				return nil
			},
		},
	)
	if err != nil {
		return nil, fmt.Errorf("find user by email: %w", err)
	}
	return u, nil
}

// CreateSession creates a new session for a user (30-day expiry).
func CreateSession(conn *sqlite.Conn, userID string) (*Session, error) {
	id := newSessionID()
	now := time.Now()
	expires := now.Add(30 * 24 * time.Hour)

	err := sqlitex.Execute(conn,
		`INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)`,
		&sqlitex.ExecOptions{Args: []any{id, userID, now.Unix(), expires.Unix()}},
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
	var s *Session
	err := sqlitex.Execute(conn,
		"SELECT id, user_id, created_at, expires_at FROM sessions WHERE id = ?",
		&sqlitex.ExecOptions{
			Args: []any{sessionID},
			ResultFunc: func(stmt *sqlite.Stmt) error {
				s = &Session{
					ID:        stmt.ColumnText(0),
					UserID:    stmt.ColumnText(1),
					CreatedAt: time.Unix(stmt.ColumnInt64(2), 0),
					ExpiresAt: time.Unix(stmt.ColumnInt64(3), 0),
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
		_ = sqlitex.Execute(conn, "DELETE FROM sessions WHERE id = ?",
			&sqlitex.ExecOptions{Args: []any{sessionID}},
		)
		return nil, nil
	}
	return s, nil
}

// DeleteSession removes a session from the database.
func DeleteSession(conn *sqlite.Conn, sessionID string) error {
	return sqlitex.Execute(conn, "DELETE FROM sessions WHERE id = ?",
		&sqlitex.ExecOptions{Args: []any{sessionID}},
	)
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
