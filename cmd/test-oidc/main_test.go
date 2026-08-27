package main

import (
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func decodeObject(t *testing.T, recorder *httptest.ResponseRecorder) map[string]any {
	t.Helper()
	var value map[string]any
	if err := json.Unmarshal(recorder.Body.Bytes(), &value); err != nil {
		t.Fatalf("decode response %q: %v", recorder.Body.String(), err)
	}
	return value
}

func TestDiscoveryDocument(t *testing.T) {
	recorder := httptest.NewRecorder()
	handleDiscovery("http://oidc.example.test:9001")(recorder, httptest.NewRequest(http.MethodGet, "/.well-known/openid-configuration", nil))
	if recorder.Code != http.StatusOK || recorder.Header().Get("Content-Type") != "application/json" {
		t.Fatalf("response = %d %#v", recorder.Code, recorder.Header())
	}
	document := decodeObject(t, recorder)
	if document["issuer"] != "http://oidc.example.test:9001" ||
		document["authorization_endpoint"] != "http://oidc.example.test:9001/authorize" ||
		document["token_endpoint"] != "http://oidc.example.test:9001/token" ||
		document["userinfo_endpoint"] != "http://oidc.example.test:9001/userinfo" {
		t.Fatalf("discovery = %#v", document)
	}
}

func TestAuthorizeRedirectValidationAndState(t *testing.T) {
	for _, tc := range []struct {
		name   string
		target string
		status int
	}{
		{"missing redirect", "/authorize?state=abc", http.StatusBadRequest},
		{"invalid redirect", "/authorize?redirect_uri=%25", http.StatusBadRequest},
	} {
		t.Run(tc.name, func(t *testing.T) {
			recorder := httptest.NewRecorder()
			handleAuthorize(recorder, httptest.NewRequest(http.MethodGet, tc.target, nil))
			if recorder.Code != tc.status {
				t.Fatalf("status = %d body=%s", recorder.Code, recorder.Body.String())
			}
		})
	}

	redirect := url.QueryEscape("http://dar.example.test/auth/oidc/callback?existing=value")
	recorder := httptest.NewRecorder()
	handleAuthorize(recorder, httptest.NewRequest(http.MethodGet, "/authorize?redirect_uri="+redirect+"&state=state-value", nil))
	if recorder.Code != http.StatusFound {
		t.Fatalf("status = %d body=%s", recorder.Code, recorder.Body.String())
	}
	location, err := url.Parse(recorder.Header().Get("Location"))
	if err != nil {
		t.Fatal(err)
	}
	if location.Query().Get("existing") != "value" ||
		location.Query().Get("code") != "test-auth-code-12345" ||
		location.Query().Get("state") != "state-value" {
		t.Fatalf("redirect location = %s", location)
	}

	withoutState := httptest.NewRecorder()
	handleAuthorize(withoutState, httptest.NewRequest(http.MethodGet, "/authorize?redirect_uri="+redirect, nil))
	if strings.Contains(withoutState.Header().Get("Location"), "state=") {
		t.Fatalf("unexpected state in %q", withoutState.Header().Get("Location"))
	}
}

func TestTokenUserinfoAndHealth(t *testing.T) {
	get := httptest.NewRecorder()
	handleToken(get, httptest.NewRequest(http.MethodGet, "/token", nil))
	if get.Code != http.StatusMethodNotAllowed {
		t.Fatalf("GET token status = %d", get.Code)
	}
	malformed := httptest.NewRecorder()
	malformedRequest := httptest.NewRequest(http.MethodPost, "/token", strings.NewReader("code=%zz"))
	malformedRequest.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	handleToken(malformed, malformedRequest)
	if malformed.Code != http.StatusBadRequest {
		t.Fatalf("malformed token status = %d", malformed.Code)
	}

	post := httptest.NewRecorder()
	request := httptest.NewRequest(http.MethodPost, "/token", strings.NewReader("grant_type=authorization_code&code=test-auth-code-12345"))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	handleToken(post, request)
	token := decodeObject(t, post)
	if token["access_token"] != "test-access-token-abc" || token["token_type"] != "Bearer" || token["expires_in"] != float64(3600) {
		t.Fatalf("token = %#v", token)
	}

	userinfo := httptest.NewRecorder()
	handleUserinfo(userinfo, httptest.NewRequest(http.MethodGet, "/userinfo", nil))
	profile := decodeObject(t, userinfo)
	if profile["sub"] != "test-user-123" || profile["email"] != "test@dar.local" || profile["email_verified"] != true {
		t.Fatalf("userinfo = %#v", profile)
	}

	health := httptest.NewRecorder()
	handleHealth(health, httptest.NewRequest(http.MethodGet, "/health", nil))
	if status := decodeObject(t, health)["status"]; status != "ok" {
		t.Fatalf("health status = %#v", status)
	}
}

func TestRunRegistersAnIsolatedHandler(t *testing.T) {
	sentinel := errors.New("stop")
	called := false
	err := run("127.0.0.1", 9123, func(address string, handler http.Handler) error {
		called = true
		if address != ":9123" {
			t.Fatalf("address = %q", address)
		}
		recorder := httptest.NewRecorder()
		handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/.well-known/openid-configuration", nil))
		if recorder.Code != http.StatusOK || decodeObject(t, recorder)["issuer"] != "http://127.0.0.1:9123" {
			t.Fatalf("discovery response = %d %s", recorder.Code, recorder.Body.String())
		}
		return sentinel
	})
	if !called || !errors.Is(err, sentinel) {
		t.Fatalf("run called=%v err=%v", called, err)
	}

	notFound := httptest.NewRecorder()
	newHandler("http://example.test").ServeHTTP(notFound, httptest.NewRequest(http.MethodGet, "/missing", nil))
	if notFound.Code != http.StatusNotFound {
		t.Fatalf("missing route status = %d", notFound.Code)
	}
}
