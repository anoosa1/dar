# Notes and Tasks

Delivered November 2026 (layout A, Focused workspace). Same runtime, database and
identity as the other apps; no extra dependency or worker. UI rules are in the
[design guide](../design.md).

## Notes

- Markdown stored in SpacetimeDB with title, notebook, tags and favorite; metadata
  and content commit together. Notes are not mirrored into Files.
- Notebooks with read/write sharing; create, edit, rename, move, duplicate,
  favorite, tag, search, sort and deep links. Private trash with restore (into
  another notebook if the original was deleted) and explicit purge.
- Autosave after 650 ms with revision checks. Failed or interrupted saves keep a
  tab-local recovery draft with explicit recovery and conflict choices; this is
  not offline editing.
- Preview is a small safe dialect: headings, paragraphs, single-line
  emphasis/strong/code/links, flat lists and checklists, single-line quotes and
  fenced code. HTML and images stay literal; links allow HTTP(S) and authorized
  Files routes. Source is always preserved exactly.
- APIs: `/api/v1/notes/notebooks`, `/items`, `/trash`.

## Tasks

- Standard VTODO resources in calendar collections, so CalDAV clients and the app
  edit the same records; sharing a list shares its events and tasks.
- Subtasks (no cycles), title, description, start/due (date-only or zoned),
  priority, status, progress, completion time and categories. Views: All, Inbox,
  Today, Upcoming, Important, Completed, using the Regional timezone.
- Recurring tasks: completing an occurrence stores a completed exception; the
  series continues. Completing the whole series is explicit. Parent completion
  and deletion ask how to handle children and commit atomically.
- Reminders reuse the Calendar inbox; completion, edits, moves and deletion
  invalidate old reminders.
- APIs: `/api/v1/tasks/lists`, `/items`. CalDAV advertises VEVENT and VTODO with
  query, multiget, time ranges and conditional CRUD; unsupported filters fail.

## Limits

- 100 collections per owner; searches over 10,000 resources with 200-row pages.
- Notes up to 256 KiB; notebook JSON import/export up to 500 notes / 2 MiB.
- ICS imports up to 2 MiB / 1,000 resources; subtree operations up to 500 records,
  depth 32; recurrence windows one year, 10,000 steps, 2,000 results.
- Trash up to 10,000 entries per instance.
- Moves keep IDs and require the same owner; copies get new IDs.

Verified by the workspace integration and standards tests, `workspace-browser.mjs`,
vdirsyncer round trips and forced restart. Backups include note trash.
