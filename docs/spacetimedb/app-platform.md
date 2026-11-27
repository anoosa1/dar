# Installed app platform

Delivered November 8, 2026. Administrators install trusted, prebuilt app packages;
authors build against the versioned SDK in their own repository without editing
Dar. `apps/sample-app` is the minimal SDK example (Prayer Times moved into the
built-in Rust client at `/prayers`; see [Prayer Times](../prayer-times.md)).
Operating instructions are in [apps](../apps.md); the author API
is in the [SDK guide](../../sdk/README.md).

Installed browser and Node code is trusted and runs inside Dar. Manifest
permissions prevent mistakes; they are not a sandbox. Nextcloud apps are not
compatible.

## Package

An immutable directory with `dar-app.json`, browser JS/CSS/assets and optionally a
bundled Node ESM entry, built on the author's machine or in Nix. Validation rejects
bad or reserved IDs, unsupported API majors, escaping paths, symlinks, undeclared
files and malformed manifests before any code runs.

```json
{"id": "org.dar.sample-app", "name": "Sample App", "version": "0.1.0",
 "apiVersion": 1, "license": "MIT", "client": "client.js", "styles": ["client.css"],
 "files": ["client.js", "client.css"],
 "permissions": ["ownUserData:read", "ownUserData:write"]}
```

- Browser entry: `mount(element, host) -> dispose`. The host supplies scoped data
  requests, the regional timezone, theme and shared components (`AppWorkspace`,
  dialogs, controls). `/sdk.js` provides the single shared React runtime.
- Optional Node entry: `activate(context)` returning `{handle(request)}`, with
  scoped data helpers, an abort signal and Cordis-owned effects; no raw database
  credentials or storage paths. A failing app is marked unavailable without
  breaking core startup, but in-process code can still block or crash Node.

## Routes

| Route | Purpose |
| --- | --- |
| `GET /api/v1/apps` | Enabled apps the user can open |
| `/apps/{appId}` | App page inside the shared frame |
| `/appAssets/{appId}/{digest}/…` | Declared, content-addressed assets |
| `/api/v1/apps/{appId}/userData/{key}` | Own-user records with conditional writes |
| `/api/v1/apps/{appId}/api/…` | Optional app handlers after session/CSRF checks |
| `/api/v1/admin/installedApps` | Administrator inventory, errors and toggles |

App and user identity come from the route and the session, never from request
bodies. App passwords do not authorize these APIs.

## Storage

Core-owned private records keyed by `(appId, ownerId, key)` with a JSON payload,
schema version, revision and timestamp. Get, put (create-only at revision 0,
otherwise exact revision), delete and paging; quotas of 64 KiB per record and
1 MiB / 128 records per account and app are checked transactionally. No SQL,
dynamic tables or Files/Calendar access in v1. Apps validate their own payloads
and make migrations restartable.

## Lifecycle

The host package set (NixOS `services.dar.apps`, `DAR_APPS_FILE` or the operator
registry) is authoritative and changes on restart. Administration can further
disable host-enabled apps immediately. Disable and uninstall keep data. Backups
include app records and the package inventory, never code, and restore never runs
packages. `DAR_DISABLE_APPS=1` starts without installed apps. Per-app purge/export,
a marketplace, hot reload and process isolation are not provided.

## Prayer Times

Moved from an SDK example to a built-in Rust workspace at `/prayers` with
`Today` Solar Horizon view, `Week` + Day Inspector, `Month` timetable and
`.ics`/`.csv`/JSON export, `Qibla` compass with Sun azimuth cue, saved locations,
icon-based daily prayer tracking (`Not prayed yet`, `Prayed on time`, `Prayed late`),
and save-as-you-go `SettingsSheet`. See [Prayer Times](../prayer-times.md) and the
[redesign mock](prayer-times-redesign-mock.html).
