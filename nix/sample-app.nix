{ lib, buildNpmPackage, runCommand, runtime }:
let
  appSource = lib.cleanSourceWith { src = ../apps/sample-app; filter = path: type: !(builtins.elem (baseNameOf path) [ "dist" "node_modules" ]); };
  source = runCommand "dar-sample-app-source" {} ''
    mkdir -p $out/sdk
    cp ${../package.json} $out/package.json
    cp ${../package-lock.json} $out/package-lock.json
    cp ${../sdk/build.mjs} $out/sdk/build.mjs
    cp -r ${appSource} $out/app
  '';
in buildNpmPackage {
  pname = "dar-sample-app"; version = "0.1.0";
  src = source; nodejs = runtime.nodeWithNpm;
  npmDeps = runtime.npmDeps;
  dontNpmBuild = true;
  buildPhase = "node sdk/build.mjs app";
  installPhase = "mkdir -p $out; cp -r app/dist/. $out/";
  meta = { description = "Sample installed app for Dar"; license = lib.licenses.mit; platforms = [ "x86_64-linux" "aarch64-linux" ]; };
}
