package server

import (
	"compress/gzip"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"html/template"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/theme"
)

func requestWithPrincipal(method string, principal *Principal) *http.Request {
	r := httptest.NewRequest(method, "https://dar.example/api/test", nil)
	ctx := context.WithValue(r.Context(), UserContextKey, &auth.User{ID: "alice"})
	ctx = context.WithValue(ctx, PrincipalContextKey, principal)
	if principal != nil && principal.Session != nil {
		ctx = context.WithValue(ctx, SessionContextKey, principal.Session)
	}
	return r.WithContext(ctx)
}

func TestScopedAuthorizationMatrix(t *testing.T) {
	s := New(nil, nil)
	next := func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }
	cases := []struct {
		name      string
		handler   http.HandlerFunc
		principal *Principal
		want      int
	}{
		{"missing", s.RequireScope("files", next), nil, http.StatusUnauthorized},
		{"matching app password", s.RequireScope("files", next), &Principal{AppPassword: &auth.AppPassword{Scopes: []string{"files"}}}, http.StatusNoContent},
		{"wrong app password", s.RequireScope("files", next), &Principal{AppPassword: &auth.AppPassword{Scopes: []string{"notes"}}}, http.StatusForbidden},
		{"any matching", s.RequireAnyScope([]string{"calendar", "todo"}, next), &Principal{AppPassword: &auth.AppPassword{Scopes: []string{"todo"}}}, http.StatusNoContent},
		{"any denied", s.RequireAnyScope([]string{"calendar", "todo"}, next), &Principal{AppPassword: &auth.AppPassword{Scopes: []string{"files"}}}, http.StatusForbidden},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			r := requestWithPrincipal(http.MethodGet, tc.principal)
			w := httptest.NewRecorder()
			tc.handler(w, r)
			if w.Code != tc.want {
				t.Fatalf("status=%d body=%s", w.Code, w.Body.String())
			}
		})
	}

	session := &auth.Session{ID: "session", UserID: "alice"}
	handler := s.RequireScope("files", next)
	r := requestWithPrincipal(http.MethodPost, &Principal{Session: session})
	w := httptest.NewRecorder()
	handler(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("missing browser CSRF status=%d", w.Code)
	}
	r.Header.Set("Origin", "https://dar.example")
	r.Header.Set("X-Dar-CSRF-Token", s.csrfToken(session.ID))
	w = httptest.NewRecorder()
	handler(w, r)
	if w.Code != http.StatusNoContent {
		t.Fatalf("valid browser CSRF status=%d body=%s", w.Code, w.Body.String())
	}
	anyHandler := s.RequireAnyScope([]string{"files", "notes"}, next)
	r = requestWithPrincipal(http.MethodPost, &Principal{Session: session})
	w = httptest.NewRecorder()
	anyHandler(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("any-scope missing CSRF status=%d", w.Code)
	}
	r.Header.Set("Origin", "https://dar.example")
	r.Header.Set("X-Dar-CSRF-Token", s.csrfToken(session.ID))
	w = httptest.NewRecorder()
	anyHandler(w, r)
	if w.Code != http.StatusNoContent {
		t.Fatalf("any-scope valid CSRF status=%d", w.Code)
	}
}

func TestAuthenticationWrappersCookieSettingsAndContextHelpers(t *testing.T) {
	s := New(nil, nil)
	s.SetSecureCookies(true)
	if !s.SecureCookies() {
		t.Fatal("secure cookie setting was not retained")
	}
	next := func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }
	w := httptest.NewRecorder()
	s.RequireAuth(next)(w, httptest.NewRequest(http.MethodGet, "/private", nil))
	if w.Code != http.StatusSeeOther {
		t.Fatalf("browser auth status=%d", w.Code)
	}
	w = httptest.NewRecorder()
	s.RequireAuthAPI(next)(w, httptest.NewRequest(http.MethodGet, "/api", nil))
	if w.Code != http.StatusUnauthorized || w.Header().Get("WWW-Authenticate") == "" {
		t.Fatalf("anonymous API status=%d headers=%v", w.Code, w.Header())
	}
	if token := s.CSRFToken(httptest.NewRequest(http.MethodGet, "/", nil)); token != "" {
		t.Fatalf("anonymous CSRF token=%q", token)
	}
	r := httptest.NewRequest(http.MethodPost, "https://dar.example/api", nil)
	ctx := context.WithValue(r.Context(), UserContextKey, &auth.User{ID: "alice"})
	w = httptest.NewRecorder()
	s.RequireAuthAPI(next)(w, r.WithContext(ctx))
	if w.Code != http.StatusNoContent {
		t.Fatalf("app-principal API status=%d", w.Code)
	}
	r = r.WithContext(context.WithValue(ctx, PrincipalContextKey, &Principal{Session: &auth.Session{ID: "session", UserID: "alice"}}))
	w = httptest.NewRecorder()
	s.RequireAuthAPI(next)(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("browser API without CSRF status=%d", w.Code)
	}
	r.Header.Set("Origin", "https://dar.example")
	r.Header.Set("X-Dar-CSRF-Token", s.csrfToken("session"))
	w = httptest.NewRecorder()
	s.RequireAuthAPI(next)(w, r)
	if w.Code != http.StatusNoContent {
		t.Fatalf("browser API with CSRF status=%d", w.Code)
	}
	ctx = context.WithValue(ctx, SessionContextKey, &auth.Session{ID: "session"})
	ctx = context.WithValue(ctx, AppBarContextKey, template.HTML("<nav>apps</nav>"))
	r = r.WithContext(ctx)
	if s.CSRFToken(r) == "" || AppBarHTML(r) != template.HTML("<nav>apps</nav>") {
		t.Fatal("request context helpers failed")
	}
}

func TestSecurityHeadersLimitsAndTrustedProxy(t *testing.T) {
	s := New(nil, nil)
	s.Router.Post("/api/test", func(w http.ResponseWriter, r *http.Request) {
		_, err := io.ReadAll(r.Body)
		var maxErr *http.MaxBytesError
		if errors.As(err, &maxErr) {
			http.Error(w, "too large", http.StatusRequestEntityTooLarge)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	})
	r := httptest.NewRequest(http.MethodPost, "https://dar.example/api/test", strings.NewReader(strings.Repeat("x", (2<<20)+1)))
	w := httptest.NewRecorder()
	s.Router.ServeHTTP(w, r)
	if w.Code != http.StatusRequestEntityTooLarge {
		t.Fatalf("oversized JSON status=%d", w.Code)
	}
	for _, header := range []string{"X-Content-Type-Options", "X-Frame-Options", "Referrer-Policy", "Permissions-Policy", "Strict-Transport-Security"} {
		if w.Header().Get(header) == "" {
			t.Errorf("missing %s", header)
		}
	}
	if err := s.SetTrustedProxies("bad-proxy"); err == nil {
		t.Fatal("invalid trusted proxy accepted")
	}
	if err := s.SetTrustedProxies("192.0.2.10,2001:db8::/32"); err != nil {
		t.Fatal(err)
	}
	ipRequest := httptest.NewRequest(http.MethodGet, "http://dar.example", nil)
	ipRequest.RemoteAddr = "192.0.2.10:1234"
	ipRequest.Header.Set("X-Forwarded-For", "198.51.100.20, 192.0.2.10")
	if got := s.ClientIP(ipRequest); got != "198.51.100.20" {
		t.Fatalf("trusted forwarded client=%q", got)
	}
	ipRequest.RemoteAddr = "203.0.113.30:1234"
	if got := s.ClientIP(ipRequest); got != "203.0.113.30" {
		t.Fatalf("untrusted forwarded client=%q", got)
	}
}

func TestCompressesEligibleResponsesAndSkipsStreams(t *testing.T) {
	s := New(nil, nil)
	payload := strings.Repeat("compressible response ", 200)
	s.Router.Get("/large", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		_, _ = io.WriteString(w, payload)
	})
	s.Router.Get("/events", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = io.WriteString(w, payload)
	})

	request := httptest.NewRequest(http.MethodGet, "https://dar.example/large", nil)
	request.Header.Set("Accept-Encoding", "gzip")
	response := httptest.NewRecorder()
	s.Router.ServeHTTP(response, request)
	if response.Header().Get("Content-Encoding") != "gzip" || !strings.Contains(response.Header().Get("Vary"), "Accept-Encoding") {
		t.Fatalf("compression headers=%v", response.Header())
	}
	reader, err := gzip.NewReader(response.Body)
	if err != nil {
		t.Fatal(err)
	}
	data, err := io.ReadAll(reader)
	if err != nil {
		t.Fatal(err)
	}
	_ = reader.Close()
	if string(data) != payload {
		t.Fatal("compressed response did not round-trip")
	}

	request = httptest.NewRequest(http.MethodGet, "https://dar.example/events", nil)
	request.Header.Set("Accept-Encoding", "gzip")
	response = httptest.NewRecorder()
	s.Router.ServeHTTP(response, request)
	if response.Header().Get("Content-Encoding") != "" {
		t.Fatal("event stream was compressed")
	}
}

func TestAuthenticatedStreamsDoNotHoldDatabasePool(t *testing.T) {
	database, err := db.Open(filepath.Join(t.TempDir(), "streams.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	user, _, err := auth.FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://issuer.example", "subject")
	if err != nil {
		release()
		t.Fatal(err)
	}
	session, err := auth.CreateSession(conn, user.ID)
	release()
	if err != nil {
		t.Fatal(err)
	}
	s := New(database, nil)
	started := make(chan struct{}, 5)
	unblock := make(chan struct{})
	handler := s.sessionMiddleware(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		started <- struct{}{}
		<-unblock
		w.WriteHeader(http.StatusNoContent)
	}))
	var wg sync.WaitGroup
	for i := 0; i < 5; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r := httptest.NewRequest(http.MethodGet, "https://dar.example/api/events", nil)
			r.AddCookie(&http.Cookie{Name: "dar_session", Value: session.ID})
			handler.ServeHTTP(httptest.NewRecorder(), r)
		}()
	}
	for i := 0; i < 5; i++ {
		select {
		case <-started:
		case <-time.After(2 * time.Second):
			t.Fatal("authenticated stream did not start")
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 250*time.Millisecond)
	defer cancel()
	probe, probeRelease, err := database.Conn(ctx)
	if err != nil {
		t.Fatalf("database pool exhausted by streams: %v", err)
	}
	if probe == nil {
		t.Fatal("nil probe connection")
	}
	probeRelease()
	close(unblock)
	wg.Wait()
}

func TestSessionMiddlewareAuthenticatesScopedBasicAndRejectsWrongUsername(t *testing.T) {
	database, err := db.Open(filepath.Join(t.TempDir(), "basic.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	defer database.Close()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	user, _, err := auth.FindOrCreateUser(context.Background(), conn, "alice", "alice@example.test", "Alice", "", "https://issuer.example", "subject")
	if err != nil {
		release()
		t.Fatal(err)
	}
	password, err := auth.CreateAppPassword(conn, user.ID, "client", []string{"files"})
	release()
	if err != nil {
		t.Fatal(err)
	}
	s := New(database, nil)
	handler := s.sessionMiddleware(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		principal, _ := r.Context().Value(PrincipalContextKey).(*Principal)
		if GetUser(r) == nil || principal == nil || principal.AppPassword == nil || !hasScope(principal.AppPassword.Scopes, "files") {
			http.Error(w, "missing principal", http.StatusUnauthorized)
			return
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	for _, tc := range []struct {
		username string
		want     int
	}{{"alice", http.StatusNoContent}, {"alice@example.test", http.StatusNoContent}, {"mallory", http.StatusUnauthorized}} {
		r := httptest.NewRequest(http.MethodGet, "https://dar.example/api/files", nil)
		r.SetBasicAuth(tc.username, password.Token)
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, r)
		if w.Code != tc.want {
			t.Fatalf("username %q status=%d body=%s", tc.username, w.Code, w.Body.String())
		}
	}
}

func TestCompressionStatusAndExclusionMatrix(t *testing.T) {
	body := strings.Repeat("payload", 20)
	handler := compressResponses(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Query().Get("status") {
		case "204":
			w.WriteHeader(http.StatusNoContent)
		case "304":
			w.WriteHeader(http.StatusNotModified)
		case "201":
			w.WriteHeader(http.StatusCreated)
			_, _ = io.WriteString(w, body)
		case "header-only":
			w.WriteHeader(http.StatusAccepted)
		default:
			_, _ = io.WriteString(w, body)
		}
	}))

	for _, tc := range []struct {
		name, method, path string
		headers            map[string]string
	}{
		{"head", http.MethodHead, "/large", map[string]string{"Accept-Encoding": "gzip"}},
		{"unsupported", http.MethodGet, "/large", nil},
		{"range", http.MethodGet, "/large", map[string]string{"Accept-Encoding": "gzip", "Range": "bytes=0-2"}},
		{"stream", http.MethodGet, "/api/stream/updates", map[string]string{"Accept-Encoding": "gzip"}},
		{"events", http.MethodGet, "/api/events", map[string]string{"Accept-Encoding": "gzip"}},
		{"download", http.MethodGet, "/api/files/download", map[string]string{"Accept-Encoding": "gzip"}},
		{"thumbnail", http.MethodGet, "/api/files/thumb", map[string]string{"Accept-Encoding": "gzip"}},
		{"preview", http.MethodGet, "/api/files/preview", map[string]string{"Accept-Encoding": "gzip"}},
		{"attachment", http.MethodGet, "/api/email/attachments/1", map[string]string{"Accept-Encoding": "gzip"}},
		{"font", http.MethodGet, "/assets/font.woff2", map[string]string{"Accept-Encoding": "gzip"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			request := httptest.NewRequest(tc.method, "https://dar.example"+tc.path, nil)
			for key, value := range tc.headers {
				request.Header.Set(key, value)
			}
			response := httptest.NewRecorder()
			handler.ServeHTTP(response, request)
			if got := response.Header().Get("Content-Encoding"); got != "" {
				t.Fatalf("excluded response was compressed: %q", got)
			}
		})
	}

	for _, tc := range []struct {
		query string
		want  int
		gzip  bool
	}{
		{"status=204", http.StatusNoContent, false},
		{"status=304", http.StatusNotModified, false},
		{"status=201", http.StatusCreated, true},
		{"status=header-only", http.StatusAccepted, false},
	} {
		request := httptest.NewRequest(http.MethodGet, "https://dar.example/large?"+tc.query, nil)
		request.Header.Set("Accept-Encoding", "gzip")
		response := httptest.NewRecorder()
		handler.ServeHTTP(response, request)
		if response.Code != tc.want {
			t.Errorf("%s status=%d want=%d", tc.query, response.Code, tc.want)
		}
		if got := response.Header().Get("Content-Encoding") == "gzip"; got != tc.gzip {
			t.Errorf("%s gzip=%v want=%v", tc.query, got, tc.gzip)
		}
	}

	recorder := httptest.NewRecorder()
	writer, err := gzip.NewWriterLevel(recorder, gzip.BestSpeed)
	if err != nil {
		t.Fatal(err)
	}
	wrapped := &gzipResponseWriter{ResponseWriter: recorder, writer: writer}
	wrapped.WriteHeader(http.StatusNoContent)
	wrapped.WriteHeader(http.StatusConflict)
	if err := wrapped.finish(); err != nil {
		t.Fatal(err)
	}
	if recorder.Code != http.StatusNoContent {
		t.Fatalf("header-only compressed status=%d", recorder.Code)
	}
}

func TestOIDCTransactionAndOriginFailureMatrix(t *testing.T) {
	s := New(nil, nil)
	state, verifier, value, err := s.NewOIDCTransaction()
	if err != nil || state == "" || verifier == "" || value == "" {
		t.Fatalf("transaction state=%q verifier=%q value=%q err=%v", state, verifier, value, err)
	}
	if got, ok := s.ValidateOIDCTransaction(value, state); !ok || got != verifier {
		t.Fatalf("valid transaction verifier=%q ok=%v", got, ok)
	}

	sign := func(payload string) string {
		encoded := base64.RawURLEncoding.EncodeToString([]byte(payload))
		mac := hmac.New(sha256.New, s.csrfKey[:])
		_, _ = mac.Write([]byte(encoded))
		return encoded + "." + hex.EncodeToString(mac.Sum(nil))
	}
	now := time.Now().Unix()
	for name, cookie := range map[string]string{
		"missing separator": "value",
		"invalid hex":       "value.not-hex",
		"wrong signature":   "dmFsdWU." + strings.Repeat("00", sha256.Size),
		"invalid base64": "%%." + func() string {
			mac := hmac.New(sha256.New, s.csrfKey[:])
			_, _ = mac.Write([]byte("%%"))
			return hex.EncodeToString(mac.Sum(nil))
		}(),
		"wrong parts":    sign("only\nparts"),
		"wrong state":    sign("other\nverifier\n" + strconv.FormatInt(now+60, 10)),
		"bad expiry":     sign(state + "\nverifier\nnot-a-time"),
		"expired":        sign(state + "\nverifier\n" + strconv.FormatInt(now-1, 10)),
		"far future":     sign(state + "\nverifier\n" + strconv.FormatInt(now+3600, 10)),
		"empty callback": value,
	} {
		provided := state
		if name == "empty callback" {
			provided = ""
		}
		if got, ok := s.ValidateOIDCTransaction(cookie, provided); ok || got != "" {
			t.Errorf("%s accepted verifier=%q", name, got)
		}
	}

	if err := s.SetTrustedProxies("192.0.2.0/24"); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name, origin, remote, forwardedProto string
		tls                                  bool
		want                                 bool
	}{
		{"missing origin", "", "203.0.113.1:1", "", false, true},
		{"http", "http://dar.example", "203.0.113.1:1", "", false, true},
		{"https", "https://dar.example", "203.0.113.1:1", "", true, true},
		{"trusted forwarded https", "https://dar.example", "192.0.2.2:1", "https,http", false, true},
		{"untrusted forwarded ignored", "https://dar.example", "203.0.113.1:1", "https", false, false},
		{"bad URL", "://", "203.0.113.1:1", "", false, false},
		{"credentials", "http://user@dar.example", "203.0.113.1:1", "", false, false},
		{"query", "http://dar.example?q=1", "203.0.113.1:1", "", false, false},
		{"fragment", "http://dar.example#x", "203.0.113.1:1", "", false, false},
		{"path", "http://dar.example/path", "203.0.113.1:1", "", false, false},
		{"wrong host", "http://other.example", "203.0.113.1:1", "", false, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			request := httptest.NewRequest(http.MethodPost, "http://dar.example/api", nil)
			request.RemoteAddr = tc.remote
			request.Header.Set("Origin", tc.origin)
			request.Header.Set("X-Forwarded-Proto", tc.forwardedProto)
			if tc.tls {
				request = httptest.NewRequest(http.MethodPost, "https://dar.example/api", nil)
				request.RemoteAddr = tc.remote
				request.Header.Set("Origin", tc.origin)
			}
			if got := s.sameOrigin(request); got != tc.want {
				t.Fatalf("sameOrigin=%v want=%v", got, tc.want)
			}
		})
	}
}

func TestRequestLimitClassificationAppearanceAndClosedAuthentication(t *testing.T) {
	s := New(nil, nil)
	called := 0
	handler := s.requestLimits(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		called++
		w.WriteHeader(http.StatusNoContent)
	}))
	for _, path := range []string{
		"/api/default", "/api/files/upload", "/remote.php/dav/files/alice/a",
		"/s/token", "/api/email/messages", "/dav/addressbooks/alice",
		"/remote.php/dav/addressbooks/alice", "/api/calendar/events",
		"/caldav/alice", "/remote.php/dav/calendars/alice", "/api/notes/1",
		"/api/codex/turns",
	} {
		response := httptest.NewRecorder()
		handler.ServeHTTP(response, httptest.NewRequest(http.MethodPost, path, strings.NewReader("{}")))
		if response.Code != http.StatusNoContent {
			t.Errorf("%s status=%d", path, response.Code)
		}
	}
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "/api/default", nil))
	if response.Code != http.StatusNoContent || called != 13 {
		t.Fatalf("request limit calls=%d status=%d", called, response.Code)
	}

	database, err := db.Open(filepath.Join(t.TempDir(), "closed-auth.sqlite"), auth.Migrations())
	if err != nil {
		t.Fatal(err)
	}
	if err := database.Close(); err != nil {
		t.Fatal(err)
	}
	s = New(database, nil)
	request := httptest.NewRequest(http.MethodGet, "/private", nil)
	request.AddCookie(&http.Cookie{Name: "dar_session", Value: "missing"})
	response = httptest.NewRecorder()
	s.sessionMiddleware(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if GetUser(r) != nil {
			t.Error("closed database authenticated request")
		}
		w.WriteHeader(http.StatusNoContent)
	})).ServeHTTP(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("closed database middleware status=%d", response.Code)
	}

	appearanceDB, err := db.Open(filepath.Join(t.TempDir(), "closed-theme.sqlite"), nil)
	if err != nil {
		t.Fatal(err)
	}
	appearance := theme.NewService(appearanceDB)
	if err := appearanceDB.Close(); err != nil {
		t.Fatal(err)
	}
	s = New(nil, nil, appearance)
	request = httptest.NewRequest(http.MethodGet, "/page", nil)
	request = request.WithContext(context.WithValue(request.Context(), UserContextKey, &auth.User{ID: "alice"}))
	response = httptest.NewRecorder()
	s.WithAppearance(func(w http.ResponseWriter, r *http.Request) {
		if theme.FromContext(r.Context()).Active != theme.MochaPink() {
			t.Error("appearance fallback missing")
		}
		w.WriteHeader(http.StatusNoContent)
	})(response, request)
	if response.Code != http.StatusNoContent {
		t.Fatalf("appearance middleware status=%d", response.Code)
	}
}
