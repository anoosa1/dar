# Storage and lifecycle inventory

This document records responsibilities from `dar-go-baseline-2026-08-27` and
the rewrite ownership chosen for them.

## Fresh data-root contract

```text
data/
  instance.json
  dar.sqlite
  users/<opaque-user-id>/
    files/
    trash/
    versions/
    contacts/<book-id>/
    calendars/<calendar-id>/
    tasks/<list-id>/
    mail/<account-id>/
    codex/home/
    codex/workspaces/<session-id>/attachments/
  plugins/<plugin-id>/<commit>/
```

`instance.json` identifies the ClojureScript generation and schema version.
Startup refuses an unrecognized or Go-era non-empty root without modifying it.

## Portable authoritative data

- Files: ordinary bytes and directories beneath each user's `files/` root.
- Versions and trash: immutable/versioned bytes under their dedicated roots.
- Contacts: one VCF resource per contact plus book metadata.
- Calendar: one ICS resource per event plus calendar metadata.
- Todo: one VTODO ICS resource per task plus list metadata.
- Notes: Markdown beneath the configured Files notes directory; attachments stay
  in a reserved child directory owned by Notes.
- Email: Maildir beneath each account directory plus generated `mbsyncrc`.
- Codex: isolated CLI home, workspaces, and session attachment files.

SQLite indexes for portable data are derived and repairable. Cross-filesystem/
database mutations use a durable operation journal and per-user serialization.

## Baseline database responsibilities to preserve

- Identity: users, sessions, app passwords, and user preferences.
- Activity: events, recipients, outbox, and feed settings.
- Notifications: user-visible records, deduplication, status, and preferences.
- Search: documents, staging, provider runs, and provider generations.
- Files: favorites, tags, file-tag links, trash, versions, public shares, user
  shares, comments, locks, activity metadata, external storage, and operation
  leases/journal.
- Calendar: calendar metadata and derived event index.
- Todo: lists, members, derived tasks/occurrences, preferences, and generated
  Prayer list/task mappings.
- Prayer: generated occurrence state.
- Notes: derived index and undo state.
- News: folders, feeds, items, state, refresh metadata, and errors.
- Email: accounts, folders, messages, message locations, recipients, attachments,
  sync runs, and queued operations.
- Codex: session metadata and preferences.
- Plugins: source URL, exact commit, immutable versions, active/previous version,
  enablement, config, migration ledger, activation errors, and preserved data.

The rewrite uses new table definitions, opaque string identifiers, ISO timestamp
serialization at API boundaries, foreign keys, WAL, and numbered migrations. No
old table is opened or migrated.

## Runtime lifecycle owners

- Root context: configuration, logging, server, database, auth, credentials,
  filesystem, outbound HTTP, preferences/theme, search, activity, notifications,
  dashboard, assistant registry, recipients, processes, plugins, and apps.
- Activity context: durable outbox retry timer.
- Calendar context: bounded remote-subscription refresh timer.
- News context: bounded feed-refresh timer and in-flight fetch cancellation.
- Email context: polling timer, serialized account sync, `mbsync`, index and
  operation reconciliation.
- Prayer context: occurrence/Todo reconciliation timer.
- Codex context: per-user processes, reaper timer, RPC reads, stderr drains,
  approvals, tool calls, SSE streams, and graceful termination.
- Plugin manager: staged Git subprocesses and one child Cordis context per active
  plugin version.

Every resource is registered as a Cordis effect. Startup failure disposes already
started resources in reverse order. Shutdown first stops accepting HTTP, drains
requests, closes SSE streams, cancels app/plugin work and subprocesses, waits for
bounded cleanup, then closes SQLite.

## Safety invariants

- No test or development command defaults to a live data root.
- Display names never form physical user paths.
- User paths cannot be absolute, escape through traversal, reach reserved roots,
  cross user boundaries, or traverse a symbolic link.
- Symlink creation and serving are rejected even when a symlink was introduced
  outside Dar.
- Writes use a sibling temporary file, flush, close, rename, and directory sync.
- A SQLite transaction never spans network, filesystem streaming, or subprocess
  work.
- External fetches revalidate every redirect and resolved address.
- Child processes use explicit argv/env/cwd, bounded output, captured handles,
  cancellation, and exact-PID cleanup.

