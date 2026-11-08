# SpacetimeDB application

Read ../docs/design.md before UI work. All apps follow the same interaction and
layout rules; app-specific implementations must not drift from that contract.

- Sign-in branding and app-password permissions use mock A in
  ../docs/spacetimedb/signin-app-password-mocks.html, selected November 7.


- The November 7 follow-up selects creation/appearance mock A, Focused pane.
  Creation/editing uses right panes; Files adds a New menu and text/Markdown
  editor. The November 8 Twilight mock supersedes Catppuccin: Afterglow (light) and
  Nightfall (dark), with plum/violet surfaces and orchid/rose actions. Keep the
  stored latte/mocha identifiers compatible with existing saved preferences.
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
- The shared UX revision selects B, Material 3, in
  `../docs/spacetimedb/ux-consistency-mocks.html`. Use one shared header, themed
  launcher/account menu, controls, dialog motion, lower-sidebar app settings and
  Personal/Administration global settings across all five apps and settings.
  This supersedes earlier per-app shell/settings variations; no further mock
  approval is needed for implementing the selected shared direction.
- Calendar's November 7 refinement uses Month by default, one date/today control
  beside the period arrows, per-calendar action menus, and compact paged calendar
  navigation. Import/export and cancelled-event visibility belong in Calendar
  settings; timezone belongs in Personal → Regional settings. Files storage usage
  stays directly above its sidebar settings entry.
