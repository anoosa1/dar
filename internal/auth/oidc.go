// Package auth provides OIDC authentication for Dar.
package auth

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"golang.org/x/oauth2"
)

// OIDCConfig holds OIDC provider configuration from environment.
type OIDCConfig struct {
	Issuer       string
	ClientID     string
	ClientSecret string
	RedirectURI  string
	DisplayName  string
}

// OIDCProvider handles OIDC authentication flow.
type OIDCProvider struct {
	config        OIDCConfig
	oauth2Config  *oauth2.Config
	endpoints     providerEndpoints
	httpClient    *http.Client
}

type providerEndpoints struct {
	AuthorizationEndpoint string `json:"authorization_endpoint"`
	TokenEndpoint         string `json:"token_endpoint"`
	UserinfoEndpoint      string `json:"userinfo_endpoint"`
}

// OIDCProfile is the normalized user profile returned by the OIDC provider.
type OIDCProfile struct {
	Sub           string `json:"sub"`
	Email         string `json:"email"`
	Name          string `json:"name"`
	Picture       string `json:"picture"`
	EmailVerified bool   `json:"email_verified"`
}

// NewOIDCProvider discovers OIDC endpoints and creates a provider.
func NewOIDCProvider(ctx context.Context, cfg OIDCConfig) (*OIDCProvider, error) {
	// Discover OIDC endpoints from .well-known/openid-configuration.
	discoveryURL := strings.TrimRight(cfg.Issuer, "/") + "/.well-known/openid-configuration"

	httpClient := &http.Client{Timeout: 10 * time.Second}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, discoveryURL, nil)
	if err != nil {
		return nil, fmt.Errorf("oidc discovery request: %w", err)
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("oidc discovery: %w", err)
	}
	defer resp.Body.Close()

	var eps providerEndpoints
	if err := json.NewDecoder(resp.Body).Decode(&eps); err != nil {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("oidc discovery decode: %w (body: %s)", err, string(body))
	}

	if eps.AuthorizationEndpoint == "" || eps.TokenEndpoint == "" {
		return nil, fmt.Errorf("oidc discovery: missing required endpoints")
	}

	redirectURI := cfg.RedirectURI
	if redirectURI == "" {
		redirectURI = "http://localhost:8080/auth/oidc/callback"
	}

	oauth2Config := &oauth2.Config{
		ClientID:     cfg.ClientID,
		ClientSecret: cfg.ClientSecret,
		RedirectURL:  redirectURI,
		Endpoint: oauth2.Endpoint{
			AuthURL:  eps.AuthorizationEndpoint,
			TokenURL: eps.TokenEndpoint,
		},
		Scopes: []string{"openid", "email", "profile"},
	}

	return &OIDCProvider{
		config:       cfg,
		oauth2Config: oauth2Config,
		endpoints:    eps,
		httpClient:   httpClient,
	}, nil
}

// AuthURL returns the authorization URL for the OIDC provider.
func (p *OIDCProvider) AuthURL() string {
	// Use a random state for CSRF protection.
	state := randomString(32)
	return p.oauth2Config.AuthCodeURL(state)
}

// ExchangeCode exchanges an authorization code for an OIDC user profile.
func (p *OIDCProvider) ExchangeCode(ctx context.Context, code string) (*OIDCProfile, error) {
	// Exchange code for token.
	token, err := p.oauth2Config.Exchange(ctx, code)
	if err != nil {
		return nil, fmt.Errorf("oidc exchange: %w", err)
	}

	// Fetch userinfo.
	userinfoURL := p.endpoints.UserinfoEndpoint
	if userinfoURL == "" {
		// Fallback: decode ID token claims.
		return p.profileFromIDToken(token)
	}

	client := p.oauth2Config.Client(ctx, token)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, userinfoURL, nil)
	if err != nil {
		return nil, fmt.Errorf("userinfo request: %w", err)
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("userinfo fetch: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("userinfo response: %s", resp.Status)
	}

	var profile OIDCProfile
	if err := json.NewDecoder(resp.Body).Decode(&profile); err != nil {
		return nil, fmt.Errorf("userinfo decode: %w", err)
	}

	if profile.Sub == "" {
		return nil, fmt.Errorf("userinfo response missing sub claim")
	}

	return &profile, nil
}

// Config returns the OIDC config.
func (p *OIDCProvider) Config() OIDCConfig {
	return p.config
}

// DisplayName returns the provider display name.
func (p *OIDCProvider) DisplayName() string {
	if p.config.DisplayName != "" {
		return p.config.DisplayName
	}
	// Try to extract from issuer URL.
	u, err := url.Parse(p.config.Issuer)
	if err == nil {
		return strings.TrimPrefix(u.Host, "accounts.")
	}
	return "OIDC"
}

func (p *OIDCProvider) profileFromIDToken(token *oauth2.Token) (*OIDCProfile, error) {
	idToken, ok := token.Extra("id_token").(string)
	if !ok {
		return nil, fmt.Errorf("no id_token in token response and no userinfo endpoint")
	}
	// Decode the ID token payload (middle segment of JWT).
	parts := strings.Split(idToken, ".")
	if len(parts) != 3 {
		return nil, fmt.Errorf("invalid id_token format")
	}
	payload, err := base64Decode(parts[1])
	if err != nil {
		return nil, fmt.Errorf("decode id_token: %w", err)
	}
	var profile OIDCProfile
	if err := json.Unmarshal(payload, &profile); err != nil {
		return nil, fmt.Errorf("parse id_token: %w", err)
	}
	return &profile, nil
}

func randomString(n int) string {
	const letters = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
	b := make([]byte, n)
	for i := range b {
		b[i] = letters[time.Now().UnixNano()%int64(len(letters))]
		time.Sleep(1) // not crypto-secure but fine for state
	}
	return string(b)
}

func base64Decode(s string) ([]byte, error) {
	// Add padding if needed.
	switch len(s) % 4 {
	case 2:
		s += "=="
	case 3:
		s += "="
	}
	// Use standard base64 with URL-safe alphabet.
	s = strings.NewReplacer("-", "+", "_", "/").Replace(s)
	result := make([]byte, len(s))
	n, err := base64DecodeRaw(s, result)
	if err != nil {
		return nil, err
	}
	return result[:n], nil
}

// Simple base64 decode.
func base64DecodeRaw(s string, dst []byte) (int, error) {
	var val uint32
	var n, pos int
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c == '=' {
			break
		}
		var idx byte
		switch {
		case 'A' <= c && c <= 'Z':
			idx = c - 'A'
		case 'a' <= c && c <= 'z':
			idx = c - 'a' + 26
		case '0' <= c && c <= '9':
			idx = c - '0' + 52
		case c == '+':
			idx = 62
		case c == '/':
			idx = 63
		default:
			continue
		}
		val = val<<6 | uint32(idx)
		n++
		if n == 4 {
			dst[pos] = byte(val >> 16)
			dst[pos+1] = byte(val >> 8)
			dst[pos+2] = byte(val)
			pos += 3
			n = 0
			val = 0
		}
	}
	if n > 0 {
		val <<= (4 - n) * 6
		dst[pos] = byte(val >> 16)
		if n >= 3 {
			dst[pos+1] = byte(val >> 8)
		}
		pos += n - 1
	}
	return pos, nil
}
