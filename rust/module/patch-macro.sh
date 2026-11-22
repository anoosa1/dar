#!/usr/bin/env bash
# SpacetimeDB 2.14.1's derive names a product field after `ident.to_string()`, so a raw
# identifier keeps its `r#` (`r#type`) and the module is rejected. Two of Dar's tables need a
# column named `type` (file_git_objects, immich_checkpoints), as the TypeScript module has.
# Index accessors are recorded in camelCase (`fileId`), as the TypeScript module names them, so
# a TypeScript database upgrades to this module in place; the Rust methods keep snake_case.
# This prepares the published crate, checked against its crates.io checksum, naming fields
# after the unraw identifier; Cargo.toml patches it in. Run before any cargo command.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dir="$here/target/patches/spacetimedb-bindings-macro-2.14.1"
[[ -f "$dir/.patched-2" ]] && exit 0
sum=12687a386f7994d79a66b5e483335ffa3a4d4b4793ad29dbd640e09113c6b188
mkdir -p "$here/target/patches"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl -sSfL https://static.crates.io/crates/spacetimedb-bindings-macro/spacetimedb-bindings-macro-2.14.1.crate -o "$tmp"
echo "$sum  $tmp" | sha256sum -c --quiet
rm -rf "$dir"
tar -xzf "$tmp" -C "$here/target/patches"
perl -0pi -e 's/name: field\.ident\.as_ref\(\)\.map\(syn::Ident::to_string\),/name: field.ident.as_ref().map(|i| syn::ext::IdentExt::unraw(i).to_string()),/' "$dir/src/sats.rs"
grep -q 'IdentExt::unraw(i).to_string()' "$dir/src/sats.rs"
perl -0pi -e 's/let accessor_name = self\.accessor_name\.to_string\(\);/let accessor_name = { let mut out = String::new(); let mut upper = false; for c in syn::ext::IdentExt::unraw(self.accessor_name).to_string().chars() { if c == \x27_\x27 { upper = true; } else if upper { out.extend(c.to_uppercase()); upper = false; } else { out.push(c); } } out };/' "$dir/src/table.rs"
grep -q 'upper = true' "$dir/src/table.rs"
touch "$dir/.patched-2"
