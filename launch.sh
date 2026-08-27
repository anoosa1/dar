#!/usr/bin/env bash
set -euo pipefail

HOST=${HOST:-localhost}
PORT=${PORT:-8080}
DB=${DB:-dar.db}
OIDC_PORT=${OIDC_PORT:-9001}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --db) DB="$2"; shift 2 ;;
    --oidc-port) OIDC_PORT="$2"; shift 2 ;;
    --no-build) NO_BUILD=1; shift ;;
    *) echo "Usage: ./launch.sh [--host HOST] [--port PORT] [--db PATH] [--oidc-port PORT] [--no-build]"; exit 1 ;;
  esac
done

if [[ -z "${DAR_EMAIL_ENCRYPTION_KEYS:-}" ]]; then
  EMAIL_KEY_FILE=${DAR_EMAIL_ENCRYPTION_KEY_FILE:-"${DB}.email-encryption-key"}
  if [[ -L "$EMAIL_KEY_FILE" || ( -e "$EMAIL_KEY_FILE" && ! -f "$EMAIL_KEY_FILE" ) ]]; then
    echo "Email encryption key path must be a regular file: $EMAIL_KEY_FILE" >&2
    exit 1
  fi
  if [[ ! -e "$EMAIL_KEY_FILE" ]]; then
    EMAIL_KEY_MATERIAL=$(head -c 32 /dev/urandom | base64)
    if ! (umask 077; set -o noclobber; printf 'local:%s\n' "$EMAIL_KEY_MATERIAL" > "$EMAIL_KEY_FILE") 2>/dev/null; then
      [[ -f "$EMAIL_KEY_FILE" && ! -L "$EMAIL_KEY_FILE" ]] || {
        echo "Could not create email encryption key file: $EMAIL_KEY_FILE" >&2
        exit 1
      }
    fi
    echo "Created email encryption key in $EMAIL_KEY_FILE"
  fi
  chmod 600 "$EMAIL_KEY_FILE"
  DAR_EMAIL_ENCRYPTION_KEYS=$(<"$EMAIL_KEY_FILE")
  if [[ -z "$DAR_EMAIL_ENCRYPTION_KEYS" ]]; then
    echo "Email encryption key file is empty: $EMAIL_KEY_FILE" >&2
    exit 1
  fi
fi

if [[ -n "${DAR_CODEX_PATH:-}" ]]; then
  CODEX_EXECUTABLE=$DAR_CODEX_PATH
else
  CODEX_EXECUTABLE=$(command -v codex || true)
fi
if [[ -z "$CODEX_EXECUTABLE" || ! -x "$CODEX_EXECUTABLE" ]]; then
  echo "Codex CLI is unavailable. Install Codex CLI 0.144.1 or newer or set DAR_CODEX_PATH to an executable." >&2
  exit 1
fi
DAR_CODEX_PATH=$CODEX_EXECUTABLE

BIN_SERVER=./dar-server
BIN_OIDC=./dar-test-oidc
OIDC_BASE="http://$HOST:$OIDC_PORT"

cleanup() {
  [[ -n "${DAR_PID:-}" ]] && kill "$DAR_PID" 2>/dev/null || true
  [[ -n "${OIDC_PID:-}" ]] && kill "$OIDC_PID" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

wait_for_health() {
  local name=$1
  local pid=$2
  local url=$3

  # Check the process before its health endpoint. Otherwise an old process
  # already listening on the requested port can make startup look successful.
  for _ in $(seq 1 50); do
    if ! kill -0 "$pid" 2>/dev/null; then
      wait "$pid" 2>/dev/null || true
      echo "$name failed to start. Check whether its port is already in use." >&2
      return 1
    fi
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done

  echo "Timed out waiting for $name at $url" >&2
  return 1
}

if [[ -z "${NO_BUILD:-}" ]]; then
  echo "Building Dar..."
  nix-shell -p go --command "go build -o $BIN_SERVER ./cmd/server/"
  nix-shell -p go --command "go build -o $BIN_OIDC ./cmd/test-oidc/"
fi

echo "Starting test OIDC on :$OIDC_PORT..."
"$BIN_OIDC" --port "$OIDC_PORT" --host "$HOST" &
OIDC_PID=$!
sleep 0.1
wait_for_health "Test OIDC" "$OIDC_PID" "$OIDC_BASE/health"

echo "Starting Dar on http://$HOST:$PORT"
env \
  DAR_OIDC_ISSUER="$OIDC_BASE" \
  DAR_OIDC_CLIENT_ID="test-client" \
  DAR_OIDC_CLIENT_SECRET="test-secret" \
  DAR_OIDC_DISPLAY_NAME="TestOIDC" \
  DAR_BASE_URL="http://$HOST:$PORT" \
  DAR_CODEX_PATH="$DAR_CODEX_PATH" \
  DAR_EMAIL_ENCRYPTION_KEYS="$DAR_EMAIL_ENCRYPTION_KEYS" \
  "$BIN_SERVER" --port "$PORT" --db "$DB" &
DAR_PID=$!
sleep 0.1
wait_for_health "Dar" "$DAR_PID" "http://$HOST:$PORT/health"

echo "Dar ready. Press Ctrl+C to stop."
wait "$DAR_PID"
