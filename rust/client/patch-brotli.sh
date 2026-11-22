#!/usr/bin/env bash
# SpacetimeDB's SDK depends on brotli 3.5 with its default features, which include the C API
# (`ffi-api`). Its exported functions keep the whole Brotli encoder in the client (about a third
# of the download) although the SDK only decompresses. This prepares the published crate,
# checked against its crates.io checksum, with `ffi-api` dropped from the defaults; Cargo.toml
# patches it in. Run before any cargo command (build.sh does).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dir="$here/target/patches/brotli-3.5.0"
[[ -f "$dir/.patched" ]] && exit 0
sum=d640d25bc63c50fb1f0b545ffd80207d2e10a4c965530809b40ba3386825c391
mkdir -p "$here/target/patches"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl -sSfL https://static.crates.io/crates/brotli/brotli-3.5.0.crate -o "$tmp"
echo "$sum  $tmp" | sha256sum -c --quiet
rm -rf "$dir"
tar -xzf "$tmp" -C "$here/target/patches"
perl -0pi -e 's/default = \[\n    "std",\n    "ffi-api",\n\]/default = ["std"]/' "$dir/Cargo.toml"
# Its own lints are not ours to fix.
printf '\n[lints.rust]\nunexpected_cfgs = "allow"\ndead_code = "allow"\nmismatched_lifetime_syntaxes = "allow"\n' >> "$dir/Cargo.toml"
grep -q '^default = \["std"\]$' "$dir/Cargo.toml"
touch "$dir/.patched"
