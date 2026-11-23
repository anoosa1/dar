#!/usr/bin/env bash
# Production launcher for Dar's Rust stack: starts SpacetimeDB, publishes the Rust module
# (upgrading an existing TypeScript or Rust database in place), optionally restores a backup,
# and runs dar-runtime. Defaults to production HTTPS mode; pass --development for loopback
# HTTP development mode, or --restore <backup-directory> to restore into an empty root.
set -euo pipefail
umask 077

usage() {
  echo "Usage: $(basename "$0") [--development | --production] [--restore <backup-directory>]" >&2
  exit 2
}

production=1
restore_path=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --development) production=0; shift ;;
    --production) production=1; shift ;;
    --restore)
      [[ $# -ge 2 && -n "$2" ]] || usage
      restore_path="$2"
      shift 2
      ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

repo=$(cd "$(dirname "$0")/../.." && pwd)
root="${DAR_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/dar}"
port="${DAR_PORT:-4311}"
database_port="${DAR_DATABASE_PORT:-4310}"
host="${DAR_HOST:-127.0.0.1}"
name="${DAR_DATABASE_NAME:-dar}"
database_url="http://127.0.0.1:$database_port"
cli_root="${DAR_SPACETIME_CLI_ROOT:-$root/cli}"
spacetime="${DAR_SPACETIME_BIN:-spacetime}"
runtime_bin="${DAR_RUST_RUNTIME:-$repo/target/release/dar-runtime}"
module_wasm="${DAR_RUST_MODULE:-$repo/target/wasm32-unknown-unknown/release/dar_module.wasm}"
client_dist="${DAR_CLIENT_DIST:-${DAR_RUST_CLIENT:-$repo/target/client-release}}"

if [[ $production -eq 1 ]]; then
  origin="${DAR_ORIGIN:-}"
  if [[ -z "$origin" ]]; then
    echo "DAR_ORIGIN is required in production." >&2
    exit 1
  fi
  if [[ ! "$origin" =~ ^https://[^/?#]+$ ]]; then
    echo "Production DAR_ORIGIN must be an HTTPS origin without a trailing slash or path." >&2
    exit 1
  fi
  development=0
else
  origin="${DAR_ORIGIN:-http://127.0.0.1:$port}"
  development=1
fi

if [[ -L "$root" || ( -e "$root" && ! -d "$root" ) ]]; then
  echo "Managed storage root cannot be a symlink or non-directory." >&2
  exit 1
fi
mkdir -p -m 0700 "$root"
if [[ ! -e "$root/instance.json" ]]; then
  if [[ -n "$(ls -A "$root")" ]]; then
    echo "Refusing an unknown non-empty storage root." >&2
    exit 1
  fi
  printf '{"format":"dar-spacetime-v1"}' > "$root/instance.json"
fi
mkdir -p -m 0700 "$root/secrets" "$root/database"

bootstrap="${DAR_BOOTSTRAP_TOKEN_FILE:-$root/secrets/bootstrap-token}"
if [[ ! -e "$bootstrap" ]]; then
  tmp_bootstrap="$bootstrap.$$.tmp"
  head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n' > "$tmp_bootstrap"
  mv -n "$tmp_bootstrap" "$bootstrap"
  rm -f "$tmp_bootstrap"
fi

if [[ -n "$restore_path" ]]; then
  restore_path=$(realpath "$restore_path")
fi

stopping=0
db_pid=""
runtime_pid=""

stop_child() {
  local pid=$1
  [[ -n "$pid" ]] || return 0
  kill -0 "$pid" 2>/dev/null || return 0
  kill -TERM "$pid" 2>/dev/null || true
  for _ in $(seq 1 100); do
    kill -0 "$pid" 2>/dev/null || { wait "$pid" 2>/dev/null || true; return 0; }
    sleep 0.1
  done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

cleanup() {
  trap - EXIT INT TERM
  stop_child "$runtime_pid"
  stop_child "$db_pid"
}

on_signal() {
  stopping=1
  cleanup
  exit 0
}

trap cleanup EXIT
trap on_signal INT TERM

wait_for() {
  local url=$1 pid=$2 label=$3
  for _ in $(seq 1 600); do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "$label stopped during startup." >&2
      return 1
    fi
    if curl -sf --max-time 1 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "Timed out waiting for $label ($url)." >&2
  return 1
}

export DAR_ISSUER="$origin/auth/issuer"

"$spacetime" --root-dir "$cli_root" start \
  --data-dir "$root/database" \
  --listen-addr "127.0.0.1:$database_port" \
  --non-interactive >>"$root/database.log" 2>&1 &
db_pid=$!
wait_for "$database_url/v1/ping" "$db_pid" "SpacetimeDB"

"$spacetime" --root-dir "$cli_root" publish "$name" \
  --bin-path "$module_wasm" \
  --server "$database_url" \
  --yes --no-config

token=$(grep -oE 'spacetimedb_token[[:space:]]*=[[:space:]]*"[^"]+"' "$cli_root/config/cli.toml" | head -n 1 | cut -d'"' -f2)
if [[ -z "$token" ]]; then
  echo "The local database service identity was not initialized." >&2
  exit 1
fi
token_file="${DAR_DATABASE_TOKEN_FILE:-$root/secrets/database-token}"
if [[ -f "$token_file" ]]; then
  if [[ "$(cat "$token_file")" != "$token" ]]; then
    echo "Stored database identity changed; refusing to overwrite it." >&2
    exit 1
  fi
else
  tmp_token="$token_file.$$.tmp"
  printf '%s' "$token" > "$tmp_token"
  mv -n "$tmp_token" "$token_file"
  rm -f "$tmp_token"
fi

if [[ -n "$restore_path" ]]; then
  DAR_ROOT="$root" \
  DAR_DATABASE_URL="$database_url" \
  DAR_DATABASE_NAME="$name" \
  DAR_DATABASE_TOKEN_FILE="$token_file" \
    "$runtime_bin" restore "$restore_path"
fi

DAR_ROOT="$root" \
DAR_HOST="$host" \
DAR_ORIGIN="$origin" \
DAR_PORT="$port" \
DAR_DATABASE_URL="$database_url" \
DAR_DATABASE_NAME="$name" \
DAR_DATABASE_TOKEN_FILE="$token_file" \
DAR_BOOTSTRAP_TOKEN_FILE="$bootstrap" \
DAR_DEVELOPMENT="$development" \
DAR_CLIENT_DIST="$client_dist" \
  "$runtime_bin" &
runtime_pid=$!

printf '{"database":%s,"runtime":%s}' "$db_pid" "$runtime_pid" > "$root/service-pids.json"
wait_for "http://127.0.0.1:$port/api/v1/auth/status" "$runtime_pid" "Dar runtime"
echo "Dar is ready at $origin. The operator setup key is in $bootstrap."

set +e
wait -n "$db_pid" "$runtime_pid"
status=$?
set -e
if [[ $stopping -eq 1 ]]; then
  exit 0
fi
cleanup
if [[ $status -ne 0 ]]; then
  exit "$status"
fi
exit 1
