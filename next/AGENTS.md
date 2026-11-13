# Dar application

Read `../docs/design.md` before UI work. Every app follows the same frame and
interaction rules; app-specific code must not drift from that contract.

## Scope and authority

- Stack: TypeScript, React, SpacetimeDB, Node 24, Cordis and immutable local blobs.
  This architecture (November 5–6, 2026) replaces the earlier SQLite/ClojureScript
  design for `next/`. See `../docs/spacetimedb/architecture.md`.
- No SQLite, external identity provider, offline editing or end-to-end encryption.
- Approved dependencies beyond the base stack: `ical.js`, `saxes`,
  `oidc-provider@9.13.3` (with its internal Koa) and `@types/oidc-provider@9.13.1`;
  `openid-client` for tests only.
- Do not treat passing primitive tests as completion; verify end-to-end behavior.

## Selected designs

Current direction: unified look B (`../docs/spacetimedb/unified-look-mock-b.html`,
November 11) with Twilight Afterglow/Nightfall. It supersedes the earlier shared
header, app launcher and lower-sidebar settings. Earlier selections still define
app content: Files Focused explorer (C), Contacts/Calendar and Notes/Tasks
Focused workspace (A), identity Account settings tabs (B), creation/appearance
Focused pane (A), sign-in/app-password mock A, Prayer Times layout B and the
Agent T3 parity A, Photos mock B and spotlight search mock A (November 13; see
`../docs/photos.md`), and News mock A (`../docs/spacetimedb/news-reader-mock-a.html`,
November 12). App settings, Dar settings and Prayer Times follow settings
sheet B (`../docs/spacetimedb/settings-revamp-mock-b.html`, November 12), which
supersedes the settings modal and explicit settings save. Files, Contacts,
Calendar, Notes, Tasks and shared menus follow the apps revamp
(`../docs/spacetimedb/apps-revamp-mock.html`, November 12). Details are in the
design guide.

Applying a selected pattern needs no new mock; a new visual direction does.
