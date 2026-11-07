# Contacts and Calendar delivery contract

Requested November 6, 2026, on `codex/spacetimedb-auth-files`, following the
verified auth/Files milestone. This extends `next/`; it does not migrate the old
Dar implementation, require its services, or alter unrelated local work.

Status: mock A, Focused workspace, implemented and verified on November 6, 2026.
The full isolated verification command passed: 31 Node tests, both desktop/phone
browser journeys, independent vdirsyncer round trips, module upgrade/restore,
and forced crash/restart with reminder recovery. No required checks remain failing.

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

| Area | Required proof | Passing evidence |
| --- | --- | --- |
| Contacts | Multi-value CRUD, UID conflicts, unknown-field round trip, groups and pagination | `pim-standards.test.ts`, `pim.integration.test.ts`, `pim-browser.mjs` |
| Calendar basics | Timed/floating/all-day CRUD, timezone validation, exclusive end dates | `pim-standards.test.ts`, `pim.integration.test.ts` |
| Recurrence | DST boundary, leap day, EXDATE/RDATE periods, changed/cancelled instance, bounded infinite RRULE | `pim-standards.test.ts`, browser occurrence edit/cancel/restore |
| Concurrency | Stale revisions and conditional native/DAV mutations without lost updates | `pim.integration.test.ts`, independent vdirsyncer round trip |
| Authorization | Two-user isolation, read-only grants, live revocation, credential scope and expiry | `pim.integration.test.ts`, existing auth/session security tests |
| DAV | Discovery, namespace-aware query/multiget, preconditions and real sync client | `pim.integration.test.ts`, `pim-dav-client.mjs` with vdirsyncer 0.20.0 |
| Reminders | Due delivery, acknowledgement, edit/delete invalidation, restart and duplicate suppression | `pim.integration.test.ts`, `restart.mjs` |
| Imports/exports | Multiple resources, invalid input, preserved properties/parameters, stable UID and Unicode folding | `pim-standards.test.ts`, `pim.integration.test.ts`, browser export |
| Persistence | Existing-module upgrade, old backup import, new backup round trip and relationship validation | `pim-upgrade.integration.test.ts`, extended `backup.integration.test.ts` |
| Browser | Desktop/phone CRUD, search, recurring edits, move/resize, focus and console | `pim-browser.mjs` plus retained Files journey |
| Lifecycle | Bounded subscriptions/work, reconnect, crash/restart and Cordis cleanup | Existing reconnect journey, extended `restart.mjs`, watermark conflict checks |

## Running and operational limits

From the repository root, use `nix develop`, `npm ci --prefix next`, and
`npm --prefix next run dev`. The Application selector opens Contacts and Calendar;
no account or Files migration is required. Full verification is
`npm --prefix next run verify`; it builds a real isolated module/database and
runs 31 Node tests, both app browser journeys, vdirsyncer and a forced restart.
The module upgrade proof uses immutable auth/Files commit `bf782c0` and therefore
requires this checkout's Git history. It never deletes the existing database.

The final Chromium runs use 1440×950 and 390×844 viewports and verify a clean
console, no page-level phone overflow, saved-contact focus restoration, stable
contact URLs, all calendar views, recurring edits/cancellation/reversal and real
move/resize saves. The independent client is vdirsyncer 0.20.0. Capacity fixtures
include 250 Contacts records, the retained 250-entry Files window proof and 205
calendar resources for reminder queue fairness. The final database process-tree
RSS was 451 MiB, including auxiliary upgrade/restore databases; this is a fixture
measurement, not a production capacity guarantee. Test data and browser artifacts
remain outside the repository.

Contacts native views have opaque cursors, 200-row pages and a bounded
10,000-contact search set. They scan validated projections inside that bound;
this is not a full-text index or an unbounded directory service. Each owner can
create 100 collections and each collection holds at most 10,000 resources.
Resources are limited to 256 KiB, imports to 2 MiB/1,000 resources, and native
collection exports to 8 MiB. Oversized queries fail explicitly.

Native calendar windows span at most one year. Each series expansion is bounded
to 10,000 iterator steps and 2,000 returned occurrences; calendar windows cap the
aggregate at 5,000 occurrences. The native UI edits RRULE and occurrence overrides;
RDATE, EXDATE, custom properties and organizer/attendee parameters are preserved
through standards import/export. Native IANA timezone exports include VTIMEZONE;
the runtime snapshots explicit transitions for two centuries and recurring annual
tails where the IANA data is stable. Political timezone changes still require
runtime/tzdata updates and re-exporting affected events.

Move and resize open a reviewable editor. Editing/deleting recurring events asks
whether to change one occurrence or the whole series. Show cancelled events makes
cancelled resources and occurrences visible for reversal. Permanent deletion is
explicitly confirmed; collection confirmations show the affected resource count.

Reminders use durable per-resource revision/watermark state and an indexed due
queue, catching up in seven-day chunks. Batches advance beyond the first 200
resources; failed resources retain their watermark and retry after five minutes
without blocking later work. A minute timer is owned and drained by the existing Cordis
runtime. The inbox belongs to the calendar owner. Native edits support a DISPLAY
reminder zero to 10,080 minutes before start; unsupported imported alarm effects
are preserved as data, not executed. Browser notification permission is never
requested automatically. External invitation/RSVP delivery, remote calendar feeds
and a general scheduling inbox/outbox remain outside this local-app milestone.

CardDAV and CalDAV expose only the capabilities they implement: principal/home
discovery, PROPFIND, GET/HEAD/PUT/DELETE, query and multiget. Collections are created
and managed in the native UI. There is no advertised sync-collection, LOCK,
PROPPATCH, MKCALENDAR, VTODO or full iTIP scheduling support. Existing Files
credentials retain their original scope. Unsupported filters fail rather than
returning misleading results. See `next/README.md` for self-hosting and operator
backup/recovery instructions.

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
