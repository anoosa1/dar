// Package auth provides OIDC authentication for Dar.
package auth

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
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
	config       OIDCConfig
	oauth2Config *oauth2.Config
	endpoints    providerEndpoints
	httpClient   *http.Client
}

type providerEndpoints struct {
	AuthorizationEndpoint string `json:"authorization_endpoint"`
	TokenEndpoint         string `json:"token_endpoint"`
	UserinfoEndpoint      string `json:"userinfo_endpoint"`
}

// OIDCProfile is the normalized user profile returned by the OIDC provider.
type OIDCProfile struct {
	Sub               string   `json:"sub"`
	Email             string   `json:"email"`
	PreferredUsername string   `json:"preferred_username"`
	Name              string   `json:"name"`
	Picture           string   `json:"picture"`
	EmailVerified     bool     `json:"email_verified"`
	Groups            []string `json:"groups"`
}

// InGroup reports whether the identity provider included group in the standard
// groups claim. Group matching is exact because providers commonly use
// case-sensitive identifiers.
func (p *OIDCProfile) InGroup(group string) bool {
	group = strings.TrimSpace(group)
	if group == "" {
		return false
	}
	for _, candidate := range p.Groups {
		if candidate == group {
			return true
		}
	}
	return false
}

// NewOIDCProvider discovers OIDC endpoints and creates a provider.
func NewOIDCProvider(ctx context.Context, cfg OIDCConfig) (*OIDCProvider, error) {
	if err := requireSecureOIDCURL(cfg.Issuer); err != nil {
		return nil, fmt.Errorf("oidc issuer: %w", err)
	}
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
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("oidc discovery response: %s", resp.Status)
	}

	var eps providerEndpoints
	if err := decodeLimitedJSON(resp.Body, &eps, 1<<20); err != nil {
		return nil, fmt.Errorf("oidc discovery decode: %w", err)
	}

	if eps.AuthorizationEndpoint == "" || eps.TokenEndpoint == "" || eps.UserinfoEndpoint == "" {
		return nil, fmt.Errorf("oidc discovery: missing required endpoints")
	}
	for label, endpoint := range map[string]string{"authorization": eps.AuthorizationEndpoint, "token": eps.TokenEndpoint, "userinfo": eps.UserinfoEndpoint} {
		if err := requireSecureOIDCURL(endpoint); err != nil {
			return nil, fmt.Errorf("oidc %s endpoint: %w", label, err)
		}
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

// AuthURL returns an authorization URL bound to a caller-persisted state and
// PKCE verifier.
func (p *OIDCProvider) AuthURL(state, verifier string) string {
	challenge := sha256.Sum256([]byte(verifier))
	return p.oauth2Config.AuthCodeURL(state,
		oauth2.SetAuthURLParam("code_challenge", base64.RawURLEncoding.EncodeToString(challenge[:])),
		oauth2.SetAuthURLParam("code_challenge_method", "S256"),
	)
}

// ExchangeCode exchanges an authorization code for an OIDC user profile.
func (p *OIDCProvider) ExchangeCode(ctx context.Context, code, verifier string) (*OIDCProfile, error) {
	ctx = context.WithValue(ctx, oauth2.HTTPClient, p.httpClient)
	token, err := p.oauth2Config.Exchange(ctx, code, oauth2.VerifierOption(verifier))
	if err != nil {
		return nil, fmt.Errorf("oidc exchange (token_url=%s): %w", p.endpoints.TokenEndpoint, err)
	}
	userinfoURL := p.endpoints.UserinfoEndpoint

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
	if err := decodeLimitedJSON(resp.Body, &profile, 1<<20); err != nil {
		return nil, fmt.Errorf("userinfo decode: %w", err)
	}

	if profile.Sub == "" {
		return nil, fmt.Errorf("userinfo response missing sub claim")
	}
	if strings.TrimSpace(profile.Email) == "" || !profile.EmailVerified {
		return nil, fmt.Errorf("userinfo response requires a verified email")
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

// NewOIDCState returns a cryptographically random one-time state value.
func NewOIDCState() (string, error) {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", fmt.Errorf("generate OIDC state: %w", err)
	}
	return hex.EncodeToString(b[:]), nil
}

// NewPKCEVerifier returns an RFC 7636 verifier with 256 bits of entropy.
func NewPKCEVerifier() (string, error) {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", fmt.Errorf("generate PKCE verifier: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(b[:]), nil
}

func decodeLimitedJSON(r io.Reader, dst any, max int64) error {
	data, err := io.ReadAll(io.LimitReader(r, max+1))
	if err != nil {
		return err
	}
	if int64(len(data)) > max {
		return fmt.Errorf("response exceeds %d bytes", max)
	}
	if err := json.Unmarshal(data, dst); err != nil {
		return err
	}
	return nil
}

func requireSecureOIDCURL(raw string) error {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Hostname() == "" {
		return fmt.Errorf("invalid URL")
	}
	if strings.EqualFold(u.Scheme, "https") {
		return nil
	}
	host := strings.ToLower(u.Hostname())
	if strings.EqualFold(u.Scheme, "http") && (host == "localhost" || net.ParseIP(host).IsLoopback()) {
		return nil
	}
	return fmt.Errorf("HTTPS is required outside loopback development")
}
