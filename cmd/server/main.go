// Dar server — main entry point.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"html/template"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/anoosa/dar/internal/apps"
	"github.com/anoosa/dar/internal/auth"
	"github.com/anoosa/dar/internal/caldav"
	"github.com/anoosa/dar/internal/credentials"
	"github.com/anoosa/dar/internal/db"
	"github.com/anoosa/dar/internal/geocode"
	"github.com/anoosa/dar/internal/notifications"
	"github.com/anoosa/dar/internal/outbound"
	"github.com/anoosa/dar/internal/preferences"
	searchsvc "github.com/anoosa/dar/internal/search"
	"github.com/anoosa/dar/internal/server"
	"github.com/anoosa/dar/internal/theme"
	"github.com/anoosa/dar/internal/webui"

	activityApp "github.com/anoosa/dar/internal/apps/activity"
	calendarApp "github.com/anoosa/dar/internal/apps/calendar"
	codexApp "github.com/anoosa/dar/internal/apps/codex"
	contactsApp "github.com/anoosa/dar/internal/apps/contacts"
	coreassistant "github.com/anoosa/dar/internal/apps/coreassistant"
	emailApp "github.com/anoosa/dar/internal/apps/email"
	filesApp "github.com/anoosa/dar/internal/apps/files"
	newsApp "github.com/anoosa/dar/internal/apps/news"
	notesApp "github.com/anoosa/dar/internal/apps/notes"
	prayerApp "github.com/anoosa/dar/internal/apps/prayer"
	todoApp "github.com/anoosa/dar/internal/apps/todo"
	weatherApp "github.com/anoosa/dar/internal/apps/weather"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	if err := run(ctx, os.Args[1:]); err != nil {
		log.Fatal(err)
	}
}

func establishedServerMigrations() []string {
	var migrations []string
	migrations = append(migrations, auth.Migrations()...)
	migrations = append(migrations, preferences.Migrations()...)
	migrations = append(migrations, calendarApp.Migrations()...)
	migrations = append(migrations, todoApp.Migrations()...)
	migrations = append(migrations, newsApp.Migrations()...)
	migrations = append(migrations, filesApp.Migrations()...)
	migrations = append(migrations, notesApp.Migrations()...)
	migrations = append(migrations, emailApp.Migrations()...)
	migrations = append(migrations, codexApp.Migrations()...)
	migrations = append(migrations, searchsvc.Migrations()...)
	migrations = append(migrations, activityApp.Migrations()...)
	migrations = append(migrations, notifications.Migrations()...)
	return migrations
}

func serverMigrations() []string {
	migrations := establishedServerMigrations()
	featureMigrationCount := len(todoApp.GeneratedTaskMigrations()) + len(prayerApp.Migrations())
	// The first Prayer build inserted its migrations into the middle of Dar's
	// global sequence. A database that ran that build advanced by this many
	// versions without creating the intended tables. Reserve those version
	// slots, then append the real migrations so affected databases run them.
	for range featureMigrationCount {
		migrations = append(migrations, `SELECT 1; -- reserved: failed Prayer migration ordering`)
	}
	migrations = append(migrations, todoApp.GeneratedTaskMigrations()...)
	migrations = append(migrations, prayerApp.Migrations()...)
	return migrations
}

func run(ctx context.Context, args []string) error {
	flags := flag.NewFlagSet("dar-server", flag.ContinueOnError)
	port := flags.Int("port", 8080, "HTTP port")
	dbPath := flags.String("db", "dar.db", "SQLite database path")
	mailTunnelHost := flags.String("mail-tunnel-host", "", "internal: guarded mbsync tunnel host")
	mailTunnelPort := flags.Int("mail-tunnel-port", 0, "internal: guarded mbsync tunnel port")
	if err := flags.Parse(args); err != nil {
		return err
	}
	if *mailTunnelHost != "" || *mailTunnelPort != 0 {
		if err := runMailTunnel(ctx, *mailTunnelHost, *mailTunnelPort); err != nil {
			return fmt.Errorf("mail tunnel: %w", err)
		}
		return nil
	}
	dataRoot, err := configuredDataRoot()
	if err != nil {
		return fmt.Errorf("invalid DAR_DATA_ROOT: %w", err)
	}

	// ── Database ──
	database, err := db.Open(*dbPath, serverMigrations())
	if err != nil {
		return fmt.Errorf("open database: %w", err)
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
			return fmt.Errorf("OIDC discovery: %w", err)
		} else {
			oidcProvider = p
			log.Printf("OIDC provider configured: %s", oidcProvider.DisplayName())
		}
	}

	// ── Server ──
	appearance := theme.NewService(database)
	srv := server.New(database, oidcProvider, appearance)
	parsedBaseURL, err := url.Parse(baseURL)
	if err != nil || parsedBaseURL.Host == "" {
		return fmt.Errorf("invalid DAR_BASE_URL: %q", baseURL)
	}
	srv.SetSecureCookies(strings.EqualFold(parsedBaseURL.Scheme, "https"))
	if err := srv.SetTrustedProxies(os.Getenv("DAR_TRUSTED_PROXIES")); err != nil {
		return fmt.Errorf("invalid DAR_TRUSTED_PROXIES: %w", err)
	}
	credentialConfig := strings.TrimSpace(os.Getenv("DAR_CREDENTIAL_ENCRYPTION_KEYS"))
	if credentialConfig == "" {
		credentialConfig = strings.TrimSpace(os.Getenv("DAR_EMAIL_ENCRYPTION_KEYS"))
	}
	sharedKeys, keyErr := credentials.Parse(credentialConfig)
	if keyErr != nil {
		log.Printf("Credential encryption is not configured: %v", keyErr)
	}
	var sharePasswordKey []byte
	if len(sharedKeys) > 0 {
		sharePasswordKey = sharedKeys[0].Material
	}
	outboundPolicy, err := outbound.ParsePolicy(os.Getenv("DAR_OUTBOUND_ALLOWED_HOSTS"), os.Getenv("DAR_OUTBOUND_ALLOWED_CIDRS"))
	if err != nil {
		return fmt.Errorf("invalid outbound allowlist: %w", err)
	}
	userQuotaBytes, err := envOptionalInt64("DAR_USER_STORAGE_QUOTA_BYTES")
	if err != nil {
		return err
	}
	maxCodexProcesses, err := envOptionalInt64("DAR_CODEX_MAX_PROCESSES")
	if err != nil {
		return err
	}
	geocoder := geocode.New(outboundPolicy.Client(8*time.Second), "")

	// ── App Registry ──
	registry := apps.NewRegistry()
	notificationService := notifications.New(database)
	registry.RegisterNotificationRecorder(notificationService)
	srv.HTTP.RegisterOnShutdown(notificationService.Close)
	searchService := searchsvc.New(database, registry, 30*time.Minute)
	registry.RegisterAssistantProvider("core", coreassistant.New(database))
	registry.Register(activityApp.New(activityApp.Config{DB: database, Registry: registry, BaseURL: baseURL}))
	files := filesApp.New(filesApp.Config{DataRoot: dataRoot, DB: database, SharePasswordKey: sharePasswordKey, SecureCookies: srv.SecureCookies(), UserQuotaBytes: userQuotaBytes, Search: searchService.Writer("files"), Registry: registry, Appearance: appearance})
	appearance.BindFiles(files)
	registry.Register(files)
	contacts := contactsApp.New(contactsApp.Config{DataRoot: dataRoot, DB: database, Search: searchService.Writer("contacts"), Registry: registry})
	registry.Register(contacts)
	dav := caldav.New()
	calendar := calendarApp.New(calendarApp.Config{
		DataRoot:        dataRoot,
		DB:              database,
		CalDAV:          dav,
		RefreshInterval: time.Duration(envInt("DAR_CALENDAR_REFRESH_MINUTES", 30)) * time.Minute,
		OutboundPolicy:  outboundPolicy,
		Search:          searchService.Writer("calendar"),
		Registry:        registry,
	})
	todo := todoApp.New(todoApp.Config{DataRoot: dataRoot, DB: database, Search: searchService.Writer("todo"), Registry: registry})
	dav.Register(todo)
	registry.Register(calendar)
	registry.Register(prayerApp.New(prayerApp.Config{DB: database, Geocoder: geocoder, Todo: todo}))
	registry.Register(todo)
	registry.Register(notesApp.New(notesApp.Config{DB: database, Files: files, Search: searchService.Writer("notes"), Registry: registry}))
	registry.Register(newsApp.New(newsApp.Config{
		DataRoot: dataRoot, DB: database,
		RefreshMinutes: envInt("DAR_NEWS_REFRESH_MINUTES", 30),
		ExploreURL:     os.Getenv("DAR_NEWS_EXPLORE_URL"),
		CredentialKeys: sharedKeys,
		OutboundPolicy: outboundPolicy,
		Search:         searchService.Writer("news"),
		Registry:       registry,
	}))
	emailKeys := make([]emailApp.CredentialKey, 0, len(sharedKeys))
	for _, key := range sharedKeys {
		emailKeys = append(emailKeys, emailApp.CredentialKey{ID: key.ID, Key: key.Material})
	}
	registry.Register(emailApp.New(emailApp.Config{
		DataRoot: dataRoot, DB: database, Recipients: contacts,
		MBSyncPath:      envString("DAR_EMAIL_MBSYNC_PATH", "mbsync"),
		TunnelPath:      executablePath(),
		SyncMinutes:     envInt("DAR_EMAIL_SYNC_MINUTES", 5),
		SyncConcurrency: envInt("DAR_EMAIL_SYNC_CONCURRENCY", 2),
		CredentialKeys:  emailKeys,
		OutboundPolicy:  outboundPolicy,
		Search:          searchService.Writer("email"),
		Registry:        registry,
	}))
	registry.RegisterDashboardProvider(10, weatherApp.New(weatherApp.Config{
		DB: database, Contacts: contacts, Client: outboundPolicy.Client(8 * time.Second), Geocoder: geocoder,
	}))
	registry.Register(codexApp.New(codexApp.Config{
		DB: database, Registry: registry,
		CodexPath:    envString("DAR_CODEX_PATH", "codex"),
		HomeRoot:     envString("DAR_CODEX_HOME_ROOT", filepath.Join(dataRoot, ".codex-users")),
		IdleTimeout:  time.Duration(envInt("DAR_CODEX_IDLE_MINUTES", 5)) * time.Minute,
		MaxProcesses: int(maxCodexProcesses),
		CSRFToken:    srv.CSRFToken,
		Search:       searchService.Writer("codex"),
	}))

	// ── Routes ──
	srv.Router.Get("/", handleIndex(srv))
	srv.Router.Get("/login", handleLogin(srv))
	srv.Router.Get("/auth/oidc/callback", handleOIDCCallback(srv, registry))
	srv.Router.Get("/assets/app.v1.css", http.HandlerFunc(webui.HandlePublicAsset))
	srv.Router.Get("/assets/favicon.svg", http.HandlerFunc(webui.HandlePublicAsset))
	srv.Router.Get("/assets/fontawesome/*", http.HandlerFunc(webui.HandlePublicAsset))
	srv.Router.Get("/home", srv.RequireAuth(srv.WithAppearance(handleHome(srv, registry))))
	srv.Router.Get("/search", srv.RequireAuth(srv.WithAppearance(handleSearchPage(srv, registry))))
	srv.Router.Get("/settings", srv.RequireAuth(srv.WithAppearance(handleSettings(srv, registry))))
	srv.Router.Get("/settings/*", srv.RequireAuth(srv.WithAppearance(handleSettings(srv, registry))))
	srv.Router.Get("/api/me", srv.RequireSessionAPI(handleAPIMe))
	srv.Router.Post("/api/search/query", srv.RequireSessionAPI(http.HandlerFunc(searchService.HandleQuery)))
	srv.Router.Post("/api/search/matches", srv.RequireSessionAPI(http.HandlerFunc(searchService.HandleMatches)))
	srv.Router.Handle("/api/search/preferences", srv.RequireSessionAPI(http.HandlerFunc(searchService.HandlePreferences)))
	srv.Router.Get("/api/notifications", srv.RequireSessionAPI(http.HandlerFunc(notificationService.HandleList)))
	srv.Router.Delete("/api/notifications", srv.RequireSessionAPI(http.HandlerFunc(notificationService.HandleDismissAll)))
	srv.Router.Delete("/api/notifications/{id}", srv.RequireSessionAPI(http.HandlerFunc(notificationService.HandleDismiss)))
	srv.Router.Handle("/api/notifications/preferences", srv.RequireSessionAPI(http.HandlerFunc(notificationService.HandlePreferences)))
	srv.Router.Get("/api/notifications/stream", srv.RequireSessionAPI(http.HandlerFunc(notificationService.HandleStream)))
	srv.Router.Get("/api/dashboard/catalog", srv.RequireSessionAPI(handleDashboardCatalog(registry)))
	srv.Router.Handle("/api/dashboard/layout", srv.RequireSessionAPI(handleDashboardLayout(srv, registry)))
	srv.Router.Get("/api/dashboard/widgets/{id}", srv.RequireSessionAPI(handleDashboardWidget(registry)))
	srv.Router.Get("/api/dashboard/widgets/{id}/settings", srv.RequireSessionAPI(handleDashboardWidgetSettings(registry)))
	srv.Router.Put("/api/dashboard/widgets/{id}/settings", srv.RequireSessionAPI(handleDashboardWidgetSettings(registry)))
	srv.Router.Handle("/api/settings/appearance", srv.RequireSessionAPI(handleAppearanceSettings(appearance)))
	srv.Router.Post("/api/settings/appearance/preview", srv.RequireSessionAPI(handleAppearancePreview(appearance)))
	srv.Router.Get("/api/appearance/wallpaper", srv.RequireSessionAPI(handleAppearanceWallpaper(appearance)))
	srv.Router.Get("/assets/text-editor.js", srv.RequireAuth(http.HandlerFunc(webui.HandleTextEditorAsset)))
	srv.Router.Get("/assets/dashboard.js", srv.RequireAuth(http.HandlerFunc(webui.HandleDashboardAsset)))
	srv.Router.Get("/assets/dar-ui.js", srv.RequireAuth(http.HandlerFunc(webui.HandleDarUIAsset)))
	srv.Router.Get("/assets/notifications.js", srv.RequireAuth(http.HandlerFunc(webui.HandleNotificationsAsset)))
	srv.Router.Get("/assets/codex-panel.js", srv.RequireAuth(http.HandlerFunc(webui.HandleCodexPanelAsset)))
	srv.Router.Get("/assets/search.js", srv.RequireAuth(http.HandlerFunc(webui.HandleSearchAsset)))
	srv.Router.Get("/assets/settings.js", srv.RequireAuth(http.HandlerFunc(handleSettingsAsset)))
	srv.Router.Get("/assets/shell.js", srv.RequireAuth(http.HandlerFunc(webui.HandleShellAsset)))
	srv.Router.Post("/logout", srv.RequireSessionAPI(handleLogout(srv, registry)))
	srv.Router.Get("/health", handleHealth(srv))

	// Settings APIs (session only).
	contactsAppEntry, ok := registry.App("contacts")
	if !ok {
		panic("contacts app is not registered")
	}
	contactsCapability, ok := contactsAppEntry.(meContactUpdater)
	if !ok {
		panic("contacts app does not implement Me contact updates")
	}
	srv.Router.Post("/api/settings/personal", srv.RequireSessionAPI(handleSavePersonal(srv, contactsCapability, registry)))
	srv.Router.Get("/api/settings/time-format", srv.RequireSessionAPI(handleGetTimeFormat(srv)))
	srv.Router.Post("/api/settings/time-format", srv.RequireSessionAPI(handleSaveTimeFormat(srv, registry)))
	srv.Router.Get("/api/settings/files", srv.RequireSessionAPI(handleGetFilesSettings(srv)))
	srv.Router.Post("/api/settings/files", srv.RequireSessionAPI(handleSaveFilesSettings(srv, registry)))
	srv.Router.Get("/api/settings/calendar", srv.RequireSessionAPI(handleGetCalendarSettings(srv)))
	srv.Router.Post("/api/settings/calendar", srv.RequireSessionAPI(handleSaveCalendarSettings(srv, registry)))
	srv.Router.Get("/api/settings/app-passwords", srv.RequireSessionAPI(handleListAppPasswords(srv)))
	srv.Router.Post("/api/settings/app-passwords", srv.RequireSessionAPI(handleCreateAppPassword(srv, registry)))
	srv.Router.Delete("/api/settings/app-passwords/{id}", srv.RequireSessionAPI(handleDeleteAppPassword(srv, registry)))

	// CardDAV discovery: well-known must work without auth (RFC 6764) so DAVx5
	// can locate the service; it only redirects to /dav/.
	srv.Router.Handle("/.well-known/carddav", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/dav/", http.StatusMovedPermanently)
	}))
	srv.Router.Handle("/.well-known/caldav", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "/remote.php/dav/calendars/", http.StatusMovedPermanently)
	}))
	srv.Router.Handle("/remote.php/dav", srv.RequireAnyScope([]string{"calendar", "todo"}, http.HandlerFunc(dav.Handler)))
	srv.Router.Handle("/remote.php/dav/*", srv.RequireAnyScope([]string{"calendar", "todo"}, http.HandlerFunc(dav.Handler)))
	// DAVx5 PROPFINDs the base URL first. Redirect DAV methods on / → /dav/.
	// (GET / stays the normal browser index redirect.)
	rootDAV := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Relative Location so clients re-use the same host + Basic auth.
		http.Redirect(w, r, "/dav/", http.StatusMovedPermanently)
	})
	srv.Router.Method("PROPFIND", "/", rootDAV)
	srv.Router.Method("OPTIONS", "/", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Allow", "OPTIONS, GET, HEAD, PROPFIND, REPORT, PUT, DELETE")
		w.Header().Set("DAV", "1, 3, addressbook")
		w.WriteHeader(http.StatusOK)
	}))
	registerAppRoutes(srv, registry)

	// ── Start ──
	if err := registry.Start(ctx); err != nil {
		return fmt.Errorf("start apps: %w", err)
	}
	defer registry.Shutdown(context.Background())
	if err := searchService.Start(ctx); err != nil {
		return fmt.Errorf("start search: %w", err)
	}
	defer searchService.Shutdown(context.Background())

	addr := fmt.Sprintf(":%d", *port)
	log.Printf("Dar server listening on http://localhost%s", addr)
	log.Printf("Database: %s", *dbPath)
	errCh := make(chan error, 1)
	go func() { errCh <- srv.Start(addr) }()
	select {
	case <-ctx.Done():
		log.Printf("Shutting down: %v", ctx.Err())
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := srv.Shutdown(shutdownCtx); err != nil {
			log.Printf("HTTP shutdown: %v", err)
		}
	case err := <-errCh:
		if !errors.Is(err, http.ErrServerClosed) {
			return fmt.Errorf("server: %w", err)
		}
	}
	return nil
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
		w.Header().Set("Cache-Control", "no-store")
		var providers []webui.LoginProvider
		if s.OIDC != nil {
			state, verifier, transaction, err := s.NewOIDCTransaction()
			if err != nil {
				log.Printf("OIDC state generation failed: %v", err)
				http.Error(w, "Authentication is temporarily unavailable", http.StatusInternalServerError)
				return
			}
			http.SetCookie(w, &http.Cookie{Name: "dar_oidc_transaction", Value: transaction, Path: "/auth/oidc/callback", MaxAge: 600, HttpOnly: true, Secure: s.SecureCookies(), SameSite: http.SameSiteLaxMode})
			providers = append(providers, webui.LoginProvider{
				Name: s.OIDC.DisplayName(),
				Icon: "🔑",
				URL:  s.OIDC.AuthURL(state, verifier),
			})
		}
		if err := webui.RenderLogin(w, providers); err != nil {
			http.Error(w, "Internal server error", http.StatusInternalServerError)
		}
	}
}

func handleOIDCCallback(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		if s.OIDC == nil {
			http.Error(w, "OIDC not configured", http.StatusBadRequest)
			return
		}
		transactionCookie, cookieErr := r.Cookie("dar_oidc_transaction")
		http.SetCookie(w, &http.Cookie{Name: "dar_oidc_transaction", Value: "", Path: "/auth/oidc/callback", MaxAge: -1, HttpOnly: true, Secure: s.SecureCookies(), SameSite: http.SameSiteLaxMode})
		providedState := r.URL.Query().Get("state")
		verifier, validTransaction := "", false
		if cookieErr == nil {
			verifier, validTransaction = s.ValidateOIDCTransaction(transactionCookie.Value, providedState)
		}
		if !validTransaction {
			http.Error(w, "Invalid or expired authentication state", http.StatusBadRequest)
			return
		}

		code := r.URL.Query().Get("code")
		if code == "" {
			http.Error(w, "Missing authorization code", http.StatusBadRequest)
			return
		}

		profile, err := s.OIDC.ExchangeCode(r.Context(), code, verifier)
		if err != nil {
			log.Printf("OIDC exchange failed: %v", err)
			http.Error(w, "Authentication failed", http.StatusBadGateway)
			return
		}

		conn, release, err := s.DB.Conn(r.Context())
		if err != nil {
			log.Printf("DB conn failed: %v", err)
			http.Error(w, "Authentication failed", http.StatusInternalServerError)
			return
		}
		defer release()

		username, err := oidcUsername(profile.PreferredUsername, profile.Email)
		if err != nil {
			log.Printf("OIDC username rejected: %v", err)
			http.Error(w, "Identity provider returned an invalid username", http.StatusBadRequest)
			return
		}
		user, _, err := auth.FindOrCreateUser(r.Context(), conn,
			username, profile.Email, profile.Name, profile.Picture,
			s.OIDC.Config().Issuer, profile.Sub,
		)
		if err != nil {
			log.Printf("FindOrCreateUser failed: %v", err)
			http.Error(w, "Account could not be provisioned", http.StatusConflict)
			return
		}
		adminGroup := strings.TrimSpace(os.Getenv("DAR_OIDC_ADMIN_CLAIM"))
		if err := auth.SetUserAdmin(conn, user.ID, profile.InGroup(adminGroup)); err != nil {
			log.Printf("SetUserAdmin failed: %v", err)
			http.Error(w, "Could not synchronize administrator status", http.StatusInternalServerError)
			return
		}

		session, err := auth.CreateSession(conn, user.ID)
		if err != nil {
			log.Printf("CreateSession failed: %v", err)
			http.Error(w, "Session creation failed", http.StatusInternalServerError)
			return
		}

		http.SetCookie(w, &http.Cookie{
			Name:     "dar_session",
			Value:    session.ID,
			Path:     "/",
			MaxAge:   30 * 24 * 60 * 60,
			HttpOnly: true,
			Secure:   s.SecureCookies(),
			SameSite: http.SameSiteLaxMode,
		})

		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "login", "signed in", "session", session.ID, "Dar session", "")
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

		if err := webui.RenderHome(w, r, user.DisplayName,
			webui.DashboardHTML(),
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
	_ = json.NewEncoder(w).Encode(map[string]string{"username": user.Username, "email": user.Email, "display_name": user.DisplayName, "avatar_url": user.AvatarURL, "issuer": user.OIDCIssuer})
}

func handleLogout(s *server.Server, registries ...*apps.Registry) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		user := server.GetUser(r)
		if cookie, err := r.Cookie("dar_session"); err == nil && cookie.Value != "" {
			conn, release, err := s.DB.Conn(r.Context())
			if err != nil {
				http.Error(w, "Logout failed", http.StatusInternalServerError)
				return
			}
			err = auth.DeleteSession(conn, cookie.Value)
			release()
			if err != nil {
				log.Printf("delete session during logout: %v", err)
				http.Error(w, "Logout failed", http.StatusInternalServerError)
				return
			}
		}
		http.SetCookie(w, &http.Cookie{
			Name:     "dar_session",
			Value:    "",
			Path:     "/",
			MaxAge:   -1,
			HttpOnly: true,
			Secure:   s.SecureCookies(),
			SameSite: http.SameSiteLaxMode,
		})
		http.SetCookie(w, &http.Cookie{Name: "dar_csrf", Value: "", Path: "/", MaxAge: -1, Secure: s.SecureCookies(), SameSite: http.SameSiteStrictMode})
		recordUserActivity(firstRegistry(registries), r.Context(), user, "system", "logout", "signed out", "session", "", "Dar session", "")
		http.Redirect(w, r, "/login", http.StatusSeeOther)
	}
}

func firstRegistry(registries []*apps.Registry) *apps.Registry {
	if len(registries) == 0 {
		return nil
	}
	return registries[0]
}

func recordUserActivity(reg *apps.Registry, ctx context.Context, user *auth.User, source, action, summary, objectType, objectID, objectName, objectURL string) {
	if reg == nil || user == nil {
		return
	}
	if err := reg.EnqueueActivity(ctx, apps.ActivityEvent{
		ActorID: user.ID, ActorName: user.DisplayName, ActorKind: "user",
		Source: source, Action: action, Summary: summary,
		ObjectType: objectType, ObjectID: objectID, ObjectName: objectName, ObjectURL: objectURL,
		Origin: "browser", RecipientIDs: []string{user.ID},
	}); err != nil {
		log.Printf("activity omitted after successful %s action: %v", action, err)
	}
}

func handleHealth(s *server.Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if err := s.DB.ExecNoArgs(r.Context(), "SELECT 1"); err != nil {
			http.Error(w, `{"status":"unhealthy"}`, http.StatusServiceUnavailable)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"status":"ok"}`))
	}
}

func envString(name, fallback string) string {
	if value := strings.TrimSpace(os.Getenv(name)); value != "" {
		return value
	}
	return fallback
}

func executablePath() string {
	path, err := os.Executable()
	if err != nil {
		return os.Args[0]
	}
	return path
}

func runMailTunnel(ctx context.Context, host string, port int) error {
	if port < 1 || port > 65535 {
		return fmt.Errorf("invalid port")
	}
	policy, err := outbound.ParsePolicy(os.Getenv("DAR_OUTBOUND_ALLOWED_HOSTS"), os.Getenv("DAR_OUTBOUND_ALLOWED_CIDRS"))
	if err != nil {
		return err
	}
	dialCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	conn, err := policy.DialContext(dialCtx, "tcp", net.JoinHostPort(host, strconv.Itoa(port)))
	if err != nil {
		return err
	}
	defer conn.Close()

	type copyResult struct {
		remote bool
		err    error
	}
	results := make(chan copyResult, 2)
	go func() {
		_, copyErr := io.Copy(conn, os.Stdin)
		if tcp, ok := conn.(*net.TCPConn); ok {
			_ = tcp.CloseWrite()
		}
		results <- copyResult{err: copyErr}
	}()
	go func() {
		_, copyErr := io.Copy(os.Stdout, conn)
		results <- copyResult{remote: true, err: copyErr}
	}()
	first := <-results
	if first.remote {
		_ = conn.Close()
		return first.err
	}
	second := <-results
	if first.err != nil {
		return first.err
	}
	return second.err
}

func configuredDataRoot() (string, error) {
	path, err := filepath.Abs(envString("DAR_DATA_ROOT", "data"))
	if err != nil {
		return "", fmt.Errorf("resolve absolute path: %w", err)
	}
	return filepath.Clean(path), nil
}

func envInt(name string, fallback int) int {
	value, err := strconv.Atoi(strings.TrimSpace(os.Getenv(name)))
	if err != nil || value <= 0 {
		return fallback
	}
	return value
}

func envOptionalInt64(name string) (int64, error) {
	value := strings.TrimSpace(os.Getenv(name))
	if value == "" {
		return 0, nil
	}
	n, err := strconv.ParseInt(value, 10, 64)
	if err != nil || n < 0 {
		return 0, fmt.Errorf("%s must be a non-negative integer", name)
	}
	return n, nil
}

func registerAppRoutes(srv *server.Server, registry *apps.Registry) {
	for _, app := range registry.Apps() {
		for _, route := range app.Routes() {
			handler := http.HandlerFunc(route.Handler.ServeHTTP)
			switch route.Auth {
			case apps.AuthPublic:
			case apps.AuthBrowser:
				handler = srv.RequireAuth(srv.WithAppearance(injectAppBar(registry.AppBarHTML(), handler)))
			case apps.AuthAPI:
				handler = srv.RequireScope(app.Name(), handler)
			case apps.AuthSessionAPI:
				handler = srv.RequireSessionAPI(handler)
			default:
				panic(fmt.Sprintf("app %q route %q has invalid auth mode", app.Name(), route.Pattern))
			}
			if route.Method == apps.MethodAny {
				srv.Router.Handle(route.Pattern, handler)
			} else {
				srv.Router.Method(route.Method, route.Pattern, handler)
			}
		}
	}
}

func oidcUsername(preferred, email string) (string, error) {
	username := strings.TrimSpace(preferred)
	if username == "" {
		username, _, _ = strings.Cut(strings.TrimSpace(email), "@")
	}
	if username == "" || len(username) > 128 {
		return "", fmt.Errorf("missing or oversized username")
	}
	for _, r := range username {
		if !((r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '.' || r == '_' || r == '-') {
			return "", fmt.Errorf("unsupported character in username")
		}
	}
	return username, nil
}

// injectAppBar wraps a handler to inject app bar HTML into the request context.
func injectAppBar(appBar template.HTML, next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := context.WithValue(r.Context(), server.AppBarContextKey, appBar)
		next.ServeHTTP(w, r.WithContext(ctx))
	}
}
