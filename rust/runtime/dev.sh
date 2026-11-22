#!/usr/bin/env bash
# Runs Dar on the Rust stack for development: SpacetimeDB with the Rust module, the Rust
# runtime and the Rust client, laid out as next/scripts/dev.mjs lays out the TypeScript stack.
# Its own data root and ports keep it apart from a TypeScript instance.
# Usage, from Dar's root dev shell (for the spacetime CLI): rust/runtime/dev.sh
# Environment: DAR_ROOT (default $XDG_DATA_HOME/dar-rust), DAR_PORT (4331), DAR_DATABASE_PORT
# (4330), DAR_HOST (127.0.0.1; 0.0.0.0 listens on every interface), DAR_ORIGIN (the address
# browsers use, default http://127.0.0.1:$DAR_PORT; an https:// origin, such as `tailscale serve`
# in front of the loopback port, runs in production mode), DAR_PREBUILT=1 to skip building.
set -euo pipefail
umask 077
rust=$(cd "$(dirname "$0")/.." && pwd)
root=${DAR_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/dar-rust}
port=${DAR_PORT:-4331}
database_port=${DAR_DATABASE_PORT:-4330}
host=${DAR_HOST:-127.0.0.1}
origin=${DAR_ORIGIN:-http://127.0.0.1:$port}
# Plain HTTP is development (loopback only); an HTTPS origin behind a proxy is production.
development=1; [[ $origin == https://* ]] && development=
database_url="http://127.0.0.1:$database_port"
name=${DAR_DATABASE_NAME:-dar}
cli_root="$root/cli"
# As openRoot does: a new root is marked as a Dar root before anything is put in it.
mkdir -p "$root"
if [[ ! -e "$root/instance.json" && -z "$(ls -A "$root")" ]]; then
  printf '{"format":"dar-spacetime-v1"}' > "$root/instance.json"
fi
mkdir -p "$root/secrets" "$root/database"

if [[ "${DAR_PREBUILT:-}" != 1 ]]; then
  nix develop "path:$rust/nix" -c bash -c "
    set -e
    cd '$rust/module' && ./patch-macro.sh && cargo build -q --release --target wasm32-unknown-unknown
    cd '$rust/client' && ./build.sh --release >/dev/null
    cd '$rust' && cargo build -q -p dar-runtime"
fi

bootstrap="$root/secrets/bootstrap-token"
[[ -f "$bootstrap" ]] || head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n' > "$bootstrap"

pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; wait 2>/dev/null || true; }
trap cleanup EXIT INT TERM

spacetime --root-dir "$cli_root" start --data-dir "$root/database" --listen-addr "127.0.0.1:$database_port" --non-interactive >>"$root/database.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 300); do curl -sf "$database_url/v1/ping" >/dev/null && break; sleep 0.1; done

DAR_ISSUER="$origin/auth/issuer" spacetime --root-dir "$cli_root" publish "$name" --bin-path "$rust/module/target/wasm32-unknown-unknown/release/dar_module.wasm" --server "$database_url" --yes --no-config >/dev/null
token=$(grep -oE 'spacetimedb_token = "[^"]+"' "$cli_root/config/cli.toml" | cut -d'"' -f2)
[[ -n "$token" ]] || { echo "The local database service identity was not initialized." >&2; exit 1; }
token_file="$root/secrets/database-token"
if [[ -f "$token_file" ]]; then
  [[ "$(cat "$token_file")" == "$token" ]] || { echo "Stored database identity changed; refusing to overwrite it." >&2; exit 1; }
else
  printf '%s' "$token" > "$token_file"
fi

DAR_ROOT="$root" DAR_HOST="$host" DAR_ORIGIN="$origin" DAR_PORT="$port" DAR_DATABASE_URL="$database_url" DAR_DATABASE_NAME="$name" \
DAR_DATABASE_TOKEN_FILE="$token_file" DAR_BOOTSTRAP_TOKEN_FILE="$bootstrap" DAR_DEVELOPMENT="$development" \
DAR_CLIENT_DIST="$rust/client/dist" "$rust/target/debug/dar-runtime" &
pids+=($!)
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$port/api/v1/auth/status" >/dev/null && break; sleep 0.1; done
echo "Dar (Rust stack) is ready at $origin. The operator setup key is in $bootstrap."
wait -n "${pids[@]}"
