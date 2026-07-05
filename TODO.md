# Dar — TODO

A self-hosted Nextcloud-like platform in Zig. Web server, OAuth login, app system.
Each app can run its own daemon process. No dependencies except SQLite. Tailwind CSS via CDN.

## Phase 0: Skeleton — Hello World Web Server
- [ ] create build.zig + build.zig.zon (zig 0.17, links sqlite3)
- [ ] create src/common/compat.zig (global Io instance)
- [ ] create src/common/c.zig (sqlite3 FFI only)
- [ ] create src/common/common.zig (shared types: User, Session, App, DaemonConfig)
- [ ] create src/server/response.zig (html, json, redirect helpers)
- [ ] create src/webui/layout.zig (HTML shell + Tailwind CDN + empty top bar placeholder)
- [ ] create src/webui/login.zig (login page with placeholder OAuth buttons)
- [ ] create src/main.zig (CLI args --port --db, HTTP accept loop, static routing)
- [ ] verify: zig build succeeds, curl returns login page HTML with Tailwind

## Phase 1: Database + Users + OAuth
- [ ] create src/db/db.zig (open, exec, prepared statements, WAL mode, migrations)
- [ ] create src/auth/users.zig (findByEmail, findOrCreate from OAuth profile)
- [ ] create src/auth/sessions.zig (random token, create, validate, delete, cookie helpers)
- [ ] create src/auth/auth.zig (OAuth2: /login redirect, /auth/callback, token exchange, user profile fetch via std.http.Client)
- [ ] create src/server/request.zig (parse cookies, query params, JSON body)
- [ ] create src/server/router.zig (method + path → handler matching)
- [ ] create src/server/middleware.zig (session auth: check cookie → load user → attach to request)
- [ ] wire login page buttons to actual OAuth URLs (config via env vars)
- [ ] verify: OAuth login flow works, session cookie set, /api/me returns user info

## Phase 2: Home Screen + App Bar
- [ ] create src/apps/app.zig (App struct: name, display_name, icon, routes, daemon config, app_bar_order)
- [ ] create src/apps/registry.zig (register, getApps, runMigrations, buildAppBar HTML)
- [ ] update src/webui/layout.zig (top bar renders from registry: app icons + user avatar + logout)
- [ ] create src/webui/home.zig (home screen: app launcher grid with cards)
- [ ] create src/daemon/daemon.zig (daemon manager skeleton: spawn, health-check, kill — used later)
- [ ] wire main.zig: register apps, run migrations, build app bar on /home
- [ ] verify: after login, /home shows app bar with icons; clicking navigates to app placeholder; logout works

## Phase 3: App — Files (WebDAV daemon + Web UI)
- [ ] create src/apps/files/files.zig (app definition + daemon config + web UI routes)
- [ ] Files daemon: WebDAV on port 8081, stores files in data/files/, health at /health
- [ ] Files daemon: PROPFIND, GET, PUT, DELETE, MKCOL, MOVE, COPY
- [ ] Files daemon: SQLite files table (id, path, name, size, mime_type, timestamps)
- [ ] Web UI: GET /files — file browser (breadcrumbs, table, upload button, drag-and-drop)
- [ ] Web UI: POST /api/files/upload, GET /api/files/download, POST /api/files/mkdir, DELETE, POST /api/files/move
- [ ] Daemon manager spawns files daemon on startup, health-checks it
- [ ] Main server proxies /api/files/* to daemon
- [ ] verify: upload via web UI, download, WebDAV curl PROPFIND, mount in file manager

## Phase 4: App — Calendar (CalDAV daemon + Web UI)
- [ ] create src/apps/calendar/calendar.zig (app definition + daemon config + web UI routes)
- [ ] Calendar daemon: CalDAV on port 8082, SQLite calendars + events tables (RRULE support)
- [ ] Calendar daemon: PROPFIND, REPORT calendar-query, MKCALENDAR, iCal import/export, health /health
- [ ] Web UI: GET /calendar — month grid view with event dots, click day → event list
- [ ] Web UI: create/edit/delete event modal, recurring event expansion
- [ ] Daemon manager spawns calendar daemon
- [ ] verify: create events in web UI, recurring events expand, import/export .ics, Thunderbird sync

## Phase 5: App — Contacts (CardDAV daemon + Web UI)
- [ ] create src/apps/contacts/contacts.zig (app definition + daemon config + web UI routes)
- [ ] Contacts daemon: CardDAV on port 8083, SQLite address_books + contacts tables (vCard)
- [ ] Contacts daemon: PROPFIND, REPORT addressbook-query, vCard import/export, health /health
- [ ] Web UI: GET /contacts — list with search, alphabetical grouping, avatar circles
- [ ] Web UI: contact detail, create/edit/delete, import/export .vcf
- [ ] verify: CRUD contacts, search, import .vcf, phone CardDAV sync

## Phase 6: App — Soosa (AI Agent web UI)
- [ ] create src/apps/soosa/soosa.zig (app definition + web UI routes)
- [ ] Web UI: GET /soosa — chat interface (message bubbles, input box)
- [ ] Web UI: POST /api/soosa/chat — SSE streaming response from soosa subprocess
- [ ] Web UI: GET /api/soosa/sessions, GET /api/soosa/sessions/:id — session management
- [ ] Subprocess integration: spawn soosa chat, pipe stdin/stdout
- [ ] verify: chat works, streaming tokens appear, multiple sessions, tools work

## Phase 7: Polish
- [ ] Mobile-responsive top bar (hamburger menu on small screens)
- [ ] Error pages (404, 500) with Dar branding + Tailwind
- [ ] CSRF tokens on all POST/PUT/DELETE
- [ ] Input validation on all endpoints
- [ ] File upload size limits
- [ ] Security headers (CSP, X-Frame-Options)
- [ ] Daemon manager: graceful shutdown on SIGTERM (kill daemons, close DB)
- [ ] Dark mode toggle (Tailwind dark: classes + localStorage)
- [ ] Request logging (timestamp, method, path, status, duration)
- [ ] verify: responsive on mobile, secure headers set, dark mode works, graceful shutdown
