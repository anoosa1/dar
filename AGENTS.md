# Dar engineering guide

Dar is a Linux-first personal cloud in Rust: a SpacetimeDB module (`rust/module`), the
runtime (`rust/runtime`, with `rust/oidc` and `rust/dav`), a Leptos client (`rust/client`)
and immutable local blobs. Beside it: the Go chat bridge (`bridge/`), the installed-app SDK
(`sdk/`, JavaScript by design), the acceptance suite (`test/`), Nix packaging and the NixOS
module (`nix/`; the root flake is authoritative). The historical Go baseline tag is
immutable. Do not restore legacy apps or the TypeScript stack (last in commit `799b524`).

Read `docs/design.md` before any UI work and keep it current with maintainer decisions.
Read `rust/PORTING.md` for the stack's history, its Leptos pitfalls and instance moves.

- Keep systems simple and type boundaries explicit. Use patch-based edits.
- Preserve unrelated work. Questions are read-only; answer before changing files.
- No dependencies (crates, npm packages), databases, major abstractions or public contract
  changes without maintainer authorization. No SQLite, external identity provider, offline
  editing or end-to-end encryption.
- Runtime effects have an owner and are disposed deterministically (tasks, workers, Leptos
  effects); deferred UI code uses `try_` accessors, never a disposed signal.
- Keep account scope and revision checks explicit. No managed symlinks. Never expose
  credentials in logs, Git, Nix expressions or the Nix store.
- The module's tables, reducers and procedures keep their names and arguments: existing
  databases upgrade in place by publishing, and the client and tests depend on them.
- All apps share one frame (app rail, top bar with centered search, floating sheets, right
  editing panes), controls and settings; see the design guide. Markup and class names follow
  the stylesheet in `rust/client/styles` (cascade order in `order.txt`).
- No browser prompt/confirm dialogs or inline product styles. Preserve keyboard use, focus,
  responsive layouts, reduced motion and error/recovery states.
- Standards routes live under `/dav`; camelCase APIs under `/api/v1`.
- Tests use isolated temporary roots, identities and browser contexts. Never test against
  live data. Verify UI in a real browser along with the saved result. Do not treat passing
  unit tests as completion; verify end-to-end behavior.
- Do not delegate ordinary work or create agent panels unless asked. When asked, assign file
  ownership to avoid collisions.
- Keep docs short, accurate and current. Do not commit build outputs, credentials, data,
  browser profiles or caches.

Selected designs: unified look B with Twilight Afterglow/Nightfall; Files Focused explorer
(C); Contacts/Calendar and Notes/Tasks Focused workspace (A); identity Account settings tabs
(B); settings sheet B; Photos mock B; spotlight search A; News A; Books A; Git B; Chat A with
account tabs and Discord B with the voice dock; Agent T3 parity A. Mocks and details are in
`docs/`. Applying a selected pattern needs no new mock; a new visual direction does.

Verify: `nix develop`; `npm ci`; `node rust/verify.mjs` (the whole suite, on a throwaway
instance); `cargo test` in `rust/` (and `--lib` in `rust/module`, `rust/client`).
Deploy: `nix build .#dar` and `nix/module.nix`; see `docs/deployment.md`.
