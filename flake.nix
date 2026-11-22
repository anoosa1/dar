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
        chat-bridge = pkgs.callPackage ./nix/chat-bridge.nix { };
        dar = pkgs.callPackage ./nix/package.nix { runtime = runtimeFor system; chatBridge = self.packages.${system}.chat-bridge; };
        prayer-times = pkgs.callPackage ./nix/prayer-times.nix { runtime = runtimeFor system; };
        default = self.packages.${system}.dar;
      });
      apps = eachSystem (system: { default = { type = "app"; program = "${self.packages.${system}.dar}/bin/dar"; }; });
      devShells = eachSystem (system: let pkgs = pkgsFor system; runtime = runtimeFor system; in {
        default = pkgs.mkShell {
          packages = [
            runtime.node runtime.npm runtime.spacetime
            (pkgs.callPackage ./rust/nix/wasm-split.nix { }) pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.lld pkgs.wasm-bindgen-cli pkgs.binaryen pkgs.brotli pkgs.pkg-config pkgs.openssl pkgs.perl pkgs.gzip
            pkgs.go pkgs.chromium pkgs.curl pkgs.git pkgs.vdirsyncer pkgs.exiftool pkgs.vips pkgs.ffmpeg-headless pkgs.libarchive
          ];
          # SpacetimeDB's test datastore builds jemalloc at -O0, which glibc's _FORTIFY_SOURCE rejects.
          hardeningDisable = [ "fortify" ];
        };
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
        claudeOnly = evaluated.extendModules { modules = [ { services.dar.agent = {
          enable = true; claude.package = pkgs.writeShellScriptBin "claude" "exit 1";
          userId = "00000000-0000-0000-0000-000000000000";
        }; } ]; };
        claudeService = claudeOnly.config.systemd.services.dar;
        mailEnabled = evaluated.extendModules { modules = [ { services.dar.mail.providers = {
          google = { preset = "google"; name = "Google"; oauth = { clientId = "client.apps.example"; clientSecretFile = "/run/secrets/dar-google-mail"; }; };
          work = { name = "Work"; domains = [ "example.test" ]; imap = { host = "mail.example.test"; port = 993; }; smtp = { host = "mail.example.test"; port = 587; security = "starttls"; }; };
        }; } ]; };
        mailService = mailEnabled.config.systemd.services.dar;
        invalidMail = evaluated.extendModules { modules = [ { services.dar.mail.providers.broken = { name = "Broken"; auth = "oauth"; imap = { host = "x"; port = 993; }; smtp = { host = "x"; port = 465; }; }; } ]; };
      in { module-evaluation = assert service.serviceConfig.User == "dar";
        assert service.environment.DAR_ROOT == "/var/lib/dar";
        assert service.serviceConfig.ProtectSystem == "strict";
        assert service.serviceConfig.LoadCredential == [ "oidc-gitea:/run/secrets/dar-gitea" "oidc-photos:/run/secrets/dar-photos" ];
        assert evaluated.config.services.dar.oidc.clients.gitea.requirePkce == false;
        assert evaluated.config.services.dar.oidc.clients.photos.requirePkce;
        assert evaluated.config.services.dar.oidc.clients.public.requirePkce;
        assert agentService.serviceConfig.ProtectHome;
        assert service.serviceConfig.ProtectKernelTunables;
        assert !agentService.serviceConfig.ProtectKernelTunables;
        assert agentService.serviceConfig.ProtectSystem == "strict";
        assert !(builtins.elem "AF_NETLINK" service.serviceConfig.RestrictAddressFamilies);
        assert builtins.elem "AF_NETLINK" agentService.serviceConfig.RestrictAddressFamilies;
        assert agentService.serviceConfig.MemoryMax == "2G";
        assert agentService.serviceConfig.TasksMax == 256;
        assert builtins.elem "/var/lib/dar-codex" agentService.serviceConfig.ReadWritePaths;
        assert builtins.hasAttr "DAR_CODEX_FILE" agentService.environment;
        assert builtins.elem agentEnabled.config.services.dar.agent.package agentService.path;
        assert !(builtins.hasAttr "DAR_CLAUDE_FILE" agentService.environment);
        assert builtins.hasAttr "DAR_CLAUDE_FILE" claudeService.environment && !(builtins.hasAttr "DAR_CODEX_FILE" claudeService.environment);
        assert builtins.elem "/var/lib/dar-claude" claudeService.serviceConfig.ReadWritePaths && !(builtins.elem "/var/lib/dar-codex" claudeService.serviceConfig.ReadWritePaths);
        assert builtins.elem pkgs.bubblewrap claudeService.path && builtins.elem pkgs.socat claudeService.path;
        assert builtins.elem "AF_NETLINK" claudeService.serviceConfig.RestrictAddressFamilies;
        assert builtins.any (item: !item.assertion && nixpkgs.lib.hasPrefix "services.dar.oidc.clients.public " item.message) invalidPublic.config.assertions;
        assert builtins.hasAttr "DAR_OIDC_FILE" service.environment;
        assert mailService.serviceConfig.LoadCredential == [ "oidc-gitea:/run/secrets/dar-gitea" "oidc-photos:/run/secrets/dar-photos" "mail-google:/run/secrets/dar-google-mail" ];
        assert mailEnabled.config.services.dar.mail.providers.google.imap.host == "imap.gmail.com";
        assert !mailEnabled.config.services.dar.mail.providers.google.saveSent;
        assert builtins.any (item: !item.assertion && nixpkgs.lib.hasPrefix "services.dar.mail.providers.broken " item.message) invalidMail.config.assertions;
        assert builtins.hasAttr "dar.example.test" evaluated.config.services.nginx.virtualHosts;
        assert builtins.elem "dar.example.test" evaluated.config.networking.hosts."127.0.0.1";
        pkgs.runCommand "dar-module-evaluation" {} ''
          grep -F '/run/credentials/dar.service/oidc-photos' ${service.environment.DAR_OIDC_FILE}
          if grep -F '/run/secrets/dar-photos' ${service.environment.DAR_OIDC_FILE}; then exit 1; fi
          grep -F '/run/credentials/dar.service/mail-google' ${mailService.environment.DAR_MAIL_PROVIDERS_FILE}
          grep -F '"host":"mail.example.test"' ${mailService.environment.DAR_MAIL_PROVIDERS_FILE}
          if grep -F '/run/secrets/dar-google-mail' ${mailService.environment.DAR_MAIL_PROVIDERS_FILE}; then exit 1; fi
          grep -F '127.0.0.1:4311' ${nginxConfig}
          grep -F '127.0.0.1:4310/v1/database/dar/subscribe' ${nginxConfig}
          grep -F '127.0.0.1:4310/v1/identity/websocket-token' ${nginxConfig}
          grep -F 'proxy_set_header Host dar.example.test;' ${nginxConfig}
          mkdir $out
          cp ${nginxConfig} $out/nginx.conf
          ln -s ${pkgs.nginx}/bin/nginx $out/nginx
          ln -s ${pkgs.openssl}/bin/openssl $out/openssl
        '';
        nixos-vm = pkgs.testers.runNixOSTest {
          name = "dar-nixos-vm";
          nodes.machine = { ... }: {
            imports = [ self.nixosModules.default ];
            virtualisation.memorySize = 1536;
            services.dar = {
              enable = true;
              origin = "https://dar.example.test";
              apps = [ self.packages.${system}.prayer-times ];
            };
            system.stateVersion = "26.05";
          };
          testScript = ''
            import json
            machine.wait_for_unit("dar.service")
            machine.wait_until_succeeds("curl -sf http://127.0.0.1:4311/api/v1/auth/status", timeout=90)
            status = json.loads(machine.succeed("curl -sf http://127.0.0.1:4311/api/v1/auth/status"))
            assert status == {"initialized": False}, f"unexpected auth status: {status}"
            html = machine.succeed("curl -sf http://127.0.0.1:4311/")
            assert "/rust/start." in html, "expected Rust client HTML"
            sdk = machine.succeed("curl -sf http://127.0.0.1:4311/sdk.js")
            assert "export * from" in sdk, "expected SDK bundle"
            machine.succeed("su -s /bin/sh dar -c 'DAR_ROOT=/var/lib/dar DAR_DATABASE_URL=http://127.0.0.1:4310 DAR_DATABASE_NAME=dar DAR_DATABASE_TOKEN_FILE=/var/lib/dar/secrets/database-token ${self.packages.${system}.dar}/bin/dar-operator reconcile-storage'")
            machine.succeed("systemctl stop dar.service")
            machine.succeed("su -s /bin/sh dar -c 'DAR_ROOT=/var/lib/dar ${self.packages.${system}.dar}/bin/dar-operator apps-list'")
            machine.succeed("systemctl start dar.service")
            machine.wait_until_succeeds("curl -sf http://127.0.0.1:4311/api/v1/auth/status", timeout=90)
            status_after = json.loads(machine.succeed("curl -sf http://127.0.0.1:4311/api/v1/auth/status"))
            assert status_after == {"initialized": False}, f"unexpected auth status after restart: {status_after}"
          '';
        };
      });
    };
}
