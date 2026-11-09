{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.dar;
  appsFile = pkgs.writeText "dar-apps.json" (builtins.toJSON (map (package: { path = toString package; }) cfg.apps));
  oidcFile = pkgs.writeText "dar-oidc.json" (builtins.toJSON {
    clients = lib.mapAttrsToList (id: client:
      (removeAttrs client [ "secretFile" ]) // { inherit id; }
      // lib.optionalAttrs (client.secretFile != null) { secretFile = "/run/credentials/dar.service/oidc-${id}"; }
    ) cfg.oidc.clients;
  });
  hostMatch = builtins.match "https://([A-Za-z0-9][A-Za-z0-9.-]*)(:[0-9]+)?" cfg.origin;
  host = if hostMatch == null then "invalid" else builtins.head hostMatch;
  proxyHeaders = ''
    proxy_set_header Host ${lib.removePrefix "https://" cfg.origin};
    proxy_set_header X-Forwarded-Proto https;
    proxy_set_header X-Dar-Client-IP $remote_addr;
  '';
in {
  options.services.dar = {
    enable = lib.mkEnableOption "Dar personal cloud";
    apps = lib.mkOption { type = lib.types.listOf lib.types.package; default = []; description = "Trusted prebuilt Dar app packages. Changes activate on service restart."; };
    oidc.clients = lib.mkOption {
      default = {};
      description = "OIDC relying parties applied on restart. Omitted clients are retained; set enabled = false to revoke access. Secrets stay outside the Nix store.";
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          name = lib.mkOption { type = lib.types.str; description = "Client display name."; };
          type = lib.mkOption { type = lib.types.enum [ "public" "confidential" ]; default = "confidential"; description = "OIDC client authentication type. All clients require PKCE."; };
          redirectUris = lib.mkOption { type = lib.types.listOf lib.types.str; description = "Exact HTTPS callback URLs."; };
          postLogoutRedirectUris = lib.mkOption { type = lib.types.listOf lib.types.str; default = []; description = "Exact HTTPS logout destinations."; };
          scopes = lib.mkOption { type = lib.types.listOf (lib.types.enum [ "openid" "profile" "email" "groups" "offline_access" ]); default = [ "openid" "profile" "email" ]; description = "Allowed scopes; must include openid."; };
          enabled = lib.mkOption { type = lib.types.bool; default = true; description = "Whether this client may sign in."; };
          secretFile = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; description = "Absolute runtime path to a secret of at least 32 characters. Loaded through systemd credentials; never use a Nix path literal or a store path."; };
        };
      });
    };
    package = lib.mkOption { type = lib.types.package; default = self.packages.${pkgs.stdenv.hostPlatform.system}.dar; description = "Dar package to run."; };
    origin = lib.mkOption { type = lib.types.str; default = ""; example = "https://cloud.example.com"; description = "Canonical public HTTPS origin without a trailing slash."; };
    port = lib.mkOption { type = lib.types.port; default = 4311; description = "Loopback HTTP port."; };
    databasePort = lib.mkOption { type = lib.types.port; default = 4310; description = "Private loopback SpacetimeDB port."; };
    databaseName = lib.mkOption { type = lib.types.strMatching "[a-z0-9][a-z0-9-]*"; default = "dar"; description = "SpacetimeDB database name; retain across restarts."; };
    stateDirectory = lib.mkOption { type = lib.types.strMatching "[a-zA-Z0-9_-]+"; default = "dar"; description = "Private systemd-managed directory below /var/lib."; };
    configureNginx = lib.mkEnableOption "an HTTPS nginx proxy with ACME and restricted database WebSocket routing";
    openFirewall = lib.mkOption { type = lib.types.bool; default = cfg.configureNginx; description = "Open 80/443 for nginx. Internal ports stay private."; };
  };
  config = lib.mkIf cfg.enable {
    assertions = [
      { assertion = hostMatch != null; message = "services.dar.origin must be HTTPS without a path/trailing slash."; }
      { assertion = cfg.port != cfg.databasePort; message = "Dar HTTP and database ports must differ."; }
    ] ++ lib.mapAttrsToList (id: client: {
      assertion = builtins.match "[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}" id != null
        && client.name != "" && client.redirectUris != [] && builtins.elem "openid" client.scopes
        && (if client.type == "confidential" then client.secretFile != null else client.secretFile == null)
        && (client.secretFile == null || (lib.hasPrefix "/" client.secretFile && !(lib.hasPrefix "/nix/store/" client.secretFile)));
      message = "services.dar.oidc.clients.${id} requires a valid ID, name, redirectUris, openid scope, and a non-store absolute secretFile for confidential clients only.";
    }) cfg.oidc.clients;
    # DynamicUser state uses private-directory symlinks, which managed paths reject.
    users.groups.dar = {};
    users.users.dar = { isSystemUser = true; group = "dar"; home = "/var/lib/${cfg.stateDirectory}"; };
    systemd.services.dar = {
      description = "Dar personal cloud";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ]; after = [ "network-online.target" ];
      environment = {
        DAR_ROOT = "/var/lib/${cfg.stateDirectory}"; DAR_ORIGIN = cfg.origin;
        DAR_PORT = toString cfg.port; DAR_DATABASE_PORT = toString cfg.databasePort;
        DAR_DATABASE_NAME = cfg.databaseName;
        DAR_APPS_FILE = toString appsFile;
      } // lib.optionalAttrs (cfg.oidc.clients != {}) { DAR_OIDC_FILE = toString oidcFile; };
      serviceConfig = {
        Type = "simple"; ExecStart = "${cfg.package}/bin/dar";
        User = "dar"; Group = "dar";
        LoadCredential = lib.mapAttrsToList (id: client: "oidc-${id}:${client.secretFile}")
          (lib.filterAttrs (_: client: client.secretFile != null) cfg.oidc.clients);
        StateDirectory = cfg.stateDirectory; StateDirectoryMode = "0700";
        WorkingDirectory = "/var/lib/${cfg.stateDirectory}";
        UMask = "0077"; Restart = "on-failure"; RestartSec = 3;
        TimeoutStopSec = 30; KillMode = "mixed";
        NoNewPrivileges = true; PrivateTmp = true; PrivateDevices = true;
        ProtectSystem = "strict"; ProtectHome = true;
        ProtectKernelTunables = true; ProtectKernelModules = true; ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ];
        ReadWritePaths = [ "/var/lib/${cfg.stateDirectory}" ]; LimitNOFILE = 65536;
      };
    };
    # SpacetimeDB fetches signing keys through the canonical HTTPS issuer.
    # Use the colocated TLS proxy even when the host cannot reach its public IP.
    networking.hosts."127.0.0.1" = lib.mkIf cfg.configureNginx [ host ];
    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ 80 443 ];
    services.nginx = lib.mkIf cfg.configureNginx {
      enable = true;
      virtualHosts.${host} = {
        forceSSL = true; enableACME = true;
        locations = {
          "/" = { proxyPass = "http://127.0.0.1:${toString cfg.port}"; recommendedProxySettings = false; extraConfig = proxyHeaders + ''
            client_max_body_size 1024m;
            proxy_request_buffering off;
            proxy_buffering off;
            proxy_read_timeout 120s;
          ''; };
          "= /spacetime/v1/database/${cfg.databaseName}/subscribe" = {
            proxyPass = "http://127.0.0.1:${toString cfg.databasePort}/v1/database/${cfg.databaseName}/subscribe";
            proxyWebsockets = true; recommendedProxySettings = false; extraConfig = proxyHeaders;
          };
          "= /spacetime/v1/identity/websocket-token" = {
            proxyPass = "http://127.0.0.1:${toString cfg.databasePort}/v1/identity/websocket-token";
            recommendedProxySettings = false;
            extraConfig = proxyHeaders + ''
              limit_except POST { deny all; }
              client_max_body_size 1k;
            '';
          };
          "/spacetime/".return = "404";
        };
      };
    };
  };
}
