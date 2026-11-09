{ lib, buildNpmPackage, makeWrapper, runtime, git }:
buildNpmPackage {
  pname = "dar";
  version = "0.1.0";
  src = lib.cleanSourceWith {
    src = ../next;
    filter = path: type: !(lib.hasPrefix ".env" (baseNameOf path)) && !(builtins.elem (baseNameOf path) [ "node_modules" "dist" "module_bindings" "state" ".git" ".npmrc" "test" "flake.nix" "flake.lock" "README.md" "AGENTS.md" ]);
  };
  nodejs = runtime.nodeWithNpm;
  npmDepsHash = "sha256-yiyj4iuq1zivPfP2z0/Y3QL+USR7zMwrG78Ag2SxYS0=";
  nativeBuildInputs = [ makeWrapper runtime.spacetime ];
  dontNpmBuild = true;
  buildPhase = ''
    runHook preBuild
    export HOME=$TMPDIR/home
    mkdir -p "$HOME"
    export DAR_SPACETIME_BIN=${runtime.spacetime}/bin/spacetime
    export DAR_SPACETIME_CLI_ROOT=$TMPDIR/spacetime-cli
    node scripts/build-package.mjs
    npm prune --omit=dev --offline --ignore-scripts
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/dar $out/bin
    cp -r runtime shared sdk dist module package.json package-lock.json node_modules $out/lib/dar/
    mkdir -p $out/lib/dar/scripts
    cp scripts/dev.mjs scripts/serve.mjs scripts/operator.ts $out/lib/dar/scripts/
    rm -rf $out/lib/dar/module/src
    makeWrapper ${runtime.node}/bin/node $out/bin/dar \
      --add-flags "$out/lib/dar/scripts/serve.mjs" \
      --set DAR_PREBUILT 1 \
      --prefix PATH : ${lib.makeBinPath [ git runtime.node ]} \
      --set DAR_SPACETIME_BIN ${runtime.spacetime}/bin/spacetime
    makeWrapper ${runtime.node}/bin/node $out/bin/dar-operator \
      --add-flags "$out/lib/dar/scripts/operator.ts"
    runHook postInstall
  '';
  meta = { description = "Self-hosted personal cloud with SpacetimeDB"; platforms = [ "x86_64-linux" "aarch64-linux" ]; mainProgram = "dar"; };
}
