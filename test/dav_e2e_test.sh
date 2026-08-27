#!/usr/bin/env bash
set -euo pipefail

ROOT=$(mktemp -d /tmp/dar_dav_e2e_XXXXXX)
DAR_PORT=$((RANDOM % 1000 + 21000))
OIDC_PORT=$((DAR_PORT + 1000))
DAR_BIN="$ROOT/dar-server"
OIDC_BIN="$ROOT/dar-test-oidc"
COOKIE="$ROOT/cookies.txt"
BASE="http://localhost:$DAR_PORT/remote.php/dav/files/test"

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
DAR_OIDC_CLIENT_ID=test-client DAR_OIDC_CLIENT_SECRET=test-secret \
DAR_BASE_URL="http://localhost:$DAR_PORT" DAR_DATA_ROOT="$ROOT/data" \
"$DAR_BIN" --port "$DAR_PORT" --db "$ROOT/dar.sqlite" >"$ROOT/dar.log" 2>&1 &
DAR_PID=$!

for _ in $(seq 1 100); do
  curl -fsS "http://localhost:$DAR_PORT/health" >/dev/null 2>&1 && break
  sleep 0.1
done
LOGIN_HTML=$(curl -sS -c "$COOKIE" "http://localhost:$DAR_PORT/login")
AUTH_URL=$(printf '%s' "$LOGIN_HTML" | grep -o 'href="[^"]*/authorize[^"]*"' | head -1 | cut -d'"' -f2)
AUTH_URL=${AUTH_URL//\&amp;/\&}
AUTH_URL=${AUTH_URL//\&#43;/+}
[[ -n "$AUTH_URL" ]]
curl -LsS -b "$COOKIE" -c "$COOKIE" "$AUTH_URL" >/dev/null
CSRF=$(awk '$6 == "dar_csrf" { print $7 }' "$COOKIE")
[[ -n "$CSRF" ]]

[[ $(curl -sS -o /dev/null -w '%{http_code}' -b "$COOKIE" -H "X-Dar-CSRF-Token: $CSRF" -X MKCOL "$BASE/docs") == 201 ]]
[[ $(curl -sS -o /dev/null -w '%{http_code}' -b "$COOKIE" -H "X-Dar-CSRF-Token: $CSRF" -X PUT --data 'hello dav' "$BASE/docs/a.txt") == 201 ]]
curl -fsS -b "$COOKIE" -H "X-Dar-CSRF-Token: $CSRF" -X PROPFIND -H 'Depth: 1' "$BASE/docs" | grep -q 'a.txt'
[[ $(curl -fsS -b "$COOKIE" "$BASE/docs/a.txt") == 'hello dav' ]]
[[ $(curl -sS -o /dev/null -w '%{http_code}' -b "$COOKIE" -H "X-Dar-CSRF-Token: $CSRF" -X DELETE "$BASE/docs/a.txt") == 204 ]]
[[ $(curl -sS -o /dev/null -w '%{http_code}' -X PROPFIND "$BASE/docs") == 401 ]]
echo "in-process WebDAV e2e: PASS"
