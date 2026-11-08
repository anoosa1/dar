{ lib, buildNpmPackage, runCommand, runtime }:
let
  appSource = lib.cleanSourceWith { src = ../apps/prayer-times; filter = path: type: !(builtins.elem (baseNameOf path) [ "dist" "node_modules" ]); };
  source = runCommand "dar-prayer-times-source" {} ''
    mkdir -p $out/sdk
    cp ${../next/package.json} $out/package.json
    cp ${../next/package-lock.json} $out/package-lock.json
    cp ${../next/sdk/build.mjs} $out/sdk/build.mjs
    cp -r ${appSource} $out/app
  '';
in buildNpmPackage {
  pname = "dar-prayer-times"; version = "0.1.0";
  src = source; nodejs = runtime.nodeWithNpm;
  npmDepsHash = "sha256-yiyj4iuq1zivPfP2z0/Y3QL+USR7zMwrG78Ag2SxYS0=";
  dontNpmBuild = true;
  buildPhase = "node sdk/build.mjs app";
  installPhase = "mkdir -p $out; cp -r app/dist/. $out/";
  meta = { description = "Prayer Times app for Dar"; license = lib.licenses.mit; platforms = [ "x86_64-linux" "aarch64-linux" ]; };
}
