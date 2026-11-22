{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.dar;
  codexFile = pkgs.writeText "dar-codex.json" (builtins.toJSON {
    executable = if cfg.agent.package == null then "" else "${cfg.agent.package}/bin/codex";
    inherit (cfg.agent) home userId gitHosts;
  });
  claude = cfg.agent.claude;
  claudeFile = pkgs.writeText "dar-claude.json" (builtins.toJSON {
    executable = if claude.package == null then "" else "${claude.package}/bin/claude";
    inherit (claude) home; inherit (cfg.agent) userId gitHosts;
  });
  appsFile = pkgs.writeText "dar-apps.json" (builtins.toJSON (map (package: { path = toString package; }) cfg.apps));
  oidcFile = pkgs.writeText "dar-oidc.json" (builtins.toJSON {
    clients = lib.mapAttrsToList (id: client:
      (removeAttrs client [ "secretFile" ]) // { inherit id; }
      // lib.optionalAttrs (client.secretFile != null) { secretFile = "/run/credentials/dar.service/oidc-${id}"; }
    ) cfg.oidc.clients;
  });
  # Mail providers people may add accounts with. Secrets reach the service as systemd credentials.
  mailProvidersFile = pkgs.writeText "dar-mail-providers.json" (builtins.toJSON (lib.mapAttrsToList (id: p: {
    inherit id; inherit (p) name;
    document = {
      inherit (p) protocol auth domains saveSent;
    } // lib.optionalAttrs (p.protocol == "imap") { imap = p.imap; smtp = p.smtp; }
      // lib.optionalAttrs (p.protocol == "jmap") { jmap = { url = p.jmapUrl; }; }
      // lib.optionalAttrs (p.auth == "oauth") { oauth = { inherit (p.oauth) authorizeUrl tokenUrl clientId scopes params; }; };
  } // lib.optionalAttrs (p.auth == "oauth" && p.oauth.clientSecretFile != null) { secretFile = "/run/credentials/dar.service/mail-${id}"; }) cfg.mail.providers));
  mailServer = lib.types.submodule {
    options = {
      host = lib.mkOption { type = lib.types.str; description = "Server host name."; };
      port = lib.mkOption { type = lib.types.port; description = "Server port."; };
      security = lib.mkOption { type = lib.types.enum [ "tls" "starttls" ]; default = "tls"; description = "TLS from the start (993, 465) or STARTTLS (143, 587). Plain connections are never used."; };
    };
  };
  mailPresets = {
    google = { protocol = "imap"; auth = "oauth"; domains = [ "gmail.com" "googlemail.com" ]; saveSent = false;
      imap = { host = "imap.gmail.com"; port = 993; security = "tls"; }; smtp = { host = "smtp.gmail.com"; port = 465; security = "tls"; };
      oauth = { authorizeUrl = "https://accounts.google.com/o/oauth2/v2/auth"; tokenUrl = "https://oauth2.googleapis.com/token"; scopes = [ "https://mail.google.com/" ]; params = { access_type = "offline"; prompt = "consent"; }; }; };
    microsoft = { protocol = "imap"; auth = "oauth"; domains = [ "outlook.com" "hotmail.com" "live.com" ]; saveSent = false;
      imap = { host = "outlook.office365.com"; port = 993; security = "tls"; }; smtp = { host = "smtp.office365.com"; port = 587; security = "starttls"; };
      oauth = { authorizeUrl = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"; tokenUrl = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
        scopes = [ "https://outlook.office.com/IMAP.AccessAsUser.All" "https://outlook.office.com/SMTP.Send" "offline_access" ]; params = {}; }; };
    fastmail = { protocol = "jmap"; auth = "token"; domains = [ "fastmail.com" ]; jmapUrl = "https://api.fastmail.com/jmap/session"; };
  };
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
    agent = {
      enable = lib.mkEnableOption "Agent execution for one Dar account using existing service-user Codex and/or Claude Code logins";
      package = lib.mkOption { type = lib.types.nullOr lib.types.package; default = null; description = "Codex CLI with named permission-profile support. Tested with 0.166.0. No automatic download. Leave null to offer only Claude Code."; };
      claude = {
        package = lib.mkOption { type = lib.types.nullOr lib.types.package; default = null; description = "Claude Code CLI (for example pkgs.claude-code). Runs use the service user's existing Claude login, so they count as Claude Code usage. Tested with 2.1.322. No automatic download."; };
        home = lib.mkOption { type = lib.types.strMatching "/var/lib/[a-zA-Z0-9_/-]+"; default = "/var/lib/dar-claude"; description = "Private Claude Code home (CLAUDE_CONFIG_DIR) for the dar service user. Login tokens never enter the Nix store."; };
        commands = lib.mkOption { type = lib.types.bool; default = true; description = "Put bubblewrap and socat on the service PATH so Claude Code can run commands inside its sandbox (no network, workspace-only writes). Without them Claude only reads and edits files."; };
      };
      home = lib.mkOption { type = lib.types.strMatching "/var/lib/[a-zA-Z0-9_/-]+"; default = "/var/lib/dar-codex"; description = "Private Codex home for the dar service user. Login tokens never enter the Nix store."; };
      userId = lib.mkOption { type = lib.types.str; default = ""; description = "Immutable Dar account UUID permitted to use this Codex login. Shown in Agent settings."; };
      gitHosts = lib.mkOption { type = lib.types.listOf (lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9.-]*"); default = []; description = "Allowed HTTPS repository hosts. Redirects and credential-bearing URLs are rejected."; };
    };
    news.allowPrivateNetworks = lib.mkOption { type = lib.types.bool; default = false; description = "Let News fetch feeds and images from private, loopback and link-local addresses (for feeds on your own network). Off by default to prevent server-side request forgery."; };
    mail.providers = lib.mkOption {
      default = {};
      example = lib.literalExpression ''{ google = { preset = "google"; name = "Google"; oauth = { clientId = "123.apps.googleusercontent.com"; clientSecretFile = "/run/secrets/dar-google-mail"; }; }; work = { name = "Work"; domains = [ "example.com" ]; imap = { host = "mail.example.com"; port = 993; }; smtp = { host = "mail.example.com"; port = 465; }; }; }'';
      description = "Mail providers people can add accounts with; Mail reaches no other servers. Applied at every start; providers added in Administration stay. OAuth providers need <origin>/api/v1/mail/oauth/callback registered as a redirect URL.";
      type = lib.types.attrsOf (lib.types.submodule ({ config, ... }: let preset = if config.preset == null then {} else mailPresets.${config.preset}; in {
        options = {
          preset = lib.mkOption { type = lib.types.nullOr (lib.types.enum [ "google" "microsoft" "fastmail" ]); default = null; description = "Fill in a well-known provider's servers and OAuth endpoints; the OAuth client is still yours to give."; };
          name = lib.mkOption { type = lib.types.str; description = "Name shown when people add an account."; };
          protocol = lib.mkOption { type = lib.types.enum [ "imap" "jmap" ]; default = preset.protocol or "imap"; description = "IMAP with SMTP submission, or JMAP."; };
          auth = lib.mkOption { type = lib.types.enum [ "password" "token" "oauth" ]; default = preset.auth or "password"; description = "Password (or app password), API token (JMAP) or OAuth2 (XOAUTH2)."; };
          domains = lib.mkOption { type = lib.types.listOf lib.types.str; default = preset.domains or []; description = "Email domains this provider is suggested for."; };
          saveSent = lib.mkOption { type = lib.types.bool; default = preset.saveSent or true; description = "Save sent messages to the Sent mailbox; off for servers that do it themselves (Gmail, Microsoft 365)."; };
          imap = lib.mkOption { type = lib.types.nullOr mailServer; default = preset.imap or null; description = "IMAP server (protocol imap)."; };
          smtp = lib.mkOption { type = lib.types.nullOr mailServer; default = preset.smtp or null; description = "SMTP submission server (protocol imap)."; };
          jmapUrl = lib.mkOption { type = lib.types.nullOr lib.types.str; default = preset.jmapUrl or null; description = "JMAP session URL (protocol jmap)."; };
          oauth = {
            authorizeUrl = lib.mkOption { type = lib.types.str; default = preset.oauth.authorizeUrl or ""; description = "OAuth2 authorization endpoint."; };
            tokenUrl = lib.mkOption { type = lib.types.str; default = preset.oauth.tokenUrl or ""; description = "OAuth2 token endpoint."; };
            clientId = lib.mkOption { type = lib.types.str; default = ""; description = "OAuth2 client ID registered with the provider."; };
            clientSecretFile = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; description = "Absolute runtime path to the OAuth2 client secret, loaded through systemd credentials; never a Nix path or store path. Null for public clients."; };
            scopes = lib.mkOption { type = lib.types.listOf lib.types.str; default = preset.oauth.scopes or []; description = "OAuth2 scopes."; };
            params = lib.mkOption { type = lib.types.attrsOf lib.types.str; default = preset.oauth.params or {}; description = "Extra authorization parameters."; };
          };
        };
      }));
    };
    photos.immichAllowLocalHost = lib.mkOption { type = lib.types.bool; default = false; description = "Let the Photos Immich import reach an Immich server on this machine (http://localhost:2283). Off by default so account owners cannot point the connector at local services. LAN and Tailscale addresses are always allowed."; };
    apps = lib.mkOption { type = lib.types.listOf lib.types.package; default = []; description = "Trusted prebuilt Dar app packages. Changes activate on service restart."; };
    oidc.clients = lib.mkOption {
      default = {};
      description = "OIDC relying parties applied on restart. Omitted clients are retained; set enabled = false to revoke access. Secrets stay outside the Nix store.";
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          name = lib.mkOption { type = lib.types.str; description = "Client display name."; };
          type = lib.mkOption { type = lib.types.enum [ "public" "confidential" ]; default = "confidential"; description = "OIDC client authentication type."; };
          requirePkce = lib.mkOption { type = lib.types.bool; default = true; description = "Require S256 PKCE. May be disabled only for confidential clients that cannot send PKCE, such as Gitea."; };
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
      { assertion = !cfg.agent.enable || (cfg.agent.package != null || claude.package != null) && !(lib.hasInfix "/../" claude.home) && claude.home != "/var/lib/" && claude.home != cfg.agent.home && builtins.match "[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}" cfg.agent.userId != null && !(lib.hasInfix "/../" cfg.agent.home) && !(lib.hasInfix "/./" cfg.agent.home) && cfg.agent.home != "/var/lib/"; message = "Dar Agent needs an explicit Codex and/or Claude Code package, a Dar account UUID and separate private /var/lib homes."; }
    ] ++ lib.mapAttrsToList (id: p: {
      assertion = builtins.match "[a-z0-9][a-z0-9-]{0,63}" id != null
        && (if p.protocol == "imap" then p.imap != null && p.smtp != null && p.auth != "token" else p.jmapUrl != null)
        && (p.auth != "oauth" || (p.oauth.authorizeUrl != "" && p.oauth.tokenUrl != "" && p.oauth.clientId != ""))
        && (p.oauth.clientSecretFile == null || (lib.hasPrefix "/" p.oauth.clientSecretFile && !(lib.hasPrefix "/nix/store/" p.oauth.clientSecretFile)));
      message = "services.dar.mail.providers.${id} needs a lowercase ID, IMAP and SMTP servers (or a JMAP URL), the OAuth endpoints and client ID for OAuth, and a non-store absolute clientSecretFile.";
    }) cfg.mail.providers ++ lib.mapAttrsToList (id: client: {

      assertion = builtins.match "[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}" id != null
        && client.name != "" && client.redirectUris != [] && builtins.elem "openid" client.scopes
        && (client.type == "confidential" || client.requirePkce)
        && (if client.type == "confidential" then client.secretFile != null else client.secretFile == null)
        && (client.secretFile == null || (lib.hasPrefix "/" client.secretFile && !(lib.hasPrefix "/nix/store/" client.secretFile)));
      message = "services.dar.oidc.clients.${id} requires a valid ID, name, redirectUris, openid scope, PKCE for public clients, and a non-store absolute secretFile for confidential clients only.";
    }) cfg.oidc.clients;
    # DynamicUser state uses private-directory symlinks, which managed paths reject.
    users.groups.dar = {};
    users.users.dar = { isSystemUser = true; group = "dar"; home = "/var/lib/${cfg.stateDirectory}"; };
    systemd.tmpfiles.rules = lib.optional (cfg.agent.enable && cfg.agent.package != null) "d ${cfg.agent.home} 0700 dar dar -"
      ++ lib.optional (cfg.agent.enable && claude.package != null) "d ${claude.home} 0700 dar dar -";
    systemd.services.dar = {
      description = "Dar personal cloud";
      path = lib.optionals cfg.agent.enable (lib.optional (cfg.agent.package != null) cfg.agent.package
        ++ lib.optionals (claude.package != null) ([ claude.package ] ++ lib.optionals claude.commands [ pkgs.bubblewrap pkgs.socat ]));
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ]; after = [ "network-online.target" ];
      environment = {
        DAR_ROOT = "/var/lib/${cfg.stateDirectory}"; DAR_ORIGIN = cfg.origin;
        DAR_PORT = toString cfg.port; DAR_DATABASE_PORT = toString cfg.databasePort;
        DAR_DATABASE_NAME = cfg.databaseName;
        DAR_APPS_FILE = toString appsFile;
        DAR_MAIL_PROVIDERS_FILE = toString mailProvidersFile;
      } // lib.optionalAttrs cfg.news.allowPrivateNetworks { DAR_NEWS_ALLOW_PRIVATE_NETWORKS = "1"; } // lib.optionalAttrs cfg.photos.immichAllowLocalHost { DAR_IMMICH_ALLOW_LOCAL_HOST = "1"; } // lib.optionalAttrs (cfg.oidc.clients != {}) { DAR_OIDC_FILE = toString oidcFile; }
        // lib.optionalAttrs (cfg.agent.enable && cfg.agent.package != null) { DAR_CODEX_FILE = toString codexFile; }
        // lib.optionalAttrs (cfg.agent.enable && claude.package != null) { DAR_CLAUDE_FILE = toString claudeFile; };
      serviceConfig = {
        Type = "simple"; ExecStart = "${cfg.package}/bin/dar";
        User = "dar"; Group = "dar";
        LoadCredential = lib.mapAttrsToList (id: client: "oidc-${id}:${client.secretFile}")
          (lib.filterAttrs (_: client: client.secretFile != null) cfg.oidc.clients)
          ++ lib.mapAttrsToList (id: p: "mail-${id}:${p.oauth.clientSecretFile}")
          (lib.filterAttrs (_: p: p.auth == "oauth" && p.oauth.clientSecretFile != null) cfg.mail.providers);
        StateDirectory = cfg.stateDirectory; StateDirectoryMode = "0700";
        WorkingDirectory = "/var/lib/${cfg.stateDirectory}";
        UMask = "0077"; Restart = "on-failure"; RestartSec = 3;
        TimeoutStopSec = 30; KillMode = "mixed";
        NoNewPrivileges = true; PrivateTmp = true; PrivateDevices = true;
        ProtectSystem = "strict"; ProtectHome = true;
        # Its locked /proc submounts prevent bubblewrap from mounting a fresh
        # procfs in its user namespace. Codex itself denies /proc and /sys to tools.
        ProtectKernelTunables = !cfg.agent.enable;
        ProtectKernelModules = true; ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        # Bubblewrap needs NETLINK_ROUTE to configure loopback inside its private
        # network namespace. Codex still denies network access to workspace tools.
        RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ] ++ lib.optional cfg.agent.enable "AF_NETLINK";
        ReadWritePaths = [ "/var/lib/${cfg.stateDirectory}" ] ++ lib.optional (cfg.agent.enable && cfg.agent.package != null) cfg.agent.home
          ++ lib.optional (cfg.agent.enable && claude.package != null) claude.home; LimitNOFILE = 65536;
        TasksMax = lib.mkIf cfg.agent.enable 256; MemoryMax = lib.mkIf cfg.agent.enable "2G";
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
            client_max_body_size 16g;
            proxy_request_buffering off;
            proxy_buffering off;
            proxy_read_timeout 120s;
          ''; };
          # Chat's live socket: voice call state and audio.
          "= /api/v1/chat/live" = {
            proxyPass = "http://127.0.0.1:${toString cfg.port}";
            proxyWebsockets = true; recommendedProxySettings = false; extraConfig = proxyHeaders + ''
              proxy_buffering off;
              proxy_read_timeout 120s;
            '';
          };
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
