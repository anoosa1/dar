#!/usr/bin/env bash
set -euo pipefail
HOST=${HOST:-astra}
PORT=8080
DB=${DB:-dar.db}
OIDC_PORT=9001
SOOSA_PORT=8081
FILES_PORT=8082
CONTACTS_PORT=8083
CALENDAR_PORT=8084
TODO_PORT=8085
NOTES_PORT=8086
NEWS_PORT=8087
EMAIL_PORT=8088
PHOTOS_PORT=8089

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)       PORT="$2";        shift 2;;
    --oidc-port)  OIDC_PORT="$2";   shift 2;;
    --soosa-port) SOOSA_PORT="$2";  shift 2;;
    --files-port) FILES_PORT="$2";  shift 2;;
    --contacts-port) CONTACTS_PORT="$2"; shift 2;;
    --calendar-port) CALENDAR_PORT="$2"; shift 2;;
    --todo-port) TODO_PORT="$2"; shift 2;;
    --notes-port) NOTES_PORT="$2"; shift 2;;
    --news-port) NEWS_PORT="$2"; shift 2;;
    --email-port) EMAIL_PORT="$2"; shift 2;;
    --photos-port) PHOTOS_PORT="$2"; shift 2;;
    --host)       HOST="$2";        shift 2;;
    --db)         DB="$2";          shift 2;;
    --no-build)   NO_BUILD=1;       shift;;
    *) echo "Usage: ./launch.sh [--host HOST] [--port PORT] [--oidc-port PORT] [--soosa-port PORT] [--files-port PORT] [--db DB_PATH] [--no-build]"; exit 1;;
  esac
done

BIN_SERVER=./dar-server
BIN_OIDC=./dar-test-oidc
BIN_SOOSA=./dar-daemon-soosa
BIN_FILES=./dar-daemon-files
BIN_CONTACTS=./dar-daemon-contacts
BIN_CALENDAR=./dar-daemon-calendar
BIN_TODO=./dar-daemon-todo
BIN_NOTES=./dar-daemon-notes
BIN_NEWS=./dar-daemon-news
BIN_EMAIL=./dar-daemon-email
BIN_PHOTOS=./dar-daemon-photos
OIDC_BASE="http://$HOST:$OIDC_PORT"

trap 'kill $OIDC_PID 2>/dev/null; kill $SOOSA_PID 2>/dev/null; kill $FILES_PID 2>/dev/null; kill $CONTACTS_PID 2>/dev/null; kill $CALENDAR_PID 2>/dev/null; kill $TODO_PID 2>/dev/null; kill $NOTES_PID 2>/dev/null; kill $NEWS_PID 2>/dev/null; kill $EMAIL_PID 2>/dev/null; kill $PHOTOS_PID 2>/dev/null; kill $DAR_PID 2>/dev/null; wait' EXIT INT TERM

if [[ -z "${NO_BUILD:-}" ]]; then
  echo "Building..."
  nix-shell -p go --command "go build -o $BIN_SERVER ./cmd/server/"
  nix-shell -p go --command "go build -o $BIN_OIDC ./cmd/test-oidc/"
  nix-shell -p go --command "go build -o $BIN_SOOSA ./cmd/soosa/"
  nix-shell -p go --command "go build -o $BIN_FILES ./cmd/files/"
  nix-shell -p go --command "go build -o $BIN_CONTACTS ./cmd/contacts/"
  nix-shell -p go --command "go build -o $BIN_CALENDAR ./cmd/calendar/"
  nix-shell -p go --command "go build -o $BIN_TODO ./cmd/todo/"
  nix-shell -p go --command "go build -o $BIN_NOTES ./cmd/notes/"
  nix-shell -p go --command "go build -o $BIN_NEWS ./cmd/news/"
  nix-shell -p go --command "go build -o $BIN_EMAIL ./cmd/email/"
  nix-shell -p go --command "go build -o $BIN_PHOTOS ./cmd/photos/"
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
echo "Starting Soosa daemon on :$SOOSA_PORT..."
$BIN_SOOSA --port "$SOOSA_PORT" &
SOOSA_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$SOOSA_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Soosa ready"

echo ""
echo "Starting Files daemon on :$FILES_PORT..."
$BIN_FILES --port "$FILES_PORT" &
FILES_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$FILES_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Files ready"

echo ""
echo "Starting Contacts daemon on :$CONTACTS_PORT..."
$BIN_CONTACTS --port "$CONTACTS_PORT" &
CONTACTS_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$CONTACTS_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Contacts ready"

echo ""
echo "Starting Calendar daemon on :$CALENDAR_PORT..."
$BIN_CALENDAR --port "$CALENDAR_PORT" &
CALENDAR_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$CALENDAR_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Calendar ready"

echo ""
echo "Starting Todo daemon on :$TODO_PORT..."
$BIN_TODO --port "$TODO_PORT" &
TODO_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$TODO_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Todo ready"

echo ""
echo "Starting Notes daemon on :$NOTES_PORT..."
$BIN_NOTES --port "$NOTES_PORT" &
NOTES_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$NOTES_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Notes ready"

echo ""
echo "Starting News daemon on :$NEWS_PORT..."
$BIN_NEWS --port "$NEWS_PORT" &
NEWS_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$NEWS_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  News ready"

echo ""
echo "Starting Email daemon on :$EMAIL_PORT..."
$BIN_EMAIL --port "$EMAIL_PORT" &
EMAIL_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$EMAIL_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Email ready"

echo ""
echo "Starting Photos daemon on :$PHOTOS_PORT..."
$BIN_PHOTOS --port "$PHOTOS_PORT" &
PHOTOS_PID=$!
for i in $(seq 1 20); do
  curl -s "http://$HOST:$PHOTOS_PORT/health" >/dev/null 2>&1 && break
  sleep 0.2
done
echo "  Photos ready"

echo ""
echo "Starting Dar on :$PORT..."
echo "  Home:   http://$HOST:$PORT"
echo "  OIDC:   $OIDC_BASE"
echo "  Soosa:  :$SOOSA_PORT"
echo "  Files:  :$FILES_PORT"
echo "  DB:     $DB"
echo ""
echo "Press Ctrl+C to stop."
echo ""

env \
  DAR_OIDC_ISSUER="$OIDC_BASE" \
  DAR_OIDC_CLIENT_ID="test-client" \
  DAR_OIDC_CLIENT_SECRET="test-secret" \
  DAR_OIDC_DISPLAY_NAME="TestOIDC" \
  DAR_BASE_URL="http://$HOST:$PORT" \
  DAR_SOOSA_PORT="$SOOSA_PORT" \
  DAR_FILES_PORT="$FILES_PORT" \
  DAR_CONTACTS_PORT="$CONTACTS_PORT" \
  DAR_CALENDAR_PORT="$CALENDAR_PORT" \
  DAR_TODO_PORT="$TODO_PORT" \
  DAR_NOTES_PORT="$NOTES_PORT" \
  DAR_NEWS_PORT="$NEWS_PORT" \
  DAR_EMAIL_PORT="$EMAIL_PORT" \
  DAR_PHOTOS_PORT="$PHOTOS_PORT" \
  $BIN_SERVER --port "$PORT" --db "$DB" &
DAR_PID=$!

wait $DAR_PID
