{ lib, buildNpmPackage, runCommand, runtime }:
let
  appSource = lib.cleanSourceWith { src = ../apps/prayer-times; filter = path: type: !(builtins.elem (baseNameOf path) [ "dist" "node_modules" ]); };
  source = runCommand "dar-prayer-times-source" {} ''
    mkdir -p $out/sdk $out/vendor
    cp ${../package.json} $out/package.json
    cp ${../package-lock.json} $out/package-lock.json
    cp ${../sdk/build.mjs} $out/sdk/build.mjs
    cp -r ${../vendor/no-native-canvas} $out/vendor/no-native-canvas
    cp -r ${appSource} $out/app
  '';
in buildNpmPackage {
  pname = "dar-prayer-times"; version = "0.1.0";
  src = source; nodejs = runtime.nodeWithNpm;
  npmDeps = runtime.npmDeps;
  dontNpmBuild = true;
  buildPhase = "node sdk/build.mjs app";
  installPhase = "mkdir -p $out; cp -r app/dist/. $out/";
  meta = { description = "Prayer Times app for Dar"; license = lib.licenses.mit; platforms = [ "x86_64-linux" "aarch64-linux" ]; };
}
