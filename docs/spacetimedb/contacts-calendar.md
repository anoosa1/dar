# Contacts and Calendar

Delivered November 6, 2026 (layout A, Focused workspace). UI rules are in the
[design guide](../design.md); sync setup in [DAV sync](../davx5.md).

## Features

- **Contacts:** multiple address books; contacts with photo (`PHOTO`, served via `?photo=1`),
  structured name, nickname, pronouns, organization, department, title, labelled emails/phones/URLs/
  social profiles/addresses/related people, birthday (with optional omit-year), anniversary, notes,
  groups and favorites; List/Table/Cards views, A–Z jump rail, quick filters, inline star, hover
  multi-select with bulk group/move/export/delete, client-side Merge & fix duplicate cleanup,
  vCard QR sharing, upcoming birthdays, deep links, single-key shortcuts and paging. vCard 3.0/4.0
  and CSV import/export keep unknown properties; malformed cards are reported per item and duplicate
  UIDs conflict explicitly.
- **Calendar:** multiple coloured calendars; day/week/month/year/agenda; visibility,
  solo focus and quick palette; IANA timezone (Personal → Regional), 12h/24h time format
  and week start; timed, floating and all-day/multi-day spanning events; location links,
  description, Contacts-assisted attendees, status and alarms; recurrence with visual
  builder plus RRULE/RDATE/EXDATE, overrides and cancelled instances, edited as one
  occurrence or the whole series. Floating Quick-Peek and Quick-Create popovers; direct
  drag-move and bottom-edge resize with an Undo toast; single-key keyboard shortcuts.
  Read/write sharing, ICS import/export and in-app reminders.
- Organizer/attendee data is preserved, but no invitations are sent (no mail
  transport).
- Subscriptions: a calendar can follow an iCalendar link (https://, http:// or
  webcal://, tried as HTTPS first). Dar fetches it every 15 minutes to daily
  (conditional requests, 8 MiB and 10,000 events at most), syncs events by UID and
  removes events the source dropped. Subscribed events are read-only in the apps
  and over CalDAV, raise no reminders and are not listed as task lists. The link
  can hold a secret, so only the owner sees it; it is never logged. Fetches use the
  News network guard (public addresses only unless `news.allowPrivateNetworks`).
  Failures are shown on the calendar and retried within the hour.
- Dated tasks from task lists appear in Calendar at their due time (date-only
  tasks on their due day) and open in Tasks; Calendar settings can hide them.

## Data and protocols

- SpacetimeDB holds private, indexed collections, resources, grants, preferences
  and reminder state, each with owner, revision and timestamps. Resources store
  validated vCard/iCalendar data plus a projection for search and rendering; one
  event resource holds its master and RECURRENCE-ID exceptions.
- `ical.js` parses and expands recurrence; `saxes` parses DAV XML. Parsing happens
  in Node; reducers enforce scope, revisions and invariants.
- Browser APIs: `/api/v1`, camelCase, revision ETags with required `If-Match`.
  CardDAV/CalDAV under `/dav` share the same domain logic. App passwords carry
  explicit Contacts/Calendar Read/Write scopes.
- Reminders keep per-resource watermarks and an indexed due queue, catch up in
  seven-day chunks, retry failures after five minutes and run on an owned minute
  timer. DISPLAY alarms 0–10,080 minutes before start are delivered to the owner's
  in-app inbox; other alarm effects are kept as data. Notification permission is
  never requested automatically.

## Limits

- 100 collections per owner, 10,000 resources per collection, 256 KiB per
  resource. Native contact pages are 200 rows over a 10,000-contact search bound.
- Imports up to 2 MiB / 1,000 resources; collection exports up to 8 MiB.
- Calendar windows span at most one year; each series expands at most 10,000
  steps / 2,000 occurrences, and a window at most 5,000 occurrences.
- Exports include VTIMEZONE data; political timezone changes need tzdata updates.
- Oversized requests fail explicitly rather than truncating.

Verified by `pim-standards.test.ts`, `pim.integration.test.ts`,
`pim-upgrade.integration.test.ts`, `pim-browser.mjs`, `contacts-layout-browser.mjs`,
`pim-dav-client.mjs` (vdirsyncer 0.20.0) and `restart.mjs`.
References: [RFC 6352](https://www.rfc-editor.org/rfc/rfc6352) ·
[RFC 4791](https://www.rfc-editor.org/rfc/rfc4791) ·
[RFC 5545](https://www.rfc-editor.org/rfc/rfc5545).
