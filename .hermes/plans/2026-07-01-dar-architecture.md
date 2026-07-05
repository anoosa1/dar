# Dar Architecture & Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan phase-by-phase.

**Goal:** A self-hosted Nextcloud-like platform in Zig — web server with user accounts, OAuth login, and an app system where each app can run its own daemon process and expose a web UI in the dashboard.

**Architecture:** Single binary (one `zig build-exe`). Apps are Zig modules compiled in — each can spawn a daemon subprocess (e.g., a DAV server) that the web UI connects to. SQLite is the only database. HTML is server-rendered with Tailwind CSS loaded from CDN. OAuth2 client for login (Google, GitHub) — the app never stores passwords. Cookie-based sessions.

**Tech Stack:** Zig 0.17, SQLite (via FFI), Tailwind CSS (CDN), Vanilla JS (progressive enhancement), no NPM, no bundler, no build step for frontend.

---

## Architecture Decisions

### 1. Single Binary, Compile-Time Apps + Daemon Processes

Each app is a Zig module in `src/apps/<name>/`. It registers web UI routes and can optionally spawn a daemon subprocess that runs alongside the main server. The web UI communicates with the daemon via HTTP (localhost port) or stdin/stdout pipe.

For example, the Files app might spawn a WebDAV daemon on `:8081`. The web UI at `/files` proxies requests to it, or the daemon handles DAV protocol directly on its own port.

**Why daemons as subprocesses?** A DAV server needs to handle PROPFIND, locks, and ETags — reimplementing that inside the main binary would bloat the core. Instead, each app manages its own daemon lifecycle: spawn on startup, health-check, restart on crash, kill on shutdown. The main binary stays focused on auth, routing, and the dashboard.

**Why not dynamic loading?** Zig has no stable ABI. Compile-time app modules are simpler, faster, and type-safe. Adding an app = add the import + register call in `main.zig` + rebuild.

### 2. Server-Rendered HTML, Not SPA

Pages are rendered server-side with `std.fmt` formatting into HTML strings. No client-side routing, no JSON-in-HTML hydration, no frontend framework. JavaScript is used only for progressive enhancement (dropdowns, modals, file upload previews).

**Why?** An SPA would require a client-side router, state management, an API client, and a bundler. For a self-hosted app with one user per instance (initial target), server-rendered pages are faster to build and simpler to maintain. HTMX can be added later if needed.

### 3. OAuth2 Client, No Password Storage

The app acts as an OAuth2 client. Users log in via external providers (Google, GitHub). The app stores only the OAuth provider + external user ID + email + display name. Session tokens are random strings stored in cookies.

**Why?** No password hashing, no password reset flow, no email verification. OAuth2 is battle-tested and users already have accounts. The app never touches a password.

### 4. Tailwind CSS via CDN

A single `<script src="https://cdn.tailwindcss.com">` in the `<head>`. No build step, no `tailwind.config.js`, no PostCSS. Classes are used directly in HTML templates.

**Why?** Tailwind's CDN script compiles used classes on-the-fly in the browser. For development and self-hosted deployments (where the browser caches the CDN script), this is fast enough. A production optimization (pre-compiled CSS) can be added later.

### 5. SQLite for Everything

One SQLite database file (`dar.db`). Each plugin creates its own tables via migrations. No foreign keys between plugins (loose coupling). WAL mode for concurrent reads.

### 6. RESTful JSON API

Plugin operations that need client-side interactivity use RESTful JSON endpoints. The web UI fetches these with `fetch()`. Example: `/api/files/list`, `/api/calendar/events`.

---

## Module Layout

```
dar/
├── build.zig                    # Build: links sqlite3, zig build-exe src/main.zig
├── build.zig.zon                # Package manifest (zero deps, minimum_zig_version 0.17.0)
├── AGENTS.md                    # Project conventions
├── TODO.md                      # Task list
├── src/
│   ├── main.zig                 # Entry: parse CLI, init DB, register apps + spawn daemons, start server
│   ├── lib.zig                  # Module registry (all pub imports for test coverage)
│   ├── common/
│   │   ├── common.zig           # Shared types: User, Session, App, Daemon, Route
│   │   ├── compat.zig           # Zig 0.17 compat: global Io instance
│   │   └── c.zig                # FFI: sqlite3 declarations only
│   ├── db/
│   │   └── db.zig               # SQLite wrapper: init DB, run migrations, query helpers
│   ├── server/
│   │   ├── server.zig           # HTTP server: accept loop, connection handling
│   │   ├── router.zig           # Path router: match method+path → handler
│   │   ├── request.zig          # Request parsing: query params, form data, cookies, JSON body
│   │   ├── response.zig         # Response builders: html(), json(), redirect(), error()
│   │   └── middleware.zig       # Middleware chain: session auth, logging, CORS
│   ├── auth/
│   │   ├── auth.zig             # OAuth2 client: redirect, callback, token exchange
│   │   ├── sessions.zig         # Session management: create, validate, delete
│   │   └── users.zig            # User CRUD: find-or-create from OAuth profile
│   ├── daemon/
│   │   └── daemon.zig           # Daemon manager: spawn, health-check, restart, kill per app
│   ├── apps/
│   │   ├── app.zig              # App interface: App struct, register, daemon config
│   │   ├── registry.zig         # App registry: collect apps, init DB, spawn daemons, build app bar
│   │   ├── files/               # Files app (WebDAV daemon + web UI)
│   │   │   └── files.zig        # App definition, web routes, daemon spawn config
│   │   ├── calendar/            # Calendar app (CalDAV daemon + web UI)
│   │   │   └── calendar.zig
│   │   ├── contacts/            # Contacts app (CardDAV daemon + web UI)
│   │   │   └── contacts.zig
│   │   └── soosa/               # Soosa AI agent app
│   │       └── soosa.zig
│   └── webui/
│       ├── layout.zig           # HTML page shell: <head>, top bar with app icons, main area
│       ├── home.zig             # Home screen: app launcher grid when logged in
│       ├── login.zig            # Login page: OAuth provider buttons
│       ├── components.zig       # Reusable HTML components: buttons, cards, modals
│       └── static.zig           # Static file serving: favicon, robots.txt
└── .hermes/
    └── plans/
```

---

## App Interface

Every app exposes:

```zig
pub const App = struct {
    name: []const u8,           // "files", "calendar", etc.
    display_name: []const u8,   // "Files", "Calendar"
    icon: []const u8,           // SVG or emoji for the app bar
    description: []const u8,
    migrations: []const Migration, // SQL statements for app's tables
    routes: []const Route,      // Web UI routes served by the main server
    app_bar_order: u8,          // Position in top app bar (lower = leftmost)
    daemon: ?DaemonConfig,      // If set, spawn this daemon on startup
};

pub const DaemonConfig = struct {
    command: []const u8,        // Binary to spawn (e.g. "./dar-files-daemon")
    args: []const []const u8,   // CLI args (e.g. &[_][]const u8{"--port", "8081"})
    port: u16,                  // Port the daemon listens on
    health_path: []const u8,    // GET this path to health-check (e.g. "/health")
    startup_timeout_ms: u32,    // Max ms to wait for daemon to become healthy
};
```

Apps are registered in `main.zig`:

```zig
const files_app = @import("apps/files/files.zig").app;
const calendar_app = @import("apps/calendar/calendar.zig").app;
try registry.register(files_app);
try registry.register(calendar_app);
try registry.runMigrations(db);

// For each app that has a daemon, spawn it:
try daemon_mgr.spawn(files_app.daemon.?);
try daemon_mgr.spawn(calendar_app.daemon.?);

// Daemon manager monitors all spawned daemons, restarts on crash
```

### Daemon Lifecycle

1. **Spawn** — `main.zig` calls `daemon_mgr.spawn(config)` for each app with a daemon
2. **Health check** — Daemon manager polls `GET localhost:<port>/health` until 200 or timeout
3. **Running** — Web UI routes proxy to the daemon, or the daemon handles its own port directly
4. **Crash** — Daemon manager detects exit, logs, restarts after backoff
5. **Shutdown** — On SIGTERM/SIGINT, daemon manager kills all spawned daemons

### Data Flow (with daemon)

```
Browser → GET /files → Main server (auth, routing) → Render web UI HTML
Browser → fetch('/api/files/list') → Main server → Proxy to localhost:8081 (files daemon)
  → Files daemon handles request → JSON response → Main server → Browser
```

Or, for DAV protocol clients (native apps):

```
Finder/Nautilus → PROPFIND dav://localhost:8081/ → Files daemon handles directly
```

The daemon port is NOT exposed to the internet — only localhost. The main server acts as the public-facing gateway with auth. DAV protocol endpoints can be exposed directly on the daemon port for native client compatibility (Finder, Thunderbird, phone address books).

---

## Data Flow

### Login Flow
```
Browser → GET / → Server → No session → Redirect to /login
  → Click "Login with Google" → Redirect to Google OAuth
  → User approves → Google redirects to /auth/callback?code=...
  → Server → Exchange code for token → Get user profile
  → Find-or-create user in SQLite → Create session → Set cookie
  → Redirect to /home
```

### Home Screen
```
Browser → GET /home → Server → Auth middleware (valid session)
  → Render layout.zig with app bar (icons from registry)
  → Render home.zig (app launcher grid or empty state)
  → Response (HTML) → Browser renders
```

### App Page (with daemon)
```
Browser → Click "Files" in app bar → GET /files
  → Main server → Auth middleware → Files app handler
  → Render app HTML (JS makes API calls to the daemon via main server proxy)
  → Browser JS → fetch('/api/files/list') → Main server → Proxy to daemon :8081
  → Daemon responds with JSON → Main server → Browser
```

### DAV Protocol (native clients)
```
Finder → PROPFIND dav://host:8081/ → Files daemon (direct, no auth proxy)
Thunderbird → REPORT calendar-query → Calendar daemon (direct)
```

---

## Implementation Phases

### Phase 0: Skeleton — Hello World Web Server
Bare minimum to see HTML in a browser with Tailwind.

**Deliverables:**
- `build.zig` + `build.zig.zon` (zig 0.17, links sqlite3)
- `src/common/compat.zig` — global Io instance
- `src/common/c.zig` — sqlite3 FFI declarations
- `src/common/common.zig` — shared types (User, Session, App, DaemonConfig)
- `src/server/response.zig` — `html()`, `json()`, `redirect()` helpers
- `src/webui/layout.zig` — `<!DOCTYPE html>` shell with Tailwind CDN + empty top bar placeholder
- `src/webui/login.zig` — login page with placeholder OAuth buttons (non-functional)
- `src/main.zig` — CLI args (`--port`, `--db`), HTTP accept loop, routing (static paths only)

**Verification:**
- `zig build` succeeds
- `curl http://localhost:8080/` returns HTML with "Dar" + Tailwind classes
- `curl http://localhost:8080/login` returns login page HTML
- No database yet, no auth — just static HTML

### Phase 1: Database + Users + OAuth
Users can log in via OAuth, sessions persist in SQLite.

**Deliverables:**
- `src/db/db.zig` — `open()`, `exec()`, prepared statements, WAL mode, migration runner
- `src/auth/users.zig` — `findByEmail()`, `findOrCreate()` from OAuth profile
- `src/auth/sessions.zig` — random token generation, create/validate/delete, cookie helpers
- `src/auth/auth.zig` — OAuth2 client: `/login` redirect, `/auth/callback` handler, token exchange, user profile fetch via `std.http.Client`
- `src/server/router.zig` — method + path → handler function matching
- `src/server/middleware.zig` — session auth middleware (check cookie → load user → attach to request)
- `src/server/request.zig` — parse cookies, query params, JSON body
- Wire login page buttons to actual OAuth URLs
- Config via env vars: `DAR_GOOGLE_CLIENT_ID`, `DAR_GOOGLE_CLIENT_SECRET`, `DAR_BASE_URL`

**Verification:**
- Visit `/login` → click Google → OAuth flow completes → redirected to `/home`
- Session cookie (`dar_session`) is set, HttpOnly, works across requests
- User row exists in SQLite after first login
- Second login finds existing user, updates `last_login_at`
- `GET /api/me` returns `{email, display_name, avatar_url}`

### Phase 2: Home Screen + App Bar
Logged-in users see a home screen with an app bar showing enabled apps.

**Deliverables:**
- `src/apps/app.zig` — `App` struct definition (name, display_name, icon, routes, daemon config, app_bar_order)
- `src/apps/registry.zig` — `register()`, `getApps()`, `runMigrations()`, `buildAppBar()` (returns HTML for app bar icons)
- `src/webui/layout.zig` — top bar rendering from registry (app icons + user avatar + logout)
- `src/webui/home.zig` — home screen: app launcher grid (cards for each app with icon, name, description)
- `src/daemon/daemon.zig` — daemon manager skeleton (spawn/health-check/kill — used later)
- Wire `main.zig` to register apps, run migrations, build app bar
- At this stage, apps are registered but their routes return a placeholder "coming soon" page

**Verification:**
- After login, `/home` shows app bar with icons for enabled apps
- Clicking an app icon navigates to `/files` (shows placeholder)
- Home screen shows app cards in a grid
- Disabling an app (not importing it) removes it from the bar
- Logout clears session, redirects to `/login`

### Phase 3: App — Files (WebDAV daemon + Web UI)
File browser app with its own WebDAV daemon process.

**Deliverables:**
- `src/apps/files/files.zig` — app definition + daemon config + web UI routes
- Files daemon binary (separate Zig binary or same binary in daemon mode — whatever's simpler):
  - Serves WebDAV protocol on a configurable port (default 8081)
  - Stores files on disk under `data/files/`
  - SQLite table: `files` (id, path, name, size, mime_type, created_at, updated_at)
  - Endpoints: PROPFIND, GET, PUT, DELETE, MKCOL, MOVE, COPY
  - Health check at `/health` returns 200
- Web UI at `/files`:
  - File browser with breadcrumbs, table view, upload button
  - Drag-and-drop upload to `/api/files/upload` (proxied to daemon)
  - Create folder, delete, rename, move
  - Download link per file
- Daemon manager spawns files daemon on startup, health-checks it
- Main server proxies `/api/files/*` to the daemon

**Verification:**
- Files daemon starts, passes health check, appears in daemon manager status
- Upload a file via web UI, see it in the list
- Download returns correct file with correct Content-Type
- WebDAV: `curl -X PROPFIND http://localhost:8081/` returns directory listing
- Mount in Nautilus/Finder via `dav://localhost:8081/`

### Phase 4: App — Calendar (CalDAV daemon + Web UI)

**Deliverables:**
- `src/apps/calendar/calendar.zig` — app definition + daemon config + web UI routes
- Calendar daemon (separate binary or daemon mode):
  - Serves CalDAV on port 8082
  - SQLite: `calendars` + `events` tables (with RRULE support)
  - Endpoints: PROPFIND, REPORT calendar-query, MKCALENDAR
  - iCal import (POST .ics) and export (GET .ics)
  - Health check at `/health`
- Web UI at `/calendar`:
  - Month grid view with event dots
  - Click day → list of events
  - Create/edit/delete event modal
  - Recurring event expansion (daily, weekly, monthly, yearly)
- Daemon manager spawns calendar daemon

**Verification:**
- Create event in web UI, see it on the calendar grid
- Recurring weekly event shows all occurrences for the month
- Import .ics file, events appear
- CalDAV: Thunderbird can sync with `http://localhost:8082/`

### Phase 5: App — Contacts (CardDAV daemon + Web UI)

**Deliverables:**
- `src/apps/contacts/contacts.zig` — app definition + daemon config + web UI routes
- Contacts daemon on port 8083:
  - SQLite: `address_books` + `contacts` tables (vCard data)
  - Endpoints: PROPFIND, REPORT addressbook-query
  - vCard import/export
  - Health check at `/health`
- Web UI at `/contacts`:
  - Contact list with search, alphabetical grouping, avatar circles
  - Contact detail: name, email, phone, address, notes
  - Create/edit/delete contact
  - Import .vcf, export .vcf

**Verification:**
- CRUD contacts in web UI
- Search finds contacts by name/email
- Import .vcf, contact appears
- CardDAV: Phone address book syncs with `http://localhost:8083/`

### Phase 6: App — Soosa (AI Agent web UI)

**Deliverables:**
- `src/apps/soosa/soosa.zig` — app definition + web UI routes
- No separate daemon — Soosa runs as a subprocess per request (or persistent daemon, decided later)
- Web UI at `/soosa`:
  - Chat interface: message bubbles (user right, agent left)
  - SSE streaming: agent tokens appear in real-time
  - Session list: create new chat, browse past sessions
- Subprocess integration: spawn `soosa chat` via raw syscalls, pipe stdin/stdout
- Session persistence: reuse Soosa's SQLite sessions

**Verification:**
- Type message, agent responds with streaming output
- Multiple sessions, switch between them
- Agent uses tools from within the web chat

### Phase 7: Polish
**Deliverables:**
- Mobile-responsive top bar (hamburger menu on small screens)
- Error pages (404, 500) with Dar branding and Tailwind styling
- CSRF tokens on all POST/PUT/DELETE
- Input validation on all endpoints
- File upload size limits
- Security headers (CSP, X-Frame-Options, etc.)
- Daemon manager: graceful shutdown on SIGTERM (kill daemons, close DB)
- Dark mode toggle (Tailwind `dark:` classes + localStorage)
- Request logging (timestamp, method, path, status, duration)

---

## Database Schema (Core Tables)

```sql
-- Users (from OAuth)
CREATE TABLE users (
    id TEXT PRIMARY KEY,        -- UUID
    email TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    avatar_url TEXT,
    provider TEXT NOT NULL,     -- 'google', 'github'
    provider_id TEXT NOT NULL,  -- external user ID
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Sessions
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,        -- random token (stored in cookie)
    user_id TEXT NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
    user_agent TEXT,
    ip_address TEXT
);

-- Plugin migrations table (track which migrations ran)
CREATE TABLE _migrations (
    plugin TEXT NOT NULL,
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (plugin, version)
);
```

Plugin tables (files, calendar events, contacts) are created by each plugin's migrations.

---

## Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| App loading | Compile-time Zig modules | No stable ABI, simpler, type-safe |
| App daemons | Separate OS processes | Each app owns its protocol (DAV), main binary stays lean |
| Daemon comms | HTTP on localhost | Standard, debuggable, no IPC protocol to invent |
| Frontend | Server-rendered HTML | No bundler, faster to build, good enough for self-hosted |
| CSS | Tailwind CDN | No build step, productive, easy to customize |
| Auth | OAuth2 client only | No password management, users already have accounts |
| Sessions | Cookie + SQLite | Simple, survives restarts, no Redis needed |
| File storage | Local filesystem | No S3 dependency for self-hosted single-user |
| API style | RESTful JSON | Simple, well-understood, easy to test with curl |
| DAV protocols | Handled by app daemon, not main server | PROPFIND/REPORT are complex — isolate in purpose-built daemon |

---

## What Dar Is NOT

- **Not multi-tenant** (initially). One instance = one user. Multi-user can be added later by making all plugin tables include `user_id`.
- **Not a CDN/origin for production traffic**. The Zig HTTP server is for self-hosted personal use, not thousands of concurrent users.
- **Not a Nextcloud replacement** (yet). Nextcloud has 10+ years of plugins. Dar starts with 4 core apps and grows.
- **Not an SPA**. If heavy client-side interactivity is needed later, HTMX or Alpine.js can be added without a build step.

---

## First Steps After Plan Approval

1. `git init && git add -A && git commit -m "initial plan"`
2. Create `build.zig` + `build.zig.zon` (copy pattern from soosa)
3. Create `src/main.zig` — minimal HTTP server returning "hello world" HTML
4. Verify `curl http://localhost:8080` works
5. Proceed through phases per this plan
