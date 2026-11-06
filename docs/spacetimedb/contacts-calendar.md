# Contacts and Calendar delivery contract

Requested November 6, 2026, on `codex/spacetimedb-auth-files`, following the
verified auth/Files milestone. This extends `next/`; it does not migrate the old
Dar implementation, require its services, or alter unrelated local work.

Status: scope and static visual options prepared; implementation and verification
pending. Do not describe this milestone as complete until the evidence below exists.

## Product scope

Contacts includes multiple local address books; create/rename/delete books;
contact CRUD; structured names, organization, title, labeled multiple emails,
phones, URLs and postal addresses; birthday, notes, groups and favorites; search,
book/group filters, stable deep links and pagination. Import and export vCard
3.0/4.0, retaining unknown properties so a sync/import/edit/export round trip does
not silently erase data. Reject empty/malformed cards and report import errors
per item. Duplicate UIDs must have an explicit conflict policy. Book deletion
requires a concrete confirmation showing the affected contact count.

Calendar includes multiple named/color calendars; day/week/month/agenda views;
calendar visibility; configurable IANA timezone and week start; timed, floating
and all-day events; exclusive all-day end dates; location, description, attendees,
status and alarms; recurring series with RRULE/RDATE/EXDATE, occurrence overrides
and cancelled instances. Editing/deleting one occurrence versus the whole series
must be explicit. Dragging/resizing is an alternate path to the same revisioned
edit operation, with keyboard-accessible controls. Read/write calendar sharing,
ICS import/export, and native due-reminder delivery belong to the milestone.

Preserve organizer, attendee and participation properties on imported events.
There is no Email app or configured SMTP transport in the new application yet:
external invitation mail/RSVP delivery must be implemented and tested with an
explicit operator-configured transport before claiming scheduling support. Do
not silently present a saved attendee as a sent invitation. Remote calendar-feed
subscriptions likewise require bounded, DNS/redirect-safe fetching and lifecycle
tests before being exposed. They are tracked separately from local calendar CRUD.

Native reminder delivery needs a durable reminder identity, acknowledgement,
bounded due-work processing and an in-app inbox; browser notification permission
is optional and must never be requested on page load. Restart and series edits
must neither lose reminders nor deliver an obsolete occurrence.

Both apps retain the approved Nextcloud-style Material 3 direction, self-hosted
assets, centered search and native account/session controls. All visible controls
must work. Cover loading, empty, conflict, validation, permission, offline and
reconnect states; keyboard operation, focus restoration and phone layouts.

## Ownership and persistence

SpacetimeDB remains authoritative for structured data and authorization. Use
private indexed books, contacts, calendars, calendar grants, event resources,
preferences and reminder records. Every resource carries an opaque ID, owner,
revision and mutation timestamps. Expose bounded user-scoped views and stable
cursor windows. Use the current live-session/account checks for subscriptions
and every reducer; an old signed token never overrides revoked access.

Store validated vCard/iCalendar component data with the projection needed for
search and rendering. Preserve UID and unknown standards properties separately
from application IDs. One event resource contains a master and its RECURRENCE-ID
exceptions; do not model each recurring occurrence as an independently mutable
event. Expand recurrence in bounded date windows with explicit limits and errors,
never an unbounded subscription or a promise to enumerate an infinite series.

Use the already-approved `ical.js` for standards parsing/recurrence and `saxes`
for bounded namespace-aware DAV XML. Add them explicitly to the independent
`next/` lockfile when implementing. Avoid a second database, framework, broker
or duplicate permissions engine. Parsing and projection validation happen in the
Node/Cordis boundary; module reducers enforce scope, revisions and invariants.

Backups must atomically include new resource tables, grants and preferences;
restore validates relationships and reconstructs indexes/reminders. Version the
backup format, retain a tested import path for `dar-backup-v1` auth/Files backups,
and explicitly reject newer unknown formats. Test module upgrades against an
existing auth/Files database without deleting or resetting it.

Existing Files app passwords remain Files-scoped. Introduce explicit Contacts
and Calendar read/write scopes with a migration-safe default for existing
credentials. Never reinterpret a Files write credential as address-book access.

## APIs and protocols

Browser HTTP APIs use `/api/v1`, camelCase JSON, the existing error envelope,
revision ETags and required If-Match for updates/deletes. Domain mutations commit
data, revisions and domain activity atomically. Contacts/Calendar must be
reachable from the app switcher without duplicating authentication or breaking
Files URLs, uploads or account security.

Provide standards-first CardDAV and CalDAV homes under `/dav`, principal and home
discovery, collection/resource PROPFIND, resource GET/PUT/DELETE, CardDAV query and
multiget, CalDAV calendar-query and multiget, conditional writes and explicit
supported-report/property discovery. Parse properties by namespace, escape XML
and URIs correctly, bound request bodies/depth/results/recurrence expansion and
return standards-appropriate errors. A truncated resource list is never a
successful complete sync. Implement sync tokens/tombstones if advertising
sync-collection. Do not advertise capabilities without their actual behavior.

Native/DAV reads and writes must use the same domain logic and immediately
reflect each other's changes. Test at least one real sync client in addition to
raw HTTP requests before claiming interoperability. Document tested client and
capability limits. Full WebDAV class compliance and full iTIP scheduling are
separate claims requiring their own evidence.

## Delivery and acceptance

1. Select a static Contacts/Calendar layout from `contacts-calendar-mocks.html`.
2. Add models, validators, domain operations and focused tests. Establish schema
   upgrade and backward-compatible backup/restore before populating user data.
3. Add browser/native APIs, standards parsing/import/export and scoped DAV.
4. Implement selected React views, editors, filters, sharing, recurrence controls,
   reminders and app navigation against real state.
5. Verify every row below and update this record with actual commands/results.

| Area | Required proof | Status |
| --- | --- | --- |
| Contacts | Multi-value CRUD, UID conflicts, unknown-field round trip, groups and pagination | Pending |
| Calendar basics | Timed/floating/all-day CRUD, timezone validation, exclusive end dates | Pending |
| Recurrence | DST boundary, leap day, EXDATE/RDATE, changed/cancelled instance, bounded infinite RRULE | Pending |
| Concurrency | Stale revision and concurrent native/DAV write conflict without lost updates | Pending |
| Authorization | Two-user isolation, read-only grants, revocation, credential scope and expiry | Pending |
| DAV | Discovery, query/multiget filters, XML namespaces, preconditions and real sync client | Pending |
| Reminders | Due delivery, edit/cancel invalidation, restart and duplicate suppression | Pending |
| Imports/exports | Multiple resources, invalid input, lossless fields, stable UID and byte-safe Unicode | Pending |
| Persistence | Existing-module upgrade, old backup import, new backup round trip and relationship validation | Pending |
| Browser | Desktop/phone CRUD, search, recurring edits, keyboard/focus and clean console | Pending |
| Lifecycle | Bounded subscriptions/work, reconnect, restart and deterministic Cordis cleanup | Pending |

“100%” means the agreed features have working end-to-end paths and passing
evidence. It is not a license to mark incomplete scheduling, sync or reminder
behavior as done, or to claim compatibility from screenshots and mocked tests.

## Standards references

- [CardDAV, RFC 6352](https://www.rfc-editor.org/rfc/rfc6352)
- [CalDAV, RFC 4791](https://www.rfc-editor.org/rfc/rfc4791)
- [iCalendar, RFC 5545](https://www.rfc-editor.org/rfc/rfc5545)

Consult their updates and errata when implementing individual capabilities;
format preservation, calendar access and invitation delivery are distinct
contracts and must not be conflated.
