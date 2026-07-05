#!/usr/bin/env bash
# Dar end-to-end test with dummy OAuth server.
# Tests: server startup, OIDC discovery, login flow, session persistence, auth middleware.
set -euo pipefail

BIN="./zig-out/bin/dar"
OAUTH_BIN="./zig-out/bin/dar-oauth-test"
RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'

# Use random high ports to avoid conflicts
OAUTH_PORT=$((RANDOM % 1000 + 9000))
DAR_PORT=$((RANDOM % 1000 + 9100))
DB="/tmp/dar_test_e2e_$$.sqlite"
USER_ID=""

pass=0; fail=0
pass_msg() { echo -e "${GREEN}PASS${NC} $1"; pass=$((pass + 1)); }
fail_msg() { echo -e "${RED}FAIL${NC} $1"; fail=$((fail + 1)); }

cleanup() {
    kill $OAUTH_PID 2>/dev/null || true
    kill $DAR_PID 2>/dev/null || true
    rm -f "$DB"
    rm -rf "data/users/Bad ID !"
    if [ -n "${USER_ID:-}" ]; then
        rm -rf "data/users/$USER_ID"
    fi
    wait 2>/dev/null || true
}
trap cleanup EXIT

# ── Build ──
echo "=== Building ==="
nix develop /home/anas/soosa --command zig build 2>&1 || { echo "BUILD FAILED"; exit 1; }
echo ""

# ── Start dummy OAuth server ──
echo "=== Starting dummy OAuth server on :$OAUTH_PORT ==="
rm -f "$DB"
$OAUTH_BIN --port $OAUTH_PORT &>/dev/null &
OAUTH_PID=$!
sleep 0.5

# Verify OAuth server
if curl -s --connect-timeout 3 "http://localhost:$OAUTH_PORT/.well-known/openid-configuration" | grep -q authorization_endpoint; then
    pass_msg "OAuth discovery endpoint"
else
    fail_msg "OAuth discovery endpoint"
fi

# ── Start Dar server ──
echo "=== Starting Dar server on :$DAR_PORT ==="
DAR_OIDC_ISSUER="http://localhost:$OAUTH_PORT" \
DAR_OIDC_CLIENT_ID="test-client" \
DAR_OIDC_CLIENT_SECRET=*** \
DAR_OIDC_DISPLAY_NAME="TestOIDC" \
DAR_BASE_URL="http://localhost:$DAR_PORT" \
$BIN --port $DAR_PORT --db "$DB" &
DAR_PID=$!
sleep 1

# Verify startup succeeded (process still running)
if kill -0 $DAR_PID 2>/dev/null; then
    pass_msg "Dar server started"
else
    fail_msg "Dar server started (process died)"
fi

# ── Test 1: Server is up ──
HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 "http://localhost:$DAR_PORT/")
if [ "$HTTP_CODE" = "302" ]; then
    pass_msg "Server / → 302 redirect"
else
    fail_msg "Server / → 302 redirect (got $HTTP_CODE)"
fi

# ── Test 2: Login page has provider button ──
LOGIN_PAGE=$(curl -s --connect-timeout 3 "http://localhost:$DAR_PORT/login")
if echo "$LOGIN_PAGE" | grep -q 'Continue with TestOIDC'; then
    pass_msg "Login page shows OIDC provider"
else
    fail_msg "Login page shows OIDC provider"
fi

# Seed an old malformed ID for the same OIDC identity. Login should migrate it to the username.
rm -rf "data/users/Bad ID !" "data/users/test"
if command -v sqlite3 >/dev/null 2>&1; then
    sqlite3 "$DB" "INSERT INTO users (id, email, display_name, provider, provider_id) VALUES ('Bad ID !', 'test@dar.local', 'Test User', 'http://localhost:$OAUTH_PORT', 'test-user-123');"
else
    nix develop /home/anas/soosa --command sqlite3 "$DB" "INSERT INTO users (id, email, display_name, provider, provider_id) VALUES ('Bad ID !', 'test@dar.local', 'Test User', 'http://localhost:$OAUTH_PORT', 'test-user-123');" >/dev/null
fi
mkdir -p "data/users/Bad ID !/files"

# ── Test 3: OAuth callback creates session ──
CALLBACK=$(curl -s -D- --connect-timeout 3 "http://localhost:$DAR_PORT/auth/oidc/callback?code=test-auth-code-12345" 2>&1)
SESSION_COOKIE=$(echo "$CALLBACK" | grep 'set-cookie: dar_session=' | head -1 | sed 's/.*dar_session=//;s/;.*//')

if [ -n "$SESSION_COOKIE" ]; then
    pass_msg "OAuth callback sets session cookie"
else
    fail_msg "OAuth callback sets session cookie"
fi

if echo "$CALLBACK" | grep -q 'location: /home'; then
    pass_msg "OAuth callback → /home redirect"
else
    fail_msg "OAuth callback → /home redirect"
fi

if command -v sqlite3 >/dev/null 2>&1; then
    USER_ID=$(sqlite3 "$DB" "SELECT id FROM users LIMIT 1;")
else
    USER_ID=$(nix develop /home/anas/soosa --command sqlite3 "$DB" "SELECT id FROM users LIMIT 1;" 2>/dev/null | tail -n 1)
fi

if [ -n "$USER_ID" ] && [ -d "data/users/$USER_ID" ]; then
    pass_msg "OIDC login creates per-user data directory"
else
    fail_msg "OIDC login creates per-user data directory"
fi

if [ "$USER_ID" = "test" ]; then
    pass_msg "OIDC user id uses normalized username"
else
    fail_msg "OIDC user id uses normalized username (got $USER_ID)"
fi

if [ ! -d "data/users/Bad ID !" ] && [ -d "data/users/test" ]; then
    pass_msg "OIDC login migrates malformed user directory to username"
else
    fail_msg "OIDC login migrates malformed user directory to username"
fi

if [ -n "$USER_ID" ] && [ -d "data/users/$USER_ID/files" ]; then
    pass_msg "OIDC login creates files app directory"
else
    fail_msg "OIDC login creates files app directory"
fi

# ── Test 4: /api/me with valid session ──
if [ -n "$SESSION_COOKIE" ]; then
    API_RESPONSE=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/me")
    if echo "$API_RESPONSE" | grep -q '"email":'; then
        pass_msg "/api/me returns email with session"
    else
        fail_msg "/api/me returns email with session"
    fi

    if echo "$API_RESPONSE" | grep -q 'Test User'; then
        pass_msg "/api/me returns display_name"
    else
        fail_msg "/api/me returns display_name"
    fi
fi

# ── Test 5: /home accessible with session ──
if [ -n "$SESSION_COOKIE" ]; then
    HOME_CODE=$(curl -s -o /dev/null -w '%{http_code}' -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/home")
    if [ "$HOME_CODE" = "200" ]; then
        pass_msg "/home returns 200 with session"
    else
        fail_msg "/home returns 200 with session (got $HOME_CODE)"
    fi

    HOME_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/home")
    if echo "$HOME_HTML" | grep -q 'id="dar-app-bar"' && echo "$HOME_HTML" | grep -q 'aria-label="Files"'; then
        pass_msg "Top bar renders icon-only app nav"
    else
        fail_msg "Top bar renders icon-only app nav"
    fi

    if echo "$HOME_HTML" | grep -q 'aria-label="Home"' && echo "$HOME_HTML" | grep -q 'M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z' && ! echo "$HOME_HTML" | grep -q '>Dar</a>'; then
        pass_msg "Top bar uses home SVG instead of Dar title"
    else
        fail_msg "Top bar uses home SVG instead of Dar title"
    fi

    if echo "$HOME_HTML" | grep -q 'id="user-menu-button"' && echo "$HOME_HTML" | grep -q 'id="user-menu"'; then
        pass_msg "Top bar renders avatar user menu"
    else
        fail_msg "Top bar renders avatar user menu"
    fi
fi

# ── Test 5b: Files app default scope and all-files scope ──
if [ -n "$SESSION_COOKIE" ] && [ -n "$USER_ID" ]; then
    mkdir -p "data/users/$USER_ID/files" "data/users/$USER_ID/trash"
    printf 'hello' > "data/users/$USER_ID/files/note.txt"
    printf 'trashed' > "data/users/$USER_ID/trash/old.txt"
    printf 'hidden' > "data/users/$USER_ID/files/.secret"

    FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
    if echo "$FILES_HTML" | grep -q 'id="app-sidebar"' && echo "$FILES_HTML" | grep -q 'data-scope="files"' && echo "$FILES_HTML" | grep -q 'data-scope="all"'; then
        pass_msg "Files page renders shared left sidebar"
    else
        fail_msg "Files page renders shared left sidebar"
    fi

    if echo "$FILES_HTML" | grep -q 'id="settings-dialog"' && echo "$FILES_HTML" | grep -q 'openSettingsDialog' && echo "$FILES_HTML" | grep -q 'id="toggle-hidden"' && echo "$FILES_HTML" | grep -q 'id="toggle-extensions"' && echo "$FILES_HTML" | grep -q 'id="columns-menu"'; then
        pass_msg "Files page renders view preferences in settings dialog"
    else
        fail_msg "Files page renders view preferences in settings dialog"
    fi

    if echo "$FILES_HTML" | grep -q 'id="dav-url-field"' && echo "$FILES_HTML" | grep -q "http://localhost:8081/$USER_ID/files" && echo "$FILES_HTML" | grep -q 'copyDavUrl' && echo "$FILES_HTML" | grep -q 'Copy link'; then
        pass_msg "Files settings shows user-specific WebDAV URL with copy link"
    else
        fail_msg "Files settings shows user-specific WebDAV URL with copy link"
    fi


    if echo "$FILES_HTML" | grep -q 'id="dav-api-key-field"' && echo "$FILES_HTML" | grep -q 'rerollDavApiKey' && echo "$FILES_HTML" | grep -q 'Reroll' && echo "$FILES_HTML" | grep -q 'WebDAV API key'; then
        pass_msg "Files settings shows WebDAV API key with reroll control"
    else
        fail_msg "Files settings shows WebDAV API key with reroll control"
    fi

    DAV_KEY_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/dav-key")
    DAV_KEY=$(printf '%s' "$DAV_KEY_BODY" | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')
    DAV_REROLL_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -X POST --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/dav-key/reroll")
    DAV_REROLL_KEY=$(printf '%s' "$DAV_REROLL_BODY" | sed -n 's/.*"key":"\([^"]*\)".*/\1/p')
    if [ "${#DAV_KEY}" = "64" ] && [ "${#DAV_REROLL_KEY}" = "64" ] && [ "$DAV_KEY" != "$DAV_REROLL_KEY" ]; then
        pass_msg "Files WebDAV API key endpoint returns stable key and reroll generates a new key"
    else
        fail_msg "Files WebDAV API key endpoint returns stable key and reroll generates a new key"
    fi

    if ! echo "$FILES_HTML" | grep -q '>Show hidden</button>' && ! echo "$FILES_HTML" | grep -q '>Extensions</button>' && ! echo "$FILES_HTML" | grep -q '>Columns</button>' && ! echo "$FILES_HTML" | grep -q '>WebDAV</a>'; then
        pass_msg "Files toolbar does not duplicate settings controls or WebDAV"
    else
        fail_msg "Files toolbar does not duplicate settings controls or WebDAV"
    fi

    if echo "$FILES_HTML" | grep -q 'function sortBy' && echo "$FILES_HTML" | grep -q 'sortKey'; then
        pass_msg "Files table headers are sortable"
    else
        fail_msg "Files table headers are sortable"
    fi

    if echo "$FILES_HTML" | grep -q 'id="file-search-input"' && echo "$FILES_HTML" | grep -q 'function searchFiles' && echo "$FILES_HTML" | grep -q '/api/files/search'; then
        pass_msg "Files page renders filename search controls"
    else
        fail_msg "Files page renders filename search controls"
    fi

    if echo "$FILES_HTML" | grep -q 'id="drop-zone"' && echo "$FILES_HTML" | grep -q 'function handleDrop' && echo "$FILES_HTML" | grep -q 'dragover'; then
        pass_msg "Files page supports drag and drop upload"
    else
        fail_msg "Files page supports drag and drop upload"
    fi

    if echo "$FILES_HTML" | grep -q 'id="select-all-files"' && echo "$FILES_HTML" | grep -q 'function toggleSelection' && echo "$FILES_HTML" | grep -q 'function bulkDeleteSelected'; then
        pass_msg "Files page supports multi-select bulk actions"
    else
        fail_msg "Files page supports multi-select bulk actions"
    fi

    if echo "$FILES_HTML" | grep -q 'id="dar-dialog"' && echo "$FILES_HTML" | grep -q 'function showPromptDialog' && echo "$FILES_HTML" | grep -q 'function showConfirmDialog'; then
        pass_msg "Files page uses in-page dialogs instead of prompt/confirm"
    else
        fail_msg "Files page uses in-page dialogs instead of prompt/confirm"
    fi

    if echo "$FILES_HTML" | grep -q 'id="details-pane"' && echo "$FILES_HTML" | grep -q 'function showDetails'; then
        pass_msg "Files page renders details pane"
    else
        fail_msg "Files page renders details pane"
    fi

    if echo "$FILES_HTML" | grep -q 'id="details-pane"' && echo "$FILES_HTML" | grep -q 'flex items-stretch gap-4 h-\[calc(100vh-12rem)\] overflow-hidden' && echo "$FILES_HTML" | grep -q 'w-80 shrink-0 h-full overflow-y-auto'; then
        pass_msg "Files details pane scrolls without expanding page"
    else
        fail_msg "Files details pane scrolls without expanding page"
    fi

    if echo "$FILES_HTML" | grep -q 'id="preview-content"' && echo "$FILES_HTML" | grep -q 'function loadPreview' && echo "$FILES_HTML" | grep -q '/api/files/preview'; then
        pass_msg "Files details pane exposes preview controls"
    else
        fail_msg "Files details pane exposes preview controls"
    fi

    if echo "$FILES_HTML" | grep -q 'id="sharing-list"' && echo "$FILES_HTML" | grep -q 'function loadShares' && echo "$FILES_HTML" | grep -q 'function createShare' && echo "$FILES_HTML" | grep -q 'copyShareLink'; then
        pass_msg "Files details pane exposes public sharing controls"
    else
        fail_msg "Files details pane exposes public sharing controls"
    fi

    if echo "$FILES_HTML" | grep -q 'data-scope="favorites"' && echo "$FILES_HTML" | grep -q 'data-scope="tags"' && echo "$FILES_HTML" | grep -q 'data-scope="recent"' && echo "$FILES_HTML" | grep -q 'function toggleFavoritePath'; then
        pass_msg "Files page exposes metadata saved views and favorites"
    else
        fail_msg "Files page exposes metadata saved views and favorites"
    fi

    if echo "$FILES_HTML" | grep -q 'id="tags-list"' && echo "$FILES_HTML" | grep -q 'function createAndAssignTag' && echo "$FILES_HTML" | grep -q 'function loadTagFiles' && echo "$FILES_HTML" | grep -q 'id="comments-list"' && echo "$FILES_HTML" | grep -q 'function addComment'; then
        pass_msg "Files details pane exposes tags and comments controls"
    else
        fail_msg "Files details pane exposes tags and comments controls"
    fi

    if echo "$FILES_HTML" | grep -q 'function openActionMenu' && echo "$FILES_HTML" | grep -q 'aria-label="File actions"'; then
        pass_msg "Files rows use 3-dot action menus"
    else
        fail_msg "Files rows use 3-dot action menus"
    fi

    if echo "$FILES_HTML" | grep -q 'getBoundingClientRect' && echo "$FILES_HTML" | grep -q 'window.innerWidth' && echo "$FILES_HTML" | grep -q 'window.innerHeight'; then
        pass_msg "Files action menu clamps to viewport"
    else
        fail_msg "Files action menu clamps to viewport"
    fi

    if echo "$FILES_HTML" | grep -q 'id="file-picker-dialog"' && echo "$FILES_HTML" | grep -q 'function renderFilePicker' && echo "$FILES_HTML" | grep -q 'confirmFilePicker'; then
        pass_msg "Files move/copy opens folder picker dialog"
    else
        fail_msg "Files move/copy opens folder picker dialog"
    fi

    if echo "$FILES_HTML" | grep -q '/api/files/picker?mode=folder' && echo "$FILES_HTML" | grep -q 'Move here' && echo "$FILES_HTML" | grep -q 'Copy here'; then
        pass_msg "Files picker uses API-backed mini file browser"
    else
        fail_msg "Files picker uses API-backed mini file browser"
    fi

    if echo "$FILES_HTML" | grep -q 'function extOf' && echo "$FILES_HTML" | grep -q 'changes the file extension'; then
        pass_msg "Files rename warns on extension changes"
    else
        fail_msg "Files rename warns on extension changes"
    fi

    if echo "$FILES_HTML" | grep -q 'function folderIcon' && echo "$FILES_HTML" | grep -q '#0082c9' && ! echo "$FILES_HTML" | grep -q "f.is_dir?'FOLDER'"; then
        pass_msg "Files UI uses Nextcloud folder SVG instead of folder text"
    else
        fail_msg "Files UI uses Nextcloud folder SVG instead of folder text"
    fi

    if echo "$FILES_HTML" | grep -q 'function pdfIcon' && echo "$FILES_HTML" | grep -q 'function textIcon' && echo "$FILES_HTML" | grep -q 'function imageIcon' && echo "$FILES_HTML" | grep -q 'function audioIcon' && echo "$FILES_HTML" | grep -q 'function videoIcon' && ! echo "$FILES_HTML" | grep -q '>FILE<'; then
        pass_msg "Files UI renders SVG icons for common file types"
    else
        fail_msg "Files UI renders SVG icons for common file types"
    fi

    FILES_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/list?path=/")
    if echo "$FILES_LIST" | grep -q '"name":"note.txt"' && ! echo "$FILES_LIST" | grep -q '"name":"trash"'; then
        pass_msg "Files default view lists user files folder only"
    else
        fail_msg "Files default view lists user files folder only"
    fi

    if echo "$FILES_LIST" | grep -q '"name":".secret"'; then
        pass_msg "Files API returns hidden files for client-side filtering"
    else
        fail_msg "Files API returns hidden files for client-side filtering"
    fi

    SEARCH_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/search?path=/&q=note")
    SEARCH_TRAVERSAL_CODE=$(curl -s -o /dev/null -w '%{http_code}' -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/search?path=../&q=note")
    if echo "$SEARCH_LIST" | grep -q '"name":"note.txt"' && ! echo "$SEARCH_LIST" | grep -q '"name":".secret"' && [ "$SEARCH_TRAVERSAL_CODE" = "400" ]; then
        pass_msg "Files search API returns matching names and rejects traversal"
    else
        fail_msg "Files search API returns matching names and rejects traversal"
    fi

    ALL_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/list?scope=all&path=/")
    if echo "$ALL_LIST" | grep -q '"name":"files"' && echo "$ALL_LIST" | grep -q '"name":"trash"'; then
        pass_msg "Files all-files view lists whole user directory"
    else
        fail_msg "Files all-files view lists whole user directory"
    fi

    DOWNLOAD_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/download?path=/note.txt")
    if [ "$DOWNLOAD_BODY" = "hello" ]; then
        pass_msg "Files download reads from default files folder"
    else
        fail_msg "Files download reads from default files folder"
    fi

    DOWNLOAD_HEADERS=$(curl -s -D- -o /tmp/dar_download_test.out -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/download?path=/note.txt")
    if echo "$DOWNLOAD_HEADERS" | grep -qi 'content-disposition: attachment; filename="note.txt"'; then
        pass_msg "Files download sets attachment filename"
    else
        fail_msg "Files download sets attachment filename"
    fi
    rm -f /tmp/dar_download_test.out

    PREVIEW_HEADERS=$(curl -s -D- -o /tmp/dar_preview_test.out -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/preview?path=/note.txt")
    PREVIEW_BODY=$(cat /tmp/dar_preview_test.out 2>/dev/null || true)
    if [ "$PREVIEW_BODY" = "hello" ] && echo "$PREVIEW_HEADERS" | grep -qi 'content-type: text/plain' && echo "$PREVIEW_HEADERS" | grep -qi 'content-disposition: inline'; then
        pass_msg "Files preview serves previewable files inline"
    else
        fail_msg "Files preview serves previewable files inline"
    fi
    rm -f /tmp/dar_preview_test.out

    SHARE_FILE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/note.txt"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/create")
    SHARE_FILE_TOKEN=$(printf '%s' "$SHARE_FILE_BODY" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
    if [ -n "$SHARE_FILE_TOKEN" ] && echo "$SHARE_FILE_BODY" | grep -q '"url":"/s?token='; then
        pass_msg "Files API creates public share token for file"
    else
        fail_msg "Files API creates public share token for file"
    fi

    PUBLIC_FILE_BODY=$(curl -s --connect-timeout 3 "http://localhost:$DAR_PORT/s?token=$SHARE_FILE_TOKEN")
    if [ "$PUBLIC_FILE_BODY" = "hello" ]; then
        pass_msg "Public file share downloads without session"
    else
        fail_msg "Public file share downloads without session"
    fi

    SHARE_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/list?path=/note.txt")
    if echo "$SHARE_LIST" | grep -q "$SHARE_FILE_TOKEN" && echo "$SHARE_LIST" | grep -q '"path":"/note.txt"'; then
        pass_msg "Files API lists public shares for path"
    else
        fail_msg "Files API lists public shares for path"
    fi

    FAVORITE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/note.txt"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/favorites/toggle")
    FAVORITE_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/favorites/list")
    FILES_LIST_FAVORITE=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/list?path=/")
    if echo "$FAVORITE_BODY" | grep -q '"favorite":true' && echo "$FAVORITE_LIST" | grep -q '"path":"/note.txt"' && echo "$FILES_LIST_FAVORITE" | grep -q '"favorite":true'; then
        pass_msg "Files favorites API toggles and saved view lists favorited files"
    else
        fail_msg "Files favorites API toggles and saved view lists favorited files"
    fi

    TAG_CREATE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"name":"work","color":"#6750A4"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/tags/create")
    TAG_ID=$(printf '%s' "$TAG_CREATE_BODY" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
    TAG_ASSIGN_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data "{\"path\":\"/note.txt\",\"tag_id\":\"$TAG_ID\"}" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/tags/assign")
    TAGS_FOR_PATH=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/tags?path=/note.txt")
    TAG_FILES=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/tags/files?tag_id=$TAG_ID")
    if [ -n "$TAG_ID" ] && echo "$TAG_ASSIGN_BODY" | grep -q '"status":"ok"' && echo "$TAGS_FOR_PATH" | grep -q '"name":"work"' && echo "$TAG_FILES" | grep -q '"path":"/note.txt"'; then
        pass_msg "Files tags API creates tags, assigns them, and lists tagged files"
    else
        fail_msg "Files tags API creates tags, assigns them, and lists tagged files"
    fi

    COMMENT_CREATE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/note.txt","body":"looks good"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/comments/create")
    COMMENT_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/comments?path=/note.txt")
    RECENT_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/recent")
    if echo "$COMMENT_CREATE_BODY" | grep -q '"status":"ok"' && echo "$COMMENT_LIST" | grep -q 'looks good' && echo "$RECENT_LIST" | grep -q '"path":"/note.txt"'; then
        pass_msg "Files comments API stores comments and Recent view lists active files"
    else
        fail_msg "Files comments API stores comments and Recent view lists active files"
    fi

    EXPIRED_SHARE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/note.txt","expires_at":1}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/create")
    EXPIRED_SHARE_TOKEN=$(printf '%s' "$EXPIRED_SHARE_BODY" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
    EXPIRED_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 "http://localhost:$DAR_PORT/s?token=$EXPIRED_SHARE_TOKEN")
    EXPIRED_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/list?path=/note.txt")
    if [ -n "$EXPIRED_SHARE_TOKEN" ] && [ "$EXPIRED_CODE" = "404" ] && ! echo "$EXPIRED_LIST" | grep -q "$EXPIRED_SHARE_TOKEN"; then
        pass_msg "Expired public share behaves as deleted"
    else
        fail_msg "Expired public share behaves as deleted"
    fi

    UPLOAD_SRC=$(mktemp /tmp/dar_upload_XXXXXX.txt)
    printf 'uploaded from e2e' > "$UPLOAD_SRC"
    UPLOAD_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -F "file=@$UPLOAD_SRC;filename=upload.txt" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/upload?path=/")
    rm -f "$UPLOAD_SRC"
    if echo "$UPLOAD_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/upload.txt" 2>/dev/null || true)" = "uploaded from e2e" ]; then
        pass_msg "Files upload writes to default files folder"
    else
        fail_msg "Files upload writes to default files folder"
    fi

    CONFLICT_SRC=$(mktemp /tmp/dar_conflict_XXXXXX.txt)
    printf 'renamed conflict' > "$CONFLICT_SRC"
    CONFLICT_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -F "file=@$CONFLICT_SRC;filename=upload.txt" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/upload?path=/&conflict=rename")
    rm -f "$CONFLICT_SRC"
    if echo "$CONFLICT_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/upload.txt" 2>/dev/null || true)" = "uploaded from e2e" ] && [ "$(cat "data/users/$USER_ID/files/upload (2).txt" 2>/dev/null || true)" = "renamed conflict" ]; then
        pass_msg "Files upload conflict rename keeps original and adds copy"
    else
        fail_msg "Files upload conflict rename keeps original and adds copy"
    fi

    OVERWRITE_SRC=$(mktemp /tmp/dar_overwrite_XXXXXX.txt)
    printf 'overwritten from e2e' > "$OVERWRITE_SRC"
    OVERWRITE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -F "file=@$OVERWRITE_SRC;filename=upload.txt" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/upload?path=/&conflict=overwrite")
    rm -f "$OVERWRITE_SRC"
    VERSIONS_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/versions/list?path=/upload.txt")
    VERSION_ID=$(printf '%s' "$VERSIONS_LIST" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
    VERSION_BYTES=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/versions/download?id=$VERSION_ID")
    if echo "$OVERWRITE_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/upload.txt" 2>/dev/null || true)" = "overwritten from e2e" ] && [ -n "$VERSION_ID" ] && [ "$VERSION_BYTES" = "uploaded from e2e" ]; then
        pass_msg "Files overwrite creates downloadable previous version"
    else
        fail_msg "Files overwrite creates downloadable previous version"
    fi

    RESTORE_VERSION_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data "{\"id\":\"$VERSION_ID\"}" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/versions/restore")
    if echo "$RESTORE_VERSION_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/upload.txt" 2>/dev/null || true)" = "uploaded from e2e" ]; then
        pass_msg "Files version restore returns previous bytes"
    else
        fail_msg "Files version restore returns previous bytes"
    fi

    LARGE_SRC=$(mktemp /tmp/dar_large_upload_XXXXXX.bin)
    dd if=/dev/zero of="$LARGE_SRC" bs=1M count=20 status=none
    LARGE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -F "file=@$LARGE_SRC;filename=large.bin" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/upload?path=/")
    rm -f "$LARGE_SRC"
    LARGE_SIZE=$(wc -c < "data/users/$USER_ID/files/large.bin" 2>/dev/null || echo 0)
    if echo "$LARGE_BODY" | grep -q '"status":"ok"' && [ "$LARGE_SIZE" = "20971520" ]; then
        pass_msg "Files upload accepts large everyday files above JSON body limit"
    else
        fail_msg "Files upload accepts large everyday files above JSON body limit"
    fi

    MKDIR_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/","name":"docs"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/mkdir")
    if echo "$MKDIR_BODY" | grep -q '"status":"ok"' && [ -d "data/users/$USER_ID/files/docs" ]; then
        pass_msg "Files mkdir creates folder in default files root"
    else
        fail_msg "Files mkdir creates folder in default files root"
    fi

    printf 'shared folder file' > "data/users/$USER_ID/files/docs/shared.txt"
    SHARE_FOLDER_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/docs"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/create")
    SHARE_FOLDER_TOKEN=$(printf '%s' "$SHARE_FOLDER_BODY" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
    PUBLIC_FOLDER_HTML=$(curl -s --connect-timeout 3 "http://localhost:$DAR_PORT/s?token=$SHARE_FOLDER_TOKEN")
    if [ -n "$SHARE_FOLDER_TOKEN" ] && echo "$PUBLIC_FOLDER_HTML" | grep -q 'shared.txt'; then
        pass_msg "Public folder share lists folder contents without session"
    else
        fail_msg "Public folder share lists folder contents without session"
    fi

    PUBLIC_FOLDER_FILE=$(curl -s --connect-timeout 3 "http://localhost:$DAR_PORT/s?token=$SHARE_FOLDER_TOKEN&path=/shared.txt")
    if [ "$PUBLIC_FOLDER_FILE" = "shared folder file" ]; then
        pass_msg "Public folder share serves child file without session"
    else
        fail_msg "Public folder share serves child file without session"
    fi

    DELETE_SHARE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data "{\"token\":\"$SHARE_FILE_TOKEN\"}" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/shares/delete")
    DELETED_SHARE_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 "http://localhost:$DAR_PORT/s?token=$SHARE_FILE_TOKEN")
    if echo "$DELETE_SHARE_BODY" | grep -q '"status":"ok"' && [ "$DELETED_SHARE_CODE" = "404" ]; then
        pass_msg "Files API deletes public share links"
    else
        fail_msg "Files API deletes public share links"
    fi

    MOVE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/docs","new_name":"renamed"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/move")
    if echo "$MOVE_BODY" | grep -q '"status":"ok"' && [ -d "data/users/$USER_ID/files/renamed" ]; then
        pass_msg "Files move renames folder in default files root"
    else
        fail_msg "Files move renames folder in default files root"
    fi

    DELETE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/renamed"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/delete")
    TRASH_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/trash/list")
    TRASH_ID=$(printf '%s' "$TRASH_LIST" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
    if echo "$DELETE_BODY" | grep -q '"status":"ok"' && [ ! -e "data/users/$USER_ID/files/renamed" ] && [ -n "$TRASH_ID" ] && echo "$TRASH_LIST" | grep -q '"original_path":"/renamed"'; then
        pass_msg "Files delete moves folder to deleted files"
    else
        fail_msg "Files delete moves folder to deleted files"
    fi

    RESTORE_TRASH_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data "{\"id\":\"$TRASH_ID\"}" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/trash/restore")
    if echo "$RESTORE_TRASH_BODY" | grep -q '"status":"ok"' && [ -d "data/users/$USER_ID/files/renamed" ]; then
        pass_msg "Files deleted files restore returns folder to original path"
    else
        fail_msg "Files deleted files restore returns folder to original path"
    fi

    DELETE_AGAIN_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/renamed"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/delete")
    TRASH_LIST_AGAIN=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/trash/list")
    TRASH_ID_AGAIN=$(printf '%s' "$TRASH_LIST_AGAIN" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
    PURGE_TRASH_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data "{\"id\":\"$TRASH_ID_AGAIN\"}" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/trash/purge")
    TRASH_AFTER_PURGE=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/trash/list")
    if echo "$DELETE_AGAIN_BODY" | grep -q '"status":"ok"' && echo "$PURGE_TRASH_BODY" | grep -q '"status":"ok"' && ! echo "$TRASH_AFTER_PURGE" | grep -q "$TRASH_ID_AGAIN"; then
        pass_msg "Files deleted files purge permanently removes trash entry"
    else
        fail_msg "Files deleted files purge permanently removes trash entry"
    fi

    ACTIVITY_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/activity/list")
    if echo "$ACTIVITY_LIST" | grep -q '"action":"uploaded"' && echo "$ACTIVITY_LIST" | grep -q '"action":"shared"' && echo "$ACTIVITY_LIST" | grep -q '"action":"deleted"' && echo "$ACTIVITY_LIST" | grep -q '"action":"restored from trash"' && echo "$ACTIVITY_LIST" | grep -q '"action":"restored version"'; then
        pass_msg "Files activity API records everyday file operations"
    else
        fail_msg "Files activity API records everyday file operations"
    fi

    mkdir -p "data/users/$USER_ID/files/source" "data/users/$USER_ID/files/target"
    printf 'copy me' > "data/users/$USER_ID/files/source/copy.txt"
    PICKER_LIST=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/picker?mode=folder&path=/")
    if echo "$PICKER_LIST" | grep -q '"name":"source"' && echo "$PICKER_LIST" | grep -q '"is_dir":true'; then
        pass_msg "Files picker API returns folders for cross-app picker use"
    else
        fail_msg "Files picker API returns folders for cross-app picker use"
    fi
    COPY_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/source/copy.txt","target_path":"/target","mode":"copy"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/transfer")
    if echo "$COPY_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/target/copy.txt" 2>/dev/null || true)" = "copy me" ]; then
        pass_msg "Files copy copies file to target folder"
    else
        fail_msg "Files copy copies file to target folder"
    fi

    rm -f "data/users/$USER_ID/files/target/copy.txt"
    MOVE_FILE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/source/copy.txt","target_path":"/target","mode":"move"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/transfer")
    if echo "$MOVE_FILE_BODY" | grep -q '"status":"ok"' && [ ! -e "data/users/$USER_ID/files/source/copy.txt" ] && [ "$(cat "data/users/$USER_ID/files/target/copy.txt" 2>/dev/null || true)" = "copy me" ]; then
        pass_msg "Files move moves file to target folder"
    else
        fail_msg "Files move moves file to target folder"
    fi
fi

# ── Test 6: /api/me without session → redirect ──
API_NOAUTH=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/me")
if [ "$API_NOAUTH" = "302" ]; then
    pass_msg "/api/me redirects to /login without session"
else
    fail_msg "/api/me redirects to /login without session (got $API_NOAUTH)"
fi

# ── Test 7: Logout clears session ──
if [ -n "$SESSION_COOKIE" ]; then
    LOGOUT=$(curl -s -D- -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/logout" 2>&1)
    if echo "$LOGOUT" | grep -q 'set-cookie: dar_session=;'; then
        pass_msg "Logout clears session cookie"
    else
        fail_msg "Logout clears session cookie"
    fi
fi

# ── Test 8: Unknown path → 404 ──
NOTFOUND=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 "http://localhost:$DAR_PORT/nonexistent")
if [ "$NOTFOUND" = "404" ]; then
    pass_msg "Unknown path returns 404"
else
    fail_msg "Unknown path returns 404 (got $NOTFOUND)"
fi

# ── Test 9: --help works ──
if $BIN --help 2>&1 | grep -q 'Usage: dar'; then
    pass_msg "--help prints usage"
else
    fail_msg "--help prints usage"
fi

# ── Test 10: --help doesn't start server ──
HELP_OUT=$($BIN --help 2>&1)
if ! echo "$HELP_OUT" | grep -q 'listening'; then
    pass_msg "--help exits without starting server"
else
    fail_msg "--help exits without starting server"
fi

# ── Summary ──
echo ""
echo "============================="
echo -e "Passed: ${GREEN}$pass${NC}  Failed: ${RED}$fail${NC}"
echo "============================="

if [ "$fail" -gt 0 ]; then
    exit 1
fi
