// Package auth provides user and session management for Dar.
package auth

import "time"

// UserProfile holds Nextcloud-style personal info fields (editable in Settings).
type UserProfile struct {
	Phone        string `json:"phone,omitempty"`
	Address      string `json:"address,omitempty"`
	Website      string `json:"website,omitempty"`
	Twitter      string `json:"twitter,omitempty"`
	Fediverse    string `json:"fediverse,omitempty"`
	Organisation string `json:"organisation,omitempty"`
	Role         string `json:"role,omitempty"`
	Headline     string `json:"headline,omitempty"`
	Biography    string `json:"biography,omitempty"`
	Location     string `json:"location,omitempty"`
	Language     string `json:"language,omitempty"`
	Locale       string `json:"locale,omitempty"`
	Birthday     string `json:"birthday,omitempty"`
}

// User represents an authenticated Dar user (from OIDC provider).
type User struct {
	ID          string
	Username    string
	Email       string
	DisplayName string
	AvatarURL   string
	OIDCIssuer  string
	OIDCSubject string
	IsAdmin     bool
	Profile     UserProfile
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

// AppPassword is a named device token (Nextcloud-style). The secret is never stored;
// Token is only set on Create (shown once to the user).
type AppPassword struct {
	ID          string    `json:"id"`
	UserID      string    `json:"user_id,omitempty"`
	Name        string    `json:"name"`
	TokenPrefix string    `json:"token_prefix"`
	Scopes      []string  `json:"scopes"`
	CreatedAt   time.Time `json:"created_at"`
	LastUsedAt  time.Time `json:"last_used_at,omitempty"`
	// Token is the plaintext secret; only populated on create.
	Token string `json:"token,omitempty"`
}
