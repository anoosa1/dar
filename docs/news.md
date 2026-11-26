# News

Optional built-in feed reader at `/news` (administrators can disable it in
Administration → Apps; data is kept). Layout follows the selected
[mock A](spacetimedb/news-reader-mock-a.html) (November 12, 2026) and the
[November 26 redesign](spacetimedb/news-redesign-mock.html): feeds, pinned shelf and
folders in the sidebar, a chronologically day-grouped article list or magazine cards
grid, and an editorial reader in the right sheet.

## Features

- **Feeds:** RSS 0.9–2.0, RSS 1.0 (RDF), Atom and JSON Feed. Subscribe finds feeds
  from a site address (alternate links, then common paths) and previews the latest
  items. Folders, rename, move, pin (dedicated `Pinned` shelf), pause, update now,
  14-day publishing cadence (`Feed Pulse` sparkline) and unsubscribe.
- **Reading:** `Today`, `Unread`, `Starred`, `Podcasts & media` and `All articles`
  views; `Split` and `Cards` layouts; inline filter chips (`All`, `Unread`, `Starred`,
  `Media`), sort toggle and bulk selection bar (mark read/unread, star/unstar);
  read on open or on scroll; mark all read only up to the newest loaded article, with
  Undo; keep unread; read articles optionally shown in feeds and folders.
- **Editorial reader:** `Aa` reading-appearance popover (Dar, Paper, Sepia and Night
  surfaces; Editorial Serif or Clean Sans; Compact, Default or Large size), distraction-free
  Focus mode (`f`), reading progress bar, Clip to Notes (saves clean Markdown into the
  user's Notes folder), and article footer navigation (`Previous`, `Next`, `Next unread`).
- **Full text:** per-feed automatic extraction, plus on-demand `Fetch full text`
  (`POST /api/v1/news/items/:id/fulltext`) for any individual article right from the reader.
- **Media:** podcast and video enclosures play inline with a custom transport
  (`-15s`, `+30s`, `1×`–`2×` playback speed, timeline scrubber) and remember playback position.
- **Updates:** background refresh every 15 minutes to 24 hours with conditional
  GET (ETag/Last-Modified). Failures back off (up to a day), roll up into a clickable
  sidebar health pill (`N feed(s) need attention`) and show above the feed's articles with Retry.
- **Retention:** read articles are kept 7–365 days. Starred articles and articles
  still in a feed are always kept, so they never return as unread.
- **Portability:** OPML import/export and a JSON export of starred articles.
- **Search and keys:** titles appear in the shared search. `j`/`k`, `n`/`p` (next/previous
  unread), `s` (star), `m` (read/unread), `o` (open original), `f` (focus mode),
  `a` (appearance), `v` (split/cards layout), `r` (refresh), `Shift+A` (mark all read), `Escape`.

## Mobile apps (Nextcloud News API)

Dar serves the Nextcloud News API v1.2 and v1.3 at `<origin>/dav/news`. In a
Nextcloud News client enter that address as the server, your Dar username and an
app password with News read (and write, to change state) access. Status, user and
capabilities endpoints are provided for client login checks. Item lists are capped
at 2,000 per request and updates since a time at 5,000.

## Safety

- Fetching is server-side only, without cookies or credentials. Private, loopback,
  link-local and other non-public addresses are refused at connection time and
  on every redirect, unless the operator sets `services.dar.news.allowPrivateNetworks`
  (or `DAR_NEWS_ALLOW_PRIVATE_NETWORKS=1`), which also applies to Calendar
  subscriptions. Responses are limited in size and time.
- Article HTML is reduced to an allowlist of formatting elements with absolute
  http(s) links on the server, and again in the browser. Remote images load
  through Dar's image proxy (raster types only, served inert) or are blocked.
  Enclosures play from their https address (`media-src https:`).
- Feeds, folders and articles are private per account and integer-numbered for
  client compatibility. Limits: 500 feeds, 100 folders, 1,000 articles per feed,
  60,000 per account. Backups (v7) include News; deleting an account deletes it.

Verify with `nix develop -c cargo xtask verify --only news` (local feed
fixture; no outside network).
