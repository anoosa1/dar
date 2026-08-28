# Rewrite parity ledger

This ledger is binding for the ClojureScript and Cordis rewrite. Its source is
Git tag `dar-go-baseline-2026-08-27` at commit `0be89a2`. A capability is not
complete until every applicable surface is implemented and verified.

## Approved contract breaks

- The rewrite accepts only a fresh, versioned data root and database.
- Browser APIs move to `/api/v1` and use the new JSON, error, pagination, and
  revision contracts. Nextcloud-shaped News and Notes APIs are removed.
- DAV moves to the standards-first paths in `routes-and-protocols.md`; legacy
  `/remote.php` aliases are removed.
- Managed Files storage does not create, follow, index, or serve symbolic links.
  The legacy `files.create_symlink` assistant tool is removed.
- Photos and Soosa remain removed. `TODO.md` and `ideas` are not requirements.

Everything else below requires behavioral parity, though route payloads and UI
composition may be redesigned.

## Completion rule for every app

For an applicable capability, implement all of these together:

- SPA routes, navigation, loading, empty, success, conflict, and error states.
- Authenticated `/api/v1` resources with Malli validation and stable errors.
- Portable-file storage and SQLite metadata/index responsibilities.
- Standards protocols and import/export formats.
- Unified search indexing, deletion, reconciliation, and result URLs.
- Activity, recipient delivery, notification behavior, and preferences.
- Dashboard widgets and their size/settings contracts.
- Typed, user-scoped assistant tools with confirmation/effect classification.
- Startup, cancellation, worker cleanup, subprocess cleanup, and reverse-order
  teardown.
- Focused tests plus a browser or protocol pass for user-visible behavior.

## Shared platform

- OIDC authorization code flow with PKCE, state and nonce validation, secure
  HTTP-only sessions, logout, session expiry, and user/profile synchronization.
- Scoped app passwords for Files, Calendar, Todo, and Contacts protocol clients.
- CSRF and same-origin protection on session writes, security headers and CSP,
  request/body limits, gzip exclusions, trusted-proxy handling, and rate limits.
- One WAL-mode SQLite database with foreign keys, numbered migrations, explicit
  transactions, user scoping, and deterministic close after HTTP/workers stop.
- AES-GCM credential keyring with active-key rotation and no secret disclosure.
- Safe atomic files, quotas, path confinement, version/trash roots, mutation
  journaling, and recovery reconciliation.
- SSRF-resistant outbound HTTP including restricted-address checks, DNS
  revalidation, redirect validation, timeouts, response limits, and cancellation.
- User preferences, locale/time format, appearance, wallpaper, and theme tokens.
- Unified search with provider generations, staged reconciliation, bounded
  queries, incremental writes/deletes, and user-scoped result URLs.
- Durable activity events, recipients, outbox retry, feed settings, cursor reads,
  and per-user RSS tokens.
- Notification persistence, deduplication, preferences, dismissal, and SSE.
- Dashboard catalog, layout, widget sizes, widget settings, refresh, and errors.
- Recipient lookup across Dar users and Contacts.
- Assistant provider discovery, Malli schemas, stable errors, and read/safe-write/
  external/destructive effects.
- Managed subprocess capacity, idle/LRU cleanup, cancellation, output bounds,
  crash handling, environment allowlisting, and deterministic shutdown.

## Application capabilities

### Core shell

- Login, logout, home/dashboard, profile, personal settings, appearance,
  wallpaper, time format, Files and Calendar settings, app-password management,
  global search, notifications, administrator settings, responsive navigation,
  accessibility, keyboard behavior, and health endpoint.

### Activity

- User-scoped feed, object/subject metadata, recipient fan-out, durable outbox,
  retry, cursor pagination, feed filters, RSS token regeneration, and public RSS.

### Files

- File and folder list/create/read/write/upload/download, text editor, picker,
  validation, touch, rename, move, copy, quota, safe concurrent operations, and
  atomic replacement.
- Favorites, tags, versions, version read/restore, diffs, trash, restore,
  permanent deletion, previews, thumbnails, statistics, and activity history.
- Public shares, password-protected shares, expiry, drop-only upload, user shares,
  shared-with-me, permissions, comments, locks, and rate limits.
- External-storage configuration and a controlled read-only Maildir view.
- WebDAV including conditional requests, ranges, properties, locks, copy/move,
  user isolation, unsafe-path rejection, search, widget, activity, and assistant.

Rewrite status: browse/mutations/trash/favorites/quota (Stage 1), the editor
with If-Match 428/412 semantics, versions with read/restore, tags, comments,
and locks (Stage 2), and public shares, user shares, shared-with-me, drop
uploads, share unlock rate limits (Stage 3) are implemented and verified under
`/api/v1` with the new error envelope. Public share visitors use the pinned
`/share/{token}` JSON namespace; drop uploads take raw bytes with a name
query; the baseline `/Shared` virtual folder is superseded by the
shared-with-me API (payload/UI redesign is allowed by this ledger). New stable
codes this stage: `gone` (410) for expired shares. Previews and thumbnails
(Stage 4) are implemented under `/api/v1/files/preview` and `/api/v1/files/
thumb`: previews are UTF-8-text only (binary content, including NUL bytes,
answers 415, files over 8 MiB answer 413, content truncates at 512k code
units) where the baseline returned raw bytes as a string; thumbnails decode
through Sharp with the baseline dimension guards (reject over 20,000 px or
40 M pixels with 413, undecodable data with 415) and re-encode to a JPEG
quality 70, max width 200 without enlargement. WebDAV files (Stage 5) serves
`/dav/files/{username}/...` with scoped Basic app passwords only (browser
sessions never authorize DAV, per the routes contract): OPTIONS/PROPFIND with
Depth 0/1 handling (infinity rejected) and the target included in the
multistatus, GET/HEAD with ETag conditionals and single byte ranges, PUT with
If-Match/If-None-Match (412) and 201/204 create/overwrite distinction,
MKCOL/DELETE/COPY/MOVE with Destination checking, and LOCK/UNLOCK backed by
the engine's advisory locks with derived opaque tokens (423, 409). The
baseline's `/webdav/` and `/remote.php/dav/files/**` aliases are removed.
Search, stats, and version diffs (Stage 6) are implemented under
`/api/v1/files/search` (name substring, baseline type filters
folder/image/document/video/audio, folder scope, limit/offset paging with
totals), `/api/v1/files/stats` (`{used, total}`), and
`/api/v1/files/versions/diff` (baseline's greedy line diff), plus
`/api/v1/files/download` (raw bytes, `content-disposition: attachment`,
folders answer 400). Search never follows or indexes symlinks. Assistant
tools (Stage 6) register `files.*` tool descriptors through the platform
contributions registry with Malli object schemas, explicit user scope, and
one effect classification each: `files.list`/`files.search`/`files.read`
(read; text reads reject non-UTF-8 and truncate at 64k characters),
`files.write-text`/`files.create-folder`/`files.rename`/`files.move`/
`files.copy`/`files.favorite`/`files.tag`/`files.restore` (safeWrite;
assistant writes cap at 1 MiB and reject NUL bytes),
`files.share` (external; create/remove, confirmation-gated),
`files.trash`/`files.delete-permanently` (destructive, confirmation-gated;
both return the trash id for the follow-up call). The baseline
`create_symlink` tool is not ported (managed storage never creates or follows
symlinks) and the baseline's `to_user`/`permission` share options are
superseded by the share APIs (public `/share/{token}` links and the
user-shares surface). The `files.favorites` dashboard widget registers
through the dashboard catalog with the baseline per-size item limits
(1/3/6/10). External storage is deliberately not ported: the baseline only
shipped an unused `files_external_storage` table, and the rewrite contract
is one data root of fresh managed storage that never mounts or follows
external paths. The shared cross-app picker (Stage 7) is implemented under
`/api/v1/files/picker` (one-folder browse without trash, dotfiles, or
mutations; query narrowing; folders-first ordering) and
`/api/v1/files/picker/validate` (selection/multiple/MIME-prefix checking);
MIME filtering uses the entry's name-based type rather than the baseline's
content sniffing. Mutations record `files` activity events (upload, edit, create,
rename, copy, trash, restore, purge, share, share removal) through the
platform activity service for both the HTTP surface and the assistant
tools; recipients are the acting user and the baseline's share-recipient
fan-out is not ported. Browser navigations to `/share/{token}` get the
SPA shell (Accept negotiation) and render the public share visitor page
(folder listing, single-file download, password unlock, drop upload);
fetches keep the pinned JSON namespace. DAV mutations record the same
events with `origin "dav"` in the subject, so protocol-client writes are
visible in the feed like the baseline's per-origin logActivity. The
rewrite's DAV surface exceeds the baseline suite (RFC 4918 depth
handling, LOCK/UNLOCK with tokens, COPY/MOVE, ETag conditionals,
scoped app passwords, 405 for unsupported methods), closing the
WebDAV hardening item.

### Contacts

- Multiple address books, contacts and groups; people and organizations; rich
  names, multiple email/phone/address values, birthday, organization, role,
  website, notes, photo, and group membership.
- Book create/rename/delete/visibility, preferences, Me-card synchronization,
  import/export, recipient lookup, VCF authority, CardDAV, search, activity, and
  assistant tools.

### Calendar

- Multiple calendars, event CRUD, colors, all-day events, timezone-aware times,
  recurrence, recurrence exceptions/deletions, alarms, status, location, and
  descriptions.
- ICS import/export, remote WebCal/ICS subscriptions and refresh, birthdays,
  preferences, CalDAV, search, activity, assistant tools, and upcoming/on-this-day
  widgets.

### Todo

- Lists, membership and read/write shares, list trash/restore, tasks/subtasks,
  notes, tags, priority, start/due values, completion, task trash/restore/purge,
  smart collections, and preferences.
- Recurrence, occurrence completion, generated Prayer mappings, VTODO authority,
  DAV, search, activity, assistant tools, and upcoming widget.

### Notes

- Markdown files under Files, stable IDs, configurable notes path and suffix,
  title/content/category/favorite values, external rename/edit reconciliation,
  conditional updates, conflicts, versions, undo, attachments, and preferences.
- Search, activity, assistant tools, widget, and correct behavior after Files-side
  changes.

### News

- Folder and feed CRUD, public-feed discovery, RSS/Atom ingestion, OPML import and
  export, refresh scheduling/manual refresh, item pagination, read/unread and
  star state, archive behavior, sanitization, enclosure/video metadata, search,
  activity, preferences, and assistant tools.

### Email

- Account discovery/configuration, encrypted credentials, Maildir layout,
  generated `mbsyncrc`, bounded `mbsync` execution, polling/manual sync, status,
  operation queue, and index reconciliation.
- Accounts, folders/roles, messages, threads, recipients, MIME bodies,
  attachments, drafts, SMTP send, move, read/unread, flags, delete, preferences,
  notifications, search, activity, assistant tools, and unread widget.

### Prayer

- Prayer-time calculation methods, Asr options, high-latitude policies, DST,
  polar cases, date selection, settings, occurrence persistence, Todo generation
  and reconciliation, analytics, worker lifecycle, and widget.

### Weather

- User/profile location, geocoding, unit preference, provider fetch, cache,
  stale fallback, configuration errors, and dashboard widget.

### Codex

- One isolated Codex home and managed CLI app-server per user; executable version
  check; capacity, LRU and idle policies; crash/restart behavior; environment
  redaction; stdout/stderr handling; and graceful process termination.
- Session list/create/read/update/archive/delete, protocol multiplexing, RPC,
  approvals, tool calls, interruption, replayable events/SSE, attachments,
  workspace isolation, preferences, search, activity, and every registered Dar
  assistant provider.

## Dashboard widgets

- `calendar.upcoming`
- `calendar.on_this_day`
- `email.unread` (configurable)
- `files.favorites`
- `notes.list`
- `todo.upcoming`
- `prayer.times`
- `weather.current`

