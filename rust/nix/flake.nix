{
  description = "Dar Rust prototypes (not part of the Dar package yet)";
  # Kept in its own directory: `nix develop path:rust/nix` copies only this folder to the store, not the build outputs.
  # Same pin as the root flake.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/624af665418d3c65d544145b4d34ad696439570e";
  outputs = { nixpkgs, ... }:
    let eachSystem = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ];
    in {
      devShells = eachSystem (system: let pkgs = import nixpkgs { inherit system; }; in {
        # nixpkgs' rustc ships the wasm32-unknown-unknown standard library the module needs.
        default = pkgs.mkShell {
          # wasm-bindgen-cli matches the client's pinned wasm-bindgen (0.2.121).
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.lld pkgs.wasm-bindgen-cli pkgs.binaryen pkgs.brotli pkgs.pkg-config pkgs.openssl ];
          # SpacetimeDB's test datastore builds jemalloc at -O0, which glibc's _FORTIFY_SOURCE rejects.
          hardeningDisable = [ "fortify" ];
        };
      });
    };
}
