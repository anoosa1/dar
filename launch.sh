#!/usr/bin/env bash
set -euo pipefail
HOST=${HOST:-astra}
PORT=8080
DB=${DB:-dar.db}
OIDC_PORT=9001

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)     PORT="$2";      shift 2;;
    --oidc-port) OIDC_PORT="$2"; shift 2;;
    --host)     HOST="$2";      shift 2;;
    --db)       DB="$2";        shift 2;;
    --no-build) NO_BUILD=1;     shift;;
    *) echo "Usage: ./launch.sh [--host HOST] [--port PORT] [--oidc-port PORT] [--db DB_PATH] [--no-build]"; exit 1;;
  esac
done

BIN_SERVER=./dar-server
BIN_OIDC=./dar-test-oidc
OIDC_BASE="http://$HOST:$OIDC_PORT"

trap 'kill $OIDC_PID 2>/dev/null; kill $DAR_PID 2>/dev/null; wait' EXIT INT TERM

if [[ -z "${NO_BUILD:-}" ]]; then
  echo "Building..."
  nix-shell -p go --command "go build -o $BIN_SERVER ./cmd/server/"
  nix-shell -p go --command "go build -o $BIN_OIDC ./cmd/test-oidc/"
  echo "  ok"
fi

echo ""
echo "Starting dummy OIDC on :$OIDC_PORT..."
$BIN_OIDC --port "$OIDC_PORT" --host "$HOST" &
OIDC_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$OIDC_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  OIDC ready ($OIDC_BASE)"

echo ""
echo "Starting Dar on :$PORT..."
echo "  Home:  http://$HOST:$PORT"
echo "  OIDC:  $OIDC_BASE"
echo "  DB:    $DB"
echo ""
echo "Credentials: any email + password work (dummy OIDC auto-approves)"
echo ""
echo "Press Ctrl+C to stop."
echo ""

env \
  DAR_OIDC_ISSUER="$OIDC_BASE" \
  DAR_OIDC_CLIENT_ID="test-client" \
  DAR_OIDC_CLIENT_SECRET="test-secret" \
  DAR_OIDC_DISPLAY_NAME="TestOIDC" \
  DAR_BASE_URL="http://$HOST:$PORT" \
  $BIN_SERVER --port "$PORT" --db "$DB" &
DAR_PID=$!

wait $DAR_PID
