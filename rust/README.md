# Rust prototypes

A Rust version of Dar's stack, built beside it. **Nothing here is wired into Dar yet**; the
TypeScript application in `next/` is unchanged. For Files, Contacts and Calendar, the whole
Rust stack runs end to end: the Rust client, the Rust runtime (sign-in, uploads, `/dav`, `/oidc`) and the Rust module
on SpacetimeDB 2.14.1. Toolchain: `nix develop path:./rust/nix` (the same nixpkgs pin as the
root flake: Rust 1.97 with the wasm32 standard library, wasm-bindgen, binaryen, OpenSSL for
native tests). The flake has its own folder so entering the shell copies only that folder to
the Nix store, not the build outputs.

| Crate | Ports | Verify |
|---|---|---|
| `oidc/` (`dar-oidc`) | `next/runtime/identity/provider.ts` on oxide-auth 0.6 | `cargo test -p dar-oidc` |
| `dav/` (`dar-dav`) | `next/runtime/files/dav.ts`, `next/runtime/pim/dav.ts`, `dav-xml.ts` | `cargo test -p dar-dav` |
| `module/` (`dar-module`) | the identity, Files and PIM parts of `next/module/src` | `cd module && cargo test --lib` |
| `runtime/` (`dar-runtime`) | `next/runtime` for Files, Contacts and Calendar: auth, issuer, blobs, API, `/dav`, `/oidc` | `cargo test -p dar-runtime`; `runtime/dev.sh` |
| `client/` (`dar-client`) | `next/client`: shell, sign-in, Files, Contacts and Calendar, in Leptos | `cd client && cargo test --lib` |
| `client-serve/` | serves the client in front of a TypeScript Dar | used by `client/test/run.sh` |

**End to end:** `rust/client/test/run.sh` (from Dar's root dev shell) starts a TypeScript
instance and a Rust instance on throwaway roots and checks:
- 43 screenshots of the same states (Files: 19; Contacts: 11; Calendar: 13 — light, dark,
  phone, menus, panes, editors, dialogs, the month, week, day and agenda views), React on the
  TypeScript stack against the Rust client on the Rust stack, diffed pixel by pixel, with the
  content made through each client's own forms and imports;
- 14 Files operations through the Rust client, on the Rust stack and in front of the
  TypeScript stack, and 12 Contacts and Calendar operations on the Rust stack (edits, a
  moved and a cancelled occurrence, exports, live updates from another tab, deletes);
- WebDAV, CardDAV and CalDAV with app passwords, and an OIDC sign-in by `openid-client` as an
  independent relying party, through the Rust runtime.

## Running the Rust stack

`rust/runtime/dev.sh` (from Dar's root dev shell) builds the module, client and runtime,
starts SpacetimeDB, publishes the module and runs the runtime at http://127.0.0.1:4331, with
its own root (`$XDG_DATA_HOME/dar-rust`) and setup key, as `scripts/dev.mjs` does for the
TypeScript stack. `DAR_OIDC_CLIENTS_FILE` registers OIDC clients (a JSON list of
`ClientInput`) until the module stores them.

## Runtime (`runtime/`)

Same on-disk and wire formats as the Node runtime: scrypt password verifiers (a verifier made
by `password.ts` checks here), the AES-GCM sealed ES256 signing key in `secrets/`, CSRF tokens
bound to the session secret, the browser tokens SpacetimeDB validates through
`/auth/issuer`, content stored under its SHA-256 in `blobs/`, and the cookie, Origin and CSRF
checks. Routes: sign-in and setup, session, browser tokens, logout, uploads (streamed and
hashed), downloads with ranges, thumbnails (`vipsthumbnail`, as Photos makes them), the folder
picker, the Locked folder, search, preferences (merged and validated like `pim/http.ts`), app
passwords, public links, `/dav` (`dar-dav` on `service_dav`) and `/oidc` (`dar-oidc` with the
sign-in interaction API), and Contacts and Calendar (`pim/`: books and calendars, items with
revision preconditions, people search, sharing grants, vCard and ICS import and export,
occurrences, the reminders inbox, subscriptions). vCard and iCalendar are read and written
here (`pim/vobject.rs`, `contacts.rs`, `calendar.rs`) in the formats `ical.js` produces;
recurrence uses `rrule` and `chrono-tz` in wall-clock time, so series keep their local time
across daylight saving changes. Rust procedures report errors in-band (`{"error":{"code":…}}`) and
`db.rs` treats that shape as an error.
- Not yet: the other apps' APIs, rate limits beyond sign-in, unlock and identity, the TLS
  proxy peer header, persistence of OIDC provider state (codes and tokens are in memory),
  fetching subscribed calendars (they can be added, not refreshed), planning reminders (the
  inbox is served; nothing fills it yet), full VTODO projections (Tasks), custom embedded
  VTIMEZONE definitions (read as UTC), maintenance and backups.

## Module (`module/`)

Tables and functions with the TypeScript names and argument order, so the client works with
either module: identity, sessions and app passwords, Files entries, uploads, blobs,
`service_dav`, and the browser surface (`my_files` and `my_usage` views, `browse`, favorites,
sharing, public links, version restore, `file_details`), plus the runtime's picker, Locked
folder, search (Files names and app names), preferences, appearance and OIDC directory
procedures, and PIM (`pim.rs`: collections, resources, grants, change feed, reminders inbox
and subscriptions behind `service_pim`, and the `my_pim_changes` view). Not yet ported: the
reminder planner and subscription sync procedures, Notes trash, Photos/Immich, Notes/Tasks, History, Git, Chat, News, Books,
Agent, installed apps, provider tables, maintenance, backups and restore.

- Procedures need the SDK's `unstable` feature. A Rust procedure cannot fail without
  trapping, so procedures roll back and return `{"error":{"code":…}}`.
- Views have no primary key here, so clients see a changed row as a delete and an insert.
- Tests use SpacetimeDB's in-process datastore (`test-utils`) with simulated browser tabs;
  run `cargo test --lib` (the cdylib cannot link it). `Cargo.lock` is seeded from
  SpacetimeDB v2.14.1's lockfile.

## Client (`client/`)

A Leptos 0.8 (client-side rendered) port of `next/client` that renders the same markup and
class names, so Dar's stylesheet applies unchanged: `build.sh` concatenates
`next/client/*.css` in `main.tsx`'s import order. Live data comes from SpacetimeDB's Rust SDK
(`browser` feature) with bindings generated from the Rust module (`generate.sh`; the 2.14.1
generator leaves fields named `type` unescaped, which the script fixes). Like
`next/client/module_bindings`, the generated files are ignored by Git; build the module and
run `generate.sh` from the root dev shell before `build.sh`.

- Ported: the app root and routing, sign-in and first-account setup, theme, app rail, top
  bar, account menu, spotlight search, shared controls (menus, select, dialog, editing pane,
  settings sheet), and Files: sections, breadcrumbs, list and grid, selection actions, row
  menus, keyboard shortcuts, the details panel (details, versions, sharing, public links),
  uploads with progress, new version uploads, move/copy with the folder picker,
  trash/undo/restore/delete, favorites, the Locked folder, the text editor (autosave,
  recovery draft, conflict review, Markdown toolbar and preview), file preview and Files
  settings; Contacts (address books, people list with letters and paging, favorites and
  groups, the contact card and editor, sharing, vCard import and export) and Calendar (month,
  week, day and agenda views, overlapping-event lanes, drag to move or resize, the date
  popover and sidebar month, the event card and editor with repeat and reminder fields,
  editing or deleting one occurrence or the series, calendar and subscription editors,
  sharing, reminders, ICS import and export), both live through `my_pim_changes`.
- Not yet: every other app (on the Rust stack their pages say so; through `client-serve`
  they open in the React client), folder history, PDF preview (pdf.js), installed apps.
- Size: 2.5 MB of wasm, 751 KB with brotli (Contacts and Calendar added about 200 KB; the
  React client's JavaScript for every app is about 410 KB). The SDK depends on brotli with
  its C API enabled, which kept the whole compressor in the client; `patch-brotli.sh` builds
  against the published crate (checksum verified) with that default turned off, which cut
  the download by a third.
- `client-serve` serves the client for ported pages and proxies everything else to a Dar
  instance: `cargo run -p dar-client-serve -- client/dist http://127.0.0.1:4311 127.0.0.1:4312`.

## OIDC (`oidc/`)

oxide-auth supplies the client registry, single-use codes, the token issuer and refresh
flows, and S256 PKCE. Written on top: ID tokens (RS256, `nonce`, `auth_time`, `at_hash`),
discovery, JWKS with rotation, UserInfo, introspection, revocation, RP-initiated logout,
login/consent interactions bound to the browser, provider sessions tied to the native Dar
session, refresh-token rotation with reuse detection, code-replay revocation, `form_post`,
`iss` in responses, and the built-in Immich code redemption. Confidential clients
authenticate with `client_secret_basic`, as Dar registers them.

## DAV (`dav/`)

A port of Dar's handlers, not dav-server: Files (PROPFIND/PROPPATCH/GET with ranges/PUT/
MKCOL/COPY/MOVE/DELETE/LOCK/UNLOCK), CalDAV and CardDAV (discovery, multiget, query
reports with text-match, prop-filter and time-range, preconditions). Calendar parsing and
RFC 4791 time ranges are written here (`ical.rs`) with the `rrule` and `chrono-tz` crates
for recurrence. Gaps: custom embedded VTIMEZONE definitions fall back to UTC; uploads are
buffered instead of streamed. `cargo run -p dar-dav --example demo` serves it on memory
backends; vdirsyncer discovers, uploads, edits and deletes against it.
