package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/apps"
	calendarApp "github.com/anoosa/dar/internal/apps/calendar"
	prayerApp "github.com/anoosa/dar/internal/apps/prayer"
	todoApp "github.com/anoosa/dar/internal/apps/todo"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/preferences"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/testutil"
)

func testOIDCProvider(t *testing.T) (*auth.OIDCProvider, *httptest.Server) {
	return testOIDCProviderProfile(t, auth.OIDCProfile{Sub: "subject", Email: "alice@example.test", PreferredUsername: "alice", Name: "Alice", EmailVerified: true})
}

func testOIDCProviderProfile(t *testing.T, profile auth.OIDCProfile) (*auth.OIDCProvider, *httptest.Server) {
	t.Helper()
	mux := http.NewServeMux()
	fixture := httptest.NewServer(mux)
	mux.HandleFunc("/.well-known/openid-configuration", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{
			"authorization_endpoint": fixture.URL + "/authorize",
			"token_endpoint":         fixture.URL + "/token",
			"userinfo_endpoint":      fixture.URL + "/userinfo",
		})
	})
	mux.HandleFunc("/token", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "access", "token_type": "Bearer", "expires_in": 60})
	})
	mux.HandleFunc("/userinfo", func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(profile)
	})
	provider, err := auth.NewOIDCProvider(context.Background(), auth.OIDCConfig{Issuer: fixture.URL, ClientID: "client", ClientSecret: "secret", DisplayName: "Test OIDC"})
	if err != nil {
		fixture.Close()
		t.Fatal(err)
	}
	return provider, fixture
}

func oidcCallbackRequest(t *testing.T, srv *server.Server, code string) *http.Request {
	t.Helper()
	state, _, transaction, err := srv.NewOIDCTransaction()
	if err != nil {
		t.Fatal(err)
	}
	request := httptest.NewRequest(http.MethodGet, "https://dar.example/auth/oidc/callback?state="+state+"&code="+code, nil)
	request.AddCookie(&http.Cookie{Name: "dar_oidc_transaction", Value: transaction})
	return request
}

type routeTestApp struct{}

func (routeTestApp) Name() string        { return "routes" }
func (routeTestApp) DisplayName() string { return "Routes" }
func (routeTestApp) Icon() string        { return "" }
func (routeTestApp) Description() string { return "" }
func (routeTestApp) AppBarOrder() int    { return 1 }
func (routeTestApp) Routes() []apps.Route {
	ok := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	return []apps.Route{
		{Method: http.MethodGet, Pattern: "/public-test", Auth: apps.AuthPublic, Handler: ok},
		{Method: http.MethodGet, Pattern: "/browser-test", Auth: apps.AuthBrowser, Handler: ok},
		{Method: http.MethodGet, Pattern: "/api-test", Auth: apps.AuthAPI, Handler: ok},
	}
}

func TestEnvString(t *testing.T) {
	t.Setenv("DAR_TEST_STRING", " configured ")
	if got := envString("DAR_TEST_STRING", "fallback"); got != "configured" {
		t.Fatalf("envString = %q", got)
	}
	if got := envString("DAR_TEST_MISSING", "fallback"); got != "fallback" {
		t.Fatalf("fallback = %q", got)
	}
}

func TestPrayerMigrationsUpgradeExistingDatabase(t *testing.T) {
	all := serverMigrations()
	newCount := len(todoApp.GeneratedTaskMigrations()) + len(prayerApp.Migrations())
	legacy := all[:len(all)-(2*newCount)]
	path := filepath.Join(t.TempDir(), "prayer-upgrade.sqlite")

	oldDatabase, err := db.Open(path, legacy)
	if err != nil {
		t.Fatal(err)
	}
	if err := oldDatabase.Close(); err != nil {
		t.Fatal(err)
	}

	database, err := db.Open(path, all)
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	for _, table := range []string{"todo_generated_lists", "todo_generated_tasks", "prayer_occurrences"} {
		if err := database.Exec(context.Background(), "SELECT * FROM "+table+" LIMIT 0;"); err != nil {
			t.Errorf("upgraded database is missing %s: %v", table, err)
		}
	}
}

func TestPrayerMigrationsRepairPreviouslyAttemptedUpgrade(t *testing.T) {
	path := filepath.Join(t.TempDir(), "prayer-repair.sqlite")
	legacy := establishedServerMigrations()
	oldDatabase, err := db.Open(path, legacy)
	if err != nil {
		t.Fatal(err)
	}
	if err := oldDatabase.Close(); err != nil {
		t.Fatal(err)
	}

	// This is the ordering shipped by the first Prayer build: the new tables
	// were inserted before older app migrations instead of appended globally.
	prefixCount := len(auth.Migrations()) + len(preferences.Migrations()) + len(calendarApp.Migrations()) + len(todoApp.Migrations())
	broken := append([]string{}, legacy[:prefixCount]...)
	broken = append(broken, todoApp.GeneratedTaskMigrations()...)
	broken = append(broken, prayerApp.Migrations()...)
	broken = append(broken, legacy[prefixCount:]...)
	brokenDatabase, err := db.Open(path, broken)
	if err != nil {
		t.Fatal(err)
	}
	if err := brokenDatabase.Close(); err != nil {
		t.Fatal(err)
	}

	database, err := db.Open(path, serverMigrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	for _, table := range []string{"todo_generated_lists", "todo_generated_tasks", "prayer_occurrences"} {
		if err := database.Exec(context.Background(), "SELECT * FROM "+table+" LIMIT 0;"); err != nil {
			t.Errorf("repaired database is missing %s: %v", table, err)
		}
	}
}

func TestConfiguredDataRootIsAbsolute(t *testing.T) {
	workingDirectory, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name  string
		value string
		want  string
	}{
		{name: "default", want: filepath.Join(workingDirectory, "data")},
		{name: "relative override", value: filepath.Join("var", "dar"), want: filepath.Join(workingDirectory, "var", "dar")},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("DAR_DATA_ROOT", tc.value)
			got, err := configuredDataRoot()
			if err != nil {
				t.Fatal(err)
			}
			if !filepath.IsAbs(got) {
				t.Fatalf("configuredDataRoot = %q, want absolute path", got)
			}
			if got != filepath.Clean(tc.want) {
				t.Fatalf("configuredDataRoot = %q, want %q", got, filepath.Clean(tc.want))
			}
		})
	}
}

func TestEnvInt(t *testing.T) {
	t.Setenv("DAR_TEST_INT", "17")
	if got := envInt("DAR_TEST_INT", 5); got != 17 {
		t.Fatalf("envInt = %d", got)
	}
	for _, value := range []string{"", "invalid", "0", "-2"} {
		if err := os.Setenv("DAR_TEST_INT", value); err != nil {
			t.Fatal(err)
		}
		if got := envInt("DAR_TEST_INT", 5); got != 5 {
			t.Fatalf("envInt(%q) = %d", value, got)
		}
	}
}

func TestRunRejectsInvalidConfiguration(t *testing.T) {
	t.Setenv("DAR_OIDC_ISSUER", "")
	t.Setenv("DAR_OIDC_CLIENT_ID", "")
	t.Setenv("DAR_OIDC_CLIENT_SECRET", "")
	for _, tc := range []struct {
		name string
		env  string
		val  string
		args []string
	}{
		{name: "flag", args: []string{"--unknown"}},
		{name: "base URL", env: "DAR_BASE_URL", val: "not a URL"},
		{name: "proxy", env: "DAR_TRUSTED_PROXIES", val: "invalid"},
		{name: "outbound CIDR", env: "DAR_OUTBOUND_ALLOWED_CIDRS", val: "invalid"},
		{name: "quota", env: "DAR_USER_STORAGE_QUOTA_BYTES", val: "-1"},
		{name: "process capacity", env: "DAR_CODEX_MAX_PROCESSES", val: "invalid"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if tc.env != "" {
				t.Setenv(tc.env, tc.val)
			}
			t.Setenv("DAR_DATA_ROOT", t.TempDir())
			args := append([]string{"--db", filepath.Join(t.TempDir(), "run.sqlite")}, tc.args...)
			if err := run(context.Background(), args); err == nil {
				t.Fatal("invalid configuration was accepted")
			}
		})
	}
}

func TestRunStartsAndShutsDownFromContext(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	if err := listener.Close(); err != nil {
		t.Fatal(err)
	}
	baseURL := fmt.Sprintf("http://127.0.0.1:%d", port)
	for name, value := range map[string]string{
		"DAR_OIDC_ISSUER":                "",
		"DAR_OIDC_CLIENT_ID":             "",
		"DAR_OIDC_CLIENT_SECRET":         "",
		"DAR_BASE_URL":                   baseURL,
		"DAR_DATA_ROOT":                  t.TempDir(),
		"DAR_TRUSTED_PROXIES":            "",
		"DAR_OUTBOUND_ALLOWED_CIDRS":     "",
		"DAR_OUTBOUND_ALLOWED_HOSTS":     "127.0.0.1",
		"DAR_CREDENTIAL_ENCRYPTION_KEYS": "",
		"DAR_EMAIL_ENCRYPTION_KEYS":      "",
		"DAR_EMAIL_MBSYNC_PATH":          "/bin/true",
		"DAR_CODEX_PATH":                 "/bin/true",
		"DAR_NEWS_REFRESH_MINUTES":       "60",
		"DAR_CALENDAR_REFRESH_MINUTES":   "60",
		"DAR_EMAIL_SYNC_MINUTES":         "60",
	} {
		t.Setenv(name, value)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- run(ctx, []string{"--port", fmt.Sprint(port), "--db", filepath.Join(t.TempDir(), "server.sqlite")})
	}()
	testutil.Wait(t, 10*time.Second, func() bool {
		response, err := (&http.Client{Timeout: 100 * time.Millisecond}).Get(baseURL + "/health")
		if err != nil {
			return false
		}
		_ = response.Body.Close()
		return response.StatusCode == http.StatusOK
	})
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("run did not shut down after context cancellation")
	}
}

func TestRegisterAppRoutesAppliesAuthentication(t *testing.T) {
	database, err := db.Open(filepath.Join(t.TempDir(), "routes.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Close() })
	srv := server.New(database, nil)
	registry := apps.NewRegistry()
	registry.Register(routeTestApp{})
	registerAppRoutes(srv, registry)

	for _, tc := range []struct {
		path string
		want int
	}{
		{path: "/public-test", want: http.StatusNoContent},
		{path: "/browser-test", want: http.StatusSeeOther},
		{path: "/api-test", want: http.StatusUnauthorized},
	} {
		recorder := httptest.NewRecorder()
		srv.Router.ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, tc.path, nil))
		if recorder.Code != tc.want {
			t.Errorf("%s status = %d, want %d", tc.path, recorder.Code, tc.want)
		}
	}
}

func TestRunMailTunnelValidationPolicyAndRemoteEOF(t *testing.T) {
	if err := runMailTunnel(context.Background(), "127.0.0.1", 0); err == nil {
		t.Fatal("invalid port accepted")
	}
	t.Setenv("DAR_OUTBOUND_ALLOWED_HOSTS", "example.test")
	t.Setenv("DAR_OUTBOUND_ALLOWED_CIDRS", "")
	if err := runMailTunnel(context.Background(), "127.0.0.1", 9); err == nil {
		t.Fatal("disallowed host accepted")
	}

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	accepted := make(chan error, 1)
	go func() {
		conn, acceptErr := listener.Accept()
		if acceptErr == nil {
			_, acceptErr = conn.Write([]byte("server greeting"))
			_ = conn.Close()
		}
		accepted <- acceptErr
	}()

	stdinReader, stdinWriter, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	stdoutReader, stdoutWriter, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	originalStdin, originalStdout := os.Stdin, os.Stdout
	os.Stdin, os.Stdout = stdinReader, stdoutWriter
	t.Cleanup(func() {
		os.Stdin, os.Stdout = originalStdin, originalStdout
		_ = stdinReader.Close()
		_ = stdinWriter.Close()
		_ = stdoutReader.Close()
		_ = stdoutWriter.Close()
	})
	t.Setenv("DAR_OUTBOUND_ALLOWED_HOSTS", "127.0.0.1")
	port := listener.Addr().(*net.TCPAddr).Port
	if err := runMailTunnel(context.Background(), "127.0.0.1", port); err != nil {
		t.Fatal(err)
	}
	_ = stdoutWriter.Close()
	output := make([]byte, 64)
	n, err := stdoutReader.Read(output)
	if err != nil {
		t.Fatal(err)
	}
	if string(output[:n]) != "server greeting" {
		t.Fatalf("tunnel output=%q", output[:n])
	}
	if err := <-accepted; err != nil {
		t.Fatal(err)
	}
}

func TestOIDCStateCookieIsOneTimeAndSessionLogoutInvalidates(t *testing.T) {
	provider, fixture := testOIDCProvider(t)
	defer fixture.Close()
	database, err := db.Open(filepath.Join(t.TempDir(), "oidc.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	srv := server.New(database, provider)
	srv.SetSecureCookies(true)

	loginResponse := httptest.NewRecorder()
	handleLogin(srv)(loginResponse, httptest.NewRequest(http.MethodGet, "https://dar.example/login", nil))
	var stateCookie *http.Cookie
	for _, cookie := range loginResponse.Result().Cookies() {
		if cookie.Name == "dar_oidc_transaction" {
			stateCookie = cookie
		}
	}
	if stateCookie == nil || !stateCookie.HttpOnly || !stateCookie.Secure || stateCookie.MaxAge != 600 || stateCookie.Path != "/auth/oidc/callback" {
		t.Fatalf("state cookie=%+v", stateCookie)
	}
	body := loginResponse.Body.String()
	stateStart := strings.Index(body, "state=")
	if stateStart < 0 || !strings.Contains(body, "code_challenge=") || !strings.Contains(body, "code_challenge_method=S256") {
		t.Fatalf("authorization link is missing state or PKCE: %s", body)
	}
	state := body[stateStart+len("state="):]
	if end := strings.IndexAny(state, "&#\"&"); end >= 0 {
		state = state[:end]
	}
	if _, ok := srv.ValidateOIDCTransaction(stateCookie.Value, state); !ok {
		t.Fatal("authorization state was not bound to signed transaction cookie")
	}

	callback := httptest.NewRequest(http.MethodGet, "https://dar.example/auth/oidc/callback?state="+state+"&code=ok", nil)
	callback.AddCookie(stateCookie)
	callbackResponse := httptest.NewRecorder()
	handleOIDCCallback(srv)(callbackResponse, callback)
	if callbackResponse.Code != http.StatusSeeOther {
		t.Fatalf("callback status=%d body=%s", callbackResponse.Code, callbackResponse.Body.String())
	}
	var sessionCookie *http.Cookie
	stateCleared := false
	for _, cookie := range callbackResponse.Result().Cookies() {
		if cookie.Name == "dar_session" {
			sessionCookie = cookie
		}
		if cookie.Name == "dar_oidc_transaction" && cookie.MaxAge < 0 {
			stateCleared = true
		}
	}
	if sessionCookie == nil || !sessionCookie.HttpOnly || !sessionCookie.Secure || sessionCookie.MaxAge != 30*24*60*60 || !stateCleared {
		t.Fatalf("session=%+v stateCleared=%v", sessionCookie, stateCleared)
	}

	replay := httptest.NewRecorder()
	handleOIDCCallback(srv)(replay, httptest.NewRequest(http.MethodGet, "https://dar.example/auth/oidc/callback?state="+state+"&code=ok", nil))
	if replay.Code != http.StatusBadRequest {
		t.Fatalf("replayed state status=%d", replay.Code)
	}

	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	session, err := auth.ValidateSession(conn, sessionCookie.Value)
	release()
	if err != nil || session == nil {
		t.Fatalf("created session=%+v err=%v", session, err)
	}
	logoutRequest := httptest.NewRequest(http.MethodPost, "https://dar.example/logout", nil)
	logoutRequest.AddCookie(sessionCookie)
	ctx := context.WithValue(logoutRequest.Context(), server.UserContextKey, &auth.User{ID: session.UserID})
	ctx = context.WithValue(ctx, server.SessionContextKey, session)
	logoutRequest = logoutRequest.WithContext(ctx)
	logoutRequest.Header.Set("Origin", "https://dar.example")
	logoutRequest.Header.Set("X-Dar-CSRF-Token", srv.CSRFToken(logoutRequest))
	logoutResponse := httptest.NewRecorder()
	srv.RequireSessionAPI(handleLogout(srv))(logoutResponse, logoutRequest)
	if logoutResponse.Code != http.StatusSeeOther {
		t.Fatalf("logout status=%d body=%s", logoutResponse.Code, logoutResponse.Body.String())
	}
	conn, release, err = database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	session, err = auth.ValidateSession(conn, sessionCookie.Value)
	release()
	if err != nil || session != nil {
		t.Fatalf("session after logout=%+v err=%v", session, err)
	}
}

func TestCoreHandlerAndOIDCCallbackFailureMatrix(t *testing.T) {
	database, err := db.Open(filepath.Join(t.TempDir(), "core-errors.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	plain := server.New(database, nil)
	rec := httptest.NewRecorder()
	handleOIDCCallback(plain)(rec, httptest.NewRequest(http.MethodGet, "/auth/oidc/callback", nil))
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("callback without OIDC=%d", rec.Code)
	}
	for _, user := range []*auth.User{nil, {ID: "alice", DisplayName: "Alice"}} {
		req := httptest.NewRequest(http.MethodGet, "/", nil)
		if user != nil {
			req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
		}
		rec = httptest.NewRecorder()
		handleIndex(plain)(rec, req)
		want := "/login"
		if user != nil {
			want = "/home"
		}
		if rec.Header().Get("Location") != want {
			t.Errorf("index location=%q want %q", rec.Header().Get("Location"), want)
		}
	}
	rec = httptest.NewRecorder()
	handleHome(plain, apps.NewRegistry())(rec, httptest.NewRequest(http.MethodGet, "/home", nil))
	if rec.Code != http.StatusSeeOther {
		t.Fatalf("anonymous home=%d", rec.Code)
	}
	rec = httptest.NewRecorder()
	handleLogout(plain)(rec, httptest.NewRequest(http.MethodPost, "/logout", nil))
	if rec.Code != http.StatusSeeOther {
		t.Fatalf("cookieless logout=%d", rec.Code)
	}

	provider, fixture := testOIDCProvider(t)
	srv := server.New(database, provider)
	missingCode := oidcCallbackRequest(t, srv, "")
	rec = httptest.NewRecorder()
	handleOIDCCallback(srv)(rec, missingCode)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("missing callback code=%d", rec.Code)
	}
	exchange := oidcCallbackRequest(t, srv, "code")
	fixture.Close()
	rec = httptest.NewRecorder()
	handleOIDCCallback(srv)(rec, exchange)
	if rec.Code != http.StatusBadGateway {
		t.Fatalf("exchange failure=%d %s", rec.Code, rec.Body.String())
	}

	badProvider, badFixture := testOIDCProviderProfile(t, auth.OIDCProfile{
		Sub: "bad-subject", Email: "bad@example.test", PreferredUsername: "bad/name", Name: "Bad", EmailVerified: true,
	})
	defer badFixture.Close()
	badServer := server.New(database, badProvider)
	rec = httptest.NewRecorder()
	handleOIDCCallback(badServer)(rec, oidcCallbackRequest(t, badServer, "code"))
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid username callback=%d %s", rec.Code, rec.Body.String())
	}

	closedDB, err := db.Open(filepath.Join(t.TempDir(), "closed-oidc.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	closedProvider, closedFixture := testOIDCProvider(t)
	defer closedFixture.Close()
	closedServer := server.New(closedDB, closedProvider)
	request := oidcCallbackRequest(t, closedServer, "code")
	if err := closedDB.Close(); err != nil {
		t.Fatal(err)
	}
	rec = httptest.NewRecorder()
	handleOIDCCallback(closedServer)(rec, request)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("closed DB callback=%d %s", rec.Code, rec.Body.String())
	}
	rec = httptest.NewRecorder()
	handleHealth(closedServer)(rec, httptest.NewRequest(http.MethodGet, "/health", nil))
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("closed DB health=%d", rec.Code)
	}
	logout := httptest.NewRequest(http.MethodPost, "/logout", nil)
	logout.AddCookie(&http.Cookie{Name: "dar_session", Value: "session"})
	rec = httptest.NewRecorder()
	handleLogout(closedServer)(rec, logout)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("closed DB logout=%d", rec.Code)
	}
}
