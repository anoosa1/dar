package auth

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func oidcFixture(t *testing.T, userinfo func(http.ResponseWriter, *http.Request)) (*OIDCProvider, *httptest.Server) {
	t.Helper()
	mux := http.NewServeMux()
	server := httptest.NewServer(mux)
	mux.HandleFunc("/.well-known/openid-configuration", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{
			"authorization_endpoint": server.URL + "/authorize",
			"token_endpoint":         server.URL + "/token",
			"userinfo_endpoint":      server.URL + "/userinfo",
		})
	})
	mux.HandleFunc("/token", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "token", "token_type": "Bearer", "expires_in": 60})
	})
	mux.HandleFunc("/userinfo", userinfo)
	provider, err := NewOIDCProvider(context.Background(), OIDCConfig{Issuer: server.URL, ClientID: "client", ClientSecret: "secret"})
	if err != nil {
		server.Close()
		t.Fatal(err)
	}
	return provider, server
}

func TestOIDCUserinfoIsRequiredAndVerified(t *testing.T) {
	provider, fixture := oidcFixture(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer token" {
			t.Errorf("authorization=%q", r.Header.Get("Authorization"))
		}
		_ = json.NewEncoder(w).Encode(OIDCProfile{Sub: "subject", Email: "alice@example.test", EmailVerified: true, PreferredUsername: "alice"})
	})
	defer fixture.Close()
	profile, err := provider.ExchangeCode(context.Background(), "code", "verifier")
	if err != nil || profile.Sub != "subject" || profile.Email != "alice@example.test" {
		t.Fatalf("profile=%+v err=%v", profile, err)
	}

	for _, profile := range []OIDCProfile{
		{Email: "alice@example.test", EmailVerified: true},
		{Sub: "subject", Email: "alice@example.test", EmailVerified: false},
		{Sub: "subject", EmailVerified: true},
	} {
		profile := profile
		p, server := oidcFixture(t, func(w http.ResponseWriter, _ *http.Request) { _ = json.NewEncoder(w).Encode(profile) })
		if _, err := p.ExchangeCode(context.Background(), "code", "verifier"); err == nil {
			server.Close()
			t.Fatalf("invalid profile accepted: %+v", profile)
		}
		server.Close()
	}
}

func TestOIDCUserinfoFailureAndBodyLimit(t *testing.T) {
	p, server := oidcFixture(t, func(w http.ResponseWriter, _ *http.Request) { http.Error(w, "unavailable", http.StatusBadGateway) })
	if _, err := p.ExchangeCode(context.Background(), "code", "verifier"); err == nil {
		t.Fatal("userinfo failure was accepted")
	}
	server.Close()

	p, server = oidcFixture(t, func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"sub":"subject","email":"alice@example.test","email_verified":true,"padding":"` + strings.Repeat("x", (1<<20)+1) + `"}`))
	})
	defer server.Close()
	if _, err := p.ExchangeCode(context.Background(), "code", "verifier"); err == nil || !strings.Contains(err.Error(), "exceeds") {
		t.Fatalf("oversized userinfo error=%v", err)
	}
}

func TestOIDCSecureURLsStateAndDisplayName(t *testing.T) {
	for _, raw := range []string{"http://provider.example", "ftp://localhost/provider", "not-a-url"} {
		if err := requireSecureOIDCURL(raw); err == nil {
			t.Fatalf("insecure URL %q accepted", raw)
		}
	}
	for _, raw := range []string{"https://provider.example", "http://localhost:8080", "http://127.0.0.1:8080", "http://[::1]:8080"} {
		if err := requireSecureOIDCURL(raw); err != nil {
			t.Fatalf("URL %q rejected: %v", raw, err)
		}
	}
	state1, err := NewOIDCState()
	if err != nil {
		t.Fatal(err)
	}
	state2, _ := NewOIDCState()
	if len(state1) != 64 || state1 == state2 {
		t.Fatalf("states %q %q", state1, state2)
	}
	verifier1, err := NewPKCEVerifier()
	if err != nil {
		t.Fatal(err)
	}
	verifier2, _ := NewPKCEVerifier()
	if len(verifier1) != 43 || verifier1 == verifier2 {
		t.Fatalf("verifiers %q %q", verifier1, verifier2)
	}
	p := &OIDCProvider{config: OIDCConfig{Issuer: "https://accounts.example.test"}}
	if got := p.DisplayName(); got != "example.test" {
		t.Fatalf("display name=%q", got)
	}
}
