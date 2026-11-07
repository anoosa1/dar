# Dar engineering guide

Dar is a Linux-first personal cloud: Node 24, Cordis, SpacetimeDB, immutable local
blobs and a React/TypeScript client. The active application is in next/. Nix
packaging and the NixOS module are in nix/; the root flake is authoritative.
The historical Go baseline tag remains immutable. Do not restore legacy apps.

Read docs/design.md before any UI work. It is the shared design contract across
all apps; reuse the established interactions and keep it current with maintainer
decisions.

- Keep systems simple and type boundaries explicit. Use patch-based edits.
- Preserve unrelated work. Questions are read-only; answer before changing files.
- No dependencies, databases, major abstractions or public contract changes
  without maintainer authorization. No SQLite or external login provider.
- Use Cordis ownership and deterministic disposal for runtime effects.
- Keep account scope and revision checks explicit. No managed symlinks. Never
  expose credentials in logs, Git, Nix expressions or the Nix store.
- Built-in apps share controls and settings. The selected direction is Material 3
  with Catppuccin Latte/Mocha, centered search and focused right editing panes.
  Make distinct static mocks and wait for selection before a new visual direction.
- No browser prompt/confirm dialogs or inline product styles. Preserve keyboard
  use, focus, responsive layouts, reduced motion and error/recovery states.
- Use existing standards routes under /dav and camelCase APIs under /api/v1.
- Tests use isolated temporary roots, identities and browser contexts. Never test
  against live data. Verify UI in a real browser and resulting persistence.
- Do not delegate ordinary work or create agent panels unless explicitly asked.
  When delegation is requested, assign ownership to avoid file collisions.
- Keep docs and comments current. Do not commit build outputs, credentials, data,
  browser profiles or caches. Read next/AGENTS.md for application-specific scope.

Verification: nix develop; npm ci --prefix next; npm --prefix next run verify.
Deployment: nix build .#dar and the module in nix/module.nix. See docs/deployment.md.
