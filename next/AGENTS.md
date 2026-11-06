# SpacetimeDB application

The maintainer authorized a fresh SpacetimeDB architecture and implementation of
native auth and Files, extended to Contacts and Calendar on November 6, 2026.
Read `../docs/spacetimedb/architecture.md` for the
agreed scope and acceptance requirements. This explicitly supersedes the parent
guide's SQLite, ClojureScript backend, external OIDC login, and authoritative
portable-file requirements for this directory only. Do not alter existing work.

- Use TypeScript, React, SpacetimeDB, Node 24, Cordis, and immutable local blobs.
- No SQLite, external identity provider, unrelated app UI, offline editing, or E2EE.
- Preserve the parent's security, accessibility, isolation, cleanup, and focused
  verification rules where applicable.
- Mock C, Focused explorer, is selected: compact Nextcloud-style Material 3,
  centered persistent search, desktop details, and focused phone panels. Continue
  that direction; review new static mocks only for a new visual direction.
- Do not treat passing primitive tests as completion of auth or Files.
- Contacts and Calendar mock A, Focused workspace, was selected on November 6:
  Contacts list/details and Calendar time grid, with the existing centered search.
- OIDC/SCIM provider work was requested next. The concrete plan is
  `../docs/spacetimedb/identity-provider.md`; its new provider dependency and
  identity/consent UI selection are pending explicit maintainer approval.
