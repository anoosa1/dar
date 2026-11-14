# Deployment

The root flake exports `packages.<system>.dar` (also `default`), `prayer-times`,
`apps.<system>.default`, `nixosModules.default` (also `dar`) and the dev shell.
The package pins Node 24.22.1 and SpacetimeDB 2.14.1 and is fully built in Nix;
the server never installs npm packages or builds code. Startup publishes the
bundled database module without discarding existing state.

## NixOS service

```nix
{
  imports = [ dar.nixosModules.default ];
  services.dar = {
    enable = true;
    origin = "https://cloud.example.com";
    configureNginx = true;
  };
  security.acme = { acceptTerms = true; defaults.email = "operator@example.com"; };
}
```

Pin the flake input to the revision you deploy and point DNS at the host before
enabling ACME. Options: `package`, `origin`, `port` (4311), `databasePort` (4310),
`databaseName` (`dar`), `stateDirectory` (`dar` under `/var/lib`), `configureNginx`,
`openFirewall` (defaults to the nginx choice), `apps` ([installed apps](apps.md)),
`agent.*` ([Codex](codex.md), [Claude Code](claude-code.md)), `news.allowPrivateNetworks` ([News](news.md)), `photos.immichAllowLocalHost` ([Photos](photos.md)) and `oidc.clients` (below). Keep origin, database
name and state directory stable across upgrades. No credentials go in Nix.
The package puts exiftool, libvips and ffmpeg on the service PATH for
[Photos](photos.md); its regenerable thumbnails and converted videos live in `photo-cache/` under
the state directory (safe to delete; converted videos can be large). The Immich
mobile app uses `/.well-known/immich` and `/dav/immich/api`; a reverse proxy must pass
both through (the generated nginx does) and allow 1 GiB request bodies.

### Proxy

- Dar and its database listen on loopback only; never expose the database API.
- `configureNginx = true` sets up HTTPS, opens 80/443 and forwards only the exact
  subscription WebSocket and the POST-only `/spacetime/v1/identity/websocket-token`
  exchange; other `/spacetime/` routes are rejected. DAV and well-known routes
  go to Dar. Bodies stream with a 1 GiB limit.
- It also maps the public hostname to `127.0.0.1` in `/etc/hosts`, because
  SpacetimeDB fetches Dar's HTTPS signing keys and the host must reach its own
  origin. With your own proxy, make sure that works.
- Any proxy must set the canonical `Host`, `X-Forwarded-Proto: https` and
  overwrite `X-Dar-Client-IP` with the client address (used for rate limits).
  Never forward client-supplied values.

### Storage

The static `dar` user owns `/var/lib/dar` (mode 0700). DynamicUser is not used
because its symlinked state conflicts with Dar's no-symlink rule. Host aliases
such as `/var/lib -> /data/lib` are fine; the state directory and managed
directories like `blobs` must not be symlinks. The unit uses a read-only system,
private temp storage, and drains runtime and database on stop.

### OIDC clients

```nix
services.dar.oidc.clients.photos = {
  name = "Photos";
  redirectUris = [ "https://photos.example.com/oauth/callback" ];
  scopes = [ "openid" "profile" "email" ];
  secretFile = "/run/secrets/dar-photos";
};
```

- The attribute name is the client ID; the issuer is `https://cloud.example.com/oidc`.
- Confidential (default) clients need a random secret of at least 32 characters.
  `secretFile` is a quoted absolute runtime path (never a Nix path or store path),
  loaded with systemd `LoadCredential`.
- Public clients set `type = "public"` and omit `secretFile`. Other options:
  `postLogoutRedirectUris` (`[]`), `enabled` (`true`), `requirePkce` (`true`;
  only confidential clients may turn it off).
- Changes apply on restart (also after changing secret contents). Listed clients
  override UI edits; omitted ones are kept. Disable with `enabled = false` or
  delete in Administration. With no clients declared, `/var/lib/dar/oidc.json` is
  read instead ([format](../next/README.md#oidc-clients-from-json)).

Gitea 28.2.0 does not send PKCE from an OIDC login source:

```nix
services.dar.oidc.clients.gitea = {
  name = "Gitea";
  requirePkce = false;
  redirectUris = [ "https://git.example.com/user/oauth2/dar/callback" ];
  scopes = [ "openid" "profile" "email" ];
  secretFile = "/run/secrets/dar-gitea";
};
```

In Gitea → Site Administration → Identity & Access → Authentication Sources, add an
OAuth2 source named `dar` with provider OpenID Connect, client ID `gitea`, the
secret, discovery URL `https://cloud.example.com/oidc/.well-known/openid-configuration`
and scopes `profile email`. The source name is part of the callback URL. Changing
an existing client's settings revokes its grants, so users authorize again.

## First setup and operation

```sh
systemctl status dar
journalctl -u dar -f
sudo cat /var/lib/dar/secrets/bootstrap-token
```

Enter the setup key on the HTTPS first-administrator screen. Backups and other
operator tasks use `dar-operator` as the `dar` user; see
[operator commands](../next/README.md#operator-commands):

```sh
sudo -u dar env DAR_ROOT=/var/lib/dar DAR_DATABASE_URL=http://127.0.0.1:4310 \
  DAR_DATABASE_NAME=dar DAR_DATABASE_TOKEN_FILE=/var/lib/dar/secrets/database-token \
  /nix/store/…-dar/bin/dar-operator backup /var/backups/dar/backup-1
```

Protect backups like credentials. Back up before upgrading: rolling back the Nix
package does not roll back a schema migration. Never restore over a live root.

## Checks

```sh
nix build .#dar .#checks.x86_64-linux.module-evaluation --no-link -L
nix develop --command npm --prefix next run verify
nix develop --command node next/scripts/verify-package.mjs /nix/store/…-dar/bin/dar
```

The module check builds and lints the generated nginx configuration; `verify`
also runs it on loopback with a temporary certificate. Package verification starts
the immutable package on isolated ports and runs DAV round trips. No full NixOS VM
is booted. x86_64 Linux is exercised; ARM64 is exported but untested.

`dar` defaults to production HTTPS settings. `dar --development` is a
loopback-only development mode, never for public deployment.
