{
  description = "Dar personal cloud — package and NixOS service";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/624af665418d3c65d544145b4d34ad696439570e";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
      runtimeFor = system: import ./nix/runtime.nix { pkgs = pkgsFor system; };
    in {
      packages = eachSystem (system: let pkgs = pkgsFor system; in {
        dar = pkgs.callPackage ./nix/package.nix { runtime = runtimeFor system; };
        default = self.packages.${system}.dar;
      });
      apps = eachSystem (system: { default = { type = "app"; program = "${self.packages.${system}.dar}/bin/dar"; }; });
      devShells = eachSystem (system: let pkgs = pkgsFor system; runtime = runtimeFor system; in {
        default = pkgs.mkShell { packages = [ runtime.node runtime.npm runtime.spacetime pkgs.chromium pkgs.curl pkgs.git pkgs.vdirsyncer ]; };
      });
      nixosModules.default = import ./nix/module.nix { inherit self; };
      nixosModules.dar = self.nixosModules.default;
      checks = eachSystem (system: let
        pkgs = pkgsFor system;
        evaluated = nixpkgs.lib.nixosSystem { inherit system; modules = [ self.nixosModules.default ({...}: {
          services.dar = { enable = true; origin = "https://dar.example.test"; configureNginx = true; };
          security.acme = { acceptTerms = true; defaults.email = "operator@example.test"; };
          system.stateVersion = "26.05";
        }) ]; };
        service = evaluated.config.systemd.services.dar;
        nginxConfig = evaluated.config.environment.etc."nginx/nginx.conf".source;
      in { module-evaluation = assert service.serviceConfig.User == "dar";
        assert service.environment.DAR_ROOT == "/var/lib/dar";
        assert service.serviceConfig.ProtectSystem == "strict";
        assert builtins.hasAttr "dar.example.test" evaluated.config.services.nginx.virtualHosts;
        pkgs.runCommand "dar-module-evaluation" {} ''
          grep -F '127.0.0.1:4311' ${nginxConfig}
          grep -F '127.0.0.1:4310/v1/database/dar/subscribe' ${nginxConfig}
          grep -F '127.0.0.1:4310/v1/identity/websocket-token' ${nginxConfig}
          grep -F 'proxy_set_header Host dar.example.test;' ${nginxConfig}
          mkdir $out
          cp ${nginxConfig} $out/nginx.conf
          ln -s ${pkgs.nginx}/bin/nginx $out/nginx
          ln -s ${pkgs.openssl}/bin/openssl $out/openssl
        '';
      });
    };
}
