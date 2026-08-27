# Dar

Dar is a self-hosted, Nextcloud-like platform written in Go. One server process hosts OIDC authentication and every application, including Files, Calendar, Contacts, Codex, Todo, Notes, News, Email, Activity, and Weather.

Codex is the central AI assistant. Each user receives an isolated Codex CLI app-server process, while typed, user-scoped assistant providers expose capabilities from the other Dar applications.

## A note from Anoosa

Favor ambitious products, simple systems, and behavior that feels obvious. Do not preserve complexity merely because it already exists, and do not introduce machinery because it appears architecturally impressive. Understand the real constraint and implement the smallest model that makes the correct behavior unsurprising.

Dar is in alpha. Unless a task explicitly requires compatibility, replace obsolete storage and APIs directly instead of preserving old behavior or adding migrations for it.

Architectural decisions belong to the developer. When a task presents meaningfully different approaches, explain the options and ask before choosing. This includes:

- Introducing a package or abstraction layer.
- Changing an existing API or interface.
- Choosing between synchronization or lifecycle designs.
- Structuring a multi-step workflow.
- Adding a dependency.
- Dividing responsibilities between applications or shared services.

Present the concrete choices, their consequences, and your recommendation. Do not silently select one.

Treat the rest of this document as strong defaults rather than immovable rules. Explicit developer instructions override it.

## A small glossary

Use this terminology when discussing Dar:

- **you** means the agent reading this file and changing Dar.
- **we, us, and maintainers** mean Anoosa and the people building Dar.
- **user** means a person signed in to and using a Dar instance.
- **app** means an in-process Dar feature registered through `apps.App`.
- **assistant** means the Codex experience embedded in Dar.
- **assistant provider** means a typed, user-scoped capability exposed by an app to Codex.
- **instance** means one running Dar server together with its database, data root, configuration, and managed processes.
- **data root** means the filesystem root configured by `DAR_DATA_ROOT`.
- **surface** means one way a feature is exposed: rendered UI, session API, protocol endpoint, search, activity, notifications, dashboard, or assistant tools.
- **lifecycle app** means an app implementing `apps.Lifecycle` because it owns background work.
- **live data** means the database and data root used by a real Dar instance rather than an isolated test environment.

## The three ways to hurt yourself

1. **Making architecture by accident.** Do not add packages, interfaces, dependencies, workers, databases, or new responsibility boundaries without presenting the decision to the developer first.

2. **Breaking the platform boundaries.** Dar is one Go service with one SQLite database and no CGo. Do not introduce another database, an ORM, a frontend framework, or a runtime asset dependency.

3. **Testing against live state.** Tests and development processes must use isolated databases and data roots. Never point a test server at a developer's live database or mutate persistent data merely to verify a change.

## Approved dependencies

Dar has eight approved direct dependencies:

| Dependency | Purpose |
|---|---|
| `github.com/go-chi/chi/v5` | HTTP router |
| `golang.org/x/oauth2` | OIDC and OAuth2 client |
| `zombiezen.com/go/sqlite` | Pure-Go SQLite driver |
| `github.com/emersion/go-imap` | IMAP client |
| `github.com/emersion/go-ical` | iCalendar and RRULE parsing |
| `github.com/emersion/go-vcard` | vCard parsing |
| `github.com/mmcdole/gofeed` | RSS and Atom parsing |
| `github.com/teambition/rrule-go` | Recurrence expansion |

Do not add another direct dependency without explicit approval.

In particular:

- No CGo.
- No ORM.
- No second database such as PostgreSQL or Redis.
- No HTTP framework beyond Chi.
- No template engine beyond `html/template`.
- No React, Vue, HTMX, or comparable frontend framework.
- No dependency on externally hosted runtime assets.

Use `zombiezen.com/go/sqlite`; never introduce `mattn/go-sqlite3`.

## Hit every surface

The most common cross-app defect is implementing a change through one path while leaving related paths stale. Before calling work complete, identify which of these surfaces apply:

- **Rendered UI.** Pages, dialogs, dashboard widgets, navigation, settings, empty states, and error states.
- **Session APIs.** Browser-facing JSON endpoints and their authentication, CSRF, validation, and error behavior.
- **Protocols.** WebDAV, CalDAV, CardDAV, IMAP, SMTP, RSS, and other application-specific integrations.
- **Search.** Search documents, incremental index updates, deletion, reconciliation, and result URLs.
- **Activity and notifications.** User-visible events and notification preferences.
- **Assistant tools.** Typed schemas, user scoping, effect classification, confirmation policy, and stable error categories.
- **Dashboard providers.** Widget definitions, supported sizes, settings, and refresh behavior.
- **Application lifecycle.** Startup, cancellation, shutdown, worker cleanup, and reverse-order teardown.
- **Reverse operations.** If a user can add, enable, archive, snooze, or connect something, verify the corresponding remove, disable, restore, unsnooze, or disconnect path.
- **Documentation and tests.** Update nearby tests and explanatory comments when behavior or contracts change.

A feature is not complete merely because its primary page works.

## Frontend

Dar uses server-rendered HTML with small amounts of vanilla JavaScript for progressive enhancement.

- Render HTML through Go's `html/template` with automatic escaping.
- Keep templates with their owning app or in `internal/webui/templates`.
- Use the shared layout and existing Material 3 design tokens.
- Use Tailwind utility classes from the existing configuration.
- Keep `internal/webui/assets/tailwind.input.css` and the committed, self-hosted `app.v1.css` asset consistent.
- Do not load Tailwind or other application assets from a CDN.
- Do not introduce a JavaScript framework.
- Do not add inline `style` attributes.
- Preserve keyboard behavior, focus handling, loading states, empty states, success states, and error states.
- Keep browser-visible text and controls consistent across applications.

### Browser verification

After a UI or UX change:

1. Launch Dar in an isolated development instance.
2. Exercise the real rendered control through browser automation.
3. Verify its success and error states.
4. Check the browser console.
5. Confirm the relevant server or persistence behavior.

Source inspection and HTTP tests alone are not sufficient for user-facing frontend work.

For Nextcloud comparison work, use the configured reference instance and its designated test account. Do not place passwords or other credentials in this file.

## App contracts

Every application implements `apps.App` and declares its routes together with an authentication mode.

Use the existing contracts:

- `apps.App` for application identity, navigation metadata, and routes.
- `apps.Lifecycle` only when an app owns background work.
- `apps.SearchIndexer` and the scoped search writer for unified search.
- `apps.AssistantProvider` for typed, user-scoped Codex capabilities.
- Dashboard provider interfaces for widgets and their settings.
- Notification and activity interfaces for user-visible events.

Assistant tools must:

- Use JSON object schemas for input.
- Scope all operations to the authenticated user.
- Classify effects as read, safe write, external, or destructive.
- Return stable, non-secret error categories.
- Avoid coupling assistant behavior directly to HTTP handlers or storage internals.

The registry starts optional application workers and stops them in reverse order. Shutdown must drain HTTP work, cancel workers and helper processes, and then close SQLite.

## Storage and authentication

Dar uses one shared SQLite database.

- Use `zombiezen.com/go/sqlite` and `zombiezen.com/go/sqlite/sqlitemigration`.
- Keep WAL mode enabled.
- Use prepared statements for user-controlled values.
- Never add another database.
- Never panic inside an HTTP handler.
- Return explicit validation, authentication, authorization, conflict, and server errors.
- Keep application data scoped to the authenticated user.
- Use safe filesystem operations beneath the configured data root.
- Do not follow or create unsafe links across user or data-root boundaries.

Dar uses one OIDC provider configured through environment variables. Browser sessions use secure, HTTP-only cookies and same-origin protections. Mutating session APIs must preserve CSRF enforcement.

## Development

Dar targets the Go version declared in `go.mod`. Enter the complete development environment with:

```bash
nix develop
```

Build the server and test OIDC helper:

```bash
go build ./cmd/...
```

Run a local development instance with the test OIDC provider:

```bash
./launch.sh
```

Run the server directly when an external OIDC provider is already configured:

```bash
go build -o dar-server ./cmd/server/
./dar-server --port 8080 --db dar.db
```

Important environment variables include:

```bash
DAR_OIDC_ISSUER
DAR_OIDC_CLIENT_ID
DAR_OIDC_CLIENT_SECRET
DAR_OIDC_DISPLAY_NAME
DAR_BASE_URL
DAR_DATA_ROOT
DAR_CODEX_PATH
DAR_CODEX_HOME_ROOT
DAR_CREDENTIAL_ENCRYPTION_KEYS
```

Do not commit real values for secrets or credentials.

Track every process you start and stop it by the PID you captured. Do not kill processes by broad name, path, or worktree matching.

## Test data

Tests must use isolated state.

- Prefer `t.TempDir()` for databases, data roots, Codex homes, browser profiles, and generated files.
- Fixed files under `/tmp` must have test-specific names and explicit cleanup.
- Never run tests against the repository's `dar.db`.
- Never point tests at a real `DAR_DATA_ROOT`.
- Do not reuse a developer's credential encryption keys.
- Do not symlink test state into persistent application state.
- Keep full-stack and browser artifacts under the configured test-artifact directory.

## Verifying

Start with the smallest proof that demonstrates the change.

For a focused Go change:

```bash
go test ./internal/path/to/package -count=1
```

For the normal repository check:

```bash
./test/run.sh fast
```

The available verification modes are:

```bash
./test/run.sh fast
./test/run.sh pr
./test/run.sh nightly
```

- `fast` builds commands, runs `go vet`, and runs command and internal tests.
- `pr` adds race tests, full-stack tests, protocol E2E tests, and coverage checks.
- `nightly` adds fuzzing and repeated lifecycle/concurrency tests.

Backend behavior changes should include focused tests for the behavior being changed. Concurrency and lifecycle behavior should verify cancellation, shutdown, and cleanup without relying on arbitrary sleeps.

UI changes also require one integrated browser pass after the focused tests succeed.

Do not report completion while required verification is failing. Distinguish a failure caused by the change from an unrelated environment or infrastructure failure.

## How it works

`cmd/server` initializes the shared database, OIDC authentication, common services, and the application registry. Applications register authenticated UI and API routes on the same Chi router.

Applications may additionally provide:

- Background lifecycle work.
- Search indexing.
- Dashboard widgets.
- Activity and notifications.
- User-scoped assistant tools.
- Protocol endpoints such as WebDAV, CalDAV, or CardDAV.

Codex runs through an isolated CLI app-server process per user. Dar exposes typed app capabilities to that process rather than allowing it to bypass application boundaries.

The server handles termination signals, drains requests, cancels registered workers and managed commands, and closes SQLite during shutdown.

## Where code lives

- `cmd/server` — process entry point, service composition, application registration, and top-level routes.
- `cmd/test-oidc` — local and test OIDC provider.
- `internal/server` — Chi router, HTTP server, authentication middleware, security headers, and request limits.
- `internal/auth` — OIDC, users, sessions, and app passwords.
- `internal/db` — SQLite connection and migration infrastructure.
- `internal/apps` — shared app contracts, registry, search, assistant, dashboard, and lifecycle interfaces.
- `internal/apps/codex` — Codex process management, storage, tools, and streaming.
- `internal/apps/files` — files, sharing, persistence, and WebDAV-facing behavior.
- `internal/apps/calendar` — calendar storage, recurrence, subscriptions, and CalDAV-facing behavior.
- `internal/apps/contacts` — contacts, groups, preferences, and CardDAV-facing behavior.
- `internal/apps/todo` — tasks and CalDAV-backed todo behavior.
- `internal/apps/email` — mail storage, synchronization, IMAP/SMTP behavior, and credentials.
- `internal/apps/news` — feed storage, fetching, refresh workers, and search.
- `internal/apps/notes` — notes, file-backed content, and search.
- `internal/apps/activity` — cross-app user activity.
- `internal/apps/weather` — dashboard weather provider.
- `internal/search` — unified search storage and reconciliation.
- `internal/notifications` — user notifications and streaming.
- `internal/preferences` — shared user preferences.
- `internal/theme` — appearance and Material 3 token resolution.
- `internal/webui` — shared templates, layout, browser assets, and page rendering.
- `internal/webdav`, `internal/caldav`, and `internal/carddav` — protocol implementations.
- `test/fullstack` — integrated server and browser tests.
- `test/run.sh` — fast, PR, coverage, and nightly verification workflows.

## Taste

- Keep the process model simple: one Dar service and in-process applications.
- Keep storage simple: one SQLite database plus explicitly owned filesystem data.
- Put app-specific behavior in the owning app.
- Put truly shared contracts in `internal/apps` or the relevant shared service.
- Do not create abstractions for hypothetical future applications.
- Prefer concrete types until an interface represents an existing boundary.
- Keep user scope explicit through storage, search, assistant tools, and protocols.
- Keep orchestration understandable and lifecycle ownership obvious.
- Prefer inferred local types and direct Go code over unnecessary wrappers.
- Comments should explain contracts, invariants, ownership, or surprising behavior—not narrate every line.
- Keep comments synchronized with the code they describe.
- Avoid background work when request-time or explicit refresh behavior is sufficient.
- User-visible operations need honest loading, empty, success, and error states.
- Performance-sensitive paths should avoid unnecessary database scans, filesystem walks, process churn, and browser repainting.
- Security matters, but security machinery must solve a concrete threat rather than add ceremony to local-only development behavior.

## Additional tips

- Use patch-based editing for Go files; do not perform broad textual rewrites with `sed`.
- Preserve unrelated work in the existing dirty worktree.
- Do not introduce backward-compatibility machinery during alpha unless explicitly requested.
- Do not commit generated binaries, databases, credentials, browser profiles, or test artifacts.
- Ask before making an architectural choice.
