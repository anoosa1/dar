#!/usr/bin/env bash
# End-to-end check of the Rust stack against Dar, on throwaway instances (temporary roots and
# ports; never live data):
#   - the TypeScript stack (next/, with its built module and client: run
#     `npm --prefix next run build:module` and `build:client` first if they are stale);
#   - the Rust stack (rust/runtime/dev.sh: Rust module, runtime and client);
#   - the Rust client in front of the TypeScript stack (client-serve), for compatibility.
# compare.mjs and pim.mjs (Contacts and Calendar) screenshot the same states in React (TypeScript
# stack) and the Rust client (Rust stack) and every pair is diffed; pim.mjs also edits, exports
# and live-updates PIM items through the Rust client. behaviour.mjs runs Files operations through
# the Rust client on both backends; dav.mjs uses WebDAV with an app password and oidc.mjs signs
# in to a relying party, both through the Rust runtime.
# Usage (from Dar's root dev shell): rust/client/test/run.sh
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dar=$(cd "$here/../../.." && pwd)
work=$(mktemp -d)
base=${DAR_TEST_PORT:-4521}
ts_port=$base proxy_port=$((base + 1)) rust_port=$((base + 10))
pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; wait 2>/dev/null || true; }
trap cleanup EXIT
wait_for() { for _ in $(seq 1 240); do curl -sf "$1/api/v1/auth/status" >/dev/null && return; sleep 0.5; done; echo "$1 did not start" >&2; exit 1; }

(cd "$dar/rust/module" && ./patch-macro.sh && nix develop "path:$dar/rust/nix" -c cargo build -q --release --target wasm32-unknown-unknown)
(cd "$dar/rust/client" && nix develop "path:$dar/rust/nix" -c ./build.sh --release >/dev/null)
(cd "$dar/rust" && nix develop "path:$dar/rust/nix" -c cargo build -q -p dar-runtime -p dar-client-serve)

DAR_ROOT="$work/ts" DAR_PORT=$ts_port DAR_DATABASE_PORT=$((ts_port - 1)) DAR_PREBUILT=1 node "$dar/next/scripts/dev.mjs" >"$work/ts.log" 2>&1 &
pids+=($!)
secret=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')
mkdir -p "$work/rust"
printf '[{"id":"test-rp","name":"Test app","kind":"confidential","secret":"%s","redirectUris":["http://127.0.0.1:4599/callback"],"postLogoutRedirectUris":[],"scopes":["openid","profile"]}]' "$secret" >"$work/rust/oidc-clients.json"
DAR_ROOT="$work/rust" DAR_PORT=$rust_port DAR_DATABASE_PORT=$((rust_port - 1)) DAR_PREBUILT=1 DAR_OIDC_CLIENTS_FILE="$work/rust/oidc-clients.json" \
  "$dar/rust/runtime/dev.sh" >"$work/rust.log" 2>&1 &
pids+=($!)
wait_for "http://127.0.0.1:$ts_port"
wait_for "http://127.0.0.1:$rust_port"
"$dar/rust/target/debug/dar-client-serve" "$dar/rust/client/dist" "http://127.0.0.1:$ts_port" "127.0.0.1:$proxy_port" >"$work/serve.log" 2>&1 &
pids+=($!)

export DAR_REACT_ORIGIN="http://127.0.0.1:$ts_port" DAR_REACT_ROOT="$work/ts" DAR_RUST_ORIGIN="http://127.0.0.1:$rust_port" DAR_RUST_ROOT="$work/rust"
node "$here/compare.mjs" "$work/shots"
echo "-- Contacts and Calendar in both clients"
node "$here/pim.mjs" "$work/shots"
echo "-- Rust client on the Rust stack"
DAR_STACK=rust node "$here/behaviour.mjs" "$work/shots"
echo "-- Rust client in front of the TypeScript stack"
DAR_RUST_ORIGIN="http://127.0.0.1:$proxy_port" node "$here/behaviour.mjs" "$work/shots"
echo "-- WebDAV through the Rust runtime"
node "$here/dav.mjs"
echo "-- OIDC through the Rust runtime"
DAR_OIDC_CLIENT_ID=test-rp DAR_OIDC_CLIENT_SECRET="$secret" node "$here/oidc.mjs"

# A pixel differs when its colour is more than 2% away. Rounded corners can still differ by a
# single anti-aliased pixel between runs, so up to five are tolerated.
failed=0
mkdir -p "$work/shots/diff"
for react in "$work"/shots/*-react.png; do
  name=$(basename "$react" -react.png)
  nix shell --inputs-from "$dar" nixpkgs#imagemagick -c magick compare -fuzz 2% "$react" "$work/shots/$name-rust.png" "$work/shots/diff/$name.png" 2>/dev/null || true
  pixels=$(nix shell --inputs-from "$dar" nixpkgs#imagemagick -c magick "$work/shots/diff/$name.png" -fx "(r>0.7&&g<0.3&&b<0.3)?1:0" -format "%[fx:mean*w*h]" info:)
  echo "$name: $pixels differing pixels"
  awk -v p="$pixels" 'BEGIN { exit !(p > 5) }' && failed=1
done
echo "Screenshots and diffs: $work/shots"
trap - EXIT
cleanup
exit $failed
