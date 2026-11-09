# Dar application

Dar’s Files, Contacts, Calendar, Notes and Tasks app.

## Run locally

From the repository root:

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311>. Use the setup key at the path printed by the launcher
to create your first account.

Data is stored in `~/.local/share/dar` by default (or under
`XDG_DATA_HOME`). Set `DAR_ROOT` to a fresh directory for a separate instance.
Existing installations using `~/.local/share/dar-spacetime` should stop Dar and
move that directory to `~/.local/share/dar` before restarting (only if the new
destination does not exist), or keep `DAR_ROOT` pointing to the old directory.
Use the equivalent paths under `XDG_DATA_HOME` if set. The storage format is
unchanged; no data is moved automatically. NixOS already uses `/var/lib/dar`.

## OIDC text configuration

Dar provides OIDC sign-in to other applications. Put a JSON file at
`$DAR_ROOT/oidc.json`, or set `DAR_OIDC_FILE` to an explicit filename:

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

Generate a random secret of at least 32 characters, store it in the protected
`secretFile` readable by Dar, and configure the relying party with the same
secret and client ID. A trailing newline is removed; relative secret paths resolve
beside the JSON file. Secrets are encrypted in the database and excluded from
public client metadata. Public clients use `"type": "public"` and omit
`secretFile`. PKCE/S256 is required by default and always required for public
clients. A confidential client that cannot send PKCE (such as Gitea) may set
`"requirePkce": false`; client-secret authentication remains required. The issuer
is `<DAR_ORIGIN>/oidc`. See the [Gitea Nix example](../docs/deployment.md#oidc-clients).
Redirect URLs must be exact HTTPS URLs; development permits loopback HTTP.

Restart Dar to apply changes. Listed IDs are created or updated, including IDs
originally created through Administration. Unchanged clients retain their grants.
Changed clients revoke existing grants; omitted clients remain unchanged. Set
`enabled` to false to disable a client and revoke its grants, or delete it through
Administration. UI edits to listed clients are overwritten on the next restart.
The entire file is validated before updates begin; database failures can still
leave earlier updates applied, and restarting retries safely. A missing default
file is allowed; an explicit missing file or invalid configuration stops startup.
See [NixOS configuration](../docs/deployment.md) for declarative client options.

## Verify

```sh
nix develop --command npm --prefix next run verify
```

Checks use isolated test data.

## Guides

- [Deployment and backups](../docs/deployment.md)
- [Architecture](../docs/spacetimedb/architecture.md)
- [DAV sync](../docs/davx5.md)
- [Notes and Tasks](../docs/spacetimedb/notes-tasks.md)
- [Identity and provisioning](../docs/spacetimedb/identity-provider.md)
