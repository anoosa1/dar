# Dar on SpacetimeDB

The current application runs from this directory. Legacy source was archived
outside the checkout and removed from this branch. SpacetimeDB owns accounts, authorization and file metadata;
a Node/Cordis runtime handles native login and immutable local file content.
There is no SQLite or external identity provider. The React interface uses the
selected shared Material 3 layout B, with centered persistent search, themed menus
and shared settings. App workspaces retain the selected Files C and
Contacts/Calendar/Notes/Tasks A layouts.
See the [architecture and acceptance record](../docs/spacetimedb/architecture.md).

## Run locally

From the repository root:

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311>. The launcher prints the **path** to the operator
setup key. Read that local file and enter the key in first-administrator setup;
choose your own username and a password of at least 15 characters. Registration
is administrator-only after setup. Account security manages passwords, sessions
and scoped WebDAV app passwords. **Settings → Administration → Directory** lets
administrators create users. Recovery is an operator command below.

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
headers as identity authority: the configured origin is authoritative. Changing
it revokes old sessions. OIDC requires the local proxy to preserve the canonical
Host and send `X-Forwarded-Proto: https`; mismatched forwarding hosts are rejected.

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
internal `/auth/issuer` serves this app's SpacetimeDB authentication only.
The separate `/oidc` issuer provides third-party sign-in, while `/scim/v2`
provides directory provisioning. See [identity administration](#identity-and-provisioning).

## Files and limits

Files includes upload/cancel, folders, breadcrumbs, list/grid, selection, sorting,
rename, move/copy, download/ranges, text/image preview, favorites, versions,
trash/restore, permanent deletion, user grants and expiring public download links.
Live views react to session revocation and sharing changes. Each browser tab has
independent navigation; folder URLs survive refresh and browser history.

Three-dot menus expose ordinary file actions and an explicit Details command.
Copy/Cut/Paste supports one item or folder at a time, including Ctrl/Cmd C/X/V;
F2 renames the selected item. The clipboard is account-scoped to this browser tab.
Files settings saves view, hidden-file visibility, confirmation before trash and
the initial details tab for the account.

**New → New folder / New file** opens a right-hand creation pane. Text and
Markdown files up to 1 MiB open in the Files editor, with Markdown formatting,
safe preview, autosave, explicit Save/Ctrl/Cmd S, undo/redo, recovery drafts and
revision-conflict review. Saves use the existing immutable upload/version path;
read-only shares stay read-only. Larger/binary files retain preview/download.
Creation and editing of contacts, calendars/events, tasks and their collections
also use the shared right pane with complete forms. Unsaved forms require an
explicit discard; destructive confirmations remain dialogs.

- Uploads are limited to 1 GiB each, four concurrent runtime streams, and a
  one-hour reservation. Retained versions and copies count toward a 10 GiB logical
  quota per owner; physical capacity still needs operator monitoring.
- Collections use indexed windows of 200 entries. Search is a **filename prefix
  within the current collection**, not full-text or file-content search.
- Recursive copy is bounded to 500 entries; trash/restore/delete to 2,000.
  Offline editing, E2EE, public folder browsing, and password-protected public
  links are outside this milestone. Existing downloads may finish after revocation.
- There is no migration from the previous Dar storage or legacy Nextcloud API.
  Additive upgrades and older backup formats are tested; future schema changes
  still need explicit migration and restore compatibility work before deployment.

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

Use the themed application launcher in the top bar, or open `/contacts` and `/calendar`.
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
Month is the default for fresh navigation; explicit view/date links are retained.
The centre date control updates with the arrows and always returns to today when
clicked. Views fit the workspace; crowded month cells link to the full day, and
long calendar lists use compact paging rather than scrolling the whole sidebar.
Each owned calendar has its own Edit/Share/Delete menu.

Calendar settings contains week start, persistent Show cancelled events, and
Import and export with an explicit calendar selector. Timezone is configured
globally under **Settings → Personal → Regional settings** and is also used by
Tasks and reminders. Enable Show cancelled events in Calendar settings to find
and edit cancelled events/occurrences again.
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
collections in the native UI. Sync tokens, LOCK, PROPPATCH, MKCALENDAR and
iTIP scheduling are not advertised. Unsupported filters fail explicitly. Tested
interoperability uses vdirsyncer 0.20.0, including native/client round trips.

Each owner can create 100 collections; native Contacts views paginate 200 rows
within a bounded 10,000-contact search set. Collections hold up to 10,000 resources.
Imports accept 2 MiB/1,000 resources and exports up to 8 MiB; individual resources
are limited to 256 KiB. Native recurrence windows span at most one year, 10,000
iterator steps per resource and 2,000 returned occurrences per series. Queries
that exceed limits fail rather than returning a silently incomplete calendar.
These are explicit safety bounds, not a large-archive performance guarantee.

Instance backups use `dar-backup-v4`, including Notes, retained note trash, VTODOs, directory profiles,
groups, registered clients and the encrypted external signing keyring. Restoring
preserves IDs and client secrets, but invalidates native/provider sessions,
codes, tokens and provisioning credentials. Sign in and issue new SCIM credentials.
Restore also accepts v1 auth/Files, v2 Contacts/Calendar and v3 identity backups and rejects
unknown formats. Additive upgrades are tested against both committed baselines.
See the [identity delivery record](../docs/spacetimedb/identity-provider.md).

## Notes and Tasks

Open `/notes` or `/tasks` from the shared application launcher. Both use selected
layout A: a focused list/details workspace with the persistent centered search.

Notes provides notebooks, sharing, exact Markdown source, safe preview, tags,
favorites, autosave, explicit conflict/draft recovery, move/copy, private owner
trash/restore and Markdown/notebook import/export. Preview supports a small
documented dialect; HTML, remote images and active links remain inert. Notes live
in SpacetimeDB and are independent of Files. Linked Files retain their own access
checks. A recovery draft is scoped to the account/note in this browser tab.

Tasks uses VTODO in the same calendars as CalDAV. Lists, subtasks, priorities,
progress/status, dates/timezones, categories, recurrence/occurrence editing,
completion/reopening, search/views, subtree move/copy and explicit parent deletion
share the database authorization and revision boundary. Completing an occurrence
advances to the next pending slot; exhausted finite series finish. Reopen a
completed slot through Edit scope. The existing owner reminder inbox and scheduler
also deliver task alarms with durable restart catch-up.

Native views paginate 200 rows within 10,000 resources; Notes trash also paginates.
Notebook JSON import/export accepts 500 notes/2 MiB; individual notes accept
256 KiB. Task subtree mutations accept 500 records and depth 32. Existing calendar
recurrence/import bounds remain in force. Read the [delivery record and precise
limits](../docs/spacetimedb/notes-tasks.md) for API routes, supported Markdown,
recurrence semantics, sharing and protocol details.

## Identity and provisioning

Open **Account menu → Settings**, or `/settings`. Personal settings contains
Profile, Security and Connected applications. Administration contains Applications,
Directory, SCIM and Signing keys, and is visible only to administrators.
Administrators can register applications, manage directory people/groups, issue
SCIM credentials and rotate OIDC keys. Sensitive changes in these settings need
password confirmation, valid for five minutes. Ordinary users can review and
revoke their connected applications.

Every app has a lower-sidebar app-settings entry using the same settings dialog.
Sidebar navigation shares hover and selected highlights across all five apps and
settings in both themes. Sign-in uses a centered username/password form without
promotional copy; first-account setup retains its required fields and guidance.
Preferences persist per account with revision checks, including Contacts sorting,
Calendar week start/cancelled visibility, Notes sorting/editor mode and Tasks sorting/default
view. Profile edits save names and primary email; username and privileges cannot
be changed through Personal settings. The shared launcher and account menu remain
available in global settings. See the [UX audit and delivery record](../docs/spacetimedb/ux-consistency-audit.md)
and [Nextcloud reference screenshots](../docs/spacetimedb/nextcloud-reference/index.html).

**Personal → Appearance** persists Light (Catppuccin Latte), Dark (Catppuccin
Mocha), or Follow system for the account. Shared tokens apply to all apps,
settings, menus, creation panes and the editor. Colors derive from the
[official Catppuccin palette](https://catppuccin.com/palette/).

- Use **`https://your-instance/oidc`** as the external issuer. Discovery is at
  `/oidc/.well-known/openid-configuration`. Register exact redirect/logout URLs;
  confidential clients use HTTP Basic client authentication and public clients
  use no client secret. Every client needs S256 PKCE. Secrets are shown once.
- Allowed permissions are `openid`, `profile`, `email`, `groups` and
  `offline_access`. Group claims contain immutable directory group IDs. Supplied
  emails remain unverified. Offline access is bounded by the originating native
  session; native logout, suspension and password changes revoke live tokens.
- Point provisioners at **`https://your-instance/scim/v2`** using a dedicated
  read-only or read/write bearer credential. Credentials expire in 1–365 days;
  DAV passwords and OIDC profile access tokens cannot provision users.
- SCIM supports Users/Groups CRUD, PATCH, filters, projections and pagination.
  Groups contain users only and grant no app or administrator permissions.
  Deactivation revokes access; deletion also tombstones the directory identity
  while preserving owned app data. A tombstoned username stays reserved.
- External ID tokens last five minutes. Key rotation retains prior keys for ten
  minutes. Signed tokens already held by clients and an RP's independent session
  cookie cannot be remotely erased; the RP must implement its own session policy.

The [protocol contract and limits](../docs/spacetimedb/identity-provider.md)
documents the precise supported subset. No dynamic client registration, implicit
flow, password grant, SCIM Bulk, nested groups or outbound provisioning is enabled.

## Verify

```sh
npm --prefix next run verify
```

For focused isolated checks, run `node next/scripts/verify.mjs --workspace`,
`--pim` or `--identity` from the repository root inside the Nix shell. These
subsets do not replace the full acceptance run.

This builds and starts a fresh real database and storage root, typechecks both
projects, runs the focused Node acceptance suite, exercises Chromium at desktop and phone
sizes, and forces a runtime failure during upload before restarting the database.
It verifies native recovery and revocation, permissions, file bytes and revisions,
backup/restore, export, orphan cleanup, backup pin protection and reconnect during
navigation, Contacts/Calendar native and DAV behavior, reminders, module upgrades
and vdirsyncer interoperability, independent OIDC RP flows, SCIM lifecycle,
production proxy headers and restart/restore of provider state, Notes drafts and
trash, VTODO native/DAV interoperability, task reminders and workspace upgrades. Tests never use
the default state directory. `run test` alone skips integration tests without an
explicit test instance and does not establish acceptance.

The indexed collection fixture has 250 entries; its timing and process-tree RSS
are reported as measurements of that fixture, not a large-archive capacity claim.
The accepted app/protocol subsets are documented; this is not a claim of bug-free
software, formal OIDC certification or complete Nextcloud feature parity.
