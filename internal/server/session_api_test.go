package server

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/anoosa/dar/internal/auth"
)

func TestRequireSessionAPIRejectsAppPasswordAndEnforcesCSRF(t *testing.T) {
	s := New(nil, nil)
	next := s.RequireSessionAPI(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	user := &auth.User{ID: "alice"}
	request := httptest.NewRequest(http.MethodPost, "https://dar.example/api/codex/preferences", nil)
	request = request.WithContext(context.WithValue(request.Context(), UserContextKey, user))
	recorder := httptest.NewRecorder()
	next(recorder, request)
	if recorder.Code != http.StatusUnauthorized {
		t.Fatalf("app-password-style request = %d", recorder.Code)
	}

	session := &auth.Session{ID: "session-1", UserID: user.ID}
	request = request.WithContext(context.WithValue(request.Context(), SessionContextKey, session))
	recorder = httptest.NewRecorder()
	next(recorder, request)
	if recorder.Code != http.StatusForbidden {
		t.Fatalf("missing CSRF = %d", recorder.Code)
	}

	request.Header.Set("X-Dar-CSRF-Token", s.csrfToken(session.ID))
	request.Header.Set("Origin", "https://evil.example")
	recorder = httptest.NewRecorder()
	next(recorder, request)
	if recorder.Code != http.StatusForbidden {
		t.Fatalf("cross-origin request = %d", recorder.Code)
	}

	request.Header.Set("Origin", "https://dar.example")
	recorder = httptest.NewRecorder()
	next(recorder, request)
	if recorder.Code != http.StatusNoContent {
		t.Fatalf("valid request = %d: %s", recorder.Code, recorder.Body.String())
	}
}

func TestRequireSessionAPIAllowsAuthenticatedReadWithoutCSRF(t *testing.T) {
	s := New(nil, nil)
	next := s.RequireSessionAPI(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) })
	request := httptest.NewRequest(http.MethodGet, "https://dar.example/api/codex/status", nil)
	ctx := context.WithValue(request.Context(), UserContextKey, &auth.User{ID: "alice"})
	ctx = context.WithValue(ctx, SessionContextKey, &auth.Session{ID: "session-1", UserID: "alice"})
	recorder := httptest.NewRecorder()
	next(recorder, request.WithContext(ctx))
	if recorder.Code != http.StatusNoContent {
		t.Fatalf("read request = %d", recorder.Code)
	}
}
