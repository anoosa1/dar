# Dar

A self-hosted personal cloud for Linux with Files, Contacts, Calendar, Notes and
Tasks.

## Run locally

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311>. The launcher prints the path to the private setup key
needed to create your first account.

## Deploy

Build with `nix build .#dar`. For NixOS setup, upgrades and backups, see the
[deployment guide](docs/deployment.md).

## Development

The application lives in `next/`. Run checks with:

```sh
nix develop --command npm --prefix next run verify
```

See the [application guide](next/README.md) for configuration and supported features.
