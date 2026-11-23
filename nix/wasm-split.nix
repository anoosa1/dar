# wasm-split-cli: splits the client's #[lazy] apps into their own wasm chunks (xtask/src/client.rs).
{ rustPlatform, fetchurl }:
rustPlatform.buildRustPackage rec {
  pname = "wasm-split-cli";
  version = "0.2.5";
  src = fetchurl {
    name = "wasm_split_cli_support-${version}.tar.gz";
    url = "https://static.crates.io/crates/wasm_split_cli_support/wasm_split_cli_support-${version}.crate";
    hash = "sha256-DTAQguJ+uDbxiq4dHGQ/Wt7SZtK6dGq1Hks5bK2t6f8=";
  };
  cargoLock.lockFile = ./wasm-split.Cargo.lock;
  postPatch = "cp ${./wasm-split.Cargo.lock} Cargo.lock";
  buildFeatures = [ "build-binary" ];
  doCheck = false;
}
