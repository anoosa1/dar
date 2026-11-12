# Dar engineering guide

Dar is a Linux-first personal cloud: Node 24, Cordis, SpacetimeDB, immutable local
blobs and a React/TypeScript client. The application is in `next/`; Nix packaging
and the NixOS module are in `nix/` (the root flake is authoritative). The historical
Go baseline tag is immutable. Do not restore legacy apps.

Read `docs/design.md` before any UI work and keep it current with maintainer
decisions. Read `next/AGENTS.md` for application scope.

- Keep systems simple and type boundaries explicit. Use patch-based edits.
- Preserve unrelated work. Questions are read-only; answer before changing files.
- No dependencies, databases, major abstractions or public contract changes
  without maintainer authorization. No SQLite or external login provider.
- Use Cordis ownership and deterministic disposal for runtime effects.
- Keep account scope and revision checks explicit. No managed symlinks. Never
  expose credentials in logs, Git, Nix expressions or the Nix store.
- All apps share one frame (app rail, top bar with centered search, floating
  sheets, right editing panes), controls and settings; see the design guide.
- No browser prompt/confirm dialogs or inline product styles. Preserve keyboard
  use, focus, responsive layouts, reduced motion and error/recovery states.
- Standards routes live under `/dav`; camelCase APIs under `/api/v1`.
- Tests use isolated temporary roots, identities and browser contexts. Never test
  against live data. Verify UI in a real browser along with the saved result.
- Do not delegate ordinary work or create agent panels unless asked. When asked,
  assign file ownership to avoid collisions.
- Keep docs short, accurate and current. Do not commit build outputs, credentials,
  data, browser profiles or caches.

Verify: `nix develop`; `npm ci --prefix next`; `npm --prefix next run verify`.
Deploy: `nix build .#dar` and `nix/module.nix`; see `docs/deployment.md`.
