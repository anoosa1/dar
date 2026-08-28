#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOST="${DAR_HOST:-127.0.0.1}"
PORT="${DAR_PORT:-8080}"
DATA_ROOT="${DAR_DATA_ROOT:-$ROOT/data}"
BUILD=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --data-root) DATA_ROOT="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    *)
      echo "usage: ./launch.sh [--host HOST] [--port PORT] [--data-root PATH] [--no-build]" >&2
      exit 2
      ;;
  esac
done

cd "$ROOT"

if [[ "$(node --version 2>/dev/null || true)" != v24.* ]]; then
  exec nix develop "path:$ROOT" --command "$0" \
    --host "$HOST" --port "$PORT" --data-root "$DATA_ROOT"
fi

if [[ ! -d node_modules ]]; then
  npm ci
fi

if [[ "$BUILD" == 1 ]]; then
  npm run build
fi

exec env DAR_HOST="$HOST" DAR_PORT="$PORT" DAR_DATA_ROOT="$DATA_ROOT" \
  node dist/server/main.js
