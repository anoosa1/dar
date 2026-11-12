# Contacts and Calendar

Delivered November 6, 2026 (layout A, Focused workspace). UI rules are in the
[design guide](../design.md); sync setup in [DAV sync](../davx5.md).

## Features

- **Contacts:** multiple address books; contacts with structured name,
  organization, title, labelled emails/phones/URLs/addresses, birthday, notes,
  groups and favorites; search, book/group filters, deep links, paging. vCard
  3.0/4.0 import/export keeps unknown properties; malformed cards are reported per
  item and duplicate UIDs conflict explicitly.
- **Calendar:** multiple coloured calendars; day/week/month/agenda; visibility;
  IANA timezone (Personal → Regional) and week start; timed, floating and all-day
  events; location, description, attendees, status and alarms; recurrence with
  RRULE/RDATE/EXDATE, overrides and cancelled instances, edited as one occurrence
  or the whole series. Move and resize open a reviewable editor. Read/write
  sharing, ICS import/export and in-app reminders.
- Organizer/attendee data is preserved, but no invitations are sent (no mail
  transport). Remote calendar subscriptions are not supported.

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
