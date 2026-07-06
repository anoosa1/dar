// Dar server — main entry point.
package main

import (
	"context"
	"flag"
	"fmt"
	"html/template"
	"log"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/daemon"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"

	calendarApp "github.com/anoosa/dar/internal/apps/calendar"
	emailApp "github.com/anoosa/dar/internal/apps/email"
	newsApp "github.com/anoosa/dar/internal/apps/news"
	notesApp "github.com/anoosa/dar/internal/apps/notes"
	todoApp "github.com/anoosa/dar/internal/apps/todo"
	contactsApp "github.com/anoosa/dar/internal/apps/contacts"
	filesApp "github.com/anoosa/dar/internal/apps/files"
	soosaApp "github.com/anoosa/dar/internal/apps/soosa"
)

func main() {
	port := flag.Int("port", 8080, "HTTP port")
	dbPath := flag.String("db", "dar.db", "SQLite database path")
	flag.Parse()

	ctx := context.Background()

	// ── Database ──
	migrations := auth.Migrations()
	database, err := db.Open(*dbPath, migrations)
	if err != nil {
		log.Fatalf("Failed to open database: %v", err)
	}
	defer database.Close()

	// ── OIDC ──
	var oidcProvider *auth.OIDCProvider
	issuer := os.Getenv("DAR_OIDC_ISSUER")
	clientID := os.Getenv("DAR_OIDC_CLIENT_ID")
	clientSecret := os.Getenv("DAR_OIDC_CLIENT_SECRET")
	displayName := os.Getenv("DAR_OIDC_DISPLAY_NAME")
	if displayName == "" {
		displayName = "OIDC"
	}
	baseURL := os.Getenv("DAR_BASE_URL")
	if baseURL == "" {
		baseURL = "http://localhost:8080"
	}

	if issuer != "" && clientID != "" && clientSecret != "" {
		p, err := auth.NewOIDCProvider(ctx, auth.OIDCConfig{
			Issuer:       issuer,
			ClientID:     clientID,
			ClientSecret: clientSecret,
			RedirectURI:  strings.TrimRight(baseURL, "/") + "/auth/oidc/callback",
			DisplayName:  displayName,
		})
		if err != nil {
			log.Printf("WARNING: OIDC discovery failed: %v — continuing without auth", err)
		} else {
			oidcProvider = p
			log.Printf("OIDC provider configured: %s", oidcProvider.DisplayName())
		}
	}

	// ── Server ──
	srv := server.New(database, oidcProvider)

	// ── App Registry ──
	registry := apps.NewRegistry()
	registry.Register(soosaApp.App{})
	registry.Register(filesApp.App{})
	registry.Register(contactsApp.App{})
	registry.Register(calendarApp.App{})
	registry.Register(todoApp.App{})
	registry.Register(notesApp.App{})
	registry.Register(newsApp.App{})
	registry.Register(emailApp.App{})

	// ── Daemon Manager ──
	dm := daemon.NewManager()
	defer dm.Shutdown()

	// ── Routes ──
	srv.Router.Get("/", handleIndex(srv))
	srv.Router.Get("/login", handleLogin(srv))
	srv.Router.Get("/auth/oidc/callback", handleOIDCCallback(srv))
	srv.Router.Get("/home", srv.RequireAuth(handleHome(srv, registry)))
	srv.Router.Get("/api/me", srv.RequireAuth(handleAPIMe))
	srv.Router.Get("/logout", handleLogout())

	// Proxy /api/soosa/* to soosa daemon on port 8081.
	srv.Router.HandleFunc("/api/soosa/*", proxySoosa)
	srv.Router.HandleFunc("/api/files/*", proxyFiles)
	srv.Router.HandleFunc("/api/contacts/*", proxyContacts)
	srv.Router.HandleFunc("/api/calendar/*", proxyCalendar)
	srv.Router.HandleFunc("/api/todo/*", proxyTodo)
	srv.Router.HandleFunc("/api/notes/*", proxyNotes)
	srv.Router.HandleFunc("/api/news/*", proxyNews)
	srv.Router.HandleFunc("/api/email/*", proxyEmail)

	// Register app routes.
	for _, app := range registry.Apps() {
		for _, route := range app.Routes() {
			appBar := registry.AppBarHTML()
			switch route.Method {
			case "GET":
				srv.Router.Get(route.Pattern, srv.RequireAuth(injectAppBar(appBar, route.Handler)))
			case "POST":
				srv.Router.Post(route.Pattern, srv.RequireAuth(injectAppBar(appBar, route.Handler)))
			}
		}
	}

	// ── Start ──
	addr := fmt.Sprintf(":%d", *port)
	log.Printf("Dar server listening on http://localhost%s", addr)
	log.Printf("Database: %s", *dbPath)
	if err := srv.Start(addr); err != nil {
		log.Fatalf("Server error: %v", err)
	}
}

// ── Handlers ──

func handleIndex(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user != nil {
			http.Redirect(w, r, "/home", http.StatusSeeOther)
		} else {
			http.Redirect(w, r, "/login", http.StatusSeeOther)
		}
	}
}

func handleLogin(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var providers []webui.LoginProvider
		if s.OIDC != nil {
			providers = append(providers, webui.LoginProvider{
				Name: s.OIDC.DisplayName(),
				Icon: "🔑",
				URL:  s.OIDC.AuthURL(),
			})
		}
		if err := webui.RenderLogin(w, providers); err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
		}
	}
}

func handleOIDCCallback(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.OIDC == nil {
			http.Error(w, "OIDC not configured", http.StatusBadRequest)
			return
		}

		code := r.URL.Query().Get("code")
		if code == "" {
			http.Error(w, "Missing authorization code", http.StatusBadRequest)
			return
		}

		profile, err := s.OIDC.ExchangeCode(r.Context(), code)
		if err != nil {
			log.Printf("OIDC exchange failed: %v", err)
			http.Error(w, fmt.Sprintf("OIDC exchange failed: %v", err), http.StatusInternalServerError)
			return
		}

		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			log.Printf("DB conn failed: %v", err)
			http.Error(w, fmt.Sprintf("Database connection failed: %v", err), http.StatusInternalServerError)
			return
		}
		defer release()

		user, _, err := auth.FindOrCreateUser(r.Context(), conn,
			profile.Email, profile.Name, profile.Picture,
			s.OIDC.DisplayName(), profile.Sub,
		)
		if err != nil {
			log.Printf("FindOrCreateUser failed: %v", err)
			http.Error(w, fmt.Sprintf("User lookup failed: %v", err), http.StatusInternalServerError)
			return
		}

		session, err := auth.CreateSession(conn, user.ID)
		if err != nil {
			log.Printf("CreateSession failed: %v", err)
			http.Error(w, fmt.Sprintf("Session creation failed: %v", err), http.StatusInternalServerError)
			return
		}

		http.SetCookie(w, &http.Cookie{
			Name:     "dar_session",
			Value:    session.ID,
			Path:     "/",
			MaxAge:   30 * 24 * 60 * 60,
			HttpOnly: true,
			SameSite: http.SameSiteLaxMode,
		})

		http.Redirect(w, r, "/home", http.StatusSeeOther)
	}
}

func handleHome(s *server.Server, reg *apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if user == nil {
			http.Redirect(w, r, "/login", http.StatusSeeOther)
			return
		}

		if err := webui.RenderHome(w, user.DisplayName,
			reg.HomeCardsHTML(),
			reg.AppBarHTML(),
		); err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
		}
	}
}

func handleAPIMe(w http.ResponseWriter, r *http.Request) {
	user := server.GetUser(r)
	if user == nil {
		http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	fmt.Fprintf(w, `{"email":"%s","display_name":"%s","avatar_url":"%s","provider":"%s"}`,
		user.Email, user.DisplayName, user.AvatarURL, user.Provider)
}

func handleLogout() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		http.SetCookie(w, &http.Cookie{
			Name:     "dar_session",
			Value:    "",
			Path:     "/",
			MaxAge:   -1,
			HttpOnly: true,
			SameSite: http.SameSiteLaxMode,
		})
		http.Redirect(w, r, "/login", http.StatusSeeOther)
	}
}

// proxySoosa forwards /api/soosa/* requests to the soosa daemon.
func proxySoosa(w http.ResponseWriter, r *http.Request) {
	soosaPort := os.Getenv("DAR_SOOSA_PORT")
	if soosaPort == "" {
		soosaPort = "8081"
	}
	target, _ := url.Parse("http://localhost:" + soosaPort)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/soosa")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// proxyFiles forwards /api/files/* requests to the files daemon.
func proxyFiles(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_FILES_PORT")
	if port == "" {
		port = "8082"
	}
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/files")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// proxyCalendar forwards /api/calendar/* requests to the calendar daemon.
func proxyCalendar(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_CALENDAR_PORT")
	if port == "" {
		port = "8084"
	}
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/calendar")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// proxyContacts forwards /api/contacts/* requests to the contacts daemon.
func proxyContacts(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_CONTACTS_PORT")
	if port == "" {
		port = "8083"
	}
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/contacts")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// proxyEmail forwards /api/email/* requests to the email daemon.
func proxyEmail(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_EMAIL_PORT")
	if port == "" { port = "8088" }
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{Rewrite: func(pr *httputil.ProxyRequest) {
		pr.SetURL(target)
		pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/email")
		if pr.Out.URL.Path == "" { pr.Out.URL.Path = "/" }
	}}
	proxy.ServeHTTP(w, r)
}

// proxyNews forwards /api/news/* requests to the news daemon.
func proxyNews(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_NEWS_PORT")
	if port == "" { port = "8087" }
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{Rewrite: func(pr *httputil.ProxyRequest) {
		pr.SetURL(target)
		pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/news")
		if pr.Out.URL.Path == "" { pr.Out.URL.Path = "/" }
	}}
	proxy.ServeHTTP(w, r)
}

// proxyNotes forwards /api/notes/* requests to the notes daemon.
func proxyNotes(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_NOTES_PORT")
	if port == "" {
		port = "8086"
	}
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/notes")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// proxyTodo forwards /api/todo/* requests to the todo daemon.
func proxyTodo(w http.ResponseWriter, r *http.Request) {
	port := os.Getenv("DAR_TODO_PORT")
	if port == "" {
		port = "8085"
	}
	target, _ := url.Parse("http://localhost:" + port)
	proxy := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(target)
			pr.Out.URL.Path = strings.TrimPrefix(r.URL.Path, "/api/todo")
			if pr.Out.URL.Path == "" {
				pr.Out.URL.Path = "/"
			}
		},
	}
	proxy.ServeHTTP(w, r)
}

// injectAppBar wraps a handler to inject app bar HTML into the request context.
func injectAppBar(appBar template.HTML, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := context.WithValue(r.Context(), server.AppBarContextKey, appBar)
		next.ServeHTTP(w, r.WithContext(ctx))
	}
}
