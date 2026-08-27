# Route and protocol inventory

Source: `dar-go-baseline-2026-08-27`. The Go baseline declares 123 app route
patterns plus core server routes. These legacy paths are an inventory of
behavior, not a compatibility requirement.

## Target browser and transport routes

- SPA: `/`, `/files`, `/contacts`, `/calendar`, `/todo`, `/notes`, `/news`,
  `/email`, `/activity`, `/prayer`, `/weather`, `/codex`, and `/settings`.
- Native API: `/api/v1/**`.
- User event stream: `GET /api/v1/events`.
- Codex event stream: `GET /api/v1/codex/sessions/{id}/events` with replay from
  `Last-Event-ID`.
- Plugin API and UI: `/api/v1/plugins/{pluginId}/**` and `/plugins/{pluginId}/**`.
- Public share pages use a new opaque-token namespace under `/share/{token}`.
- Activity RSS uses a new opaque-token namespace under `/feeds/activity/{token}`.

## Target DAV layout

- `/.well-known/caldav` redirects to `/dav/calendars/`.
- `/.well-known/carddav` redirects to `/dav/addressbooks/`.
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

