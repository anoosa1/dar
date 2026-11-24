{ lib, stdenv, stdenvNoCC, fetchurl, runCommand, buildNpmPackage, rustPlatform, cargo, rustc, lld, wasm-bindgen-cli, binaryen, brotli, gzip, bash, coreutils, gnugrep, curl, git, exiftool, vips, ffmpeg-headless, libarchive, makeWrapper, runtime, chatBridge }:
let
  fs = lib.fileset;
  wasmSplit = import ./wasm-split.nix { inherit rustPlatform fetchurl; };

  sdkSource = lib.cleanSourceWith {
    src = ../sdk;
    filter = path: _: !(builtins.elem (baseNameOf path) [ "dist" "node_modules" ]);
  };

  jsSource = runCommand "dar-js-assets-source" { } ''
    mkdir -p $out
    cp ${../package.json} $out/package.json
    cp ${../package-lock.json} $out/package-lock.json
    cp -r ${sdkSource} $out/sdk
    mkdir -p $out/crates/client
    cp -r ${../crates/client/editor} $out/crates/client/editor
    cp -r ${../crates/client/render} $out/crates/client/render
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
      node crates/client/editor/build.mjs notes-editor/notes-editor.js
      node crates/client/render/build.mjs rich-render
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/sdk
      cp -r sdk/dist $out/sdk/dist
      install -Dm644 notes-editor/notes-editor.js $out/notes-editor/notes-editor.js
      install -Dm644 rich-render/katex.js $out/rich-render/katex.js
      install -Dm644 rich-render/mermaid.js $out/rich-render/mermaid.js
      runHook postInstall
    '';
    meta.platforms = [ "x86_64-linux" "aarch64-linux" ];
  };

  # The patched crates' archives, for `xtask setup` offline (see xtask/src/patches.rs).
  patchedCrates = runCommand "dar-patched-crate-archives" { } ''
    mkdir -p $out
    cp ${fetchurl {
      url = "https://static.crates.io/crates/spacetimedb-bindings-macro/spacetimedb-bindings-macro-2.14.1.crate";
      sha256 = "12687a386f7994d79a66b5e483335ffa3a4d4b4793ad29dbd640e09113c6b188";
    }} $out/spacetimedb-bindings-macro-2.14.1.crate
    cp ${fetchurl {
      url = "https://static.crates.io/crates/brotli/brotli-3.5.0.crate";
      sha256 = "d640d25bc63c50fb1f0b545ffd80207d2e10a4c965530809b40ba3386825c391";
    }} $out/brotli-3.5.0.crate
  '';

  # The Cargo workspace (every member's manifest is needed to load it) and the xtask.
  workspaceSource = fs.toSource {
    root = ../.;
    fileset = fs.difference
      # The client's stylesheet order also names an SDK stylesheet (crates/client/styles/order.txt).
      (fs.unions [ ../Cargo.toml ../Cargo.lock ../.cargo ../crates ../tests ../xtask/Cargo.toml ../xtask/Cargo.lock ../xtask/src ../sdk/react/global-search.css ])
      (fs.unions [ (fs.maybeMissing ../crates/bindings/src/generated) (fs.maybeMissing ../crates/runtime/serve.sh) ]);
  };
  workspaceDeps = rustPlatform.importCargoLock { lockFile = ../Cargo.lock; };

  # Builds the xtask (no dependencies) and prepares the patched crates the workspace needs.
  prepareWorkspace = ''
    export CARGO_NET_OFFLINE=true
    export HOME=$TMPDIR/home
    mkdir -p "$HOME"
    cargo build -q --offline --manifest-path xtask/Cargo.toml
    export XTASK=$PWD/xtask/target/debug/xtask
    DAR_CRATE_DIR=${patchedCrates} $XTASK setup
  '';

  darModule = stdenv.mkDerivation {
    pname = "dar-module";
    version = "0.1.0";
    src = workspaceSource;
    cargoDeps = workspaceDeps;
    nativeBuildInputs = [ rustPlatform.cargoSetupHook cargo rustc lld ];
    buildPhase = ''
      runHook preBuild
      ${prepareWorkspace}
      cargo build --release --frozen --offline -p dar-module --target wasm32-unknown-unknown
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      install -Dm644 target/wasm32-unknown-unknown/release/dar_module.wasm $out/dar_module.wasm
      runHook postInstall
    '';
  };

  darClient = stdenv.mkDerivation {
    pname = "dar-client";
    version = "0.1.0";
    src = workspaceSource;
    cargoDeps = workspaceDeps;
    nativeBuildInputs = [ rustPlatform.cargoSetupHook cargo rustc lld wasm-bindgen-cli wasmSplit binaryen brotli gzip runtime.spacetime ];
    buildPhase = ''
      runHook preBuild
      ${prepareWorkspace}
      # The bindings from the built module, then the release client with its dist.
      DAR_MODULE_WASM=${darModule}/dar_module.wasm DAR_SPACETIME_BIN=${runtime.spacetime}/bin/spacetime $XTASK bindings
      DAR_SDK_DIST=${jsAssets}/sdk/dist DAR_NOTES_EDITOR=${jsAssets}/notes-editor/notes-editor.js DAR_KATEX_RENDER=${jsAssets}/rich-render/katex.js DAR_MERMAID_RENDER=${jsAssets}/rich-render/mermaid.js $XTASK client --release
      test -f target/client-release/index.html
      test -f target/client-release/sdk.js
      test -f target/client-release/notes-editor.*.js
      test -f target/client-release/katex-render.*.js
      test -f target/client-release/mermaid-render.*.js
      test -f target/client-release/pdf-worker-*/worker.js
      test -f target/client-release/pdf-worker-*/dar_pdf_worker_bg.wasm
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp -r target/client-release/. $out/
      rm -f $out/.dar-inputs
      runHook postInstall
    '';
  };

  darRuntime = rustPlatform.buildRustPackage {
    pname = "dar-runtime";
    version = "0.1.0";
    src = workspaceSource;
    cargoLock.lockFile = ../Cargo.lock;
    preBuild = prepareWorkspace;
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
    install -Dm755 ${../crates/runtime/serve.sh} $out/libexec/dar/serve.sh
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
