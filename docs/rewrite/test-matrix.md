# Rewrite test matrix

The baseline contains 109 Go test files and passes `./test/run.sh fast` at tag
`dar-go-baseline-2026-08-27`. Port behaviors, not Go implementation details.

## Verification tiers

- `fast`: compile server/browser test builds, static checks, Node-side unit and
  service tests, browser component tests, and focused contract tests.
- `pr`: `fast` plus concurrency/lifecycle stress, full-stack Playwright, raw DAV
  suites, protocol integrations, source-mapped coverage, and plugin scenarios.
- `nightly`: `pr` plus fuzz/property runs, repeated lifecycle races, real hermetic
  `mbsync`, crash recovery, and representative external DAV clients.

All state lives under test-owned temporary roots. Tests never use live databases,
data roots, Codex homes, plugin directories, credentials, or browser profiles.

## Required scenario families

- Core: OIDC state/nonce/PKCE and callback errors, session lifecycle, app-password
  scopes, admin claims, profile/settings, CSRF/origin, security headers, body
  limits, trusted proxies, gzip, rate limits, health, and graceful shutdown.
- SPA: authenticated shell, accessibility, keyboard/focus, dashboard, search,
  notifications, settings, editor interactions, responsive routes, and absence of
  browser-native prompt/confirm dialogs.
- Database/storage: migrations, foreign keys, WAL, serialized writes, atomic
  files, path normalization, traversal, reserved paths, symlink rejection,
  quotas, ETags, stale writes, journals, crash recovery, and reconciliation.
- Search: user scoping, provider generations, staging/reconciliation, incremental
  update/delete, cursor bounds, preferences, and stable result URLs.
- Activity/notifications: recipients, outbox retry, cursor ordering, RSS tokens,
  deduplication, preferences, dismissal, SSE replay/heartbeat, and disconnect.
- Calendar: CRUD, ICS, CalDAV, recurrence, exceptions, alarms, subscriptions,
  birthdays, import/export, widgets, search, activity, assistant tools, and parser
  properties/fuzzing.
- Contacts: CRUD, rich VCF fields, groups, CardDAV, books, import/export,
  preferences, Me-card, recipients, search, activity, assistant tools, and parser
  properties/fuzzing.
- Todo: list/task CRUD, sharing, trash/restore/purge, recurrence occurrences,
  VTODO DAV, preferences, widgets, search, activity, and assistant tools.
- Notes: Markdown IDs, external rename/edit reconciliation, conflicts,
  categories, versions, undo, attachments, preferences, search, widget, activity,
  and assistant tools.
- Files: confinement, symlink rejection, atomic writes, quotas, leases/journal,
  metadata, versions, trash, tags, favorites, public/user/drop shares, passwords,
  rate limits, comments, locks, diff, preview, thumbnail, WebDAV, mail virtual
  mount, search, widget, activity, assistant tools, and path fuzzing.
- News: discovery, fetch/SSRF, folders/feeds/items, OPML, archive, sanitization,
  enclosures/video, refresh lifecycle, search, activity, assistant tools, and
  feed/parser fuzzing.
- Email: discovery, encryption/rotation, Maildir and mbsync generation, indexing,
  threading, MIME, SMTP, drafts, attachments, operations, sync/status,
  notifications, search, widget, activity, assistant tools, parser fuzzing, and a
  hermetic real-mbsync integration.
- Codex: isolation, version/capacity/LRU/idle, crash/restart, environment
  redaction, protocol multiplex/RPC, provider tools, approvals, interruption,
  replayable SSE, sessions, attachments, search, activity, and shutdown.
- Prayer: calculation methods, Asr, high-latitude, DST, polar dates, Todo sync,
  analytics, lifecycle, and widget.
- Weather: geocoding, cache/stale fallback, units, settings, outbound failures,
  and widget.
- Plugins: valid install, invalid manifest, incompatible API, checksum mismatch,
  unsafe Git tree, size limits, migration failure, activation failure, client
  failure, live disable/reload, rollback, uninstall, purge, namespaces, and
  disposer completeness for routes/timers/SSE/assistant/widget contributions.

## Release gates

- At least 90% source-mapped statement coverage overall and 80% per substantive
  subsystem, excluding generated output and declarative schemas.
- No unresolved parity-ledger entries beyond the approved exceptions.
- Full browser verification includes loading, empty, success, validation,
  conflict, unavailable/stale, and destructive-confirmation states with a clean
  console and confirmed persistence.
- Shutdown leaves no listener, timer, SQLite handle, worker, SSE client, Codex
  process, `mbsync` process, Git process, or temporary staging directory behind.

