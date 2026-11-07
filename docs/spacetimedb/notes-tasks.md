# Notes and Tasks delivery plan

Requested November 6, 2026, after native identity, Files, Contacts and Calendar.
Branch: `codex/spacetimedb-auth-files`. Layout A, Focused workspace, was selected
for both apps. Implementation and full acceptance verification are complete.
The delivery contract and supported limits are recorded below.

## Architecture

Keep the existing Node 24/Cordis process, SpacetimeDB database, immutable blob
store and native identity. No additional runtime, database, worker or dependency
is proposed. The previous Dar implementation stays untouched.

Reuse the private collection/resource/grant/change boundaries where they represent
the same behavior: ownership, read/write sharing, revision checks, activity and
live invalidation. Keep Markdown and VTODO validation in separate typed adapters.
Existing PIM collections use string kinds; extend the allowed kinds for notebooks
without changing the meaning of existing Contacts or Calendar resources.

Notes hold Markdown in SpacetimeDB, with title, notebook, tags and favorite
metadata. Metadata and content updates commit together. Notes are not mirrored
into a second authoritative Files tree. Markdown export supplies portable files.
File links use existing authorized download routes; a notebook grant must never
implicitly grant access to a linked file. Do not add a duplicate attachment store.

Tasks use standard ICS VTODO resources in calendar collections, so the Tasks app
and CalDAV clients edit the same records. Calendar collections may contain event
and task resources; adapters classify components explicitly. Calendar views,
event export/import and reminders must not accidentally interpret tasks as
VEVENTs or omit tasks from a complete collection export. Task-only collections
and mixed collections advertise their supported components accurately.

Reuse the existing owned reminder maintenance loop and inbox, with task-specific
alarm/recurrence handling and durable delivery watermarks. Do not create another
background scheduler. Cancellation, edits, completion and deletion invalidate
obsolete reminders transactionally.

## Notes acceptance scope

- Notebook create/rename/delete, read/write sharing and live permission revocation.
- Note create/read/edit, rename, move between authorized notebooks, duplicate,
  favorite, tags, full title/body search, sort and deep links.
- Markdown source editing and a safe rendered preview: headings, paragraphs,
  emphasis, links, lists/checklists, quotes and fenced code. Define the supported
  Markdown dialect explicitly; raw HTML stays inert and active URL schemes fail
  closed. No CDN assets, outbound image fetching or remote embed execution.
- Debounced revision-checked saves, explicit saving/saved/error/conflict states,
  retry and unsaved-navigation protection. A reconnect must not overwrite a newer
  revision; retain the unsaved text for deliberate recovery. No offline editing
  or collaborative character-level editing is claimed.
- Recoverable trash, restore and explicit permanent deletion. Preserve note ID
  and content when moving, restoring or renaming; deletion must be reversible
  until the user explicitly purges it.
- Individual Markdown import/export and bounded notebook import/export with
  readable metadata. Preserve exact source text across edit/export/restart.

## Tasks acceptance scope

- Task-list/calendar create/rename/color/delete and existing read/write sharing.
- Tasks, subtasks and parent/child relationships; reject cycles and invalid
  cross-list parent references. Move/copy without silently breaking relationships.
- Title, description, start/due dates, date-only versus zoned times, priority,
  status, progress, completion timestamp and categories. Undated tasks stay undated.
- Inbox, Today, Upcoming, Important and Completed views; list/search/filter/sort,
  task details and keyboard completion/reopening. Show overdue tasks correctly in
  the user's timezone; mobile editing has the same capabilities.
- Recurring VTODO series, individual occurrences and exceptions, with explicit
  completion/reopening semantics. Never materialize an unbounded recurrence or
  shift a date-only due date through a timezone conversion.
- Durable in-app reminders and acknowledgement, including downtime catch-up and
  edit/completion invalidation. No outbound email invitations or push service.
- ICS import/export and CalDAV discovery, supported-component metadata, VTODO
  query/multiget, date filters and conditional GET/PUT/DELETE. Preserve UID,
  RELATED-TO, recurrence exceptions, timezone definitions and unknown properties.
- Explicit delete confirmation and safe parent/child deletion behavior; retain
  completed tasks for reopening and export.

Recurring completion applies to the selected occurrence, persisted as a completed
exception; the master remains active for subsequent occurrences. A finite series
becomes completed when no pending occurrences remain. Reopen an individual
completed occurrence through the editor's Edit scope selector. Completing the
whole series is an explicit action that stops subsequent reminders; reactivating
the master retains its completed exceptions. Native recurrence requires a start date and uses
bounded expansion. Parent completion must offer deliberate handling of unfinished
subtasks; reopening a parent must not silently reopen completed children. Parent
deletion offers detaching children or deleting the bounded subtree, with the
selected action committed atomically.

## Integration and safety

Add `/notes` and `/tasks` to the shared application switcher, keeping the centered
search and Nextcloud-style Material 3 shell. Browser APIs live under
`/api/v1/notes` and `/api/v1/tasks`, with camelCase envelopes, stable errors,
pagination and revision-backed ETags/If-Match. CalDAV remains under `/dav`.
Use existing Calendar-scoped app credentials for VTODOs and keep Files/Contacts
credentials isolated. Sharing a calendar deliberately shares its events/tasks.

Validate both sides of resource moves in the same database transaction. Keep
user scope explicit in every lookup, stream, reminder and export. Revocation,
account suspension, password changes and session expiry must invalidate access
without waiting for a browser reload. Bound imports, body size, hierarchy depth,
result windows and recurrence work; report limits instead of truncating silently.

Integrate search, activity, preferences and the existing reminder inbox. This new
project has no assistant/widget/plugin contribution host yet; do not invent one
or claim those surfaces exist. Document that platform boundary explicitly.

## Delivery order

1. Select one of the paired static layouts in `notes-tasks-mocks.html`.
2. Add typed documents, validators and additive numbered migrations. Prove existing
   accounts, Files, Contacts, Calendar and identity-provider state survive upgrade.
3. Implement Notes transactions, APIs, safe preview, search, sharing, trash and
   portable import/export; implement Tasks/VTODO transactions and protocol paths.
4. Extend reminder processing, CalDAV component filtering and collection exports;
   prove existing VEVENT clients still work with mixed collections.
5. Build the selected React layouts and all loading/empty/error/conflict/offline,
   keyboard, focus, selection and phone states.
6. Extend backup/restore for new state and retained trash, accepting existing v1,
   v2 and v3 backups. Test an actual old-database upgrade and complete restore.
7. Run the full isolated acceptance command, real desktop/phone browser journeys,
   independent CalDAV round trips and forced runtime/database restarts. Record
   precise supported capabilities, limitations and evidence before marking done.

## Verification matrix

| Surface | Required proof | Status |
| --- | --- | --- |
| Notes data | Exact Markdown round trip, atomic metadata/content, revision conflict and recovery | Pass: API, source/security unit tests and interrupted-save browser journey |
| Notes lifecycle | Move, duplicate, trash/restore/purge, sharing and revocation | Pass: workspace integration and browser trash/restore |
| Preview security | Raw HTML, active URLs, images/embeds and malformed source stay safe | Pass: renderer security test and real browser preview |
| Tasks data | Status/progress/completion, hierarchy validation, move and reopen | Pass: hierarchy/subtree integration and browser completion/reopen |
| Tasks dates | Date-only, floating/zoned due dates, DST, recurrence and exceptions | Pass: standards tests and recurring browser journey |
| CalDAV | VTODO CRUD/query/multiget, ETags, mixed VEVENT/VTODO compatibility and credential isolation | Pass: protocol integration and independent vdirsyncer round trips |
| Reminders | Restart catch-up, completion/edit invalidation, bounded recurrence and duplicate suppression | Pass: due-only task delivery/invalidation, existing Calendar checks and forced restart |
| Browser | Real desktop/phone Notes and Tasks journeys, drafts/conflicts, focus and console | Pass: integrated Chromium journeys and shared dialog/shell behavior |
| Upgrade/restore | Existing database and v1/v2/v3 restore, new state/trash survives backup and restart | Pass: all three baseline upgrades, backup restore and forced restart |
| Existing apps | Complete auth/Files/Contacts/Calendar/OIDC/SCIM acceptance suite remains green | Pass: complete acceptance command |

Final verification: `nix develop -c npm --prefix next run verify` passed
55 Node tests with no failures/skips, both TypeScript checks, four desktop/phone
browser journeys, independent vdirsyncer contact/event/task synchronization and
forced runtime/database restart. Isolated evidence is at
`/tmp/nix-shell.yjq4ci/dar-acceptance-lzWvPq`. The full multi-database acceptance
fixture measured 813 MiB database process-tree RSS; the focused workspace fixture
measured 360 MiB. These are test-fixture measurements, not production capacity
or minimum-memory promises. No tests used the default data root.

## Delivered behavior and limits

Notes use `/api/v1/notes/notebooks` and `/api/v1/notes/items`; Tasks use
`/api/v1/tasks/lists` and `/api/v1/tasks/items`. Collection grants, import and
export are subresources of each notebook/list. Item GET/PUT/DELETE use opaque IDs;
copy/move/export are item subresources. All mutations of existing resources require
the current `If-Match` ETag. Note trash is private to the notebook owner under
`/api/v1/notes/trash`, with item GET/DELETE and POST `/:id/restore`. A deleted
notebook's notes can be restored into another owned notebook.

Moves preserve IDs/UIDs and require writable source/destination collections with
the same owner. Copies receive new IDs/UIDs and require a readable source and
writable destination. Task subtree moves/copies preserve relationships inside
the selected subtree and detach its root from its former parent. Cross-owner
copies deliberately create independent records. Sharing a task list shares all
VEVENTs and VTODOs in that calendar.

Markdown preview is a deliberately small dialect: headings 1–6, separate paragraph
lines, single-line emphasis/strong/code/links, flat ordered/unordered/check lists,
single-line quotes and triple-backtick fenced code. It does not claim CommonMark
compatibility or syntax highlighting. Source HTML and image/embed syntax remain
literal text. Links accept HTTP(S) and authorized Files content routes. Nested
formatting, tables and remote images are not rendered. Editing/export always
preserves the original source independently of this preview dialect.

Autosave waits 650 ms and uses revision checks. Failed/interrupted writes retain
a tab-local, account-and-note-scoped sessionStorage draft, with explicit recovery
and conflict choices. Navigation retains the recovery draft; closing/reloading the
page with unsaved text invokes the browser's standard beforeunload protection.
Drafts are not an offline editing system and do not survive clearing browser
storage. Read-only grants disable editing. The shared live authorization boundary
closes inaccessible details after revocation and invalidates suspended sessions.

Notes/Tasks save sort preferences in the existing revision-checked PIM preferences.
Tasks use the saved Calendar display timezone, falling back to the browser timezone
when no preference exists. Timed due dates are classified into Today/Upcoming in
that timezone; date-only dates keep their literal day. Overdue rows are labeled.
Task status/progress and explicit completion timestamps survive ICS round trips.
Parent completion requires completed/cancelled children or explicit completion of
the bounded subtree. Parent deletion offers atomic detachment or subtree deletion.

The task editor offers whole-series and occurrence edits. Completing an occurrence
advances the displayed next pending date without creating separate resources.
EXDATE/RDATE, overrides, embedded timezones and unknown ICS properties are retained.
Native date input rejects invalid calendar dates and nonexistent DST local times.
Undated and due-only tasks remain valid; duration-based imported VTODOs are retained.
CalDAV advertises VEVENT and VTODO and supports conditional resource CRUD,
calendar-query/multiget, component selection, time ranges and basic property
presence/text filters. Unsupported filters fail explicitly. No scheduling/iTIP,
CalDAV sync-token, MKCALENDAR or certification claim is added.

Task reminders reuse the existing owner-only in-app inbox and durable cursor.
Relative DISPLAY alarms are delivered; absolute, repeated, AUDIO/EMAIL effects
are retained as source data without those effects. Completing/cancelling a series,
editing, moving or deleting a task invalidates its old reminder revision. Completed
individual occurrences do not emit reminders. The owned minute scheduler catches
up after downtime; the tests also drive the same delivery function at due boundaries.

Each owner has at most 100 collections. Native searches span at most 10,000 accessible
resources and paginate 200 rows. Trash is capped at 10,000 entries per instance
and paginates 200 metadata rows; source content is available through individual
trash reads. Notes are at most 256 KiB. Notebook JSON import/export is bounded to
500 notes and 2 MiB, allowing its exports to be imported again. Individual Markdown
export remains available beyond the notebook bound. ICS imports use the existing
2 MiB/1,000-resource bounds and report per-resource results; parents are imported
before children. Task subtree mutations are bounded to 500 records and hierarchy
depth 32. Recurrence windows span at most one year, 10,000 iterator steps and 2,000
results per resource; first-match protocol/pending queries allow open-ended dates
while retaining the iterator bound. Limits produce errors rather than silent
truncation. No large-archive performance guarantee is claimed.

The additive workspace migration is numbered 1 in `workspaceMigrations`. Instance
backups now use `dar-backup-v4`, including retained note trash; restore accepts
existing v1/v2/v3 backups. Identity/session invalidation follows the existing
restore policy. Upgrade tests publish the committed v3 module before upgrading,
and the complete acceptance command retains the earlier v1/v2 upgrade proofs.

The new platform has no assistant tool, widget or plugin contribution host yet;
neither app claims those integrations. No new dependency or worker was introduced.

The request for “100%” applies to the documented delivery scope, including protocols
and lifecycle. It is not a claim of every possible third-party feature, bug-free
software, or formal protocol certification.
