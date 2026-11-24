# Notes and Tasks

Delivered November 2026 (layout A, Focused workspace). Same runtime, database and
identity as the other apps; no worker. The Notes editor is CodeMirror 6 (npm packages
bundled into the client's dist, loaded the first time a note opens). UI rules are in
the [design guide](../design.md).

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
- The editor (autosave 900 ms after typing stops, with revision checks, tab-local recovery
  drafts, explicit conflicts, Ctrl/Cmd+S, Escape to close) has two modes, remembered per
  session and defaulting to Notes settings › Open notes in (`notesEditorMode`: `write` is
  Live, `preview` is Reading). Live is CodeMirror with Obsidian's live preview
  (`crates/client/editor/notes-editor.js`): the lines the selection touches show their
  Markdown; elsewhere heading, emphasis, highlight, strike, code, quote and link markers hide,
  `[[wikilinks]]` and links show as links (resolved or dashed; Ctrl/Cmd+click, Alt+Enter or
  Ctrl/Cmd+Enter follows), tags are chips, task boxes are checkboxes (click or Ctrl/Cmd+Enter),
  callouts, code blocks, tables and front matter keep their look. CodeMirror's history backs
  Undo/Redo (toolbar and keys); Ctrl/Cmd+B and +I bold and italicize. Reading is the rendered
  preview with references, embeds, hover cards and queries. The Rust side
  (`crates/client/src/workspace/note_editor.rs`) keeps suggestions, the outliner, formatting,
  attachments and saving; offsets cross as UTF-16, line endings stay as written.
- Global search finds notes by file name. Notes from the earlier database storage
  are copied once into the folder (one folder per notebook; title as a heading,
  tags as front matter, favorites kept); the old copies are left untouched and
  their notebook import/export stays in Notes settings.
- Linked notes, in Obsidian's syntax so a vault opens elsewhere unchanged: `[[note]]`,
  `[[note#heading]]`, `[[note#^block]]`, `[[note|label]]`, `![[embed]]`, relative Markdown links
  (`[x](Other%20note.md)`), `#tags` (nested as `a/b`), YAML properties (`tags`, `aliases`),
  `^block` ids, `> [!type]` callouts, `==highlights==`, `%%comments%%`, tables, footnotes,
  task lists, `$…$`/`$$…$$` math and ```` ```mermaid ```` diagrams. Links resolve by file name
  (the linking note's folder first, then the shortest path) or by path; the shared model is
  `crates/notes` (`dar-notes`).
- Math and diagrams: `$…$` inline math, `$$…$$` display math and ```` ```mermaid ```` fenced
  blocks render in every Markdown preview (Notes Reading mode, embeds, references, Files and
  Mail). KaTeX (MathML output) and Mermaid (`securityLevel: "strict"`, themed from Dar's
  light/dark CSS variables inside a per-diagram shadow root with constructed stylesheets for
  strict CSP) are bundled separately in `crates/client/render/` and loaded lazily only when a
  preview contains math or a diagram; the source stays as the fallback while loading and on error.
- Writing: `[[` suggests notes (title, file name or alias) and `#` suggests tags. The preview
  follows links, creates a missing note on click, filters by a tag and toggles checkboxes in
  the source; Outline jumps to a heading. Below each note: linked references (with the
  linking lines) and, on demand, unlinked references with a Link action.
- Embeds: `![[note]]`, `![[note#heading]]` and `![[note#^block]]` show that part of the note
  in place (read-only, one level deep); `![[photo.png]]`, `![[photo.png|300]]` and
  `![](attachments/photo.png)` show images, audio and video from the Notes folder, and
  other files as links. Pasted or dropped files go to `attachments/` (made on first use)
  and are embedded at the caret.
- `/` at a line start or after a space lists commands (headings, to-do, lists, quote,
  callout, code, table, divider, link, date, time, math) and the notes in `Templates/`;
  a template is inserted with `{{title}}`, `{{date}}` and `{{time}}` filled in and its
  properties dropped. Today opens `Journal/YYYY-MM-DD.md`, made from `Templates/Daily`
  (or `Daily note`, `Journal`) when it exists.
- Outliner keys in lists: Enter continues the list (numbers count up, checkboxes start
  open; Enter on an empty item ends it), Tab and Shift+Tab indent and outdent (outside
  lists Tab moves focus), Alt+Shift+Up/Down move the selected lines.
- Query blocks (` ```query `) list matching notes as a table or list, one condition per
  line: `tag:`, `folder:`, `text:`, `where: field op value` (`= != > < >= <= contains`,
  numbers compared as numbers), `sort:`, `show:`, `view: list`, `limit:` (at most 200).
  Fields are properties plus `title`, `folder`, `path`, `tags` and `modified`. See
  `crates/client/src/workspace/note_query.rs`.
- Hovering or focusing a resolved link in the preview opens a preview card of that note or
  section after a moment. File options › Copy block link gives the caret's line a `^id`
  (when it has none) and copies `[[note#^id]]`.
- Graph (sidebar): the notes and their links on a canvas (pan, zoom, hover neighbors, a
  text or `#tag` filter, Local graph for the open note) beside a list of the same notes by
  link count; reduced motion lays it out without animating.
- Review (sidebar, with the due count): spaced-repetition flashcards compatible with
  Obsidian's Spaced Repetition plugin — single-line `Question::Answer` (or `:::` reversible),
  multi-line `?` blocks, and Logseq-style `#card` list items with nested answers. Decks come
  from `#flashcards/...` tags or the note's folder; grading (Again/Hard/Good/Easy, keys 1–4,
  Space to reveal) runs the SM-2 variant in `dar-notes` and rewrites only that card's
  `<!--SR:!YYYY-MM-DD,interval,ease-->` comment with revision checks.
- Renaming or moving a note, or renaming a folder, rewrites the links to the affected
  notes in other notes (each write checked against the revision read); a clean open editor
  reloads. The sidebar lists tags.
- APIs: `/api/v1/notes/library` (tree, summaries, tags, aliases and properties; search also matches
  aliases), `/api/v1/notes/backlinks?id=`, `/api/v1/notes/mentions?id=`, `/api/v1/notes/graph`
  (nodes and resolved, deduplicated edges from the cache), `/api/v1/notes/cards` (due and new
  flashcards and per-deck counts from the cache) and `/api/v1/notes/attachments`
  (the folder's other files, `service_notes` operation `attachments`); writes use the
  Files APIs. The runtime reads each note version once and caches what it found by blob ID.
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

Verified by the `notes-links` and `workspace-browser` suites, the workspace integration and standards tests,
vdirsyncer round trips and forced restart. Backups include note trash.
