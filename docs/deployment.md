# Deployment

The root flake exports `packages.<system>.dar` (also `default`), `chat-bridge`,
`prayer-times`, `apps.<system>.default`, `nixosModules.default` (also `dar`),
`checks.<system>.module-evaluation`, `checks.<system>.nixos-vm` and the dev shell.
The package ships the Rust runtime (`dar-runtime`), the Rust SpacetimeDB module
(`dar_module.wasm`), the Leptos/Wasm browser client (with the installed-app SDK
bundle and the pure-Rust PDF renderer Web Worker), the Go Chat bridge (`dar-chat-bridge`), and SpacetimeDB
2.14.1. Everything is built offline in Nix; the server never compiles code,
installs packages or requires a Node.js runtime. Startup publishes the bundled
database module in place without discarding existing state.

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
`agent.*` ([Codex](codex.md), [Claude Code](claude-code.md)), `news.allowPrivateNetworks` ([News](news.md)), `mail.providers` ([Mail](mail.md)), `photos.immichAllowLocalHost` ([Photos](photos.md)) and `oidc.clients` (below). Keep origin, database
name and state directory stable across upgrades. No credentials go in Nix.
The package puts `git`, `gzip`, `curl`, `exiftool`, `libvips`, `ffmpeg` and `libarchive` on the service PATH for
Git, Books and [Photos](photos.md); Photos' regenerable thumbnails and converted videos live in `photo-cache/` under
the state directory (safe to delete; converted videos can be large). The Immich
mobile app uses `/.well-known/immich` and `/dav/immich/api`; a reverse proxy must pass
both through (the generated nginx does) and allow 16 GiB request bodies.

### Upgrading from the TypeScript stack

An existing TypeScript-stack NixOS deployment switches to the Rust stack in place
when you update the flake input and rebuild: keep the same `stateDirectory`
(`/var/lib/dar`), `origin` and `databaseName`. Back up first. On restart,
`bin/dar` starts SpacetimeDB on the existing database, publishes the Rust module
over it in place, and takes over existing sessions, app passwords, uploads,
OIDC tokens/keys, blobs and installed apps without exporting or converting data.

### Proxy

- Dar and its database listen on loopback only; never expose the database API.
- `configureNginx = true` sets up HTTPS, opens 80/443 and forwards only the exact
  subscription WebSocket and the POST-only `/spacetime/v1/identity/websocket-token`
  exchange; other `/spacetime/` routes are rejected. DAV and well-known routes
  go to Dar. Bodies stream with a 16 GiB limit.
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

Account storage is unlimited by default. Administrators can set a per-account
limit in Settings → Directory (or `PUT /api/v1/identity/users/{id}/storage` with
`{"quota": bytes}`, `null` for unlimited). Limits are kept in backups. A limit
below current use blocks new uploads and copies but keeps existing files.

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
  read instead.

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
operator tasks use `dar-operator` as the `dar` user (`backup <dir>`, `restore <dir>`,
`export-files <username> <dir>`, `release-backup <id>`, `enable-user <username>`,
`disable-user <username>`, `recover-password <username> <password-file>`,
`configure-oidc <file> [optional]`, `deliver-reminders`, `reconcile-storage`,
`sweep-orphans`, and offline
`apps-list|apps-install|apps-enable|apps-disable|apps-uninstall`):

```sh
sudo -u dar env DAR_ROOT=/var/lib/dar DAR_DATABASE_URL=http://127.0.0.1:4310 \
  DAR_DATABASE_NAME=dar DAR_DATABASE_TOKEN_FILE=/var/lib/dar/secrets/database-token \
  /nix/store/…-dar/bin/dar-operator backup /var/backups/dar/backup-1
```

Protect backups like credentials. Back up before upgrading: rolling back the Nix
package does not roll back a schema migration. Never restore over a live root;
for a cold restore into an empty root, `dar --restore <backup-directory>` starts
SpacetimeDB, publishes the module, restores the backup and starts the runtime.

## Checks

```sh
nix build .#dar .#prayer-times .#chat-bridge .#checks.x86_64-linux.module-evaluation --no-link -L
nix flake check -L
nix develop --command cargo xtask verify
```

The `module-evaluation` check evaluates the NixOS module, assertions, OIDC
credential indirection and generated nginx configuration. `nixos-vm` boots the
NixOS module in a VM (requires KVM), verifies `/api/v1/auth/status`, client
serving, `dar-operator` and service restart persistence. x86_64 Linux is
exercised; ARM64 is exported but untested.

`dar` defaults to production HTTPS settings. `dar --development` is a
loopback-only development mode, never for public deployment.
