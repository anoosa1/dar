# SpacetimeDB application

The maintainer authorized a fresh SpacetimeDB architecture and implementation of
native auth and Files only. Read `../docs/spacetimedb/architecture.md` for the
agreed scope and acceptance requirements. This explicitly supersedes the parent
guide's SQLite, ClojureScript backend, external OIDC login, and authoritative
portable-file requirements for this directory only. Do not alter existing work.

- Use TypeScript, React, SpacetimeDB, Node 24, Cordis, and immutable local blobs.
- No SQLite, external identity provider, other app UI, offline editing, or E2EE.
- Preserve the parent's security, accessibility, isolation, cleanup, and focused
  verification rules where applicable.
- Mock C, Focused explorer, is selected: compact Nextcloud-style Material 3,
  centered persistent search, desktop details, and focused phone panels. Continue
  that direction; review new static mocks only for a new visual direction.
- Do not treat passing primitive tests as completion of auth or Files.
