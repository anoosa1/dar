#!/usr/bin/env bash
# Focused WebDAV daemon compatibility test.
set -euo pipefail

BIN="./zig-out/bin/dar-files"
PORT=$((RANDOM % 1000 + 18000))
ROOT=$(mktemp -d /tmp/dar_dav_root_XXXXXX)
USER_ID="test"
BASE="http://localhost:$PORT/$USER_ID/files"
pass=0
fail=0
RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

pass_msg() { echo -e "${GREEN}PASS${NC} $1"; pass=$((pass + 1)); }
fail_msg() { echo -e "${RED}FAIL${NC} $1"; fail=$((fail + 1)); }

cleanup() {
    kill "$DAV_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$ROOT"
}
trap cleanup EXIT

mkdir -p "$ROOT/$USER_ID/files"
DAV_KEY="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
printf '%s' "$DAV_KEY" > "$ROOT/$USER_ID/.dav_api_key"

nix develop /home/anas/soosa --command zig build
"$BIN" --port "$PORT" --root "$ROOT" >/tmp/dar_dav_e2e_$PORT.log 2>&1 &
DAV_PID=$!

for _ in $(seq 1 50); do
    if curl -s --connect-timeout 1 "http://localhost:$PORT/health" | grep -q OK; then
        break
    fi
    sleep 0.1
done

if curl -s --connect-timeout 2 "http://localhost:$PORT/health" | grep -q OK; then
    pass_msg "DAV daemon health endpoint"
else
    fail_msg "DAV daemon health endpoint"
fi

NOAUTH_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X PROPFIND -H 'Depth: 0' "$BASE")
if [ "$NOAUTH_CODE" = "401" ]; then
    pass_msg "DAV rejects unauthenticated requests"
else
    fail_msg "DAV rejects unauthenticated requests (got $NOAUTH_CODE)"
fi

AUTH=( -H "X-Dar-Dav-Key: $DAV_KEY" )
OPTIONS_HEADERS=$(curl -s -D- -o /tmp/dar_dav_options_$PORT.out --connect-timeout 3 -X OPTIONS "${AUTH[@]}" "$BASE")
if echo "$OPTIONS_HEADERS" | grep -qi 'allow: .*COPY' && echo "$OPTIONS_HEADERS" | grep -qi 'allow: .*MOVE' && echo "$OPTIONS_HEADERS" | grep -qi 'dav: 1'; then
    pass_msg "DAV OPTIONS advertises COPY and MOVE"
else
    fail_msg "DAV OPTIONS advertises COPY and MOVE"
fi
rm -f /tmp/dar_dav_options_$PORT.out

MKCOL_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X MKCOL "${AUTH[@]}" "$BASE/davdir")
if [ "$MKCOL_CODE" = "201" ] && [ -d "$ROOT/$USER_ID/files/davdir" ]; then
    pass_msg "DAV MKCOL creates collection"
else
    fail_msg "DAV MKCOL creates collection (got $MKCOL_CODE)"
fi

PUT_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X PUT "${AUTH[@]}" --data 'hello dav' "$BASE/davdir/a.txt")
if [ "$PUT_CODE" = "201" ] && [ "$(cat "$ROOT/$USER_ID/files/davdir/a.txt" 2>/dev/null || true)" = "hello dav" ]; then
    pass_msg "DAV PUT writes file bytes"
else
    fail_msg "DAV PUT writes file bytes (got $PUT_CODE)"
fi

PROPFIND_BODY=$(curl -s --connect-timeout 3 -X PROPFIND "${AUTH[@]}" -H 'Depth: 1' "$BASE/davdir")
if echo "$PROPFIND_BODY" | grep -q '/test/files/davdir/a.txt' && echo "$PROPFIND_BODY" | grep -q 'D:getetag' && echo "$PROPFIND_BODY" | grep -q 'D:getlastmodified' && echo "$PROPFIND_BODY" | grep -q 'D:getcontentlength>9<'; then
    pass_msg "DAV PROPFIND Depth: 1 lists children with etag, last-modified, and length"
else
    fail_msg "DAV PROPFIND Depth: 1 lists children with etag, last-modified, and length"
fi

BASIC_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -u "$USER_ID:$DAV_KEY" -X PROPFIND -H 'Depth: 0' "$BASE/davdir")
if [ "$BASIC_CODE" = "207" ]; then
    pass_msg "DAV accepts username plus API key via Basic auth"
else
    fail_msg "DAV accepts username plus API key via Basic auth (got $BASIC_CODE)"
fi

COPY_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X COPY "${AUTH[@]}" -H "Destination: $BASE/davdir/b.txt" "$BASE/davdir/a.txt")
if [ "$COPY_CODE" = "201" ] && [ "$(cat "$ROOT/$USER_ID/files/davdir/b.txt" 2>/dev/null || true)" = "hello dav" ] && [ -e "$ROOT/$USER_ID/files/davdir/a.txt" ]; then
    pass_msg "DAV COPY duplicates file inside user root"
else
    fail_msg "DAV COPY duplicates file inside user root (got $COPY_CODE)"
fi

MOVE_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X MOVE "${AUTH[@]}" -H "Destination: $BASE/davdir/c.txt" "$BASE/davdir/b.txt")
if [ "$MOVE_CODE" = "201" ] && [ "$(cat "$ROOT/$USER_ID/files/davdir/c.txt" 2>/dev/null || true)" = "hello dav" ] && [ ! -e "$ROOT/$USER_ID/files/davdir/b.txt" ]; then
    pass_msg "DAV MOVE renames file inside user root"
else
    fail_msg "DAV MOVE renames file inside user root (got $MOVE_CODE)"
fi

BAD_COPY_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X COPY "${AUTH[@]}" -H "Destination: http://localhost:$PORT/../escape.txt" "$BASE/davdir/a.txt")
if [ "$BAD_COPY_CODE" = "400" ] && [ ! -e "$ROOT/escape.txt" ]; then
    pass_msg "DAV COPY rejects destination traversal"
else
    fail_msg "DAV COPY rejects destination traversal (got $BAD_COPY_CODE)"
fi

DELETE_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X DELETE "${AUTH[@]}" "$BASE/davdir/c.txt")
if [ "$DELETE_CODE" = "204" ] && [ ! -e "$ROOT/$USER_ID/files/davdir/c.txt" ]; then
    pass_msg "DAV DELETE removes file"
else
    fail_msg "DAV DELETE removes file (got $DELETE_CODE)"
fi

NEW_DAV_KEY="abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
printf '%s' "$NEW_DAV_KEY" > "$ROOT/$USER_ID/.dav_api_key"
OLD_KEY_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X PROPFIND "${AUTH[@]}" -H 'Depth: 0' "$BASE/davdir")
NEW_AUTH=( -H "X-Dar-Dav-Key: $NEW_DAV_KEY" )
NEW_KEY_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 -X PROPFIND "${NEW_AUTH[@]}" -H 'Depth: 0' "$BASE/davdir")
if [ "$OLD_KEY_CODE" = "401" ] && [ "$NEW_KEY_CODE" = "207" ]; then
    pass_msg "DAV rerolled key invalidates old key and accepts new key"
else
    fail_msg "DAV rerolled key invalidates old key and accepts new key (old $OLD_KEY_CODE new $NEW_KEY_CODE)"
fi

echo ""
echo "============================="
echo -e "Passed: ${GREEN}$pass${NC}  Failed: ${RED}$fail${NC}"
echo "============================="

if [ "$fail" -gt 0 ]; then
    exit 1
fi
