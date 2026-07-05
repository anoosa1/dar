// Package server provides the HTTP server, router setup, and middleware for Dar.
package server

import (
	"context"
	"html/template"
	"net/http"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

type contextKey string

const (
	// UserContextKey is the request context key for the authenticated user.
	UserContextKey contextKey = "user"
	// SessionContextKey is the request context key for the session.
	SessionContextKey contextKey = "session"
	// AppBarContextKey carries pre-rendered app bar HTML.
	AppBarContextKey contextKey = "appbar"
)

// Server wraps the HTTP server and shared dependencies.
type Server struct {
	DB     *db.DB
	OIDC   *auth.OIDCProvider
	Router chi.Router
	HTTP   *http.Server
}

// New creates a new Server with chi router and middleware.
func New(database *db.DB, oidc *auth.OIDCProvider) *Server {
	s := &Server{DB: database, OIDC: oidc}

	r := chi.NewRouter()
	r.Use(middleware.Logger)
	r.Use(middleware.Recoverer)
	r.Use(middleware.RealIP)
	r.Use(s.sessionMiddleware)

	s.Router = r
	s.HTTP = &http.Server{Handler: r}

	return s
}

// Start starts the HTTP server on the given address.
func (s *Server) Start(addr string) error {
	s.HTTP.Addr = addr
	return s.HTTP.ListenAndServe()
}

// sessionMiddleware loads the user from the session cookie and attaches it to the request context.
func (s *Server) sessionMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Skip auth for login and OIDC callback pages.
		if r.URL.Path == "/login" || r.URL.Path == "/auth/oidc/callback" || r.URL.Path == "/" {
			next.ServeHTTP(w, r)
			return
		}

		cookie, err := r.Cookie("dar_session")
		if err != nil {
			next.ServeHTTP(w, r)
			return
		}

		ctx := r.Context()
		conn, release, err := s.DB.Conn(ctx)
		if err != nil {
			next.ServeHTTP(w, r)
			return
		}
		defer release()

		session, err := auth.ValidateSession(conn, cookie.Value)
		if err != nil || session == nil {
			next.ServeHTTP(w, r)
			return
		}

		user, err := auth.GetUserByID(conn, session.UserID)
		if err != nil || user == nil {
			next.ServeHTTP(w, r)
			return
		}

		ctx = context.WithValue(ctx, UserContextKey, user)
		ctx = context.WithValue(ctx, SessionContextKey, session)
		next.ServeHTTP(w, r.WithContext(ctx))
	})
}

// RequireAuth is middleware that redirects to /login if no user is authenticated.
func (s *Server) RequireAuth(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := r.Context().Value(UserContextKey)
		if user == nil {
			http.Redirect(w, r, "/login", http.StatusSeeOther)
			return
		}
		next(w, r)
	}
}

// GetUser extracts the authenticated user from the request context.
func GetUser(r *http.Request) *auth.User {
	user, _ := r.Context().Value(UserContextKey).(*auth.User)
	return user
}

// AppBarHTML extracts the app bar HTML from the request context.
func AppBarHTML(r *http.Request) template.HTML {
	html, _ := r.Context().Value(AppBarContextKey).(template.HTML)
	return html
}
