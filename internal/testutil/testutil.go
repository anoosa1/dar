// Package testutil contains hermetic fixtures shared by Dar's integration tests.
// It deliberately depends only on the standard library and Dar's own packages.
package testutil

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
)

var fixtureSequence atomic.Uint64

// OpenDB creates a migrated private database and registers deterministic cleanup.
func OpenDB(t testing.TB, migrations ...string) *db.DB {
	t.Helper()
	database, err := db.Open(filepath.Join(t.TempDir(), "dar-test.sqlite"), migrations)
	if err != nil {
		t.Fatalf("open test database: %v", err)
	}
	t.Cleanup(func() {
		if err := database.Close(); err != nil {
			t.Errorf("close test database: %v", err)
		}
	})
	return database
}

// User creates a unique OIDC-backed user.
func User(t testing.TB, database *db.DB, label string) *auth.User {
	t.Helper()
	n := fixtureSequence.Add(1)
	if label == "" {
		label = "user"
	}
	username := fmt.Sprintf("%s-%d", label, n)
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatalf("take database connection: %v", err)
	}
	defer release()
	user, _, err := auth.FindOrCreateUser(
		context.Background(),
		conn,
		username,
		username+"@example.test",
		"Test "+label,
		"",
		"https://issuer.example.test",
		fmt.Sprintf("subject-%d", n),
	)
	if err != nil {
		t.Fatalf("create test user: %v", err)
	}
	return user
}

// Session creates a browser session for user.
func Session(t testing.TB, database *db.DB, user *auth.User) *auth.Session {
	t.Helper()
	conn, release, err := database.Conn(context.Background())
	if err != nil {
		t.Fatalf("take database connection: %v", err)
	}
	defer release()
	session, err := auth.CreateSession(conn, user.ID)
	if err != nil {
		t.Fatalf("create test session: %v", err)
	}
	return session
}

// Request creates an httptest request with Dar's authenticated context values.
func Request(t testing.TB, method, target string, body any, user *auth.User) *http.Request {
	t.Helper()
	var reader io.Reader
	switch value := body.(type) {
	case nil:
	case string:
		reader = bytes.NewBufferString(value)
	case []byte:
		reader = bytes.NewReader(value)
	default:
		data, err := json.Marshal(value)
		if err != nil {
			t.Fatalf("marshal request body: %v", err)
		}
		reader = bytes.NewReader(data)
	}
	req := httptest.NewRequest(method, target, reader)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if user != nil {
		req = req.WithContext(context.WithValue(req.Context(), server.UserContextKey, user))
	}
	return req
}

// BrowserRequest adds session and principal values to an authenticated request.
func BrowserRequest(t testing.TB, method, target string, body any, user *auth.User, session *auth.Session) *http.Request {
	t.Helper()
	req := Request(t, method, target, body, user)
	ctx := context.WithValue(req.Context(), server.SessionContextKey, session)
	ctx = context.WithValue(ctx, server.PrincipalContextKey, &server.Principal{Session: session})
	return req.WithContext(ctx)
}

// JSON decodes a recorder body and fails with a useful response dump.
func JSON[T any](t testing.TB, recorder *httptest.ResponseRecorder) T {
	t.Helper()
	var value T
	if err := json.Unmarshal(recorder.Body.Bytes(), &value); err != nil {
		t.Fatalf("decode JSON response status=%d body=%q: %v", recorder.Code, recorder.Body.String(), err)
	}
	return value
}

// LoopbackListener reserves an OS-selected loopback port.
func LoopbackListener(t testing.TB) net.Listener {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen on loopback: %v", err)
	}
	t.Cleanup(func() {
		if err := listener.Close(); err != nil && !errors.Is(err, net.ErrClosed) {
			t.Errorf("close loopback listener: %v", err)
		}
	})
	return listener
}

// Wait polls a condition using a short bounded deadline. It is intended for
// worker boundaries where no completion channel is exposed.
func Wait(t testing.TB, timeout time.Duration, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for !condition() {
		if time.Now().After(deadline) {
			t.Fatalf("condition not satisfied within %s", timeout)
		}
		time.Sleep(time.Millisecond)
	}
}

// Clock is a manually advanced concurrency-safe clock for deterministic tests.
type Clock struct {
	mu  sync.Mutex
	now time.Time
}

// NewClock returns a clock starting at now.
func NewClock(now time.Time) *Clock { return &Clock{now: now} }

// Now returns the current instant.
func (clock *Clock) Now() time.Time {
	clock.mu.Lock()
	defer clock.mu.Unlock()
	return clock.now
}

// Advance moves the clock forward.
func (clock *Clock) Advance(delta time.Duration) {
	clock.mu.Lock()
	clock.now = clock.now.Add(delta)
	clock.mu.Unlock()
}
