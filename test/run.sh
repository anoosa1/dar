#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-fast}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "$(node --version 2>/dev/null || true)" != v24.* ]]; then
  exec nix develop "path:$ROOT" --command "$0" "$MODE"
fi

if [[ ! -d node_modules ]]; then
  npm ci
fi

fast() {
  npm run compile
}

pr() {
  npm run check
  node --test test/platform.test.mjs
}

nightly() {
  pr
  # Protocol fuzzing and lifecycle stress are added here as their services land.
}

case "$MODE" in
  fast) fast ;;
  pr) pr ;;
  nightly) nightly ;;
  *) echo "usage: ./test/run.sh {fast|pr|nightly}" >&2; exit 2 ;;
esac
