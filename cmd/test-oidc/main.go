// Dummy OIDC test server for testing Dar's auth flow.
//   dar-test-oidc --port 9001 --host astra
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"net/url"
)

var (
	port = flag.Int("port", 9001, "HTTP port")
	host = flag.String("host", "astra", "Hostname for OIDC URLs")
)

func main() {
	flag.Parse()

	base := fmt.Sprintf("http://%s:%d", *host, *port)
	log.Printf("Dummy OIDC server on %s", base)

	http.HandleFunc("/.well-known/openid-configuration", handleDiscovery(base))
	http.HandleFunc("/authorize", handleAuthorize)
	http.HandleFunc("/token", handleToken)
	http.HandleFunc("/userinfo", handleUserinfo)
	http.HandleFunc("/health", handleHealth)

	addr := fmt.Sprintf(":%d", *port)
	log.Fatal(http.ListenAndServe(addr, nil))
}

func handleDiscovery(base string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, map[string]any{
			"issuer":                 base,
			"authorization_endpoint": base + "/authorize",
			"token_endpoint":         base + "/token",
			"userinfo_endpoint":      base + "/userinfo",
			"jwks_uri":               base + "/jwks",
			"scopes_supported":       []string{"openid", "email", "profile"},
			"response_types_supported": []string{"code"},
			"grant_types_supported":    []string{"authorization_code"},
			"subject_types_supported":  []string{"public"},
			"id_token_signing_alg_values_supported": []string{"none"},
		})
	}
}

func handleAuthorize(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	redirectURI := q.Get("redirect_uri")
	state := q.Get("state")

	if redirectURI == "" {
		http.Error(w, "missing redirect_uri", http.StatusBadRequest)
		return
	}

	dest, err := url.Parse(redirectURI)
	if err != nil {
		http.Error(w, "invalid redirect_uri", http.StatusBadRequest)
		return
	}

	vals := dest.Query()
	vals.Set("code", "test-auth-code-12345")
	if state != "" {
		vals.Set("state", state)
	}
	dest.RawQuery = vals.Encode()

	http.Redirect(w, r, dest.String(), http.StatusFound)
}

func handleToken(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "bad request", http.StatusBadRequest)
		return
	}

	// Accept any code — this is a dummy server.
	writeJSON(w, map[string]any{
		"access_token": "test-access-token-abc",
		"token_type":   "Bearer",
		"expires_in":   3600,
		"scope":        "openid email profile",
	})
}

func handleUserinfo(w http.ResponseWriter, r *http.Request) {
	// Accept any Bearer token.
	writeJSON(w, map[string]any{
		"sub":            "test-user-123",
		"email":          "test@dar.local",
		"name":           "Test User",
		"picture":        "",
		"email_verified": true,
	})
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{"status": "ok"})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}
