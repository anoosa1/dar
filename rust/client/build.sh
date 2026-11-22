#!/usr/bin/env bash
# Builds the Rust client into dist/: the wasm module and its JS glue, Dar's stylesheet
# (styles/, concatenated in the cascade order of styles/order.txt), the installed-app SDK
# bundle (sdk/dist, from sdk/build-bundle.mjs), pdf.js and index.html. Run in the Rust dev shell: nix develop path:../nix -c ./build.sh [--release]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo="$here/../.."
profile=debug
flags=()
if [[ "${1:-}" == "--release" ]]; then profile=release; flags=(--release); fi
cd "$here"
./patch-brotli.sh
if [[ ! -f src/module_bindings/mod.rs ]]; then
  echo "src/module_bindings is generated (and ignored by Git): run ./generate.sh in Dar's root dev shell first." >&2
  exit 1
fi
RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--cfg=erase_components" cargo build --bin dar-client --target wasm32-unknown-unknown "${flags[@]}"
rm -rf dist && mkdir -p dist
wasm-bindgen --target web --no-typescript --out-dir dist --out-name dar_client "target/wasm32-unknown-unknown/$profile/dar-client.wasm"
if [[ $profile == release ]] && command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --strip-debug --strip-producers --low-memory-unused --zero-filled-memory \
    --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-reference-types \
    -o dist/dar_client_bg.wasm dist/dar_client_bg.wasm
fi
# Stylesheets in cascade order; the shared frame and apps layers come last.
grep -vE '^(#|$)' styles/order.txt | while read -r sheet; do
  cat "styles/$sheet"; echo
done > dist/app.css
# AudioWorklet for Chat voice capture (served at /rust/mic-worklet.js).
cat > dist/mic-worklet.js <<'EOF'
class MicFrames extends AudioWorkletProcessor {
  frame = new Float32Array(960);
  filled = 0;
  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input) return true;
    let offset = 0;
    while (offset < input.length) {
      const take = Math.min(input.length - offset, 960 - this.filled);
      this.frame.set(input.subarray(offset, offset + take), this.filled);
      this.filled += take;
      offset += take;
      if (this.filled === 960) {
        this.port.postMessage(this.frame, [this.frame.buffer]);
        this.frame = new Float32Array(960);
        this.filled = 0;
      }
    }
    return true;
  }
}
registerProcessor("dar-mic", MicFrames);
EOF
# pdf.js library, worker, cmaps, fonts, wasm and iccs (served at /pdfjs/<version>/...).
pdfjs_src="$repo/node_modules/pdfjs-dist"
if [[ -d "$pdfjs_src" ]]; then
  pdfjs_ver=$(sed -nE 's/.*"version": *"([^"]+)".*/\1/p' "$pdfjs_src/package.json" | head -n 1)
  mkdir -p "dist/pdfjs/$pdfjs_ver"
  cp "$pdfjs_src/build/pdf.min.mjs" "dist/pdfjs/$pdfjs_ver/pdf.min.mjs"
  cp "$pdfjs_src/build/pdf.worker.min.mjs" "dist/pdfjs/$pdfjs_ver/pdf.worker.min.mjs"
  for folder in cmaps standard_fonts wasm iccs; do
    [[ -d "$pdfjs_src/$folder" ]] && cp -r "$pdfjs_src/$folder" "dist/pdfjs/$pdfjs_ver/$folder"
  done
fi
# Installed-app SDK bundle (sdk.js, sdk.css, chunks), built by sdk/build-bundle.mjs.
sdk_dist="$repo/sdk/dist"
if [[ -f "$sdk_dist/sdk.js" ]]; then
  cp "$sdk_dist"/sdk*.js "$sdk_dist"/sdk*.css dist/ 2>/dev/null || true
  [[ -d "$sdk_dist/chunks" ]] && cp -r "$sdk_dist/chunks" dist/chunks
fi
# Content-hashed filenames so the runtime can serve JS, CSS and wasm as immutable.
hash_of() { sha256sum "$1" | cut -c1-12; }
wasm_file="dar_client_bg.$(hash_of dist/dar_client_bg.wasm).wasm"
js_file="dar_client.$(hash_of dist/dar_client.js).js"
css_file="app.$(hash_of dist/app.css).css"
mv dist/dar_client_bg.wasm "dist/$wasm_file"
mv dist/dar_client.js "dist/$js_file"
cp dist/app.css "dist/$css_file"
printf 'import init from "/rust/%s";\ninit({module_or_path:"/rust/%s"});\n' "$js_file" "$wasm_file" > dist/start.js
start_file="start.$(hash_of dist/start.js).js"
mv dist/start.js "dist/$start_file"
cat > dist/index.html <<EOF
<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Dar</title><link rel="stylesheet" href="/rust/$css_file"><link rel="modulepreload" href="/rust/$start_file"><link rel="modulepreload" href="/rust/$js_file"><link rel="preload" href="/rust/$wasm_file" as="fetch" crossorigin="anonymous"><script type="module" src="/rust/$start_file"></script></head><body><div id="root"><main class="startup-state" aria-busy="true"><div class="brand-mark"></div><p role="status">Opening your space…</p></main></div></body></html>
EOF
# Precompressed copies, served when the browser accepts them (as the TypeScript runtime once did).
find dist -type f \( -name "*.wasm" -o -name "*.js" -o -name "*.css" -o -name "*.mjs" \) -print0 | while IFS= read -r -d "" file; do
  gzip -9 -k -f "$file"
  if command -v brotli >/dev/null; then brotli -q 11 -k -f "$file"; fi
done
ls -la dist
