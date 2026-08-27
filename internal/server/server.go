// Package server provides the HTTP server, router setup, and middleware for Dar.
package server

import (
	"compress/gzip"
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"html/template"
	"log"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/httpguard"
	"github.com/anoosa/dar/internal/theme"
	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

type contextKey string

const (
	// UserContextKey is the request context key for the authenticated user.
	UserContextKey contextKey = "user"
	// SessionContextKey is the request context key for the session.
	SessionContextKey contextKey = "session"
	// PrincipalContextKey identifies whether authentication used a browser
	// session or a scoped app password.
	PrincipalContextKey contextKey = "principal"
	// AppBarContextKey carries pre-rendered app bar HTML.
	AppBarContextKey contextKey = "appbar"
)

// Principal describes the credential attached to a request.
type Principal struct {
	Session     *auth.Session
	AppPassword *auth.AppPassword
}

// Server wraps the HTTP server and shared dependencies.
type Server struct {
	DB             *db.DB
	OIDC           *auth.OIDCProvider
	Theme          *theme.Service
	Router         chi.Router
	HTTP           *http.Server
	csrfKey        [32]byte
	secureCookies  bool
	trustedProxies []*net.IPNet
}

// New creates a new Server with chi router and middleware.
func New(database *db.DB, oidc *auth.OIDCProvider, appearance ...*theme.Service) *Server {
	s := &Server{DB: database, OIDC: oidc}
	if len(appearance) > 0 {
		s.Theme = appearance[0]
	}
	if _, err := rand.Read(s.csrfKey[:]); err != nil {
		panic("generate CSRF key: " + err.Error())
	}

	// Chi only knows standard HTTP methods by default. CardDAV clients send
	// PROPFIND/REPORT; without RegisterMethod, chi returns 405 before our handlers.
	chi.RegisterMethod("PROPFIND")
	chi.RegisterMethod("REPORT")
	chi.RegisterMethod("MKCOL")
	chi.RegisterMethod("MKCALENDAR")

	r := chi.NewRouter()
	r.Use(s.securityHeaders)
	r.Use(compressResponses)
	r.Use(s.requestLimits)
	r.Use(middleware.Logger)
	r.Use(middleware.Recoverer)
	r.Use(s.sessionMiddleware)

	s.Router = r
	s.HTTP = &http.Server{
		Handler:           r,
		ReadHeaderTimeout: 10 * time.Second,
		ReadTimeout:       5 * time.Minute,
		IdleTimeout:       2 * time.Minute,
		MaxHeaderBytes:    1 << 20,
	}

	return s
}

// WithAppearance resolves one user's appearance for a server-rendered page
// and carries it through request context to webui.RenderRequest.
func (s *Server) WithAppearance(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := GetUser(r)
		if user == nil || s.Theme == nil {
			next(w, r)
			return
		}
		resolved, err := s.Theme.Resolve(r.Context(), user.ID)
		if err != nil {
			log.Printf("resolve appearance for %s: %v", user.ID, err)
		}
		next(w, r.WithContext(theme.WithResolved(r.Context(), resolved)))
	}
}

func (s *Server) requestLimits(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if isUnsafe(r.Method) {
			path := r.URL.Path
			limit := httpguard.JSONBody
			switch {
			case strings.HasPrefix(path, "/api/files/upload"), strings.HasPrefix(path, "/remote.php/dav/files/"), strings.HasPrefix(path, "/s/"):
				limit = httpguard.FilesBody
			case strings.HasPrefix(path, "/api/email"):
				limit = httpguard.EmailBody
			case strings.HasPrefix(path, "/dav"), strings.HasPrefix(path, "/remote.php/dav/addressbooks"):
				limit = httpguard.CardDAVBody
			case strings.HasPrefix(path, "/api/calendar"), strings.HasPrefix(path, "/caldav"), strings.HasPrefix(path, "/remote.php/dav/calendars"), strings.HasPrefix(path, "/api/notes"), strings.HasPrefix(path, "/api/codex"):
				limit = httpguard.ObjectBody
			}
			httpguard.LimitBody(w, r, limit)
		}
		next.ServeHTTP(w, r)
	})
}

// SetTrustedProxies configures the only peers whose forwarding headers may be used.
func (s *Server) SetTrustedProxies(raw string) error {
	var parsed []*net.IPNet
	for _, entry := range strings.Split(raw, ",") {
		entry = strings.TrimSpace(entry)
		if entry == "" {
			continue
		}
		if ip := net.ParseIP(entry); ip != nil {
			bits := 128
			if ip.To4() != nil {
				bits = 32
			}
			parsed = append(parsed, &net.IPNet{IP: ip, Mask: net.CIDRMask(bits, bits)})
			continue
		}
		_, block, err := net.ParseCIDR(entry)
		if err != nil {
			return fmt.Errorf("invalid trusted proxy %q: %w", entry, err)
		}
		parsed = append(parsed, block)
	}
	s.trustedProxies = parsed
	return nil
}

// ClientIP returns a forwarded address only when every hop to it is reached
// through a configured trusted proxy.
func (s *Server) ClientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		host = r.RemoteAddr
	}
	peer := net.ParseIP(strings.TrimSpace(host))
	if !s.isTrustedProxy(peer) {
		return host
	}
	forwarded := strings.Split(r.Header.Get("X-Forwarded-For"), ",")
	for i := len(forwarded) - 1; i >= 0; i-- {
		candidate := net.ParseIP(strings.TrimSpace(forwarded[i]))
		if candidate == nil {
			continue
		}
		if !s.isTrustedProxy(candidate) {
			return candidate.String()
		}
	}
	return host
}

func (s *Server) isTrustedProxy(ip net.IP) bool {
	for _, block := range s.trustedProxies {
		if block.Contains(ip) {
			return true
		}
	}
	return false
}

// SetSecureCookies controls the Secure flag for session, OIDC-state, and CSRF cookies.
func (s *Server) SetSecureCookies(secure bool) { s.secureCookies = secure }

// SecureCookies reports whether cookies must only be sent over HTTPS.
func (s *Server) SecureCookies() bool { return s.secureCookies }

func (s *Server) securityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Content-Type-Options", "nosniff")
		w.Header().Set("X-Frame-Options", "DENY")
		w.Header().Set("Referrer-Policy", "strict-origin-when-cross-origin")
		w.Header().Set("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()")
		w.Header().Set("Cross-Origin-Opener-Policy", "same-origin")
		w.Header().Set("Cross-Origin-Resource-Policy", "same-origin")
		w.Header().Set("Content-Security-Policy", "default-src 'self'; base-uri 'self'; frame-ancestors 'none'; object-src 'none'; form-action 'self'; img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; worker-src 'self' blob:")
		if r.TLS != nil {
			w.Header().Set("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
		}
		next.ServeHTTP(w, r)
	})
}

type gzipResponseWriter struct {
	http.ResponseWriter
	writer *gzip.Writer
	status int
	wrote  bool
}

func (w *gzipResponseWriter) WriteHeader(status int) {
	if w.wrote {
		return
	}
	w.status = status
	if status == http.StatusNoContent || status == http.StatusNotModified || status < 200 {
		w.wrote = true
		w.ResponseWriter.WriteHeader(status)
	}
}

func (w *gzipResponseWriter) Write(p []byte) (int, error) {
	if !w.wrote {
		w.wrote = true
		if w.status == 0 {
			w.status = http.StatusOK
		}
		w.Header().Del("Content-Length")
		w.Header().Set("Content-Encoding", "gzip")
		w.ResponseWriter.WriteHeader(w.status)
	}
	return w.writer.Write(p)
}

func (w *gzipResponseWriter) finish() error {
	if w.wrote && (w.status == http.StatusNoContent || w.status == http.StatusNotModified || w.status < 200) {
		return nil
	}
	if !w.wrote {
		status := w.status
		if status == 0 {
			status = http.StatusOK
		}
		w.ResponseWriter.WriteHeader(status)
	}
	return w.writer.Close()
}

func compressResponses(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Add("Vary", "Accept-Encoding")
		path := r.URL.Path
		skip := r.Method == http.MethodHead ||
			!strings.Contains(r.Header.Get("Accept-Encoding"), "gzip") ||
			r.Header.Get("Range") != "" ||
			strings.Contains(path, "/stream") ||
			strings.HasSuffix(path, "/events") ||
			strings.Contains(path, "/download") ||
			strings.Contains(path, "/thumb") ||
			strings.Contains(path, "/preview") ||
			strings.Contains(path, "/attachments/") ||
			strings.HasSuffix(path, ".woff2")
		if skip {
			next.ServeHTTP(w, r)
			return
		}
		gz, err := gzip.NewWriterLevel(w, gzip.BestSpeed)
		if err != nil {
			next.ServeHTTP(w, r)
			return
		}
		wrapped := &gzipResponseWriter{ResponseWriter: w, writer: gz}
		next.ServeHTTP(wrapped, r)
		_ = wrapped.finish()
	})
}

// RequireSessionAPI accepts only browser OIDC sessions. App passwords are
// intentionally rejected because these endpoints can handle Codex credentials
// and approve mutations of user data.
func (s *Server) RequireSessionAPI(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		session, _ := r.Context().Value(SessionContextKey).(*auth.Session)
		if session == nil || GetUser(r) == nil {
			http.Error(w, "browser session required", http.StatusUnauthorized)
			return
		}
		if r.Method != http.MethodGet && r.Method != http.MethodHead && r.Method != http.MethodOptions {
			if !s.sameOrigin(r) {
				http.Error(w, "cross-origin request denied", http.StatusForbidden)
				return
			}
			if !hmac.Equal([]byte(r.Header.Get("X-Dar-CSRF-Token")), []byte(s.csrfToken(session.ID))) {
				http.Error(w, "invalid CSRF token", http.StatusForbidden)
				return
			}
		}
		next(w, r)
	}
}

// RequireScope permits browser sessions and app passwords containing scope.
// Unsafe browser requests additionally require the signed CSRF header.
func (s *Server) RequireScope(scope string, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		principal, _ := r.Context().Value(PrincipalContextKey).(*Principal)
		if principal == nil || GetUser(r) == nil {
			w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		if principal.AppPassword != nil && !hasScope(principal.AppPassword.Scopes, scope) {
			http.Error(w, "app password scope denied", http.StatusForbidden)
			return
		}
		if principal.Session != nil && isUnsafe(r.Method) {
			if !s.sameOrigin(r) || !hmac.Equal([]byte(r.Header.Get("X-Dar-CSRF-Token")), []byte(s.csrfToken(principal.Session.ID))) {
				http.Error(w, "invalid CSRF token", http.StatusForbidden)
				return
			}
		}
		next(w, r)
	}
}

// RequireAnyScope permits browser sessions or an app password containing one
// of the supplied scopes.
func (s *Server) RequireAnyScope(scopes []string, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		principal, _ := r.Context().Value(PrincipalContextKey).(*Principal)
		if principal == nil || GetUser(r) == nil {
			w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		if principal.AppPassword != nil {
			allowed := false
			for _, scope := range scopes {
				allowed = allowed || hasScope(principal.AppPassword.Scopes, scope)
			}
			if !allowed {
				http.Error(w, "app password scope denied", http.StatusForbidden)
				return
			}
		}
		if principal.Session != nil && isUnsafe(r.Method) {
			if !s.sameOrigin(r) || !hmac.Equal([]byte(r.Header.Get("X-Dar-CSRF-Token")), []byte(s.csrfToken(principal.Session.ID))) {
				http.Error(w, "invalid CSRF token", http.StatusForbidden)
				return
			}
		}
		next(w, r)
	}
}

func hasScope(scopes []string, required string) bool {
	for _, scope := range scopes {
		if scope == required {
			return true
		}
	}
	return false
}

func isUnsafe(method string) bool {
	return method != http.MethodGet && method != http.MethodHead && method != http.MethodOptions
}

func (s *Server) sameOrigin(r *http.Request) bool {
	origin := strings.TrimSpace(r.Header.Get("Origin"))
	if origin == "" {
		// Non-browser clients still need the unguessable CSRF token.
		return true
	}
	u, err := url.Parse(origin)
	if err != nil || u.User != nil || u.RawQuery != "" || u.Fragment != "" || u.Path != "" {
		return false
	}
	scheme := "http"
	if r.TLS != nil {
		scheme = "https"
	} else {
		host, _, splitErr := net.SplitHostPort(r.RemoteAddr)
		if splitErr != nil {
			host = r.RemoteAddr
		}
		if s.isTrustedProxy(net.ParseIP(strings.TrimSpace(host))) {
			if forwarded := strings.TrimSpace(r.Header.Get("X-Forwarded-Proto")); forwarded != "" {
				scheme = strings.ToLower(strings.TrimSpace(strings.Split(forwarded, ",")[0]))
			}
		}
	}
	return strings.EqualFold(u.Scheme, scheme) && strings.EqualFold(u.Host, r.Host)
}

// NewOIDCTransaction creates a signed, short-lived state+PKCE cookie payload.
func (s *Server) NewOIDCTransaction() (state, verifier, value string, err error) {
	state, err = auth.NewOIDCState()
	if err != nil {
		return "", "", "", err
	}
	verifier, err = auth.NewPKCEVerifier()
	if err != nil {
		return "", "", "", err
	}
	payload := strings.Join([]string{state, verifier, strconv.FormatInt(time.Now().Add(10*time.Minute).Unix(), 10)}, "\n")
	encoded := base64.RawURLEncoding.EncodeToString([]byte(payload))
	mac := hmac.New(sha256.New, s.csrfKey[:])
	_, _ = mac.Write([]byte(encoded))
	return state, verifier, encoded + "." + hex.EncodeToString(mac.Sum(nil)), nil
}

// ValidateOIDCTransaction validates a signed OIDC cookie and returns its PKCE
// verifier when it matches the callback state and has not expired.
func (s *Server) ValidateOIDCTransaction(value, providedState string) (string, bool) {
	encoded, signature, ok := strings.Cut(value, ".")
	if !ok || providedState == "" {
		return "", false
	}
	sig, err := hex.DecodeString(signature)
	if err != nil {
		return "", false
	}
	mac := hmac.New(sha256.New, s.csrfKey[:])
	_, _ = mac.Write([]byte(encoded))
	if !hmac.Equal(sig, mac.Sum(nil)) {
		return "", false
	}
	raw, err := base64.RawURLEncoding.DecodeString(encoded)
	if err != nil {
		return "", false
	}
	parts := strings.Split(string(raw), "\n")
	if len(parts) != 3 || !hmac.Equal([]byte(parts[0]), []byte(providedState)) {
		return "", false
	}
	expires, err := strconv.ParseInt(parts[2], 10, 64)
	if err != nil || time.Now().Unix() > expires || expires > time.Now().Add(10*time.Minute).Unix()+1 {
		return "", false
	}
	return parts[1], true
}

func (s *Server) csrfToken(sessionID string) string {
	mac := hmac.New(sha256.New, s.csrfKey[:])
	_, _ = mac.Write([]byte(sessionID))
	return hex.EncodeToString(mac.Sum(nil))
}

// CSRFToken returns the token for the authenticated browser session.
func (s *Server) CSRFToken(r *http.Request) string {
	session, _ := r.Context().Value(SessionContextKey).(*auth.Session)
	if session == nil {
		return ""
	}
	return s.csrfToken(session.ID)
}

// Start starts the HTTP server on the given address.
func (s *Server) Start(addr string) error {
	s.HTTP.Addr = addr
	return s.HTTP.ListenAndServe()
}

// Shutdown gracefully stops the HTTP server.
func (s *Server) Shutdown(ctx context.Context) error {
	return s.HTTP.Shutdown(ctx)
}

// sessionMiddleware loads the user from the session cookie or HTTP Basic
// (app password) and attaches them to the request context.
func (s *Server) sessionMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		cookie, cookieErr := r.Cookie("dar_session")
		basicUser, basicPass, hasBasic := r.BasicAuth()
		if (cookieErr != nil || cookie.Value == "") && !hasBasic {
			next.ServeHTTP(w, r)
			return
		}

		ctx := r.Context()
		conn, release, err := s.DB.Conn(ctx)
		if err != nil {
			next.ServeHTTP(w, r)
			return
		}
		// 1) Session cookie (browser OIDC login).
		if cookieErr == nil && cookie.Value != "" {
			session, err := auth.ValidateSession(conn, cookie.Value)
			if err == nil && session != nil {
				user, err := auth.GetUserByID(conn, session.UserID)
				if err == nil && user != nil {
					ctx = context.WithValue(ctx, UserContextKey, user)
					ctx = context.WithValue(ctx, SessionContextKey, session)
					ctx = context.WithValue(ctx, PrincipalContextKey, &Principal{Session: session})
					release()
					http.SetCookie(w, &http.Cookie{Name: "dar_csrf", Value: s.csrfToken(session.ID), Path: "/", Secure: s.secureCookies, SameSite: http.SameSiteStrictMode})
					next.ServeHTTP(w, r.WithContext(ctx))
					return
				}
			}
		}

		// 2) HTTP Basic with app password (CardDAV / CalDAV / WebDAV clients).
		if hasBasic {
			u, ap, err := auth.ValidateAppPassword(conn, basicUser, basicPass)
			if err == nil && u != nil {
				ctx = context.WithValue(ctx, UserContextKey, u)
				ctx = context.WithValue(ctx, PrincipalContextKey, &Principal{AppPassword: ap})
				release()
				next.ServeHTTP(w, r.WithContext(ctx))
				return
			}
			// Help diagnose DAVx5 "couldn't find CardDAV" which is often a 401.
			if err != nil {
				log.Printf("basic auth error for %q: %v", basicUser, err)
			} else {
				hint := "use account email + full dar_… app password from Settings → Security (shown once)"
				if len(strings.TrimSpace(basicPass)) > 200 {
					hint = "password is far too long for an app password — paste only the dar_… secret, not a page/export"
				} else if !strings.HasPrefix(strings.TrimSpace(basicPass), "dar_") {
					hint = "app passwords start with dar_ — create one in Settings → Security"
				}
				log.Printf("basic auth rejected for %q (pass_len=%d path=%s) — %s",
					basicUser, len(strings.TrimSpace(basicPass)), r.URL.Path, hint)
			}
		}
		release()
		next.ServeHTTP(w, r)
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

// RequireAuthAPI requires an authenticated user (session or app password).
// Returns 401 with WWW-Authenticate for unauthenticated API/DAV clients.
func (s *Server) RequireAuthAPI(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if GetUser(r) == nil {
			w.Header().Set("WWW-Authenticate", `Basic realm="Dar"`)
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		principal, _ := r.Context().Value(PrincipalContextKey).(*Principal)
		if principal != nil && principal.Session != nil && isUnsafe(r.Method) {
			if !s.sameOrigin(r) || !hmac.Equal([]byte(r.Header.Get("X-Dar-CSRF-Token")), []byte(s.csrfToken(principal.Session.ID))) {
				http.Error(w, "invalid CSRF token", http.StatusForbidden)
				return
			}
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
