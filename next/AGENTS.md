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
- OIDC/SCIM provider dependencies `oidc-provider@9.13.3`, its internal Koa and
  `@types/oidc-provider@9.13.1` are approved. Layout B, Account settings tabs,
  is selected. See `../docs/spacetimedb/identity-provider.md` for scope.
- Notes and Tasks were requested next. Scope and paired static mocks are in
  `../docs/spacetimedb/notes-tasks.md` and `notes-tasks-mocks.html` beside it.
  Layout A, Focused workspace, is selected for both apps. No additional
  dependencies/workers are proposed.
