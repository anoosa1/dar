#!/usr/bin/env bash
set -euo pipefail
HOST=${HOST:-localhost}
PORT=8080
DB=${DB:-dar.db}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) PORT="$2"; shift 2;;
    --host) HOST="$2"; shift 2;;
    --db)   DB="$2";   shift 2;;
    *)      echo "Usage: ./launch.sh [--host HOST] [--port PORT] [--db DB_PATH]"; exit 1;;
  esac
done

trap 'kill $DAR_PID 2>/dev/null; wait' EXIT INT TERM

echo "Building dar-server..."
nix-shell -p go --command "go build -o dar-server ./cmd/server/"

echo ""
echo "Starting Dar on :$PORT..."
echo "  Home:  http://$HOST:$PORT"
echo "  DB:    $DB"

if [[ -n "${DAR_OIDC_ISSUER:-}" ]]; then
  echo "  OIDC:  $DAR_OIDC_ISSUER"
else
  echo "  OIDC:  not configured (set DAR_OIDC_ISSUER, DAR_OIDC_CLIENT_ID, DAR_OIDC_CLIENT_SECRET)"
fi

echo ""
echo "Press Ctrl+C to stop."
echo ""

exec ./dar-server --port "$PORT" --db "$DB"
