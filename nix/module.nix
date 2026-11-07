{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.dar;
  hostMatch = builtins.match "https://([A-Za-z0-9][A-Za-z0-9.-]*)(:[0-9]+)?" cfg.origin;
  host = if hostMatch == null then "invalid" else builtins.head hostMatch;
  proxyHeaders = ''
    proxy_set_header Host ${lib.removePrefix "https://" cfg.origin};
    proxy_set_header X-Forwarded-Proto https;
  '';
in {
  options.services.dar = {
    enable = lib.mkEnableOption "Dar personal cloud";
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
    ];
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
      };
      serviceConfig = {
        Type = "simple"; ExecStart = "${cfg.package}/bin/dar";
        User = "dar"; Group = "dar";
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
