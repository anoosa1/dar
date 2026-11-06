# Dar on SpacetimeDB

The new application runs independently here, preserving unrelated work in the
previous checkout. SpacetimeDB owns accounts, authorization and file metadata;
a Node/Cordis runtime handles native login and immutable local file content.
There is no SQLite or external identity provider. The React interface implements
selected mock C, with Nextcloud-style Material 3 and centered persistent search.
Contacts and Calendar extend that workspace using selected mock A.
See the [architecture and acceptance record](../docs/spacetimedb/architecture.md).

## Run locally

From the repository root:

```sh
nix develop ./next
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311>. The launcher prints the **path** to the operator
setup key. Read that local file and enter the key in first-administrator setup;
choose your own username and a password of at least 15 characters. Registration
is administrator-only after setup. Account security manages passwords, sessions
and scoped WebDAV app passwords; administrators can create users from the account
menu. Recovery is an operator command below.

The Nix shell pins Node 24.22.1 and SpacetimeDB 2.14.1. Without Nix, install those
versions and provide Chromium for browser verification. Linux x86-64 is verified;
the shell also declares ARM64 sources, which have not been exercised here.

The default state directory is `$XDG_DATA_HOME/dar-spacetime`, falling back to
`~/.local/share/dar-spacetime`. Set `DAR_ROOT` to another **fresh** directory;
unknown non-empty roots and symlinked managed directories are refused. Existing
roots created by this application can be restarted. `Ctrl-C` drains the runtime
before stopping its database child. Never use the previous application's `data/`.

| Variable | Launcher default |
| --- | --- |
| `DAR_ROOT` | XDG directory above |
| `DAR_PORT` | `4311`, Node HTTP on loopback |
| `DAR_DATABASE_PORT` | `4310`, database on loopback |
| `DAR_DATABASE_NAME` | `dar` |
| `DAR_SPACETIME_BIN` | `spacetime` from PATH |

The launcher builds the module, publishes without deleting existing data,
generates the typed client, builds local assets, and starts both services. It
keeps its CLI credentials under the chosen state root. Do not put state inside
the source tree or pass a directory containing secrets to a Nix `path:` flake.

## Self-host with HTTPS

Install dependencies as above. Run under a dedicated OS account and supervise
the launcher using your service manager:

```sh
export DAR_ROOT=/srv/dar
export DAR_ORIGIN=https://cloud.example.com
npm --prefix next run serve
```

Create `/srv/dar` empty and owned by that account. The production launcher uses
Secure, HttpOnly, SameSite cookies and HSTS. It requires an HTTPS origin without
a path or trailing slash. It deliberately ignores forwarded identity/origin
headers: the configured origin is authoritative. Changing it revokes old sessions.

Terminate TLS at your reverse proxy. Forward ordinary requests to
`127.0.0.1:4311`. The browser's only public database route is the WebSocket
subscription endpoint. For the default database name, this Caddy configuration
restricts exposure to that route:

```caddyfile
cloud.example.com {
    @database path /spacetime/v1/database/dar/subscribe
    handle @database {
        uri strip_prefix /spacetime
        reverse_proxy 127.0.0.1:4310
    }
    handle /spacetime/* {
        respond 404
    }
    handle {
        reverse_proxy 127.0.0.1:4311
    }
}
```

Adapt the database name and ports together if changed. Keep all other database
administration/SQL/publish endpoints private. WebSocket upgrades, issuer discovery
and JWKS must be reachable through the proxy, including from the database host.
`DAR_DATABASE_PUBLIC_URL` overrides the browser database base URL; the default is
`$DAR_ORIGIN/spacetime/`. This proxy example is a deployment template; acceptance
tests exercise the application on isolated loopback instances.

The pinned SDK requires CSP `unsafe-eval` for its generated binary codecs. Inline
scripts, remote runtime assets and active-file previews remain disallowed. The
issuer surface serves this app's SpacetimeDB authentication only; third-party
OIDC and SCIM provider endpoints are future work.

## Files and limits

Files includes upload/cancel, folders, breadcrumbs, list/grid, selection, sorting,
rename, move/copy, download/ranges, text/image preview, favorites, versions,
trash/restore, permanent deletion, user grants and expiring public download links.
Live views react to session revocation and sharing changes. Each browser tab has
independent navigation; folder URLs survive refresh and browser history.

- Uploads are limited to 1 GiB each, four concurrent runtime streams, and a
  one-hour reservation. Retained versions and copies count toward a 10 GiB logical
  quota per owner; physical capacity still needs operator monitoring.
- Collections use indexed windows of 200 entries. Search is a **filename prefix
  within the current collection**, not full-text or file-content search.
- Recursive copy is bounded to 500 entries; trash/restore/delete to 2,000.
  Offline editing, E2EE, public folder browsing, and password-protected public
  links are outside this milestone. Existing downloads may finish after revocation.
- There is no migration from the previous Dar storage or legacy Nextcloud API.
  This is the initial fresh schema; future schema changes need explicit migration
  and restore compatibility work before deployment.

WebDAV is available at `/dav/files/<username>/`, using a scoped app password from
Account security. Supported operations are GET/HEAD/ranges, Depth 0/1 PROPFIND,
MKCOL, PUT, MOVE, COPY and DELETE on the user's own files. PUT requires
Content-Length; replacement and other mutable requests require the current
If-Match ETag. Read-only credentials cannot write. LOCK, PROPPATCH, dead properties,
sync tokens and shared DAV mounts are not implemented; this is a documented subset,
not full Nextcloud or WebDAV client conformance.

## Operator commands and recovery

Run commands as the application's OS account, in the Nix shell, while its database
is running. For the default launcher layout:

```sh
export DAR_ROOT=/srv/dar
export DAR_DATABASE_URL=http://127.0.0.1:4310
export DAR_DATABASE_NAME=dar
export DAR_DATABASE_TOKEN_FILE="$DAR_ROOT/secrets/database-token"

npm --prefix next run operator -- backup /srv/backups/dar-2026-11-06
npm --prefix next run operator -- export-files alice /srv/exports/alice
npm --prefix next run operator -- disable-user alice
npm --prefix next run operator -- enable-user alice
npm --prefix next run operator -- recover-password alice /run/private/new-password
```

Use a private mode-0600 password file; recovery consumes its contents with one
optional final newline removed and revokes all browser sessions and app passwords.
Remove that file afterward. Do not put passwords in command arguments or logs.

Backups use a transactional metadata snapshot and pin referenced blobs until
verified, fsynced independent copies exist. A completed backup has `complete.json`;
it includes private account verifiers, the encrypted signing key and its master
key, and must be protected as credentials. Preserve the entire backup directory.
Sessions and app passwords are deliberately invalidated on restore. Local keyring
encryption protects the JWK format at rest; possession of the root/master key is
still full operator trust.

Stop the application and restore into a **new empty root**:

```sh
DAR_ROOT=/srv/dar-restored DAR_ORIGIN=https://cloud.example.com \
  npm --prefix next run serve -- --restore /srv/backups/dar-2026-11-06
```

Use `run dev -- --restore ...` for a loopback restore instead. Restore refuses an
existing identity/content root or non-empty database; failed restores may leave
partial files, so retry in another fresh root after correcting the cause. Verify
the restored instance before retiring the previous one.

An interrupted backup retains its pins. Its `pending.json` records the ID; once
the failed backup is no longer needed, release it explicitly with
`run operator -- release-backup <id>`. Portable user exports contain ordinary
current file bytes and `manifest.json`, which maps opaque filenames back to the
original names and folder hierarchy; they exclude credentials and version history.
They are distinct from complete instance backups.

## Contacts and Calendar

Use the Application selector in the top bar, or open `/contacts` and `/calendar`.
Both share native login, account security and live permission revocation with Files.

Contacts provides multiple address books, structured names and postal addresses,
labeled emails/phones/websites, organizations, birthdays, groups, favorites,
search, saved name sorting, book sharing, and per-book/contact vCard exports.
vCard 3.0/4.0 imports report each item's result; duplicate UIDs are rejected rather
than silently overwriting records. Unchanged properties and their parameters
survive editing, including fields the editor does not expose.

Calendar provides named/color calendars, visibility, read/write sharing,
day/week/month/agenda, display timezone and week-start preferences, timed/floating/
all-day events, recurring series and individual exceptions. Move/resize opens the
same revision-checked editor as keyboard editing. All-day end dates are exclusive.
Enable Show cancelled events to find and edit cancelled events/occurrences again.
ICS import/export preserves UIDs, exceptions and unknown properties; native zoned
exports include an embedded VTIMEZONE generated from the runtime's IANA data.

Reminders have durable per-resource watermarks, duplicate suppression and an
acknowledgeable in-app inbox. The calendar owner receives reminders; shared readers
do not inherit the owner's alarms. The worker checks due work every minute and
catches up after downtime. Floating/all-day alarms use the owner's saved display
timezone, defaulting to UTC. Editing/deleting an event invalidates its previous
revision's reminders. Native edits support a display reminder up to seven days
before start. Other imported alarms are retained as data; email/audio effects,
external invitation delivery and remote feed subscriptions are not implemented.
Saving attendee information does not send invitations.

Create separate Contacts or Calendar app passwords under Account security.
CardDAV homes are `/dav/addressbooks/<username>/`; CalDAV homes are
`/dav/calendars/<username>/`. Existing Files app passwords stay Files-scoped.
Supported operations include principal/home discovery, PROPFIND, resource
GET/HEAD/PUT/DELETE, query and multiget REPORTs, and conditional writes. Create new
collections in the native UI. Sync tokens, LOCK, PROPPATCH, MKCALENDAR, VTODO and
iTIP scheduling are not advertised. Unsupported filters fail explicitly. Tested
interoperability uses vdirsyncer 0.20.0, including native/client round trips.

Each owner can create 100 collections; native Contacts views paginate 200 rows
within a bounded 10,000-contact search set. Collections hold up to 10,000 resources.
Imports accept 2 MiB/1,000 resources and exports up to 8 MiB; individual resources
are limited to 256 KiB. Native recurrence windows span at most one year, 10,000
iterator steps per resource and 2,000 returned occurrences per series. Queries
that exceed limits fail rather than returning a silently incomplete calendar.
These are explicit safety bounds, not a large-archive performance guarantee.

Instance backups now use `dar-backup-v2`, including Contacts, Calendar, grants,
preferences, reminder inbox and watermarks. Restore still accepts the initial
`dar-backup-v1` auth/Files format and rejects unknown future formats. The additive
module upgrade is tested against the committed auth/Files baseline without
resetting its database. See the [delivery and verification record](../docs/spacetimedb/contacts-calendar.md).

## Verify

```sh
npm --prefix next run verify
```

This builds and starts a fresh real database and storage root, typechecks both
projects, runs 31 focused Node tests, exercises Chromium at desktop and phone
sizes, and forces a runtime failure during upload before restarting the database.
It verifies native recovery and revocation, permissions, file bytes and revisions,
backup/restore, export, orphan cleanup, backup pin protection and reconnect during
navigation, Contacts/Calendar native and DAV behavior, reminders, module upgrades
and vdirsyncer interoperability. Tests never use
the default state directory. `run test` alone skips integration tests without an
explicit test instance and does not establish acceptance.

The indexed collection fixture has 250 entries; its timing and process-tree RSS
are reported as measurements of that fixture, not a large-archive capacity claim.
The accepted milestone is auth and Files, not bug-free software or complete
Nextcloud feature parity.
