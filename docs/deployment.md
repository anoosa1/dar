# Nix deployment

The root flake exports `packages.<system>.dar` (also `default`), `apps.<system>.default`,
`nixosModules.default` (also `dar`), and the development shell. The package pins
Node 24.22.1 and SpacetimeDB 2.14.1. It builds the client and database bundle inside
Nix, then retains only production npm dependencies. Startup publishes the bundled
database module without deleting its existing state; it never installs npm packages
or builds application code on the server.

## NixOS service

Optional trusted apps are configured with `services.dar.apps`; see the
[installation and author guide](apps.md). The flake provides a separate
`prayer-times` package. No third-party app is installed by default.

Add this repository as a flake input and import `dar.nixosModules.default`:

```nix
{
  imports = [ dar.nixosModules.default ];
  services.dar = {
    enable = true;
    origin = "https://cloud.example.com";
    configureNginx = true;
  };
  security.acme = {
    acceptTerms = true;
    defaults.email = "operator@example.com";
  };
}
```

Pin the input to the committed revision you deploy. Add your normal hardware,
filesystem and boot configuration. Point DNS at the host before enabling ACME.
The example creates nginx HTTPS routing and opens 80/443. Existing nginx users can
leave `configureNginx = false` and use their own proxy instead. The application
and database listen only on loopback; do not expose the database admin/SQL API.
The generated proxy forwards only its exact subscription WebSocket endpoint and
the SDK's POST-only `/spacetime/v1/identity/websocket-token` exchange. It rejects
other `/spacetime/` routes. It supplies the configured canonical Host and
HTTPS scheme to the application. DAV and its well-known discovery routes pass to
the same application host. Requests are streamed with a 1 GiB body limit.
The proxy also overwrites `X-Dar-Client-IP` with its client address so login and
OIDC rate limits do not combine every user into one loopback bucket. A separately
managed local proxy must overwrite this header too, along with canonical `Host`
and `X-Forwarded-Proto: https`; never forward a client-supplied value unchanged.

With `configureNginx = true`, the module also maps the public hostname to
`127.0.0.1` in the server's `/etc/hosts`. SpacetimeDB verifies browser tokens by
fetching Dar's HTTPS signing-key endpoint, so the server must be able to reach its
own origin. This local mapping avoids depending on a public-IP hairpin or VPN
route; the public issuer, TLS verification and other machines' DNS are unchanged.
With a separately managed proxy, ensure the origin is reachable from the Dar host.

Options: `package`, `origin`, `port` (4311), `databasePort` (4310), `databaseName`
(`dar`), `stateDirectory` (`dar` below `/var/lib`), `configureNginx` and
`openFirewall` (defaults to the nginx choice). Keep the origin, database name and
state directory stable across upgrades. No credentials belong in Nix expressions.

### OIDC clients

Register applications that use Dar for sign-in with `services.dar.oidc.clients`:

```nix
services.dar.oidc.clients.photos = {
  name = "Photos";
  redirectUris = [ "https://photos.example.com/oauth/callback" ];
  scopes = [ "openid" "profile" "email" ];
  secretFile = "/run/secrets/dar-photos";
};
```

The attribute name is the stable client ID (`photos`); the issuer is
`https://cloud.example.com/oidc`. Confidential clients are the default and need a
random secret of at least 32 characters, shared with the relying party. Supply
`secretFile` as a quoted absolute runtime path, never a Nix path literal or a
store path. Systemd loads the file through `LoadCredential`, so its source can be
root-only; only the credential copy is made available to Dar. Manage its contents
with your preferred secret provisioning tool. Nix stores only public metadata.

Public clients set `type = "public"` and omit `secretFile`. Optional settings are
`postLogoutRedirectUris` (default `[]`), `enabled` (default `true`) and
`requirePkce` (default `true`). Public clients always require PKCE/S256.
Confidential clients may explicitly disable the requirement for compatibility;
they still authenticate with their client secret. Changes apply on restart;
restart after changing secret contents as well. Listed clients override UI edits;
omitted clients are retained. To remove
access, set `enabled = false` and restart, or delete the client in Administration.
If no clients are declared, Dar reads the optional `/var/lib/dar/oidc.json` instead.
See the [JSON configuration guide](../next/README.md#oidc-text-configuration)
for validation, update and revocation behavior.

For Gitea 28.2.0, which does not send PKCE when using an OIDC login source:

```nix
services.dar.oidc.clients.gitea = {
  name = "Gitea";
  type = "confidential";
  requirePkce = false;
  redirectUris = [ "https://git.example.com/user/oauth2/dar/callback" ];
  scopes = [ "openid" "profile" "email" ];
  secretFile = "/run/secrets/dar-gitea";
};
```

Update your Dar flake input/package to a revision containing this option.
Provision a random secret of at least 32 characters in `/run/secrets/dar-gitea`
outside the Nix store, then run `sudo nixos-rebuild switch`. In Gitea's
Site Administration → Identity & Access → Authentication Sources, add or edit
an OAuth2 source with provider **OpenID Connect** and authentication name `dar`:

- Client ID (Key): `gitea` (or your existing Dar client ID).
- Client Secret: the contents of the secret file.
- OpenID Connect Auto Discovery URL:
  `https://cloud.example.com/oidc/.well-known/openid-configuration`.
- Additional scopes: `profile` and `email`.

Replace the example domains with your own. The authentication name is part of
Gitea's callback URL: if you use a different name, register the exact callback
URL shown by Gitea. For an existing Dar client, use its current client ID as the
Nix attribute name to update it rather than registering another client. Keeping
the same secret avoids a secret rotation; changing client settings revokes its
existing grants, so users must authorize again. Only Gitea's client opts out;
other clients retain their PKCE requirement.

The `dar` system user owns `/var/lib/dar` with mode 0700. A static user is deliberate:
DynamicUser's private-state symlinks conflict with Dar's managed-path rules. Use
real directories or bind mounts for the state directory and its managed contents.
Host directory aliases such as `/var/lib -> /data/lib` are supported: Dar resolves
the parent path and uses the canonical storage path internally. The state directory
itself and managed directories such as `blobs` must not be symlinks.
The service uses private temporary storage, a read-only system, and writable access
to its state directory. It drains the runtime and database on stop.

## First setup and operation

After `nixos-rebuild switch`:

```sh
systemctl status dar
journalctl -u dar -f
sudo cat /var/lib/dar/secrets/bootstrap-token
```

Enter that private setup key in the HTTPS first-administrator screen and choose
your own account credentials. The key's contents are never printed by the service.
Do not copy keys, tokens, state directories or browser profiles into Git.

The package also supplies `dar-operator`. For example, create a private backup
directory owned by `dar`, then run as that user while the database is available:

```sh
sudo -u dar env \
  DAR_ROOT=/var/lib/dar \
  DAR_DATABASE_URL=http://127.0.0.1:4310 \
  DAR_DATABASE_NAME=dar \
  DAR_DATABASE_TOKEN_FILE=/var/lib/dar/secrets/database-token \
  /nix/store/REPLACE-WITH-DAR-PACKAGE/bin/dar-operator backup /var/backups/dar/backup-1
```

Protect the complete backup as credentials: it includes account verifiers and
encryption keys. [Application recovery documentation](../next/README.md) describes
backup verification, fresh-root restore, operator recovery and retained pins.
Never restore over the active directory. Back up before changing versions; a Nix
package rollback alone does not roll back a database/schema migration.

## Package and checks

```sh
nix build .#dar .#checks.x86_64-linux.module-evaluation --no-link -L
nix develop --command npm ci --prefix next
nix develop --command npm --prefix next run verify
nix develop --command node next/scripts/verify-package.mjs /nix/store/REPLACE-WITH-DAR-PACKAGE/bin/dar
```

The module check evaluates the service and builds/lints the generated nginx
configuration. Full `verify` also runs that configuration on isolated loopback
ports with a temporary TLS certificate and a separate database. It verifies
first-account setup, the SDK token exchange and WebSocket subscriptions, persisted
Files/Calendar writes, restricted database routes, and desktop/phone connection
failure and retry. Package verification starts the immutable package on isolated
loopback ports, initializes fresh storage and runs DAV round trips. It does not
boot a complete NixOS VM. x86_64 Linux is the exercised platform; ARM64 sources
are pinned and exported but need a native ARM64 build/run before relying on them.

`dar` defaults to production HTTPS settings. `dar --development` is an explicit
loopback-only development mode; it must not be used as a public deployment.
