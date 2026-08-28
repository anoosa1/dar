# Dar engineering guide

Dar is a Linux-first self-hosted personal cloud written in ClojureScript. One
Node 24 process hosts a Cordis service graph, one `node:sqlite` database,
portable user files, standards endpoints, and a Reagent/re-frame SPA.

The Go baseline is immutable at `dar-go-baseline-2026-08-27`. Do not restore a
parallel Go implementation or add compatibility paths. The binding rewrite
contract lives in `docs/rewrite`.

## Product and architecture

- Favor ambitious products and simple systems. Do not preserve accidental
  complexity or introduce machinery for hypothetical needs.
- Node 24 is the only production runtime. Java/Clojure exist only for builds.
- Keep one process, one SQLite database, and one data root.
- Use Cordis contexts for services, apps, contributions, workers, and cleanup.
- Every effect must have an owning context and a deterministic disposer.
- Built-in apps use the same contribution boundaries as plugins where practical.
- Third-party plugins are trusted operator-installed prebuilt ESM, not sandboxes.
- Fresh storage only. Refuse unknown non-empty roots and unsupported markers.
- Portable VCF, ICS/VTODO, Markdown, Maildir, and ordinary files are
  authoritative; SQLite owns metadata, indexes, accounts, and operational state.
- Managed storage never creates, follows, indexes, or serves symlinks.

Architectural choices still belong to the maintainer. Ask before adding a
dependency, database, major abstraction, worker, or changing a public contract.

## Approved dependency boundary

The pinned direct dependencies are the ones in `package.json` and `deps.edn`:
ClojureScript, shadow-cljs, Reagent, re-frame, Reitit, Malli, Cordis and its
official server/logger plugins, React, openid-client, ical.js, saxes, feedsmith,
postal-mime, Nodemailer, sanitize-html, Undici, Sharp, Tailwind, and Playwright.

Do not add an ORM, a second database, another application server, another UI
framework, native CGo code, or CDN/runtime-hosted assets.

## Contracts

- Browser APIs live under `/api/v1` and use camelCase JSON.
- IDs are opaque strings; timestamps are ISO-8601 UTC; local dates are
  `YYYY-MM-DD`; timezones are IANA names.
- Collections are `{ "items": [...], "nextCursor": "..." }`.
- Errors use the documented envelope and stable codes in
  `dar.shared.contracts`.
- Mutable resources use revision-backed ETags and require `If-Match`.
- DAV paths are standards-first under `/dav`; do not restore `/remote.php` or
  Nextcloud-shaped News/Notes APIs.
- Browser sessions are secure HTTP-only cookies with OIDC PKCE, CSRF, origin
  validation, request limits, and trusted-proxy rules.
- Protocol clients use scoped Basic app passwords.

## Hit every applicable surface

For each app behavior, consider the SPA, `/api/v1`, DAV or mail/feed protocols,
search, activity, notifications, widgets, preferences, assistant tools,
lifecycle, reverse operation, docs, and tests. A page or handler alone is not a
complete feature.

Assistant tools have Malli object schemas, explicit user scope, stable errors,
and one effect classification: `read`, `safeWrite`, `external`, or
`destructive`.

## Frontend

The selected design direction is Sanctuary; its rules are in
`docs/rewrite/design.md`.

- Use Reagent and re-frame with Reitit routing.
- Keep CSS, fonts, icons, and bundles self-hosted.
- Do not use browser-native prompt/confirm dialogs or inline styles.
- Preserve keyboard navigation, focus restoration, accessible names, live
  regions, loading/empty/error/conflict/offline states, and mobile behavior.
- Before a new non-trivial visual direction, create distinct static mocks and
  wait for maintainer selection.
- After UI changes, launch an isolated instance and verify the real browser,
  console, and resulting server/persistence behavior.

## Persistence and security

- Access SQLite directly through `node:sqlite`; use WAL and foreign keys.
- Add explicit numbered migrations. Keep transactions synchronous and short.
- Send writes through the one writer queue; never await network or subprocess
  work while holding it.
- Use prepared statements for user-controlled values.
- Keep user scope explicit in every query, path, event, and contribution.
- Use atomic temporary-file, fsync, and rename operations for portable data.
- Journal filesystem/database boundary mutations and reconcile after crashes.
- Validate managed paths centrally and fail closed on every symlink component.
- Keep credentials in the AES-GCM keyring and never log secrets.
- Outbound HTTP must classify addresses and revalidate DNS and every redirect.
- Track child processes individually; apply capacity/output limits and dispose
  them deterministically. Never kill by broad process-name matching.

## Source layout

- `src/main/dar/server` — startup, HTTP, and process composition.
- `src/main/dar/platform` — shared persistence and security services.
- `src/main/dar/client` — SPA shell, state, apps, and design system.
- `src/main/dar/shared` — schemas and cross-runtime contracts.
- `src/test` — ClojureScript unit/service tests.
- `test` — process, browser, protocol, fuzz, and release tests.
- `docs/rewrite` — parity ledger and binding specifications.

## Development and verification

```bash
nix develop
npm ci
./test/run.sh fast
./test/run.sh pr
./test/run.sh nightly
```

Use isolated temporary databases, data roots, plugin repositories, Codex homes,
and browser profiles. Never test against `data/` or another live instance.

Start with the smallest proof. Backend changes need focused tests. Concurrency
and lifecycle tests must prove cancellation and cleanup without arbitrary long
sleeps. UI changes also need an integrated browser pass. Do not report success
while required verification fails; separate product failures from infrastructure
failures.

## Coding preferences

- Keep things simple and type boundaries explicit.
- Prefer concrete functions until an interface represents a real boundary.
- Comments explain ownership, invariants, or surprising behavior and stay in
  sync with code.
- Use patch-based edits; avoid broad textual rewrites.
- Preserve unrelated local work and do not commit generated bundles, databases,
  credentials, browser profiles, caches, or test artifacts.
