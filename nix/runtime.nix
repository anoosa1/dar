{ pkgs }:
let
  sources = {
    x86_64-linux = {
      nodePlatform = "x64";
      nodeHash = "sha512-ogMoFZRQLCX7VpzQAtfs7q0DVFa7D79LKhmttHZNGJrz0Yp5/LtU0MISM7gvThSo6AbVelac9uVu9Jfm1464ow==";
      spacetimePlatform = "x86_64";
      spacetimeHash = "67a3c99c19abc4ba58b4ab999b309b3c5899602b3f0f7ae75e296335c174c82b";
    };
    aarch64-linux = {
      nodePlatform = "arm64";
      nodeHash = "sha512-1uD+Qvcu14ZJ6JVhsAiOeAiCQI0Nnu+y60VJnwhq8nGr+1l7XKFnNlZ42f619d+h5UzRQtO3pwpBEM2FYR/83Q==";
      spacetimePlatform = "aarch64";
      spacetimeHash = "5be10038649944d1711ee4b88888301f7b88463024d72ea68cd32e039510fae7";
    };
  };
  source = sources.${pkgs.stdenv.hostPlatform.system};
  node = pkgs.stdenv.mkDerivation {
    pname = "dar-nodejs"; version = "24.22.1";
    src = pkgs.fetchurl { url = "https://registry.npmjs.org/node-linux-${source.nodePlatform}/-/node-linux-${source.nodePlatform}-24.22.1.tgz"; hash = source.nodeHash; };
    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = [ pkgs.stdenv.cc.cc.lib ];
    dontConfigure = true; dontBuild = true;
    installPhase = ''install -Dm755 bin/node $out/bin/node'';
  };
  npm = pkgs.writeShellScriptBin "npm" ''exec ${node}/bin/node ${pkgs.nodejs_24}/lib/node_modules/npm/bin/npm-cli.js "$@"'';
  nodeWithNpm = pkgs.symlinkJoin { name = "dar-node-with-npm-${node.version}"; paths = [ node npm ]; passthru = { inherit (node) version; python = pkgs.python3; }; };
  spacetime = pkgs.stdenv.mkDerivation {
    pname = "dar-spacetimedb"; version = "2.14.1";
    src = pkgs.fetchurl { url = "https://github.com/clockworklabs/SpacetimeDB/releases/download/v2.14.1/spacetime-${source.spacetimePlatform}-unknown-linux-gnu.tar.gz"; sha256 = source.spacetimeHash; };
    sourceRoot = ".";
    nativeBuildInputs = [ pkgs.autoPatchelfHook ];
    buildInputs = [ pkgs.stdenv.cc.cc.lib pkgs.zlib ];
    dontConfigure = true; dontBuild = true;
    installPhase = ''
      install -Dm755 spacetimedb-cli $out/bin/spacetimedb-cli
      install -Dm755 spacetimedb-standalone $out/bin/spacetimedb-standalone
      ln -s spacetimedb-cli $out/bin/spacetime
    '';
  };
in { inherit node npm nodeWithNpm spacetime; }
