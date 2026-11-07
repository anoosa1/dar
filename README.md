# Dar

A self-hosted personal cloud with Files, Contacts, Calendar, Notes, Tasks, native
accounts and an OIDC/SCIM provider. Node 24 and Cordis host the application;
SpacetimeDB owns structured data and immutable local blobs hold file content.

The UI uses Material 3 with Catppuccin Latte/Mocha. The current application lives
in next/. Legacy application sources are removed from this branch; their Git
history and the immutable baseline tag remain available.

## Run locally

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open http://127.0.0.1:4311. The launcher prints the path to the private first-setup
key. Default state is ~/.local/share/dar-spacetime (or XDG_DATA_HOME/dar-spacetime).
Use an isolated DAR_ROOT when experimenting. Never initialize over other data.

## Deploy on NixOS

```nix
{
  inputs.dar.url = "github:YOUR-ACCOUNT/dar?ref=YOUR-DEPLOYMENT-REVISION";
  outputs = { nixpkgs, dar, ... }: {
    nixosConfigurations.cloud = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [ dar.nixosModules.default ({ ... }: {
        services.dar = {
          enable = true;
          origin = "https://cloud.example.com";
          configureNginx = true;
        };
        security.acme = {
          acceptTerms = true;
          defaults.email = "operator@example.com";
        };
        # Include your normal host/hardware configuration too.
      }) ];
    };
  };
}
```

The service stores private state under /var/lib/dar. Only nginx ports 80/443 are
opened; HTTP and database listeners stay on loopback. Configure DNS before ACME.
Read [deployment, setup, upgrades and backups](docs/deployment.md).

Build the precompiled package with `nix build .#dar`. It performs no npm install or
application build at service startup. ARM64 is declared; see the deployment record
for which platform was actually built and exercised.

## DAVx⁵

Create an app password under **Settings → Personal → Security** and select
**DAVx⁵ · Contacts and Calendar**. Add an account by URL in DAVx⁵ using
`https://cloud.example.com/dav/`, your Dar username and that app password.
Create address books/calendars in Dar first, then select them in DAVx⁵.
See [DAV setup and protocol limits](docs/davx5.md).

## Verify and reference

```sh
nix develop --command npm --prefix next run verify
nix build .#dar .#checks.x86_64-linux.module-evaluation --no-link
```

The acceptance suite uses real isolated database/blob roots, Chromium, an
independent DAV client and forced restart/restore checks. No live data is used.
[Application contracts and limits](next/README.md) ·
[Architecture](docs/spacetimedb/architecture.md) ·
[UX delivery record](docs/spacetimedb/ux-consistency-audit.md)
