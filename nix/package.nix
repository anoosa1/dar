{ lib, stdenv, stdenvNoCC, fetchurl, runCommand, buildNpmPackage, rustPlatform, cargo, rustc, lld, perl, wasm-bindgen-cli, binaryen, brotli, gzip, bash, coreutils, gnugrep, curl, git, exiftool, vips, ffmpeg-headless, libarchive, makeWrapper, runtime, chatBridge }:
let
  fs = lib.fileset;
  wasmSplit = import ../rust/nix/wasm-split.nix { inherit rustPlatform fetchurl; };

  sdkSource = lib.cleanSourceWith {
    src = ../sdk;
    filter = path: _: !(builtins.elem (baseNameOf path) [ "dist" "node_modules" ]);
  };

  jsSource = runCommand "dar-js-assets-source" { } ''
    mkdir -p $out/vendor
    cp ${../package.json} $out/package.json
    cp ${../package-lock.json} $out/package-lock.json
    cp -r ${../vendor/no-native-canvas} $out/vendor/no-native-canvas
    cp -r ${sdkSource} $out/sdk
  '';

  jsAssets = buildNpmPackage {
    pname = "dar-js-assets";
    version = "0.1.0";
    src = jsSource;
    nodejs = runtime.nodeWithNpm;
    npmDeps = runtime.npmDeps;
    dontNpmBuild = true;
    buildPhase = ''
      runHook preBuild
      node sdk/build-bundle.mjs
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/sdk $out/node_modules/pdfjs-dist
      cp -r sdk/dist $out/sdk/dist
      cp node_modules/pdfjs-dist/package.json $out/node_modules/pdfjs-dist/
      cp -r node_modules/pdfjs-dist/build $out/node_modules/pdfjs-dist/
      for folder in cmaps standard_fonts wasm iccs; do
        if [[ -d "node_modules/pdfjs-dist/$folder" ]]; then
          cp -r "node_modules/pdfjs-dist/$folder" "$out/node_modules/pdfjs-dist/$folder"
        fi
      done
      runHook postInstall
    '';
    meta.platforms = [ "x86_64-linux" "aarch64-linux" ];
  };

  spacetimedbBindingsMacroCrate = fetchurl {
    url = "https://static.crates.io/crates/spacetimedb-bindings-macro/spacetimedb-bindings-macro-2.14.1.crate";
    sha256 = "12687a386f7994d79a66b5e483335ffa3a4d4b4793ad29dbd640e09113c6b188";
  };

  moduleSource = lib.cleanSourceWith {
    src = ../rust/module;
    filter = path: _: !(builtins.elem (baseNameOf path) [ "target" ".gitignore" ]);
  };

  darModule = stdenv.mkDerivation {
    pname = "dar-module";
    version = "0.1.0";
    src = moduleSource;
    cargoDeps = rustPlatform.importCargoLock { lockFile = ../rust/module/Cargo.lock; };
    nativeBuildInputs = [ rustPlatform.cargoSetupHook cargo rustc lld perl ];
    postPatch = ''
      # Replicates rust/module/patch-macro.sh without network access.
      dir="target/patches/spacetimedb-bindings-macro-2.14.1"
      mkdir -p target/patches
      tar -xzf ${spacetimedbBindingsMacroCrate} -C target/patches
      perl -0pi -e 's/name: field\.ident\.as_ref\(\)\.map\(syn::Ident::to_string\),/name: field.ident.as_ref().map(|i| syn::ext::IdentExt::unraw(i).to_string()),/' "$dir/src/sats.rs"
      grep -q 'IdentExt::unraw(i).to_string()' "$dir/src/sats.rs"
      perl -0pi -e 's/let accessor_name = self\.accessor_name\.to_string\(\);/let accessor_name = { let mut out = String::new(); let mut upper = false; for c in syn::ext::IdentExt::unraw(self.accessor_name).to_string().chars() { if c == \x27_\x27 { upper = true; } else if upper { out.extend(c.to_uppercase()); upper = false; } else { out.push(c); } } out };/' "$dir/src/table.rs"
      grep -q 'upper = true' "$dir/src/table.rs"
      touch "$dir/.patched-2"
    '';
    buildPhase = ''
      runHook preBuild
      export CARGO_NET_OFFLINE=true
      cargo build --release --frozen --offline --target wasm32-unknown-unknown
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      install -Dm644 target/wasm32-unknown-unknown/release/dar_module.wasm $out/dar_module.wasm
      runHook postInstall
    '';
  };

  brotliCrate = fetchurl {
    url = "https://static.crates.io/crates/brotli/brotli-3.5.0.crate";
    sha256 = "d640d25bc63c50fb1f0b545ffd80207d2e10a4c965530809b40ba3386825c391";
  };

  clientSource = lib.cleanSourceWith {
    src = ../rust/client;
    filter = path: _: !(builtins.elem (baseNameOf path) [ "target" "dist" "module_bindings" ".gitignore" ]);
  };

  darClient = stdenv.mkDerivation {
    pname = "dar-client";
    version = "0.1.0";
    src = clientSource;
    sourceRoot = "work/rust/client";
    cargoDeps = rustPlatform.importCargoLock { lockFile = ../rust/client/Cargo.lock; };
    nativeBuildInputs = [ rustPlatform.cargoSetupHook cargo rustc lld perl wasm-bindgen-cli wasmSplit binaryen brotli gzip runtime.spacetime ];
    unpackPhase = ''
      runHook preUnpack
      mkdir -p work/rust
      cp -r ${clientSource} work/rust/client
      cp -r ${jsAssets}/node_modules work/node_modules
      cp -r ${sdkSource} work/sdk
      chmod -R u+w work
      cp -r ${jsAssets}/sdk/dist work/sdk/dist
      chmod -R u+w work/sdk/dist
      runHook postUnpack
    '';
    postPatch = ''
      patchShebangs build.sh patch-brotli.sh

      # Replicates rust/client/patch-brotli.sh without network access.
      dir="target/patches/brotli-3.5.0"
      mkdir -p target/patches
      tar -xzf ${brotliCrate} -C target/patches
      perl -0pi -e 's/default = \[\n    "std",\n    "ffi-api",\n\]/default = ["std"]/' "$dir/Cargo.toml"
      printf '\n[lints.rust]\nunexpected_cfgs = "allow"\ndead_code = "allow"\nmismatched_lifetime_syntaxes = "allow"\n' >> "$dir/Cargo.toml"
      grep -q '^default = \["std"\]$' "$dir/Cargo.toml"
      touch "$dir/.patched"

      # Replicates rust/client/generate.sh against the built Rust module.
      cli_tmp=$(mktemp -d)
      rm -rf src/module_bindings
      spacetime --root-dir "$cli_tmp" generate --lang rust --bin-path ${darModule}/dar_module.wasm --out-dir src/module_bindings --yes --no-config 2>/dev/null || true
      test -f src/module_bindings/mod.rs
      sed -i -E 's/^(\s*)(pub )?type: /\1\2r#type: /' src/module_bindings/*.rs
    '';
    buildPhase = ''
      runHook preBuild
      export CARGO_NET_OFFLINE=true
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      bash ./build.sh --release
      test -f dist/index.html
      test -f dist/sdk.js
      test -d dist/pdfjs
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp -r dist/. $out/
      runHook postInstall
    '';
  };

  runtimeSource = fs.toSource {
    root = ../rust;
    fileset = fs.difference
      (fs.unions [
        ../rust/Cargo.toml
        ../rust/Cargo.lock
        ../rust/runtime
        ../rust/oidc
        ../rust/dav
      ])
      (fs.unions [
        (fs.maybeMissing ../rust/runtime/dev.sh)
        (fs.maybeMissing ../rust/runtime/serve.sh)
      ]);
  };

  darRuntime = rustPlatform.buildRustPackage {
    pname = "dar-runtime";
    version = "0.1.0";
    src = runtimeSource;
    cargoLock.lockFile = ../rust/Cargo.lock;
    cargoBuildFlags = [ "-p" "dar-runtime" ];
    cargoTestFlags = [ "-p" "dar-runtime" "-p" "dar-oidc" "-p" "dar-dav" ];
    nativeCheckInputs = [ git gzip ];
  };
in
stdenvNoCC.mkDerivation {
  pname = "dar";
  version = "0.1.0";
  dontUnpack = true;
  dontConfigure = true;
  dontBuild = true;
  nativeBuildInputs = [ makeWrapper ];
  buildInputs = [ bash ];
  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin $out/libexec/dar $out/share/dar
    cp ${darRuntime}/bin/dar-runtime $out/libexec/dar/dar-runtime
    install -Dm755 ${../rust/runtime/serve.sh} $out/libexec/dar/serve.sh
    cp ${darModule}/dar_module.wasm $out/share/dar/dar_module.wasm
    cp -r ${darClient} $out/share/dar/client

    makeWrapper ${bash}/bin/bash $out/bin/dar \
      --add-flags "$out/libexec/dar/serve.sh" \
      --set DAR_RUST_RUNTIME "$out/libexec/dar/dar-runtime" \
      --set DAR_RUST_MODULE "$out/share/dar/dar_module.wasm" \
      --set DAR_CLIENT_DIST "$out/share/dar/client" \
      --set DAR_SPACETIME_BIN "${runtime.spacetime}/bin/spacetime" \
      --set-default DAR_CHAT_BRIDGE "${chatBridge}/bin/dar-chat-bridge" \
      --prefix PATH : "${lib.makeBinPath [ bash coreutils gnugrep curl git gzip exiftool vips ffmpeg-headless libarchive ]}"

    {
      printf '#!%s/bin/bash\n' "${bash}"
      printf 'runtime=%q\n' "$out/libexec/dar/dar-runtime"
      cat <<'EOF'
set -euo pipefail
cmd="''${1:-}"
case "$cmd" in
  apps-*)
    if [[ -z "''${DAR_ROOT:-}" ]]; then
      echo "DAR_ROOT is required." >&2
      exit 2
    fi
    shift
    exec "$runtime" "$cmd" "$DAR_ROOT" "$@"
    ;;
  backup|restore|export-files|release-backup|enable-user|disable-user|recover-password|configure-oidc|deliver-reminders|reconcile-storage|sweep-orphans)
    exec "$runtime" "$@"
    ;;
  *)
    echo "Usage: dar-operator backup|restore <directory>, export-files <username> <directory>, release-backup <id>, enable-user|disable-user <username>, recover-password <username> <protected-password-file>, configure-oidc <file> [optional], deliver-reminders, reconcile-storage, sweep-orphans, or apps-list|apps-install|apps-enable|apps-disable|apps-uninstall [argument]" >&2
    exit 2
    ;;
esac
EOF
    } > $out/bin/dar-operator
    chmod 0755 $out/bin/dar-operator
    runHook postInstall
  '';
  passthru = {
    runtime = darRuntime;
    module = darModule;
    client = darClient;
    inherit jsAssets;
  };
  meta = {
    description = "Self-hosted personal cloud with SpacetimeDB";
    platforms = [ "x86_64-linux" "aarch64-linux" ];
    mainProgram = "dar";
  };
}
