# Route and protocol inventory

Source: `dar-go-baseline-2026-08-27`. The Go baseline declares 123 app route
patterns plus core server routes. These legacy paths are an inventory of
behavior, not a compatibility requirement.

## Target browser and transport routes

- SPA: `/`, `/files`, `/contacts`, `/calendar`, `/todo`, `/notes`, `/news`,
  `/email`, `/activity`, `/prayer`, `/weather`, `/chat`, and `/settings`.
- Native API: `/api/v1/**`.
- Notes create/update accept the maintainer-approved optional `title` field.
  Titles normalize to NFC, replace unsafe filename characters with hyphens,
  and are bounded to 200 UTF-8 bytes before and after normalization. Explicit
  titles use readable collision-safe filenames; the legacy `Note-{uuid}`
  namespace stays reserved for omitted-title defaults. IDs and revision-backed
  `If-Match` remain stable through title/category edits. Omitting a title on
  update preserves the filename; no bulk rename runs during reconciliation.
- User event stream: `GET /api/v1/events`.
- Chat stream: `GET /api/v1/chat/conversations/{id}/events`; bounded
  `Last-Event-ID` replay of saved-resource change hints. `resync` explicitly
  requests fresh turns/approvals after a buffer gap or server restart.
- News refresh progress: `GET /api/v1/news/refresh-status`; a user-scoped
  collection of active feed fetch/article retrieval/save phases, refreshed by
  the established user event stream. Completed refresh metrics remain on feeds.
- News reading history: `/api/v1/news/items?view=recent` orders by UTC
  `viewedAt`, with cursor collections and explicit user/feed/folder scope.
  `PUT /api/v1/news/items/{id}` accepts `viewed: true` with `If-Match`, records
  the view and marks the article read without changing its star/archive state.
  It rejects `viewed: false` and a simultaneous `unread: true`.
- News article archives: `GET /api/v1/news/articles?cursor=...` returns
  `{items,nextCursor}`. `POST /api/v1/news/articles` accepts
  `{version:1,items:[...]}` with at most 1,000 records / 50 MiB and returns
  `{imported,skipped,duplicates}`. Records match active subscriptions by `feedUrl`
  and stable `externalId`; existing articles are kept, and unrecognized fields,
  unsafe URLs or invalid dates reject the whole batch. Browser exports include
  `version:1`; larger archives are imported as retryable batches. Bodies have a
  30-second owned deadline and a global/per-user capacity bound.
- News media: `GET /api/v1/news/items/{id}/thumbnail` returns a sanitized saved
  MediaRSS/iTunes thumbnail. It uses the same image transport, limits and
  permission checks as article images. `GET /api/v1/news/items/{id}/media/{index}/image` returns sanitized
  JPEG bytes; `/embed` returns a fixed sandbox wrapper. Indexes refer to at most
  100 saved inert media references. Both resources enforce current user/item
  access before serving cache. Optional `version` is the article's 64-character
  `mediaVersion`; a changed body returns 412. Optional numeric `revision` checks
  the full article revision. Invalid parameters return 400, missing references
  404, and bounded-capacity failures 503, using documented JSON errors. Frame
  navigations receive a readable HTML error page with the same status and a
  restrictive CSP. Responses are private and never browser-cached.
- Plugin API and UI: `/api/v1/plugins/{pluginId}/**` and `/plugins/{pluginId}/**`.
- Public share pages use a new opaque-token namespace under `/share/{token}`.
- Activity RSS uses a new opaque-token namespace under `/feeds/activity/{token}`.

## Target DAV layout

- `/.well-known/caldav` redirects to `/dav/calendars/`.
- `/.well-known/carddav` redirects to `/dav/addressbooks/`.
- These redirects also support PROPFIND. Calendar/address-book parent roots
  resolve the authenticated scoped app-password user's principal and home;
  clients may start discovery from the server URL without knowing a user path.
- `/dav/files/{username}/...` serves WebDAV files.
- `/dav/calendars/{username}/...` serves VEVENT collections.
- `/dav/tasks/{username}/...` serves VTODO collections.
- `/dav/addressbooks/{username}/...` serves vCard collections.
- Browser sessions do not authorize DAV. DAV accepts scoped Basic app passwords
  and returns standards-appropriate challenges and multistatus errors.
- The DAV implementation retains OPTIONS, PROPFIND, REPORT, GET/HEAD, PUT,
  DELETE, MKCOL, COPY, MOVE, LOCK/UNLOCK, conditional ETags, depth handling,
  multiget/query behavior, CTags, ranges where applicable, and user isolation.

## Legacy core behavior to remap

- `/`, `/login`, `/auth/oidc/callback`, `/home`, `/search`, `/settings/**`,
  `/logout`, `/health`, public assets, and authenticated shell assets.
- `/api/me`, search query/matches/preferences, notification list/dismiss/
  preferences/stream, dashboard catalog/layout/widget/settings, appearance and
  wallpaper, personal/time/Files/Calendar settings, and app passwords.

## Legacy app routes to remap

- Activity: `/activity/**`, `/api/activity`, settings, RSS regeneration, and
  `/activity/rss/{token}`.
- Files: `/files/**`, public `/s/**`, list/picker/upload/download/editor/mkdir/
  touch/symlink/rename/move/copy/delete/restore/permanent-delete/favorite/search/
  tags/tag/thumb/preview/versions/version/diff/stats/share/shared/comments/lock/
  user-share/shared-with-me APIs, and `/remote.php/dav/files/**`.
- Contacts: `/contacts/**`, list/get/create/update/delete/search/import/export,
  books/rename/delete/visibility/preferences, and `/dav/**` CardDAV.
- Calendar: `/calendar`, calendar list/update/delete, event list/get/create/
  update/delete/search/import/export, subscriptions/refresh, and `/caldav/**`.
- Todo: `/todo`, lists, individual list routes, tasks, individual task routes,
  and preferences.
- Notes: `/notes`, `/notes/new`, `/notes/note/{id}`, native notes/categories/
  undo/versions/settings/attachment APIs, and legacy Nextcloud Notes APIs.
- News: `/news/**`, auxiliary APIs, and legacy Nextcloud News v1-3 APIs.
- Email: `/email/**` and the multiplexed `/api/email/**` resource API.
- Prayer: `/prayer`, `/prayer/analytics`, times/settings/analytics APIs.
- Codex: `/codex` and the multiplexed `/api/codex/**` API.

Email's native conversation resources are `GET /api/v1/email/threads` and
`GET /api/v1/email/threads/{id}`. Conversation IDs remain stable when the latest
message changes; results also include `messageId` for opening that message.
Search accepts `accountId`, `folderId`, `role`, `q`, `unread`, `flagged`,
`hasAttachment`, `limit` (1–100) and `cursor`. Boolean query values are exactly
`true` or `false`. Cursors are bound to the user and filter selection.

`POST /api/v1/email/messages/actions` accepts `action`, `requestId`, optional
`folderId`, and 1–200 distinct `messages` entries containing `id` and `revision`.
Its strong `If-Match` is `"selection-{sha256}"`: the lowercase SHA-256 digest of
UTF-8 JSON `[[id,revision],...]`, sorted by ID. Missing headers return 428;
mismatched selection tags or stale per-message revisions return 412. Every
scope/revision/destination is validated before any file effect. The response has
`requestId`, `complete` and `items` with each account's durable operation and
remaining messages. Filesystem failure may leave a partial result; retry the
same request and reviewed selection to resume saved plans. Changed reuse of a
request ID returns 409. Completed deletion retains an operation receipt after
the message disappears. Existing single-message routes remain supported.

`DELETE /api/v1/email/drafts/{id}` permanently discards an unsent draft. Its
JSON body contains `requestId`, and `If-Match` is required. The destructive
`email.draft-delete` assistant tool uses the same journal and ownership rules.
Every authoritative location must still be a draft. Ordinary message deletion
continues to require Trash; a send awaiting acknowledgement or local filing
blocks draft discard with 409. Retrying the same discard retains its receipt
after the draft is gone; reusing that request for a different kind of deletion
returns 409. Search/delivery cleanup follows Maildir reconciliation.

## Deliberately removed routes

- All `/remote.php/**` aliases.
- `/index.php/apps/news/api/**`.
- `/index.php/apps/notes/api/**`.
- File symlink creation.
- Separate server-rendered asset endpoints; production serves fingerprinted SPA
  and plugin bundles instead.

## Protocol and portable-format fixtures

The rewrite ports the behavioral fixtures from these baseline areas:

- `internal/carddav`, `internal/caldav`, Todo DAV tests, and Files workflow tests.
- Calendar ICS recurrence/import/export/subscription fixtures.
- Contacts VCF/CardDAV/import/export fixtures.
- Email Maildir, MIME, SMTP, IMAP and real hermetic `mbsync` fixtures.
- News RSS/Atom/OPML discovery and ingestion fixtures.
- Activity RSS and Codex line-delimited RPC/SSE replay fixtures.
