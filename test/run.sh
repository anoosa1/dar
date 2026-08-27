#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-fast}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v go >/dev/null 2>&1; then
  exec nix-shell -p go chromium isync --run "$0 $MODE"
fi

ARTIFACTS="${DAR_TEST_ARTIFACTS:-$ROOT/test-artifacts}"
mkdir -p "$ARTIFACTS"
WORK="$(mktemp -d /tmp/dar-regression.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

fast() {
  go build ./cmd/...
  go vet ./...
  go test ./cmd/... ./internal/... -count=1
}

coverage() {
  local process_dir="$WORK/process-coverage"
  mkdir -p "$process_dir"
  go test ./cmd/... ./internal/... -count=1 -covermode=atomic -coverpkg=./cmd/...,./internal/... -coverprofile="$WORK/unit.cover"
  DAR_PROCESS_COVERDIR="$process_dir" DAR_TEST_ARTIFACTS="$ARTIFACTS" go test ./test/fullstack -count=1 -timeout=3m
  go tool covdata textfmt -i="$process_dir" -o="$WORK/process.cover"
  go run ./test/coverage_gate.go -global=90 -package=80 -out="$ARTIFACTS/coverage.out" "$WORK/unit.cover" "$WORK/process.cover"
  go tool cover -html="$ARTIFACTS/coverage.out" -o="$ARTIFACTS/coverage.html"
}

pr() {
  fast
  go test -race ./internal/... -count=1
  DAR_TEST_ARTIFACTS="$ARTIFACTS" go test ./test/fullstack -count=1 -timeout=3m
  ./test/e2e_test.sh
  ./test/dav_e2e_test.sh
  coverage
}

nightly() {
  pr
  while IFS= read -r pkg; do
    while IFS= read -r fuzz; do
      [[ -z "$fuzz" ]] && continue
      go test "$pkg" -run '^$' -fuzz="^${fuzz}$" -fuzztime=30s
    done < <(go test "$pkg" -list '^Fuzz' | awk '/^Fuzz/{print $1}')
  done < <(go list ./cmd/... ./internal/...)
  go test -race ./internal/... -run 'Lifecycle|Shutdown|Concurrent|Concurrency|Cancellation|Worker' -count=20
}

case "$MODE" in
  fast) fast ;;
  pr) pr ;;
  nightly) nightly ;;
  *) echo "usage: $0 {fast|pr|nightly}" >&2; exit 2 ;;
esac
