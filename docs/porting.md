# Dar in Rust: how the port was done, and what to know

Dar was ported from TypeScript (Node, React, a TypeScript SpacetimeDB module) to Rust. The
TypeScript stack was the reference until the Rust one passed the whole acceptance suite, then
was removed; its last state is commit `799b524` (`next/`). This file keeps the port's history,
the pitfalls worth knowing and how an existing TypeScript instance moves over.

## How the suite runs

- `cargo xtask verify` builds the stack (module, bindings, runtime, client, the app SDK
  bundle), then runs the crates' unit tests, the bridge's Go tests and the acceptance suite in
  `tests/` (Rust, see `docs/testing.md`) in parallel on throwaway roots and ports.
- The Node and Playwright suite (`test/`) was ported suite for suite to Rust (last in commit
  `e2f9e03`); see the progress log.

## Test inventory (at the time of the port)

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

## Moving an existing instance to the Rust stack

The Rust stack takes over a TypeScript instance in place: the same root (`instance.json`,
`secrets/`, `blobs/`, uploads, app data) and the same SpacetimeDB database. Stop the TypeScript
runtime, then run the Rust one on that root: `cargo xtask dev` (or the NixOS service) publishes the
Rust module over the database (an in-place upgrade; the schemas are identical) and starts the
runtime. Nothing is exported or converted beforehand.

- Sessions, app passwords (DAV, CalDAV, CardDAV), uploads in progress, reminders and their
  watermarks, installed apps and their data carry over unchanged.
- OIDC: signing keys and clients are shared state. Codes, access and refresh tokens, grants and
  provider sessions that oidc-provider stored are read by `runtime/src/oidc_store.rs`
  (`from_oidc_provider`), and its keygrip-signed `dar_oidc` cookie is accepted and re-signed
  (`oidc/src/legacy.rs`), so relying parties stay signed in without a new prompt.
- Proven while both stacks were in the tree: `restart.mjs` (commit `d6dd814`) started an
  instance on TypeScript, filled it, crashed it mid-upload and took it over with Rust, checking
  all of the above plus Git, folder history, Books and Photos. With the TypeScript stack gone,
  `test/restart.mjs` checks the same through Rust-to-Rust restarts.
- On NixOS, rebuilding with the new `dar` package (`nix/`) switches the service in place.

## Progress log

Newest first. Each entry: what passes now on the Rust stack, what was added to the tests.

### 2026-11-23 — Notes live preview on CodeMirror 6
- The Notes editor's textarea is CodeMirror 6 (`crates/client/editor/notes-editor.js`, the
  maintainer-authorized `@codemirror/*`, `@lezer/markdown` and `style-mod`), bundled by
  `xtask/src/client.rs` with esbuild into `notes-editor.<hash>.js` (its URL in
  `data-notes-editor` on `<html>`; the Nix package builds it in its npm step,
  `DAR_NOTES_EDITOR`) and imported from a module snippet the first time a note opens.
  Files' and Mail's editors keep their textarea.
- Bridge (`crates/client/src/workspace/note_editor.rs`): `wasm_bindgen` methods on the object
  `create` returns, `Closure<dyn Fn>` callbacks (`Fn`, so a callback that dispatches into the
  editor, which calls another callback, never re-enters an `FnMut`). The editor is destroyed
  before its closures drop. Text crosses whole on each change; a `synced` copy passes each
  change on once in each direction.
- Dar's CSP forbids inline `<style>`, which style-mod writes; the bundle mounts CodeMirror's
  rules in a constructed stylesheet (`adoptedStyleSheets`) instead.
- Tests read and select through `el.darEditor` on the content element (`ui::note_text`,
  `ui::note_select`); `fill`, `press` and `press_sequentially` work on it unchanged.

### 2026-11-23 — PDF viewer on hayro Web Worker (replacing pdf.js)
- Browser viewer: `crates/pdf-worker` (`dar-pdf-worker`) runs `dar-pdf` (hayro) in a shared
  dedicated ES module Web Worker, fetching PDFs same-origin and returning page sizes, outlines
  and transferred RGBA bitmaps from a LIFO cancellable render queue. `crates/client/src/books/pdf.rs`
  drives the worker with the same `PdfView`/`PdfHandle` interface, scaling existing page canvases
  during zoom until crisp DPR bitmaps arrive.
- Build & serving: `xtask/src/client.rs` builds `dar-pdf-worker` alongside `dar-client` into
  content-hashed `pdf-worker-<hash>/` (with `wasm-opt` and `.gz`/`.br` in release) and passes its
  URL via `data-pdf-worker` on `<html>`; `crates/runtime/src/http.rs` serves `pdf-worker-<hash>/`
  as immutable, drops `/pdfjs/`, and adds `worker-src 'self'` to the CSP. `pdfjs-dist` and
  `DAR_PDFJS_DIR` are removed from `package.json`, `xtask` and `nix/package.nix`.

### 2026-11-22 — one workspace, the acceptance suite in Rust
- Layout: `crates/` (module, runtime, client, oidc, dav, bindings) in one Cargo workspace with one
  lockfile; `xtask/` replaces `verify.mjs`, `build.sh`, `generate.sh`, the patch scripts and
  `dev.sh`; `rust/nix` folded into `nix/` and the root flake.
- Builds: `fast` and `wasm-fast` profiles (no LTO, incremental) for verify and dev; release
  stays as shipped. A client change rebuilds in about 20 s instead of 5 min 20 s; unchanged
  steps are skipped.
- Tests: all 66 suites of `test/` are Rust (`tests/`), with an in-house browser driver (CDP over
  `--remote-debugging-pipe`, Playwright's locators and waiting rules on Chromium's computed
  accessibility). Each suite has its own instance, so suites run in parallel. Fixture
  programs (fake chat bridge, Codex and Claude stand-ins) are the test binary itself.
- Found on the way: a contended `dar-oidc` provider lock could stall a Tokio worker while the
  holder waited on SpacetimeDB in `block_in_place` (fixed in `Provider::lock`), and a calendar
  subscription refresh requested during a pass was dropped (fixed in `maintenance.rs`).
- Open: under parallel load a few browser suites still fail now and then and pass alone
  (`browser`: Files' New → Folder stays disabled; `history-browser`: an earlier snapshot's
  status message replaces "Folder restored…"; `git-tracker-browser`: Reopen does nothing;
  `codex`, `claude`: a run misses its step in time). Each failure saves screenshots.

### 2026-11-22 — Mail
- New optional app at `/mail` (direction A with B's inline reply, `docs/spacetimedb/mail-mocks.html`;
  user guide `docs/mail.md`). Module: `mail.rs` and four new tables (`mail_providers`,
  `mail_accounts`, `mail_mailboxes`, `mail_messages`), `service_mail` and `service_mail_worker`;
  nothing existing was renamed. Runtime: `mail/` with IMAP, SMTP and JMAP clients written in Dar
  on tokio-rustls (ring provider) with the platform verifier, mail-parser and mail-builder (the
  crates the maintainer authorized), OAuth2 with PKCE, and the pass that keeps each account's
  `.eml` mirror in Files and its server in step in both directions. Client: `mail/` as its own
  lazy chunk. Nix: `services.dar.mail.providers` (presets, systemd credentials).
- Tests: `test/mail.integration.test.ts` and `test/mail-browser.mjs` against
  `test/support/mail-server.mjs` (fake IMAP, SMTP, JMAP and OAuth over TLS, with a CA made
  in JavaScript and trusted through `SSL_CERT_FILE`); `instance.ts` passes extra environment.

### 2026-11-22 — the client loads per app
- Apps other than Files are `#[lazy]` functions in `client/src/lazy_apps.rs` (Leptos'
  wasm-split). `build.sh` links with relocations, splits the wasm with `wasm-split-cli`
  (`rust/nix/wasm-split.nix`, in both dev shells and the package) before `wasm-bindgen`, and
  puts the glue, main wasm, chunks and snippets in `dist/app-<hash>/`, served as immutable.
  The main wasm fell from 6.3 MB to 2.0 MB (606 KB brotli); each app is 39–249 KB brotli.
- An app's first open shows "Opening app…" until its chunk arrives; 1.5 s after the first
  page is up the rest are fetched in the background. Loaded apps render at once (the lazy
  future is polled once synchronously). A chunk returns the app's constructor, called in
  `LoadApp`'s render: building the app as the chunk arrived would subscribe its effects
  before its DOM, so effects ran first (Agent's phone panels lost focus on the hidden panel).
  wasm-bindgen needs `--keep-lld-exports` (the chunks use `__indirect_function_table`), and
  the split also writes shared `chunk_N.wasm` files.
- A new app joins the split by adding a line to the `lazy_apps!` list and routing it through
  `LoadApp`.

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
