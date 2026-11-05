#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-fast}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "$(node --version 2>/dev/null || true)" != v24.* ]]; then
  exec nix develop "path:$ROOT" --command "$0" "$MODE"
fi

if [[ ! -d node_modules ]]; then
  npm ci
fi

fast() {
  npm run compile
  node --require ./test/helpers/unit-exit-guard.cjs dist/test/tests.cjs
}

pr() {
  if [[ -n "${DAR_BROWSER_COVERAGE_DIR:-}" ]]; then
    npm run build:typescript
    npm run build:css
    ./node_modules/.bin/shadow-cljs release server client --source-maps
    npm test
  else
    npm run check
  fi
  if [[ -n "${DAR_BROWSER_COVERAGE_DIR:-}" ]]; then
    node test/coverage/freeze.mjs capture
  fi
  node --test --test-concurrency=4 test/*.test.mjs
  browser_journeys
}

browser_journeys() {
  node test/browser/files.mjs
  node test/browser/files-drafts.mjs
  node test/browser/files-settings.mjs
  node test/browser/files-operations.mjs
  node test/browser/files-incoming-shares.mjs
  node test/browser/share-visitor.mjs
  node test/browser/calendar.mjs
  node test/browser/calendar-settings.mjs
  node test/browser/calendar-recurrence.mjs
  node test/browser/calendar-subscription.mjs
  node test/browser/contacts.mjs
  node test/browser/workspace.mjs
  node test/browser/workspace-login.mjs
  node test/browser/workspace-v1.mjs
  node test/browser/home-drafts.mjs
  node test/browser/workspace-access.mjs
  node test/browser/notification-permissions.mjs
  node test/browser/activity.mjs
  node test/browser/chat.mjs
  node test/browser/chat-selection.mjs
  node test/browser/chat-history.mjs
  node test/browser/todo.mjs
  node test/browser/todo-sharing-settings.mjs
  node test/browser/todo-recovery.mjs
  node test/browser/notes.mjs
  node test/browser/notes-files.mjs
  node test/browser/prayer.mjs
  node test/browser/prayer-clock.mjs
  node test/browser/prayer-todo.mjs
  node test/browser/weather.mjs
  node test/browser/news.mjs
  node test/browser/news-drafts.mjs
  node test/browser/email.mjs
  node test/browser/email-incoming.mjs
  node test/browser/ui-polish.mjs
}

coverage() {
  local coverage_root
  coverage_root="$(mktemp -d /tmp/dar-coverage.XXXXXX)"
  echo "Coverage artifacts: $coverage_root"
  export NODE_V8_COVERAGE="$coverage_root"
  export DAR_BROWSER_COVERAGE_DIR="$coverage_root"
  # Freeze unoptimized runtime forms before building the release forms.
  # Both execute the same sources; neither is rebuilt during collection.
  npm run build:typescript
  ./node_modules/.bin/shadow-cljs compile server client
  node test/coverage/freeze.mjs prepare-client
  node test/coverage/freeze.mjs prepare-server
  pr
  DAR_SERVER_BUILD_SNAPSHOT=development node --test --test-concurrency=4 test/*.test.mjs
  DAR_BROWSER_CLIENT_SNAPSHOT=development browser_journeys
  nightly_checks
  node test/coverage/freeze.mjs verify
  node test/coverage/report.mjs "$coverage_root" --gate
}

external_dav() {
  # A test-only client from the repository's locked nixpkgs input. This does
  # not add a production dependency or consult a user's DAV configuration.
  if [[ -z "${DAR_DAV_CLIENT:-}" ]]; then
    local dav_package
    dav_package="$(nix build --no-link --print-out-paths --impure --expr \
      '(builtins.getFlake (toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem}.vdirsyncer')"
    export DAR_DAV_CLIENT="$dav_package/bin/vdirsyncer"
  fi
  node --test test/dav-client.mjs
}

nightly_checks() {
  external_dav
  # Reproducible generated inputs exercise portable parsers and confinement.
  # Use the same compiled unit artifact as PR, with a larger sample count.
  DAR_PROPERTY_ITERATIONS=4096 node --require ./test/helpers/unit-exit-guard.cjs dist/test/tests.cjs
  # Reuse the real lifecycle fixtures instead of duplicating their assertions.
  # Each test owns a fresh root; Email runs the actual hermetic mbsync binary.
  for round in 1 2 3; do
    echo "Lifecycle round $round"
    node --test test/calendar-lifecycle.test.mjs test/chat-cleanup.test.mjs test/chat-turns.test.mjs test/email.test.mjs test/events.test.mjs test/todo-sync.test.mjs
  done
}

nightly() {
  pr
  nightly_checks
}

case "$MODE" in
  fast) fast ;;
  pr) pr ;;
  nightly) nightly ;;
  coverage) coverage ;;
  *) echo "usage: ./test/run.sh {fast|pr|nightly|coverage}" >&2; exit 2 ;;
esac
