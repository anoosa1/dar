# News

Optional built-in feed reader at `/news` (administrators can disable it in
Administration → Apps; data is kept). Layout follows the selected
[mock A](spacetimedb/news-reader-mock-a.html) (November 12, 2026): feeds and folders
in the sidebar, a day-grouped article list and the article in the right sheet.

## Features

- **Feeds:** RSS 0.9–2.0, RSS 1.0 (RDF), Atom and JSON Feed. Subscribe finds feeds
  from a site address (alternate links, then common paths) and previews the latest
  items. Folders, rename, move, pin, pause, update now and unsubscribe.
- **Reading:** Unread, Starred and All views; read on open or on scroll; mark all
  read only up to the newest loaded article, with Undo; keep unread; newest or
  oldest first; read articles optionally shown in feeds and folders.
- **Full text** (per feed): new articles' pages are fetched and their main text
  extracted server-side.
- **Media:** podcast and video enclosures play inline and remember the position.
- **Updates:** background refresh every 15 minutes to 24 hours with conditional
  GET (ETag/Last-Modified). Failures back off (up to a day) and show in the
  sidebar and above the feed's articles with Retry.
- **Retention:** read articles are kept 7–365 days. Starred articles and articles
  still in a feed are always kept, so they never return as unread.
- **Portability:** OPML import/export and a JSON export of starred articles.
- **Search and keys:** titles appear in the shared search. `j`/`k`, `n` (next
  unread), `s`, `m`, `o`, `r`, `Shift+A`, `Escape`.

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
  (or `DAR_NEWS_ALLOW_PRIVATE_NETWORKS=1`). Responses are limited in size and time.
- Article HTML is reduced to an allowlist of formatting elements with absolute
  http(s) links on the server, and again in the browser. Remote images load
  through Dar's image proxy (raster types only, served inert) or are blocked.
  Enclosures play from their https address (`media-src https:`).
- Feeds, folders and articles are private per account and integer-numbered for
  client compatibility. Limits: 500 feeds, 100 folders, 1,000 articles per feed,
  60,000 per account. Backups (v7) include News; deleting an account deletes it.

Verify with `nix develop -c node next/scripts/verify.mjs --news` (local feed
fixture; no outside network).
