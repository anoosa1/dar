# Installed apps and optional apps

Dar runs trusted, administrator-installed app packages built with SDK API v1
([author guide](../next/sdk/README.md)). Prayer Times is the bundled example.
Installed code runs in Dar's process and browser origin: it is trusted, not
sandboxed. These packages are unrelated to OIDC "registered applications".

## Install

NixOS:

```nix
services.dar.apps = [ dar.packages.${pkgs.stdenv.hostPlatform.system}.prayer-times ];
```

Build with `nix build .#dar .#prayer-times`, then rebuild and restart. The
generated read-only registry is authoritative; no app is enabled by default.

Other hosts, from `nix develop` with Dar stopped:

```sh
node next/sdk/build.mjs apps/prayer-times
DAR_ROOT=/path/to/root node next/scripts/operator.ts apps-install apps/prayer-times/dist
```

Operator commands (`dar-operator` when packaged): `apps-install <dir>`,
`apps-list`, `apps-enable|apps-disable|apps-uninstall <id>`. They update the
private registry atomically and hold the instance lock, so restart afterwards.
They do not change a host registry set with `DAR_APPS_FILE` (a JSON array of
`{path, digest?, enabled?}`). `DAR_DISABLE_APPS=1` starts without installed apps.

Packages are validated (ID, API major, file inventory, size, digest, no symlinks)
before any code runs. A broken package or failing Node hook is reported in
Administration → Apps and leaves the rest of Dar working. There is no browser
upload, marketplace, automatic update or hot reload.

## Enable and disable

Administration → Apps (also in the account menu as Manage apps) enables or
disables Notes, Tasks, Agent and host-enabled installed apps immediately for the
whole instance. Files, Contacts and Calendar are always on. A disabled app leaves
the app rail and search, its APIs stop and external hooks are disposed; its data
is kept. Calendar's DAV routes, including tasks, stay available. This setting can
only restrict what the host configuration enables.

## Data

- Records are scoped to a live account and app with exact revision checks:
  64 KiB per record, 1 MiB and 128 records per account/app, 50 per page.
  App passwords do not authorize these APIs; apps cannot add database tables.
- Disable and uninstall keep data; reinstalling the same ID reconnects it.
  Deleting an account deletes its app data. Per-app purge/export is not shipped.
- Backups (format v6) include all app records and the package inventory, not
  code. Restore never runs packages; keep pinned packages with your backups.
- Back up and test against an isolated restore before installing a version that
  changes stored data. Code rollback does not undo data migrations.

## Agent

Optional built-in app at `/agent` with its own account-scoped storage (not the SDK
quota). Projects point at a credential-free Git URL or a Dar Files folder; runs use
the server's existing Codex or Claude Code login. See [Codex setup](codex.md),
[Claude Code setup](claude-code.md) and the
[Agent workspace](spacetimedb/agent-workspace.md). Without a connected agent,
messages can still be saved.

## News

Optional built-in feed reader at `/news` with account-scoped storage, OPML
import/export and Nextcloud News API sync for mobile clients at `/dav/news`. See
[News](news.md).

## Photos

Optional built-in gallery at `/photos` for the pictures and videos you keep in
Files: month-grouped library, favorites, videos, archive, albums, Live Photos,
stacks, duplicates, a trash view, the Locked folder, sharing with people and
links (`/s/…`), the Immich mobile app and an Immich server connector, video conversion for the browser and uploads into `pics/YYYY/MM`.
Media tools come with the Dar package. See [Photos](photos.md).

## Prayer Times

First use asks for a location label, coordinates, IANA timezone and calculation
method; browser location is requested only on a click. No IP lookup or geocoder.
Methods: MWL, ISNA, Egyptian and Karachi; standard or Hanafi Asr; optional
high-latitude rules and per-prayer offsets. Polar events stay unavailable.
Results are estimates; there is no adhan audio, push alert or Hijri inference.
See [calculations](../apps/prayer-times/CALCULATIONS.md) and the
[app platform](spacetimedb/app-platform.md).
