# Dar Go Agent Guide

Dar is a self-hosted Nextcloud-like platform written in Go. Web server, OIDC login, app system where each app can run its own daemon process (Files, Calendar, Contacts, Soosa, Todo, Notes, News, Email, Photos). Soosa is the central AI assistant integrated with every app.

## Critical Rule: Ask Before Architectural Decisions

**Before making any architectural decision**, prompt the user for direction. Do not guess. Examples:

- Choosing between two implementations (e.g., sync.Map vs mutex+map, interface vs concrete type)
- Adding a new package or abstraction layer
- Changing an existing API signature
- Deciding how to structure a multi-step workflow
- Picking between goroutine and subprocess for a daemon

Say: "Two approaches: A) X, B) Y. Which one?" — do not silently pick one.

## Dependencies (7 total, zero CGo)

| Dependency | Purpose |
|---|---|
| `github.com/go-chi/chi/v5` | HTTP router |
| `golang.org/x/oauth2` | OIDC/OAuth2 client |
| `zombiezen.com/go/sqlite` | Pure-Go SQLite driver |
| `github.com/emersion/go-imap` | IMAP client |
| `github.com/emersion/go-ical` | iCal/RRULE parser |
| `github.com/emersion/go-vcard` | vCard parser |
| `github.com/mmcdole/gofeed` | RSS/Atom parser |

**No other dependencies allowed.** No ORM, no web framework (beyond chi), no template engine (use `html/template`), no CSS/JS build tools.

## Frontend

- Tailwind CSS via CDN (`<script src="https://cdn.tailwindcss.com">`) — no build step
- Vanilla JavaScript for progressive enhancement — no React, no Vue, no HTMX
- HTML is server-rendered via Go `html/template` with auto-escaping
- Material 3 design tokens via Tailwind config

## Go Version

Go 1.26+ (what's available via nixpkgs). Use `nix-shell -p go` for development.

## Build / Test / Run

```bash
# Build all binaries
go build ./cmd/...

# Run all tests
go test ./internal/... -v -count=1

# Run with race detector
go test -race ./internal/... -v -count=1

# Build and run server
go build -o dar-server ./cmd/server/
./dar-server --port 8080 --db dar.db
```

## Key Conventions

### No CGo
All CGo is banned. Use `zombiezen.com/go/sqlite` for SQLite (pure Go). No `mattn/go-sqlite3`.

### SQLite
Use `zombiezen.com/go/sqlite` and `zombiezen.com/go/sqlite/sqlitemigration`. WAL mode. Prepared statements for user input. One shared database file.

### HTTP Server
Use `net/http` with `chi` router. Middleware via `chi.Use()`. Session auth via context values.

### HTML Rendering
Use `html/template` with `embed.FS` for template files. All templates in `web/templates/`. One base layout template with blocks for content.

### Tailwind CSS
Material 3 design tokens configured in Tailwind config. No custom CSS files. No inline `style=""` attributes.

### Apps
Each app implements the `apps.App` interface. Apps register routes on the main server and optionally spawn a daemon. Daemons register their capabilities as tools with Soosa.

### Daemon Manager
Spawns daemon processes, health-checks via HTTP, monitors for crashes, restarts with exponential backoff, graceful shutdown on SIGTERM.

### OIDC Auth
Single OIDC provider configured via environment variables. Session cookies (HttpOnly, SameSite=Lax, 30-day expiry).

### Error Handling
- Router returns 404 for unknown paths
- Auth middleware returns 302 to `/login`
- Database errors return 500
- Validation errors return 400 with JSON

Never panic in HTTP handlers. Always return an error response.

### Testing
- Tests next to implementation (`*_test.go`)
- Integration tests in `test/`
- Test databases in `/tmp/dar_test_*.sqlite` with cleanup
- HTTP handlers tested via `httptest.NewRecorder`

## Architecture

1. `cmd/server/main.go` — entry: parse CLI, init DB, register apps, start server + daemon manager
2. `internal/server/` — HTTP server, router setup, middleware (session auth, logging)
3. `internal/auth/` — OIDC provider, user CRUD, session management
4. `internal/db/` — SQLite wrapper, migrations
5. `internal/daemon/` — Daemon manager: spawn, health-check, restart, shutdown
6. `internal/apps/` — App interface + registry (collects apps, builds app bar)
7. `internal/soosa/` — Soosa AI core: LLM client, tool registry, SSE streaming
8. `internal/webui/` — HTML layout, components, page rendering
9. `cmd/<app>/main.go` — Daemon binaries (one per app)

## Project Layout

```
dar/
├── cmd/
│   ├── server/main.go          # Main server
│   ├── soosa/main.go           # Soosa AI daemon
│   ├── files/main.go           # Files/WebDAV daemon
│   ├── calendar/main.go        # CalDAV daemon
│   ├── contacts/main.go        # CardDAV daemon
│   └── ...                     # Other daemons
├── internal/
│   ├── server/                 # HTTP server, router, middleware
│   ├── auth/                   # OIDC, users, sessions
│   ├── db/                     # SQLite wrapper
│   ├── daemon/                 # Daemon manager
│   ├── apps/                   # App interface + registry
│   ├── soosa/                  # Soosa core
│   ├── webdav/                 # WebDAV protocol
│   ├── caldav/                 # CalDAV protocol
│   ├── carddav/                # CardDAV protocol
│   └── webui/                  # HTML templates, layout, components
├── web/templates/              # Go html/template files
├── go.mod / go.sum
└── AGENTS.md                   # This file
```

## OIDC Environment Variables

```bash
DAR_OIDC_ISSUER=https://accounts.google.com
DAR_OIDC_CLIENT_ID=...
DAR_OIDC_CLIENT_SECRET=...
DAR_OIDC_DISPLAY_NAME=Google
DAR_BASE_URL=http://localhost:8080
```

## What to Avoid

- Do NOT add a second database (no PostgreSQL, no Redis)
- Do NOT add a build step for CSS/JS
- Do NOT add a framework beyond chi
- Do NOT add CGo dependencies
- Do NOT use ORMs
- Do NOT store passwords (OIDC only)
- Do NOT use `sed` on Go files — use `patch` or `write_file`
- Do NOT commit before testing (`go test ./...` must pass)
