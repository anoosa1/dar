// Dar server — main entry point.
package main

import (
	"context"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"strings"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/daemon"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/webui"
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
	// Apps will be registered in later phases.

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
			http.Error(w, "Authentication failed", http.StatusInternalServerError)
			return
		}

		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
			return
		}
		defer release()

		user, _, err := auth.FindOrCreateUser(r.Context(), conn,
			profile.Email, profile.Name, profile.Picture,
			s.OIDC.DisplayName(), profile.Sub,
		)
		if err != nil {
			log.Printf("FindOrCreateUser failed: %v", err)
			http.Error(w, "Internal server error", http.StatusInternalServerError)
			return
		}

		session, err := auth.CreateSession(conn, user.ID)
		if err != nil {
			log.Printf("CreateSession failed: %v", err)
			http.Error(w, "Internal server error", http.StatusInternalServerError)
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
