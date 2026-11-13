# Dar application

Files, Contacts, Calendar, Notes, Tasks, Agent and installed apps, with a native
identity provider. Architecture: [docs/spacetimedb/architecture.md](../docs/spacetimedb/architecture.md).

## Run locally

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311> and use the setup key at the printed path to create
the first account. Data lives in `~/.local/share/dar` (or `$XDG_DATA_HOME/dar`);
set `DAR_ROOT` for a separate instance. Older `~/.local/share/dar-spacetime` roots
are not moved automatically: stop Dar and move the directory, or point `DAR_ROOT`
at it. NixOS uses `/var/lib/dar`.

## Operator commands

`npm --prefix next run operator -- <command>` (`dar-operator` in the package), with
`DAR_ROOT`, `DAR_DATABASE_URL`, `DAR_DATABASE_NAME` and `DAR_DATABASE_TOKEN_FILE`:

| Command | Effect |
| --- | --- |
| `backup <dir>` | Consistent backup (format v6) of the database and referenced blobs; pins retained content |
| `restore <dir>` | Restore into an empty root and database only, never over a live instance |
| `release-backup <id>` | Release a backup's retention pins so cleanup can reclaim content |
| `export-files <username> <dir>` | Export one account's files as plain files |
| `enable-user` / `disable-user <username>` | Change account status; disabling revokes access |
| `recover-password <username> <file>` | Set a password from a file and revoke sessions |
| `apps-install`, `apps-list`, `apps-enable`, `apps-disable`, `apps-uninstall` | Manage installed apps; see [docs/apps.md](../docs/apps.md) |

Backups contain credential verifiers and encryption keys: protect them like
secrets. Restore invalidates sessions, OIDC grants and SCIM credentials. Back up
before upgrading; a code rollback does not undo schema migrations.

## OIDC clients from JSON

Dar is an OIDC provider (issuer `<origin>/oidc`). Besides Administration and
`services.dar.oidc.clients` ([deployment](../docs/deployment.md#oidc-clients)),
clients can be listed in `$DAR_ROOT/oidc.json` or a file named by `DAR_OIDC_FILE`:

```json
{
  "clients": [{
    "id": "photos",
    "name": "Photos",
    "type": "confidential",
    "redirectUris": ["https://photos.example.com/oauth/callback"],
    "postLogoutRedirectUris": [],
    "scopes": ["openid", "profile", "email"],
    "secretFile": "/run/secrets/dar-photos",
    "enabled": true
  }]
}
```

- `secretFile` holds a random secret of at least 32 characters (trailing newline
  removed; relative paths resolve beside the JSON file). Secrets are stored
  encrypted. Public clients use `"type": "public"` and no secret.
- S256 PKCE is required, always for public clients. A confidential client that
  cannot send PKCE (such as Gitea) may set `"requirePkce": false`.
- Redirect URLs must be exact HTTPS URLs; development allows loopback HTTP.
- Applied on restart. Listed clients are created or updated and override UI
  edits; changed clients lose existing grants; omitted clients are kept.
  `"enabled": false` disables a client and revokes its grants.
- The whole file is validated first. A missing default file is fine; an explicit
  missing file or invalid configuration stops startup.

## Verify

`nix develop --command npm --prefix next run verify` runs typechecks, integration
tests, real-browser journeys and restart checks against isolated temporary data.
`node next/scripts/verify.mjs --agent|--identity|--pim|--workspace` runs a subset.

## Guides

[Design](../docs/design.md) · [Deployment](../docs/deployment.md) ·
[DAV sync](../docs/davx5.md) · [Installed apps](../docs/apps.md) ·
[Codex/Agent](../docs/codex.md) · [Claude Code](../docs/claude-code.md) · [News](../docs/news.md) · [Photos](../docs/photos.md) · [Identity](../docs/spacetimedb/identity-provider.md)
