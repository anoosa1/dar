# Dar

A self-hosted personal cloud for Linux: Files, Contacts, Calendar, Notes, Tasks,
an Agent, and installable apps such as Prayer Times. Dar also
provides OIDC sign-in and SCIM provisioning for other applications.

## Screenshots

| Files · light | Photos · dark |
| --- | --- |
| ![Files in the light theme](docs/screenshots/files.png) | ![Photos in the dark theme](docs/screenshots/photos.png) |

| Calendar · light | Contacts · dark |
| --- | --- |
| ![Calendar month view in the light theme](docs/screenshots/calendar.png) | ![Contacts with a contact open in the dark theme](docs/screenshots/contacts.png) |

| Notes · light | Tasks · dark |
| --- | --- |
| ![Notes with a note open in the light theme](docs/screenshots/notes.png) | ![Tasks in the dark theme](docs/screenshots/tasks.png) |

| Phone · Photos | Phone · Calendar |
| --- | --- |
| <img src="docs/screenshots/phone-photos.png" alt="Photos on a phone in the light theme" width="300"> | <img src="docs/screenshots/phone-calendar.png" alt="Calendar day view on a phone in the dark theme" width="300"> |

## Run locally

```sh
nix develop
npm ci --prefix next
npm --prefix next run dev
```

Open <http://127.0.0.1:4311>. The dev script prints the path to the private setup
key used to create the first account.

## Deploy

`nix build .#dar` and the NixOS module; see the [deployment guide](docs/deployment.md).

## Develop

The application is in `next/` ([guide](next/README.md)). Run all checks with
`nix develop --command npm --prefix next run verify`. UI work follows the
[design guide](docs/design.md).
