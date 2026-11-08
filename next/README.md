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

Data is stored in `~/.local/share/dar-spacetime` by default (or under
`XDG_DATA_HOME`). Set `DAR_ROOT` to a fresh directory for a separate instance.

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
