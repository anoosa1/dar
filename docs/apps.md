# Installable Dar apps

Dar accepts trusted administrator-installed packages using SDK API v1. Prayer
Times is the first example; it uses an original, dependency-free calculation
library and the selected week-list/daily-pane layout B. External packages are
separate from OIDC registered applications. Existing built-in apps keep their
current routes and data models.

## NixOS

```nix
services.dar = {
  enable = true;
  origin = "https://cloud.example.com";
  apps = [ dar.packages.${pkgs.stdenv.hostPlatform.system}.prayer-times ];
};
```

Build with `nix build .#dar .#prayer-times`. Each app output must be a complete
prebuilt directory with a `dar-app.json`. The generated read-only registry is
authoritative. Change `services.dar.apps` to install/remove apps, then rebuild
and restart the service. No app is enabled by default.

The runtime validates package IDs, API major, file inventory, size and digest,
and rejects symlinks within packages. Assets are content-addressed local files.
The SDK uses one shared React instance and Dar controls. Node hooks use owned
Cordis effects and app-prefixed routes. They are trusted code, not OS-isolated.

## Other hosts and development

From the repository in `nix develop`:

```sh
node next/sdk/build.mjs apps/prayer-times
# Stop the instance before changing its app configuration.
DAR_ROOT=/path/to/private/root node next/scripts/operator.ts apps-install apps/prayer-times/dist
DAR_ROOT=/path/to/private/root node next/scripts/operator.ts apps-list
```

The operator copies validated artifacts into an immutable-by-convention package
directory and atomically replaces the private `apps.json` registry. It holds the
instance lock, so running the command while Dar is active fails. Restart to apply.
Packaged installations use `dar-operator` with the same commands:

```text
apps-install <built-package-directory>
apps-list
apps-disable <app-id>
apps-enable <app-id>
apps-uninstall <app-id>
```

Set `DAR_APPS_FILE` to an explicit JSON array of `{path, digest?, enabled?}` for
read-only host configuration. Absolute package roots may resolve through host
aliases, including Nix paths, but managed package files may not be symlinks.
`DAR_DISABLE_APPS=1` is recovery mode. Restart after changing either variable.
An invalid individual package or failed Node hook leaves core apps usable and
appears in Administration → Apps. A malformed registry starts without
external apps; correct host configuration and restart.

These commands affect the default private registry only. When `DAR_APPS_FILE` is
set (including NixOS), change that configured registry instead. No browser code
upload, marketplace, automatic update, dependency resolver or hot reload is shipped.

## Data, upgrades and restore

Administrators can also disable/enable Notes, Tasks and host-enabled external apps
in Administration → Apps, or reach it through the launcher's Manage apps entry.
Changes apply immediately and persist through restart and backup/restore. Files,
Contacts and Calendar cannot be disabled. Disabled apps leave the launcher and
global search; their app APIs are unavailable and external Node effects are
disposed. Data is retained. Calendar's underlying standards routes remain available
as part of the essential Calendar service, including task synchronization.
Host configuration remains authoritative for installed packages: a package disabled
or removed on the host cannot be enabled in the browser. This additional setting
can only restrict host-enabled packages; installing/upgrading still needs a restart.

Every record is scoped to a live account and app, with exact revision checks,
64 KiB/record, 1 MiB/account/app and 128-record limits. Up to 50 records are returned
per page. App passwords do not authenticate these APIs. No app can add tables
to the core SpacetimeDB module through this SDK. Core/domain integrations beyond
regional timezone reads are future explicit SDK additions.

Back up before installing an app version that changes payload schemas. Test the
upgrade against an isolated restore. Keep the old code package. Code rollback
does not undo data migrations: use only a version that reads current data, or
restore the corresponding full backup during maintenance.

Disable and uninstall retain app data; reinstalling the same app ID reconnects
that data. Uninstall retains old code copies as rollback artifacts. Automated
code reclamation and per-app data purge/export commands are not shipped yet;
there is no implicit destructive cleanup. Account deletion removes that account's
app data while following Dar's existing account/domain retention policy.

Backup format v5 contains all app records, including disabled/orphaned data, and
the active package inventory/digests. V1–v4 backups remain readable. Restore does
not fetch or execute packages, and the restored active inventory is empty until
the configured packages are reconciled at startup. Keep pinned packages/flake
locks alongside backups; executable code is not included in backup contents.

## Prayer Times

First use requires an explicit location label, coordinates, IANA timezone and
calculation method. Browser location is optional and requested only on a click;
denial leaves manual entry available. No IP lookup or geocoder is used. The saved
location remains private; changing the account's regional default does not move it.

V1 supports MWL, ISNA, Egyptian and Karachi angle conventions, standard/Hanafi Asr,
optional night-portion bounds and per-event minute offsets. It shows sunrise as a
solar event. Missing polar events remain unavailable. Results are estimates, not
mosque-specific prayer/iqamah announcements. There is no adhan audio, push delivery,
Hijri-calendar inference or automatic Ramadan interval adjustment.

See [calculation equations, limits and sources](../apps/prayer-times/CALCULATIONS.md),
[SDK author guide](../next/sdk/README.md), and [architecture](spacetimedb/app-platform.md).
