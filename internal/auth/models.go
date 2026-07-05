// Package auth provides user and session management for Dar.
package auth

import "time"

// User represents an authenticated Dar user (from OIDC provider).
type User struct {
	ID          string
	Email       string
	DisplayName string
	AvatarURL   string
	Provider    string
	ProviderID  string
	CreatedAt   time.Time
	LastLoginAt time.Time
}

// Session represents an active login session.
type Session struct {
	ID        string
	UserID    string
	CreatedAt time.Time
	ExpiresAt time.Time
}
