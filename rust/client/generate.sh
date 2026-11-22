#!/usr/bin/env bash
# Regenerates src/module_bindings from the Rust module (rust/module, built for wasm32 in
# release), which the client and Rust runtime run on. Its Files functions keep the TypeScript
# module's names and argument order, so the client also works against next/module.
# Run from Dar's root dev shell (`nix develop /path/to/dar`), which provides the spacetime CLI.
# SpacetimeDB 2.14.1's Rust generator does not escape fields named `type`; they are fixed up here.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
wasm="$here/../module/target/wasm32-unknown-unknown/release/dar_module.wasm"
[[ -f "$wasm" ]] || { echo "Build the module first: cd rust/module && cargo build --release --target wasm32-unknown-unknown" >&2; exit 1; }
out="$here/src/module_bindings"
cli=$(mktemp -d)
trap 'rm -rf "$cli"' EXIT
rm -rf "$out"
# The generator formats its output with rustfmt, which rejects the unescaped fields: the files
# are written first, so a formatting failure is tolerated and checked for below.
spacetime --root-dir "$cli" generate --lang rust --bin-path "$wasm" --out-dir "$out" --yes --no-config 2>/dev/null || true
[[ -f "$out/mod.rs" ]] || { echo "Binding generation failed." >&2; exit 1; }
sed -i -E 's/^(\s*)(pub )?type: /\1\2r#type: /' "$out"/*.rs
if command -v rustfmt >/dev/null; then rustfmt --edition 2024 "$out"/*.rs; fi
