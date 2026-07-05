# Dar Agent Guide

Dar is a self-hosted Nextcloud-like platform written in Zig. Web server, OAuth login, app system where each app can run its own daemon process (Files, Calendar, Contacts, Soosa). This file documents conventions, project layout, and preferences for AI agents working on this codebase.

## Critical Rule: Ask Before Architectural Decisions

**Before making any architectural decision**, prompt the user for direction. Do not guess. Examples:

- Choosing between two implementations (e.g., HashMap vs array lookup, stateful vs stateless)
- Adding a new abstraction layer or module
- Changing an existing API signature
- Deciding how to structure a multi-step workflow
- Picking between synchronous and asynchronous patterns

Say: "Two approaches: A) X, B) Y. Which one?" — do not silently pick one.

## Dependencies

**Only one external library required at runtime:**
- `sqlite3` — for database (users, sessions, app data)

**Everything else must use the Zig standard library.**
No curl, no libcurl, no OpenSSL, no NPM, no bundler, no third-party C libraries.

**Frontend:**
- Tailwind CSS via CDN (`<script src="https://cdn.tailwindcss.com">`) — no build step
- Vanilla JavaScript for progressive enhancement — no React, no Vue, no HTMX (yet)
- HTML is server-rendered via `std.fmt` formatting strings

## Zig Version

Zig 0.17.0-dev (master branch). Use the Nix flake if available, or system zig:
```bash
# With Soosa's flake (shared toolchain):
nix develop /home/anas/soosa   # provides zig 0.17, sqlite3

# Or directly:
zig build
```

The project starts fresh — no 0.16 migration needed. All code is written for 0.17 APIs from day one.

## C FFI

All C function declarations live in `src/common/c.zig`. To add a new C function:
1. Declare it as `pub extern "c" fn name(...) return_type;` in `c.zig`
2. Import `c.zig` in your file: `const c = @import("../common/c.zig");`
3. Never use `@cImport` — removed in Zig 0.17

The ONLY C functions allowed are sqlite3 (`c.sqlite3_*`). No ncurses (this is a web server, not a TUI).

## Build

```bash
zig build                        # build dar binary
zig build test --summary all     # run all tests
./zig-out/bin/dar                # start server (default :8080)
./zig-out/bin/dar --port 3000    # custom port
./zig-out/bin/dar --db /tmp/dar.db  # custom DB path
```

The build links `sqlite3` only. No other system libraries.

## Testing Workflow

**After every code change**, follow this order:

1. **Build** — `zig build` must succeed with zero errors
2. **Unit tests** — `zig build test --summary all`
3. **Start server** — `./zig-out/bin/dar &` then `curl` to verify endpoints
4. **Kill server** — `kill %1` when done

Manual browser testing is required for visual changes (Tailwind classes, layouts).

## Key Conventions

### No C Functions for I/O

Use Zig-native APIs for ALL I/O. The only allowed C functions are sqlite3 (`c.sqlite3_*`).

**Stderr/stdout writing:**
```zig
fn wStderr(bytes: []const u8) void {
    _ = std.Io.File.stderr().writeStreamingAll(compat.getIo(), bytes) catch {};
}
```

**File reading:**
```zig
const content = try std.Io.Dir.cwd().readFileAlloc(io, path, allocator, std.Io.Limit.limited(64*1024));
defer allocator.free(content);
```

**File writing (create/truncate):**
```zig
var file = try std.Io.Dir.cwd().createFile(io, path, .{});
defer file.close(io);
_ = try file.writeStreamingAll(io, content);
```

**Current time:**
```zig
const now = std.Io.Clock.now(std.Io.Clock.real, compat.getIo());
const secs: i64 = @intCast(@divTrunc(now.nanoseconds, 1_000_000_000));
```

**Environment variables:**
```zig
// Use common.getEnv() which checks StringHashMap overrides then std.c.getenv
const port_str = common.getEnv("DAR_PORT") orelse "8080";
```

### HTTP Server

Zig 0.17 has `std.http.Server` in the standard library. Use it:

```zig
const address = try std.net.Address.parseIp4("0.0.0.0", port);
var listener = try address.listen(.{ .reuse_address = true });
defer listener.deinit();

while (true) {
    const conn = try listener.accept();
    defer conn.stream.close();
    var buf: [8192]u8 = undefined;
    var server = std.http.Server.init(conn, &buf);
    var req = try server.receiveHead();
    // Route and handle request
}
```

### HTML Rendering

All HTML is built with `std.fmt.allocPrint` or `ArrayList` appends. No template engine. Use helper functions for common patterns:

```zig
fn htmlPage(title: []const u8, body: []const u8, user: ?User) []const u8 {
    return std.fmt.allocPrint(alloc,
        \\<!DOCTYPE html>
        \\<html lang="en">
        \\<head>
        \\  <meta charset="UTF-8">
        \\  <meta name="viewport" content="width=device-width, initial-scale=1.0">
        \\  <title>{s} — Dar</title>
        \\  <script src="https://cdn.tailwindcss.com"></script>
        \\</head>
        \\<body class="bg-gray-100 min-h-screen">
        \\  {s}
        \\  {s}
        \\</body>
        \\</html>
    , .{ title, navbarHtml, body });
}
```

### Tailwind CSS

Always use Tailwind utility classes for styling. No custom CSS files. No inline `style=""` attributes (use Tailwind equivalents instead).

The CDN script auto-detects used classes and compiles them at runtime. For production, the CDN script can be replaced with a pre-compiled CSS file.

### SQLite

Use `c.sqlite3_*` FFI calls directly. Wrap common patterns:

```zig
pub fn exec(db: *c.sqlite3, sql: [:0]const u8) !void {
    var err_msg: [*c]u8 = undefined;
    const rc = c.sqlite3_exec(db, sql, null, null, &err_msg);
    if (rc != c.SQLITE_OK) {
        const msg = std.mem.sliceTo(err_msg, 0);
        defer c.sqlite3_free(err_msg);
        return error.SqliteError;
    }
}
```

Always use prepared statements for queries with user input. Use WAL mode for better concurrency.

### Apps

Apps are Zig modules that implement an `App` struct. Each app lives in `src/apps/<name>/`. Register all apps in `main.zig`:

```zig
const files_app = @import("apps/files/files.zig").app;
const calendar_app = @import("apps/calendar/calendar.zig").app;
try registry.register(files_app);
try registry.register(calendar_app);
```

An app's `routes` slice is iterated by the router. An app can optionally have a `daemon` config — if set, the daemon manager spawns a subprocess for it.

### Daemon Manager

The daemon manager (`src/daemon/daemon.zig`) manages app daemon processes:

1. **Spawn** — Calls `spawn(config)` to fork+exec the daemon binary
2. **Health check** — Polls `GET localhost:<port>/health` until 200 or timeout
3. **Monitor** — Detects crashes, logs, restarts with backoff
4. **Shutdown** — On SIGTERM/SIGINT, sends kill signal to all child daemons

Each daemon runs on a localhost port (e.g., 8081 for files, 8082 for calendar). The main server proxies API requests to the daemon. DAV protocol clients (Finder, Thunderbird) connect to the daemon port directly.

### Error Handling

- Router returns 404 for unknown paths
- Auth middleware returns 302 to `/login` for unauthenticated requests
- Database errors return 500 with a generic error page
- Validation errors return 400 with a JSON error message

Never panic. Always return an error response. The server must stay up.

### Memory / Allocation

- Use `std.heap.page_allocator` for long-lived allocations (DB, router, plugin registry)
- Use an arena allocator per request for response building
- `ArrayList(T).empty` then `.append(allocator, item)` — no `.init(allocator)`
- `std.json.fmt()` uses `{f}` format specifier, not `{s}`

### Format Strings

- `{s}` for strings, `{d}` for sized integers
- `{f}` for `std.json.fmt()` results
- Avoid `{s:<20}` alignment (may not work) — pad manually
- Never use `\"` inside `allocPrint` format strings

### String Literals

- `*const [N:0]u8` works with `{s}` format
- For `[]const u8`, use `@as([]const u8, literal)` or `literal[0..]`

## Project Layout

```
dar/
├── build.zig                    # Build: links sqlite3, zig build-exe src/main.zig
├── build.zig.zon                # Package manifest (zero deps, minimum_zig_version 0.17.0)
├── AGENTS.md                    # This file
├── TODO.md                      # Task list and implementation phases
├── src/
│   ├── main.zig                 # Entry: parse CLI, init DB, register apps + spawn daemons, start server
│   ├── lib.zig                  # Module registry (all pub imports for test coverage)
│   ├── common/
│   │   ├── common.zig           # Shared types: User, Session, App, DaemonConfig, Route
│   │   ├── compat.zig           # Zig 0.17 compat: global Io instance
│   │   └── c.zig                # FFI: sqlite3 declarations only
│   ├── db/
│   │   └── db.zig               # SQLite wrapper: open, exec, prepared statements, migrations
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
│   │   └── daemon.zig           # Daemon manager: spawn, health-check, restart, kill
│   ├── apps/
│   │   ├── app.zig              # App interface: App struct definition
│   │   ├── registry.zig         # App registry: collect, init DB, spawn daemons, build app bar
│   │   ├── files/
│   │   │   └── files.zig        # Files app: web UI + daemon config
│   │   ├── calendar/
│   │   │   └── calendar.zig     # Calendar app: web UI + daemon config
│   │   ├── contacts/
│   │   │   └── contacts.zig     # Contacts app: web UI + daemon config
│   │   └── soosa/
│   │       └── soosa.zig        # Soosa app: web UI (subprocess, no daemon)
│   └── webui/
│       ├── layout.zig           # HTML page shell: <head>, top app bar, main content
│       ├── home.zig             # Home screen: app launcher grid
│       ├── login.zig            # Login page: OAuth provider buttons
│       ├── components.zig       # Reusable HTML components: buttons, cards, modals
│       └── static.zig           # Static file serving: favicon, robots.txt
└── .hermes/
    └── plans/                   # Implementation plans
```

## Architecture

1. **`main.zig`** parses CLI flags (`--port`, `--db`), initializes SQLite, registers apps, spawns daemons for apps that have them, starts the HTTP server accept loop
2. **Router** matches incoming requests (method + path) to app handler functions
3. **Middleware chain** runs before each handler: parse session cookie → load user → attach to request context
4. **Handlers** receive parsed request + user, return HTML or JSON responses
5. **Apps** own their routes, database tables, and web UI. Each app can optionally spawn a daemon subprocess on a localhost port
6. **Daemon manager** monitors spawned daemons: health-checks, restart on crash, graceful shutdown
7. **Web UI** is server-rendered HTML with Tailwind classes. JavaScript is minimal — only for client-side interactivity (file upload preview, calendar navigation, chat streaming)

## OAuth2 Providers

Configure via environment variables:

```bash
DAR_GOOGLE_CLIENT_ID=...
DAR_GOOGLE_CLIENT_SECRET=...
DAR_GOOGLE_REDIRECT_URI=http://localhost:8080/auth/google/callback

DAR_GITHUB_CLIENT_ID=...
DAR_GITHUB_CLIENT_SECRET=...
DAR_GITHUB_REDIRECT_URI=http://localhost:8080/auth/github/callback
```

The login page shows buttons for each configured provider. Providers without credentials are hidden.

## Session Cookies

- Name: `dar_session`
- HttpOnly: true
- Secure: false (development) / true (production with TLS)
- SameSite: Lax
- Path: /
- Max-Age: 30 days

Session tokens are random 32-byte hex strings stored in the `sessions` SQLite table.

## Test Conventions

- Tests go inline in the module file (same `.zig` file, `test "..." { }` blocks)
- Each module imported in `lib.zig` gets test coverage via `_ = @import("...")`
- Test databases go in `/tmp/dar_test_*.sqlite` with `defer` cleanup
- HTTP handlers are tested by building a request and checking the response body/status
- Integration tests: start the server, `curl` endpoints, check responses

## What to Avoid

- Do NOT add a second database (no PostgreSQL, no Redis)
- Do NOT add a build step for CSS/JS (no Webpack, no Vite, no PostCSS)
- Do NOT add a template engine (use `std.fmt` formatting strings)
- Do NOT store passwords (OAuth2 only)
- Do NOT add NPM dependencies
- Do NOT add C dependencies beyond sqlite3
- Do NOT use `@cImport` — use `src/common/c.zig`
- Do NOT use `sed` on Zig files — use `patch` or `write_file`
- Do NOT commit before testing (`zig build test` must pass)
