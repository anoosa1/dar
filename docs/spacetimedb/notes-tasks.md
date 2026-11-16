# Notes and Tasks

Delivered November 2026 (layout A, Focused workspace). Same runtime, database and
identity as the other apps; no extra dependency or worker. UI rules are in the
[design guide](../design.md).

## Notes

- Notes are Markdown files in a Files folder, like an Obsidian vault: Files ›
  `docs/notes` by default (Notes settings › Notes folder), created on first use.
  Folders inside it are notebooks; hidden names (such as `.obsidian`) are skipped.
  Any editor, WebDAV client or the Files app edits the same files, with Files'
  versions, favorites (the note star), sharing and Deleted files (restore there).
- The list shows each note's title (front-matter `title`, a leading `#` heading,
  else the file name), a plain-text excerpt, edit time and folder. Summaries come
  from the immutable blobs and are cached by blob ID. The library lists up to
  3,000 notes and 5,000 entries, 16 folders deep.
- New notes start as `Untitled.md` and take their first heading as the file name
  when closed. Rename, move between folders, favorite and delete from the row menu.
- The editor is the shared Markdown editor (autosave with revision checks,
  tab-local recovery drafts, explicit conflicts, Write/Preview).
- Global search finds notes by file name. Notes from the earlier database storage
  are copied once into the folder (one folder per notebook; title as a heading,
  tags as front matter, favorites kept); the old copies are left untouched and
  their notebook import/export stays in Notes settings.
- APIs: `/api/v1/notes/library` (tree and summaries); writes use the Files APIs.
  The older `/api/v1/notes/notebooks`, `/items` and `/trash` remain for the copy.

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
