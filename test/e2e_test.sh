#!/usr/bin/env bash
set -euo pipefail

ROOT=$(mktemp -d /tmp/dar_e2e_XXXXXX)
DAR_PORT=$((RANDOM % 1000 + 19000))
OIDC_PORT=$((DAR_PORT + 1000))
DAR_BIN="$ROOT/dar-server"
OIDC_BIN="$ROOT/dar-test-oidc"
DB="$ROOT/dar.sqlite"
COOKIE="$ROOT/cookies.txt"

cleanup() {
  [[ -n "${DAR_PID:-}" ]] && kill "$DAR_PID" 2>/dev/null || true
  [[ -n "${OIDC_PID:-}" ]] && kill "$OIDC_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$ROOT"
}
trap cleanup EXIT

nix-shell -p go --command "go build -o $DAR_BIN ./cmd/server && go build -o $OIDC_BIN ./cmd/test-oidc"

"$OIDC_BIN" --port "$OIDC_PORT" --host localhost >"$ROOT/oidc.log" 2>&1 &
OIDC_PID=$!
DAR_OIDC_ISSUER="http://localhost:$OIDC_PORT" \
DAR_OIDC_CLIENT_ID=test-client \
DAR_OIDC_CLIENT_SECRET=test-secret \
DAR_OIDC_DISPLAY_NAME=TestOIDC \
DAR_BASE_URL="http://localhost:$DAR_PORT" \
DAR_DATA_ROOT="$ROOT/data" \
"$DAR_BIN" --port "$DAR_PORT" --db "$DB" >"$ROOT/dar.log" 2>&1 &
DAR_PID=$!

for _ in $(seq 1 100); do
  curl -fsS "http://localhost:$DAR_PORT/health" >/dev/null 2>&1 && break
  sleep 0.1
done

[[ $(curl -sS "http://localhost:$DAR_PORT/health") == '{"status":"ok"}' ]]
LOGIN_HTML=$(curl -sS -c "$COOKIE" "http://localhost:$DAR_PORT/login")
AUTH_URL=$(printf '%s' "$LOGIN_HTML" | grep -o 'href="[^"]*/authorize[^"]*"' | head -1 | cut -d'"' -f2)
AUTH_URL=${AUTH_URL//\&amp;/\&}
AUTH_URL=${AUTH_URL//\&#43;/+}
[[ -n "$AUTH_URL" ]]
curl -LsS -b "$COOKIE" -c "$COOKIE" "$AUTH_URL" >/dev/null
curl -fsS -b "$COOKIE" "http://localhost:$DAR_PORT/api/me" | grep -q 'test@dar.local'
curl -fsS -b "$COOKIE" "http://localhost:$DAR_PORT/home" | grep -q 'id="dar-app-menu-button"'
curl -fsS -b "$COOKIE" "http://localhost:$DAR_PORT/home" | grep -q 'data-app-label="Files"'
curl -fsS -b "$COOKIE" "http://localhost:$DAR_PORT/api/todo/lists" | grep -q 'lists'
[[ $(curl -sS -o /dev/null -w '%{http_code}' "http://localhost:$DAR_PORT/api/todo/lists") == 401 ]]

kill -TERM "$DAR_PID"
wait "$DAR_PID"
unset DAR_PID
echo "single-process e2e: PASS"
