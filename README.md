# Dar

Dar is a Linux-first, self-hosted personal cloud being rebuilt in
ClojureScript on Node 24. One process hosts the Cordis service graph, the
standards-first HTTP APIs, and a Reagent/re-frame SPA.

The rewrite starts from fresh storage. The previous Go implementation is kept
as the immutable Git tag `dar-go-baseline-2026-08-27`; it is not shipped or
loaded by this branch.

## Development

Enter the pinned Linux development environment and install the exact npm graph:

```bash
nix develop
npm ci
```

Build and test everything currently implemented:

```bash
./test/run.sh fast
```

Run an isolated local instance:

```bash
DAR_DATA_ROOT="$(mktemp -d)" ./launch.sh
```

The server listens on `http://127.0.0.1:8080` by default. Production builds use
Node only; Java and Clojure are build-time tools for `shadow-cljs`.

## Repository map

- `src/main/dar/server` — process composition and HTTP delivery.
- `src/main/dar/platform` — data-root, SQLite, security, and shared services.
- `src/main/dar/client` — Reagent/re-frame SPA and Sanctuary design system.
- `src/main/dar/shared` — shared schemas and wire contracts.
- `src/test` — ClojureScript unit and service tests.
- `test` — process, browser, protocol, and release verification.
- `docs/rewrite` — binding parity ledger and rewrite contracts.

See [the parity ledger](docs/rewrite/parity-ledger.md) for the complete target
and [the selected design](docs/rewrite/design.md) for the SPA direction.
