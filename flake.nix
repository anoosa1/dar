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
        prayer-times = pkgs.callPackage ./nix/prayer-times.nix { runtime = runtimeFor system; };
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
          services.dar = {
            enable = true; origin = "https://dar.example.test"; configureNginx = true;
            oidc.clients = {
              gitea = { name = "Gitea"; requirePkce = false; redirectUris = [ "https://git.example.test/user/oauth2/dar/callback" ]; secretFile = "/run/secrets/dar-gitea"; };
              photos = { name = "Photos"; redirectUris = [ "https://photos.example.test/callback" ]; secretFile = "/run/secrets/dar-photos"; };
              public = { name = "Public"; type = "public"; redirectUris = [ "https://public.example.test/callback" ]; };
            };
          };
          security.acme = { acceptTerms = true; defaults.email = "operator@example.test"; };
          system.stateVersion = "26.05";
        }) ]; };
        service = evaluated.config.systemd.services.dar;
        nginxConfig = evaluated.config.environment.etc."nginx/nginx.conf".source;
        invalidPublic = evaluated.extendModules { modules = [ { services.dar.oidc.clients.public.requirePkce = false; } ]; };
        agentEnabled = evaluated.extendModules { modules = [ { services.dar.agent = {
          enable = true; package = pkgs.writeShellScriptBin "codex" "exit 1";
          userId = "00000000-0000-0000-0000-000000000000"; gitHosts = [ "github.com" ];
        }; } ]; };
        agentService = agentEnabled.config.systemd.services.dar;
      in { module-evaluation = assert service.serviceConfig.User == "dar";
        assert service.environment.DAR_ROOT == "/var/lib/dar";
        assert service.serviceConfig.ProtectSystem == "strict";
        assert service.serviceConfig.LoadCredential == [ "oidc-gitea:/run/secrets/dar-gitea" "oidc-photos:/run/secrets/dar-photos" ];
        assert evaluated.config.services.dar.oidc.clients.gitea.requirePkce == false;
        assert evaluated.config.services.dar.oidc.clients.photos.requirePkce;
        assert evaluated.config.services.dar.oidc.clients.public.requirePkce;
        assert agentService.serviceConfig.ProtectHome;
        assert agentService.serviceConfig.MemoryMax == "2G";
        assert agentService.serviceConfig.TasksMax == 256;
        assert builtins.elem "/var/lib/dar-codex" agentService.serviceConfig.ReadWritePaths;
        assert builtins.hasAttr "DAR_CODEX_FILE" agentService.environment;
        assert builtins.elem agentEnabled.config.services.dar.agent.package agentService.path;
        assert builtins.any (item: !item.assertion && nixpkgs.lib.hasPrefix "services.dar.oidc.clients.public " item.message) invalidPublic.config.assertions;
        assert builtins.hasAttr "DAR_OIDC_FILE" service.environment;
        assert builtins.hasAttr "dar.example.test" evaluated.config.services.nginx.virtualHosts;
        assert builtins.elem "dar.example.test" evaluated.config.networking.hosts."127.0.0.1";
        pkgs.runCommand "dar-module-evaluation" {} ''
          grep -F '/run/credentials/dar.service/oidc-photos' ${service.environment.DAR_OIDC_FILE}
          if grep -F '/run/secrets/dar-photos' ${service.environment.DAR_OIDC_FILE}; then exit 1; fi
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
