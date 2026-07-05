#!/usr/bin/env bash
set -euo pipefail
BIN=./zig-out/bin/dar
OAUTH=./zig-out/bin/dar-oauth-test
DAV=./zig-out/bin/dar-files
HOST=${HOST:-astra}
DAR_PORT=8081
DAV_PORT=8082
OAUTH_PORT=9001
DB=${DB:-dar.db}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port) DAR_PORT="$2"; shift 2;;
    --host) HOST="$2"; shift 2;;
    --db)   DB="$2"; shift 2;;
    *)      echo "Usage: ./launch.sh [--host HOST] [--port PORT] [--db DB_PATH]"; exit 1;;
  esac
done

trap 'kill $OAUTH_PID 2>/dev/null; kill $DAV_PID 2>/dev/null; kill $DAR_PID 2>/dev/null; wait' EXIT INT TERM

echo "Starting dummy OAuth on :$OAUTH_PORT..."
$OAUTH --host "$HOST" --port $OAUTH_PORT &>/dev/null &
OAUTH_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do
  curl -s "http://$HOST:$OAUTH_PORT/.well-known/openid-configuration" >/dev/null 2>&1 && break
  sleep 0.3
done
echo "  OAuth ready"

echo "Starting Files DAV on :$DAV_PORT..."
$DAV --port $DAV_PORT --root data/users &>/dev/null &
DAV_PID=$!
for i in 1 2 3 4 5 6 7 8 9 10; do
  curl -s "http://$HOST:$DAV_PORT/health" >/dev/null 2>&1 && break
  sleep 0.3
done
echo "  Files DAV ready"

echo "Starting Dar on :$DAR_PORT..."
echo "  OIDC:  http://$HOST:$OAUTH_PORT"
echo "  DB:    $DB"
echo "  Home:  http://$HOST:$DAR_PORT"
echo "  DAV:   http://$HOST:$DAV_PORT"
echo ""
echo "Press Ctrl+C to stop."
echo ""

env \
  DAR_OIDC_ISSUER="http://$HOST:$OAUTH_PORT" \
  DAR_OIDC_CLIENT_ID="test-client" \
  DAR_OIDC_CLIENT_SECRET=*** \
  DAR_OIDC_DISPLAY_NAME="TestOIDC" \
  DAR_BASE_URL="http://$HOST:$DAR_PORT" \
  $BIN --port $DAR_PORT --db "$DB" &
DAR_PID=$!

wait $DAR_PID
