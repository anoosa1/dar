# Porting plan: the Rust stack passes Dar's whole test suite

Goal: every test in `next/test` runs against both stacks and passes on both. TypeScript is the
reference; a test that fails on TypeScript is a test bug, fixed in `next/test`. Application code
in `next/` is never changed; only `next/test` (tests and helpers) gains files.

## How the suite runs on each stack

- TypeScript: `npm --prefix next run verify` (unchanged entry point).
- Rust: `rust/verify.mjs` starts the Rust stack the way `next/scripts/verify.mjs` starts the
  TypeScript one (same environment variables) and runs the same test files with
  `DAR_TEST_STACK=rust`.
- `next/test/instance.ts` starts an isolated instance (its own database and root) on either
  stack. Tests that used to call `startServer` in-process use it, so they test whichever
  runtime is selected.
- Unit tests of TypeScript internals (pure functions) stay TypeScript-only; each gets a Rust
  mirror with the same cases in the crate that ports it.

## Test inventory

Kinds: S = HTTP/database against the shared fixture; I = isolated instance (`instance.ts`);
U = unit test of TypeScript code; B = browser test (needs the Rust client to match React).

| Area | Tests | Rust status |
|---|---|---|
| Auth, sessions, HTTP security | auth, auth-security, http-security, auth-primitives(U), keyring(U) | passes |
| Files, storage, DAV | files, dav, pagination, upload-timeout, upload-pace(U), blobs(U), storage(U), gc, backup(I), blob-migration(I), browser(B) | passes |
| History, Git | history, git, git-app, git-interop, git-tracker, history-browser(B), git-browser(B), git-tracker-browser(B) | passes |
| PIM | pim, pim-standards(U), pim-upgrade, calendar-subscriptions(I), calendar-navigation(U), davx5, pim-dav-client, pim-browser(B), contacts-layout-browser(B) | passes |
| Workspace (Notes, Tasks) | workspace, workspace-standards(U), workspace-upgrade, workspace-browser(B) | passes |
| Search, settings, appearance | global-search, ui-settings, appearance-scopes, appearance-scopes-browser(B), refinement-browser(B), themed-controls-browser(B), shared-ui-browser(B), twilight-browser(B) | passes |
| Installed apps | apps, app-packages(U), app-launcher-browser(B), prayer-times-browser(B) | passes |
| Identity, OIDC, SCIM | identity, identity-lifecycle, identity-upgrade, identity-proxy(I), oidc-config, scim-model(U), identity-browser(B) | passes |
| News | news(I), news(U), news-browser(B) | passes |
| Books | books, books-browser(B) | passes |
| Photos, Immich | photos(I), photos-library(I), photos-sharing(I), photos-immich(I), photos-browser(B) | passes |
| Chat | chat(I), chat-discord(I), chat(U), chat-browser(B) | passes |
| Agent | agent, agent-connection(U), codex(I), claude(I), agent-browser(B), agent-codex-browser(B) | passes |
| Operations | restart, production-proxy(I) | passes |

## Progress log

Newest first. Each entry: what passes now on the Rust stack, what was added to the tests.

### 2026-11-21 — client complete; the whole suite passes on the Rust stack (39/39)
- `node rust/verify.mjs` (release client): every suite passes, including all browser tests.
  Unit tests of TypeScript internals still run only on TypeScript; their Rust mirrors are in
  the crates. No test or `next/` file changed in this step.
- Client: every app is ported and routed in `app.rs` and `navigation.rs` (Notes, Tasks, News,
  Chat, Photos and shared links, Books, Git, Agent, installed apps, Settings/Identity and
  identity interactions); optional apps follow the administrator's switches. Folder history is
  the Files folder details' History tab; Files' PDF preview uses the Books viewer.
- Leptos pitfalls fixed across the client, worth keeping in mind for new code:
  - In `view!`, an unbraced `>`/`>=` inside an attribute closure ends the tag; brace it
    (`disabled={move || a >= b}`).
  - Callbacks may run after their owner is disposed (async work, dialogs' `close` events):
    `on_*`/`done` callbacks use `try_run`; deferred code uses `try_get`/`try_with`.
  - Derived signals re-run their dependents on every source change: memoize anything that
    decides whether a pane, sheet or dialog is mounted, or it is rebuilt (losing focus and state).
  - Unsubscribing removes rows from the client cache: `me` deletions from
    `UnsubscribeApplied` are not sign-outs.
  - Detach DOM and socket handlers before dropping their `Closure`s.
  - Structs deserialized from the API default fields a response may omit (shared history has
    no `ownerId`).
- Runtime: `/oidc-form.js` for form_post; the OIDC session cookie at `Path=/` (as
  oidc-provider); `max_age=0` always re-authenticates; whole floats (`5.0`) accepted where
  preferences need integers; `?from=`/`?to=` read as `new Date` reads them (bare dates too).
- Debugging aid: browser failures were traced with a Node `--import` preload that logs page
  console errors, panics and locator steps; Leptos panics name the defining source line.

### 2026-11-19 — module complete; runtime: apps, account routes, history, Git, Notes, Tasks
- Module: every TypeScript table, reducer, procedure and view exists in Rust with the same
  names and parameters (`tools/schema-diff.mjs`: 89/89 tables, 98/98 functions). The only
  differences are three view row-type names (`FileEntry` vs `FileEntries`, etc.), which are
  structurally identical; renaming them would also rename the client's generated types.
  New: git.rs, tracker.rs, backup.rs (maintenance, gc, checksums, blob migration,
  backup/export/restore, sessions, profile, identity, provider, connected applications),
  and PIM reminders, subscriptions, snapshot/restore and Notes rules in pim.rs. Module unit
  tests pass (7).
- Rust procedures can only fail by trapping (SpacetimeDB 2.14.1: "Procedures always either
  trap or return 0"); each failure discards the Wasm instance and logs a backtrace. It is the
  only way to fail a procedure with an error the TypeScript tests and runtime can see.
- Runtime: `installed.rs` (installed apps, user data, admin switches, app assets; app server
  hooks are Node modules and report a startup failure), the account routes of server.ts
  (users, password, sessions, profile, appearance, app passwords with every URL), `history.rs`,
  `git.rs` (repository cache, smart HTTP clone/fetch/push with a quarantine, receive-pack
  checks, bundles; objects written with `git hash-object`, gzip bodies inflated with `gzip`,
  so no SHA-1 or zlib crate), `git_api.rs` (the Git app's API and tracker routes),
  `pim/tasks.rs` (VTODO read/write, occurrences, due days), `pim/notes.rs`,
  `pim/workspace.rs` (Notes and Tasks HTTP), `notes_library.rs`. 20 runtime unit tests pass.
  The runtime uses `service_pim` preferences and `service_provider` as TypeScript does;
  `service_preferences` and `service_oidc_directory` are gone from the module.
- Harness: `verify.mjs` services log to files (`database.log`, `runtime.log` in the run's
  root). Piped output stalled SpacetimeDB once a suite, run with spawnSync, let the pipe fill
  with procedure backtraces.

Remaining runtime areas: identity HTTP and SCIM (needs a database-backed OIDC store, and
clients from `oidc.json` and the database), News (+ Nextcloud API), Books, Photos and Immich,
Chat (+ live socket), Agent (Codex, Claude), maintenance workers (storage reconcile and sweep,
blob migration, checksums, text index, reminders, calendar subscriptions, provider cleanup,
Git maintenance), backup CLI. Client: every app beyond Files, Contacts and Calendar.

### 2026-11-18 — harness and schema parity groundwork
- `next/test/instance.ts`: isolated instances on either stack. The 14 tests that called
  `startServer` in-process, and the 6 that published `module/dist/bundle.js` by hand, now use it
  (`publishModule`, `moduleArguments`, `startInstance`, `restart()`). TypeScript typechecks.
- `rust/verify.mjs`: the suite on the Rust stack, every suite run, summary at the end.
- `rust/tools/schema-diff.mjs`: tables, defaults, reducers, procedures and views compared.
  All 89 tables now exist with TypeScript's columns, indexes and defaults
  (`module/patch-macro.sh` lets a column be named `type`).
- Module procedures fail like TypeScript ones (an HTTP error naming the code): each code
  traps from a function named after it (`lib.rs` `abort`), since a trap's message is not
  reported but its backtrace is. The runtime's `db.rs` reads them as `database.ts` does.
- Runtime: `root.rs` (root, lock, durable files), `apps.rs` (registry, offline `apps-*`
  commands), `main.rs` with `main.ts`'s configuration (DAR_HOST, DAR_APPS_FILE, SIGTERM).

Next: module functions (48 missing, 33 parameter names to align), file by file from
`next/module/src`; then runtime areas; then client apps.
