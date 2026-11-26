# Dar design guide

The shared design contract for every app, sign-in and settings. Read it before UI
work. The same interaction must behave the same way everywhere; reuse the shared
implementation instead of writing an app-specific version. Newer explicit
maintainer decisions win; update this guide when they change.

## Frame (Full-canvas frame & Direction A app menu)

Built from `crates/client/styles/frame.css` and `WorkspaceHeader` / `AllAppsSwitcher` / `WorkspaceAccount` (`shell.rs`). Every app occupies the full viewport width with no permanent left app rail or mobile bottom bar.

- **Top bar (desktop, 56px):** left cluster has `[Dar mark] [9-dot All apps button] [App title]`; centered cross-app search pill (`Ctrl K`); right cluster has `[App settings gear] [User profile avatar]`.
- **All apps popover (Direction A):** opens from the 9-dot button as a frosted elevated surface (`backdrop-filter: blur(18px)`), showing compact 2-column horizontal app rows with soft Twilight-harmonized tinted icon badges and a subtle active pill + indicator dot. Defaults to a single **"Apps"** heading with no default categories; users can reorder apps and create custom categories in **Dar Settings → Appearance** (`appLauncher` preference). Footer includes `Jump with Ctrl K`, `Customize`, and `Manage apps` (admins).
- **User menu (desktop):** opens from the top-right avatar as a frosted popover with profile header, inline `Afterglow / Nightfall / System` theme switcher, Dar settings, Appearance/Personalization, Security, Manage apps (admins), and Sign out.
- **Sheets:** content and details sit in floating sheets (16px radius, soft border, 8px gutter) on the soft background. The sidebar is transparent.
- **Create:** one tonal 48px button (16px radius) at the top of every sidebar. Files uses it as a New menu (Folder, File → Text/Markdown); the menu aligns with the button and is at least as wide.
- **Selection:** tonal pill for navigation and list rows; hover stays neutral. Sidebar rows are 36px.
- **Phones (≤800px):**
  - No bottom app bar (`--bottom-bar-height: 0px`) and no horizontal bubble strip below search.
  - Separated 3-piece top bar (58px): `[☰ Navigation drawer]` `[Search all apps…]` `[User avatar]`.
  - Tapping `☰` slides out the current app's navigation sidebar (views, folders, collections, and app settings) over a blurred scrim.
  - Tapping the User avatar opens the full-screen **User & Apps Hub**: Direction A 2-column tinted app rows on top (honoring `"Apps"` or user-defined categories), and a grounded rounded Account Sheet at the bottom (profile, inline theme switch, Settings, Personalization, Manage apps for admins, Sign out).
  - Details and editors fill the screen below the top bar with their own close/Back control.

## Theme and controls

- Twilight Afterglow (light) and Nightfall (dark), plus System. Plum/violet
  structure, orchid/rose actions, neutral text, distinct error/success/warning.
  Stored `latte`/`mocha` preference values stay compatible.
- Quiet Twilight controls: outlined fields, square checkboxes, circular radios,
  accent checked states, rounded neutral scroll thumbs, themed upload buttons.
  Keep native keyboard behavior, mixed/disabled states and forced colors.
- Use shared buttons, menus, selects, panes, dialogs and empty/error states. No
  inline product styles, browser prompt/confirm dialogs or promotional copy.
- Spacing: 20px desktop / 16px phone pane insets, 16px group and field gaps,
  56px contact/prayer rows, 20px content headings.

## Workspace

- Left navigation, middle content, focused right details/editor pane. Creation
  and editing use the right pane.
- Panes scroll independently and fit the viewport; the document never grows.
  In Contacts only the middle list scrolls. Detail tabs keep a stable frame.
- Search results and menus stay above details and editors, including the Files
  text editor, on desktop and phones. On phones, menus also stay above the bar.
- Phones use the same complete forms at full width, move focus into opened
  details and return it to the originating row on close.

## Search (spotlight, November 13)

Selected [search mock A](spacetimedb/search-palette-mocks.html) (spotlight) over B
(floating island). Built in `global-search.tsx`.

- The top bar keeps a centered pill that only opens the palette. Ctrl/Cmd+K or a
  click opens a centered modal (760px, 96px from the top, blurred scrim): query
  field, app filter chips with counts (Tab/Shift+Tab cycle them), grouped results
  and a preview of the active row with Open and New tab. Phones show it full screen.
- Empty query: recent searches and recently opened results (this device, per
  account, with Clear) and actions. `>` lists commands (go to an app or settings
  section); matching commands also lead normal results.
- Escape clears the query, then closes and returns focus to the opener.
- The search covers all apps: accessible file names, the text of notes and other
  text files (indexed in the background, `files/text-index.ts`; shown as an excerpt),
  contacts, calendar resources, Tasks, Chat (chat names and message text, newest
  first; a result opens the chat at the message), Agent projects/threads and enabled
  installed apps' own-user data. Never another account's or a disabled app's data.
- Group results by app (with a count) with keyboard selection and loading, empty,
  partial-error and retry states. Rows use the app's glyph (initials for contacts,
  file-type tiles) and highlight the match. Bound scans and say when truncated.
- Ctrl/Cmd+K works outside open editors/dialogs. Apps do not add their own
  duplicate search or tag/category filter fields.

## Collections and menus

- Address books, calendars, notebooks and task lists share one pattern: select
  the row; a separate three-dot menu offers permission-aware Edit, Share and
  Delete for that row, even when another collection is selected.
- Collection lists page on desktop and show every collection as a chip on phones.
- Import/export lives in app settings with an explicit target collection
  (vCard in Contacts, ICS in Calendar, notebooks/task lists in Notes/Tasks).
- Destructive actions use the shared confirmation, naming the target and effect.
- Menus fit the viewport, open in the right direction, scroll internally and use
  shared keyboard, Escape, outside-click and focus-return behavior. Items are
  34px with an icon, grouped by separators or a small section label, with
  right-aligned shortcut hints, a chevron for submenus (Back row on the second
  page), a check on the current choice and the destructive item last. Every
  dropdown (menus, selects, search results, the date picker, Agent pickers) uses
  the same surface (14px radius, raised, one shadow) and rows; a select marks its
  choice with the menu check, not a tonal fill. Menus, selects and pickers open in
  the browser's top layer (`useFloating` in `controls.tsx`), placed beside their
  trigger and clamped to the viewport, so no rail, sheet or pane covers or clips them.

## Content sheets (apps revamp, November 12)

Requested by the maintainer from the [apps revamp mock](spacetimedb/apps-revamp-mock.html);
applied in `apps.css` over the frame. Shared pieces: `FileTile` (`file-types.tsx`),
`CollectionDot`/`avatarHue` (`pim/common.tsx`) and `MiniMonth` (`date-popover.tsx`).

- Sheet headers put the title (or breadcrumb) left and view controls right: a
  segmented control for views/modes and a compact sort menu that names the sort.
- Details sheets lead with the object (tile, avatar or colored title), then
  44px quick actions, then property cards (icon, value, small label). Secondary
  actions sit in the header (Edit, Delete, More) or a footer (Export).
- Color comes from data: collection colors through SVG fill (CSP has no inline
  styles), file types and contact avatars through fixed class palettes. Color is
  never the only signal.
- Editing panes group fields into sections with icon headings, pair related
  fields, keep rarely used fields behind a disclosure that opens when they have
  values, and pin Cancel/Save at the bottom.

## Settings (settings sheet B, November 12)

Selected from [settings mock B](spacetimedb/settings-revamp-mock-b.html) over
[mock A](spacetimedb/settings-revamp-mock-a.html). Built from `SettingsSheet`,
`SettingRow`, `SettingSwitch` and `Segmented` (`settings-sheet.tsx`).

- App settings open from the top-bar gear (last chip on phones) as a right sheet
  in place of the details pane, with tabs: General, Import and export where
  relevant, and Keyboard shortcuts. Phones show it as a bottom sheet.
- Rows put the label and a short explanation on the left and the control on the
  right: a segmented control for two or three choices, a switch for on/off and
  the shared select for longer lists.
- Each control saves when changed (text fields on blur or Enter, only when
  valid) and shows Saving… then Saved. The last change offers Undo. Invalid values
  keep their error and are never saved. A change to the same value elsewhere is a
  conflict with Reload; changes to other values are merged.
- Dar settings opens on Overview: the profile header and one card per section
  showing its current value. Cards open their section; Appearance switches theme
  from its card. Appearance and Regional settings save as you go.
- Personal: Profile, Appearance, Security, Regional settings (timezone) and
  Connected applications. Administration: Appearance (sign-in logo), Apps,
  Applications (OIDC clients), Directory, SCIM and Signing keys.
- Administration → Apps enables/disables Notes, Tasks, News, Agent and installed apps
  instance-wide, retaining data. Files, Contacts and Calendar have no toggle.
- App passwords use an app × Read/Write matrix. Keep old grants; write-only never
  exposes contents. Show the Files DAV address when issuing a Files password.
- Preferences save with account scope and revision checks, preserve other apps'
  values, and must restore after reload.

## Apps

- **Files:** sidebar views (`Recent`, `All files` with a collapsible top-level folder tree,
  `Favorites`, `Shared` with *Shared with you* / *Shared by you* / *Public links* sub-tabs,
  `Locked folder`, `Deleted files`) and a storage meter card at the bottom. Upload lives inside the
  `+ New` menu (Folder, Upload files, Upload folder, File → Text/Markdown) rather than a separate
  header button; a picked or dropped folder is recreated with its subfolders (a name already used
  gets a number). The breadcrumb is the title with a current-folder actions menu, beside the
  list/grid switch and an Inspector toggle (`i`); a command strip below holds lightweight type
  filter chips (`All`, `Docs`, `Images`, `Code`, `Archives`, `Media`) and a sort menu (Name,
  Modified, Size, Type) that works in both List and Grid views. Folders sit in a compact horizontal
  shelf (with item counts and a Git branch badge when tracked) above the sortable Files list/grid.
  Rows show a type tile, relative modified date, size and hover quick actions (favorite, share,
  download) beside the three-dot menu; drag-and-drop moves items onto folders and breadcrumbs;
  selection turns the toolbar into a tonal bar with direct or ZIP download (multi-file/folder),
  Move to…, Copy to…, Move to deleted files (Delete permanently in the Locked folder) and More.
  Item actions live in three-dot menus (Open, Organize, Clipboard, Delete sections); Details is its
  own action and does not repeat Rename/Move/Copy/Delete. Details: preview, Preview/Download/Favorite/
  Share quick actions, then Details/Versions (with version badges, download and restore)/Sharing
  (people, selectable expiry and one-click copy for public links) tabs; folders show History in
  place of Versions ([folder history](spacetimedb/folder-history.md)): Track history, uncommitted
  changes with Review and commit (right pane with per-file diffs) and Snapshot, agent branches,
  the commit list (a commit opens in the right pane with Restore folder to this point), a Git card
  (clone address with Copy, pushed branches and tags, Open in Git), Download as Git bundle and Stop
  keeping history. Moving to deleted files shows a toast with Undo; Deleted files adds original
  folder subtitles, inline Restore and Empty deleted files. The Locked folder hides view controls
  until unlocked, then works as a normal folder with a Lock button whose Delete is permanent.
  Text and Markdown files open in the Notes editor layout; other files preview in the same sheet
  (`file-preview.tsx`) with image zoom (`-`/`+`/`Fit`), direct Share, side-by-side Details toggle,
  previous/next (arrow keys), Download and the file menu. On phones, rows show size and modified
  time as a subtitle without permanent checkboxes and leave clearance above the `+` button.
  The shared folder picker (`file-picker.tsx`) is a small browser: back, breadcrumb, filter, rows
  with type tiles (click selects, the chevron opens) and the destination below.
- **Contacts:** scrollable sidebar with smart views (`All contacts`, `Favorites`, `Ungrouped`,
  `Merge & fix` with duplicate badge), address books with live counts and three-dot menus,
  manageable groups (`+`, Rename, Remove), and an Upcoming birthdays footer card. The middle sheet
  header pairs the title and count pill with a List / Table / Cards view switcher and inline sort
  menu (First name, Last name, Organization); quick filter chips (`All`, `Has email`, `Has phone`,
  `Birthdays`) switch to a tonal bulk selection bar (Group, Move, Export, Delete) when rows are
  checked via hover-reveal avatar checkboxes. Rows show contact photos or hued initials, subtitle,
  inline favorite star and address-book dot beside an A–Z quick-jump rail. The details inspector
  offers photo portrait, Email/Call/Favorite/Edit/QR quick actions, one-click copy on fields,
  wrapping interactive group chips with inline `+ Group`, Clone, vCard QR modal, cross-app links
  (`In Dar`), Export and Delete. The editor adds photo upload/crop, address book move, typed label
  selects, group chips, date-picker birthday (with omit-year toggle), and progressive `+ Add field`
  pills (Nickname, Pronouns, Department, Anniversary, Social profile, Related person), plus single-key
  shortcuts (`C`, `J`/`K`, `E`, `F`, `X`, `Escape`). vCard/CSV import/export, display format, default
  view, birthdays toggle and CardDAV sync URL live in Contacts settings.
- **Calendar:** Month by default; Day/Week/Month/Year/Agenda is a segmented control,
  paired with a clickable period title (`DatePopover`), 1-click Today button, period arrows,
  and single-key keyboard shortcuts (`C`, `T`, `D`/`W`/`M`/`Y`/`A`, `J`/`K`, `?`). A sidebar
  mini month shows event-dot indicators and highlights the visible range above a scrollable
  calendar list (My calendars + first-class Tasks toggle + Subscribed, with quick solo and
  color actions) and an Up next / Reminders footer card. Events sort chronologically; all-day
  and multi-day spans draw as continuous horizontal ribbons (overlaid in Month and pinned in a
  dedicated sticky All-day band in Week/Day). Week and Day draw duration-sized blocks in lanes
  with a live now-line pill and bottom-edge drag-to-resize; drag-move and resize save directly
  with an Undo toast. Clicking an event opens a floating Quick-Peek popover (zero layout shift)
  with clickable location links, Duplicate, Edit, Delete, and ICS export; clicking an empty grid
  cell opens a Quick-Create popover with duration chips and "More options". Month overflow links
  open an in-place day popover. The full Event editor supports moving events between calendars,
  a visual recurrence builder (`RepeatField`), and Contacts-assisted attendee search. Week start,
  12h/24h time format, cancelled events, tasks and ICS live in Calendar settings; timezone in
  Regional settings. Subscribed calendars carry a link mark (or alert mark on failure) and are
  read-only. Dated tasks render with clickable completion checks and open in Tasks. Phones use a
  compact 2-row header with a 7-day dot strip and bottom-sheet popovers.
- **Notes:** notes are Markdown files in a Files folder (`docs/notes` by default,
  set in Notes settings); its folders are the sidebar's notebooks. List rows show
  title, a plain-text excerpt, edit time and folder, with a row menu (favorite,
  rename, move, Show in Files, delete). A note opens in the Notes editor
  in the right sheet: Close, folder, save status, Live/Reading and the file menu;
  autosave, private recovery drafts and explicit conflicts. Live (the default) is
  Obsidian's live preview on CodeMirror: the lines being edited show their Markdown, the
  rest hide their markers and take the preview's look (heading sizes, emphasis, links and
  wikilinks that Ctrl/Cmd+click or Alt+Enter follows, tag chips, clickable checkboxes,
  quotes and callouts with their rule, code panels); Reading is the rendered preview
  (including LaTeX math as MathML and themed Mermaid diagrams, shared across Markdown previews).
  Open notes are tabs in that sheet (Obsidian's tabs, no new chrome): a tab strip above the
  editor (title, unsaved dot, close; middle-click closes, right-click or the strip's menu for
  options), Ctrl/Cmd+Tab or Alt+]/Alt+[ cycles and Ctrl/Cmd+W or Alt+W closes. A row click replaces the shown tab
  (never one with unsaved changes); Ctrl/Cmd+click, the middle button or the row menu's Open in
  new tab opens another; in Live a link takes Ctrl/Cmd+Shift+click for a new tab. Open to the
  right (row and tab menus, Ctrl/Cmd+Alt+click) splits the sheet into two panes with their own
  tabs; below 1100px one pane shows, with a Left/Right switch.
  New notes take their first heading as the file name when closed. Linked writing follows Obsidian and
  Logseq without new chrome: `[[`/`#` suggestions in a top-layer list at the caret,
  Outline in the editor bar, properties above the preview, callouts, tag chips,
  dashed links for missing notes, and Linked/Unlinked references as disclosure
  groups at the end of the note (Logseq's place). Tags follow Folders in the sidebar;
  Today, Random note, Graph and Review sit under the views; Graph replaces the
  list with the canvas and its note list, and Review shows due flashcards with
  deck filters, one card at a time (Space to reveal, 1–4 to grade, Open note)
  storing schedules inline as `<!--SR:...-->` comments. Query and ` ```base `
  blocks share one result table (or list), with a quiet line naming the parts of
  a base left out. Notes settings › Import and export holds Import a Logseq graph
  (files or the graph folder, then a summary). Canvases (`*.canvas`, JSON Canvas 1.0)
  appear in the Notes list with a grid icon and open in the same sheet (New canvas
  in the sidebar): positioned text, note/file, link and group cards with SVG
  connections, pan/zoom, keyboard movement and an accessible card outline. Embedded notes are a quiet inset with an accent rule
  and an open link; `/` commands share the suggestion list.
- **Tasks:** scrollable sidebar with smart views (`Today` with overdue badge,
  `Next 7 days`, `Inbox`, `Starred`, `All tasks`, `Completed`, each configurable
  as Show/Auto/Hide), task lists with live counts and three-dot menus (becoming
  column-visibility toggles in List Board mode), clickable `#tags`, a `Today's
  pace` completion card, and `Reminders` with live badge and timezone at the
  bottom. The middle sheet header pairs the title and count pill with a List /
  Board (side-by-side list columns with drag-and-drop) / Status (To do / In
  progress / Completed Kanban) switcher, a 6-way sort menu (Due date, Priority,
  My order, Start date, Title, Recently edited) and `?` shortcuts; quick filter
  chips (`All`, `Overdue`, `High priority`, `Has subtasks`, `Repeating`) swap to
  a tonal bulk selection bar (Complete, Today, Star, Move, Delete) when rows are
  checked. Smart Quick-Add includes inline Today/Tomorrow/High/list pills. Due-date
  sort groups Overdue (with 1-click Move to Today), Today, Tomorrow, Upcoming and
  No date, with a collapsible Completed drawer below. Rows show hover checkboxes,
  priority rings, expandable nested subtask trees (`done/total`), inline star and
  action menus; reversible actions show a floating Undo toast. The 368px right
  inspector supports direct inline editing (title, description, quick-schedule
  pills, priority pills, interactive progress slider, checkable subtasks with
  inline `+ Add subtask`, Location, clickable URL, interactive `#tags` chips)
  beside the full VTODO editor pane. Single-key shortcuts (`C`, `J`/`K`, `Space`,
  `S`, `E`, `X`, `1`/`2`/`3`, `?`, `Escape`) work outside inputs and dialogs.
  Defaults, display switches, smart-view visibility, CalDAV sync URL and ICS /
  Markdown export live in Tasks settings. Hierarchy, recurrence scope and timezone
  are preserved.
- **Scheduling forms:** common repetition, priority and reminder choices, with
  advanced controls for exact imported values. Never simplify an unknown rule.
- **Prayer Times:** a today band (next prayer, countdown and a timeline of the
  day with a now marker) above a compact week where every row lists its times;
  a day opens the right daily timetable, phones open it full width with Back to
  week and restored focus. Settings use the settings sheet and mark the times a
  change affected. See [app platform](spacetimedb/app-platform.md).
- **News:** selected [mock A](spacetimedb/news-reader-mock-a.html) over
  [mock B](spacetimedb/news-reader-mock-b.html) (November 12). Sidebar views
  (Unread, Starred, All) then folders with nested feeds and unread counts, feed and
  folder three-dot menus, a day-grouped list (unread dot, feed mark, title, excerpt,
  time) and the article in the right sheet with star, keep unread, open original
  and share. Read rows fade in place instead of disappearing. Subscribe and Edit
  feed use the right editing pane; News settings is the settings sheet. Phones show
  views and folders as chips and the article full screen with Back. See [News](news.md).
- **Mail:** selected direction [A](spacetimedb/mail-mocks.html) with B's inline reply (November 22; refreshed November 24).
  Scrollable sidebar: Compose, unified views (`All inboxes`, `Unread`, `Flagged`, `Drafts`, `Sent`,
  `All mail`) with distinct icons and live counts, then collapsible accounts (colour mark,
  protocol badge, warning mark, reactive collapsed unread badge, three-dot menu with `New folder`)
  with nested server mailboxes, inline `+ New folder…`, and a live sync-status footer.
  The middle sheet header pairs a `Select all ▾` menu (`All`/`None`/`Unread`/`Read`/`Flagged`),
  title and count pill with a `Split / List` layout switch, a `Sort & density` menu (`Newest`,
  `Oldest`, `Sender`, `Largest`; `Comfortable` / `Compact 1-line`), refresh, mark-all-read and `?`
  shortcuts; a filter-chip strip (`All`, `Unread`, `Flagged`, `Has files` plus account pills in
  unified views) swaps to the tonal selection bar when rows are checked (Shift-click range select
  supported). Trash and Junk show an action banner (`Empty Trash` confirms permanent delete).
  List rows collapse threads into one conversation row (toggleable in Reading settings), show
  `To <recipient>` in Drafts/Sent, mailbox badges in unified views, and reveal a hover quick-triage
  pill (Archive, Delete, Read/Unread, Flag).
  Split view keeps the reading pane beside the list with a calm empty state; opening a thread shows
  the account · mailbox crumb, triage actions, Expand/Collapse All (`;`), Print, More (including
  `View raw source`, `Show in Files`, `Download .eml`), a `List-Unsubscribe` strip when present,
  remote-image privacy bar (`Show images` + `Always allow` trusted sender), collapsible message cards
  with per-message Info/Reply/More actions, expandable header details with Copy address, trimmed
  plain-text quote toggle (`···`), attachment chips with image Preview modal + Download, and a
  pinned inline reply dock at the bottom.
  Compose is a floating bottom-right sheet with Minimize-to-draft-tray, Expand, aligned From with
  account colour and automatic signature swap, To/Cc/Bcc chips with Contacts and recent-correspondent
  autocomplete, drag-and-drop and `Attach from Files`, the Notes formatting bar (plain text until a
  format is used, then Markdown sent as HTML with Write/Preview), Save draft, Discard and Send
  (10-second Undo). Add account and Sign in again use the right editing pane; Mail settings uses the
  settings sheet (`Accounts`, `Reading`, `Shortcuts`, and `Providers` for administrators).
  Phones keep Dar's mobile chip strip, hide the empty split pane, open conversations full-bleed with
  Back, and show compose full screen. See [Mail](mail.md).
- **Photos:** selected [mock B](spacetimedb/photos-mock-b.html) over
  [mock A](spacetimedb/photos-mock-a.html) (November 13; refreshed November 24).
  Sidebar views (`Photos`, `All photos`, `Explore`, `Favorites`, `Albums`,
  `Shared with me`, `Videos`, `Folders`), then albums with cover thumbnails and
  three-dot menus, a `More` utility group (`Duplicates`, `Archive`, `Locked
  folder`, `Trash`), and a library storage footer card. The sheet header pairs
  title and count with a Justified / Square layout switch and a density/column
  slider (`3–12`), over a filter-chip (`All`, `Stills`, `Clips`, `Live`, `RAW`,
  `Starred`, `Unfiled`) and sort (`Newest`, `Oldest`, `Largest`) sub-bar. The
  timeline groups by day with a hover Select-all-day circle, Shift-click range
  selection, 1-click tile favorite star, collapsible `On this day` memories
  ribbon, and a right-edge month/year jump scrubber; selection turns the header
  into a tonal bar with Download ZIP, favorite, add/remove album, shared link,
  stack, archive, Locked folder and trash. `Albums` opens a filterable/sortable
  4:3 cover hub; opening an album displays an editorial cover hero with date
  span, inline editable description, `+ Add photos` picker, Slideshow, Share and
  Download ZIP. `Explore` surfaces zero-ML Places (EXIF GPS clusters), Cameras,
  Quick collections (`Recently added`, `Not in any album`, `Large files`) and
  Files folders. Opening a photo folds away the sidebar (like Books) into a
  darkroom viewer stage with floating edge Prev/Next arrows, interactive zoom
  (`-`/`Fit`/`+`, wheel, double-click), Slideshow (`s`), Quick Edit (rotate 90°
  and filter presets saved as a new copy in Files), collapsible Inspector (`i`),
  and a filmstrip hidden for single-item sets. Details offer a 4-action quick
  strip, editable description, inline editable capture date/time, monospace EXIF
  pills, a styled location mini-map card with OpenStreetMap link, album chips
  with inline `+ Add`, and Show in Files. Live Photos and stacks are one tile
  with badges; Duplicates highlight the `Best copy`, show reclaimable bytes and
  offer `Keep best in all groups`. Phones keep Dar's shared mobile app bar, top
  search pill, view chips and `+` FAB untouched, hide tile checkboxes behind a
  header `Select` toggle, and open the viewer and details below the top bar with
  `Back`. See [Photos](photos.md).
- **Books:** selected [mock A](spacetimedb/books-mocks.html) over B (November 16). Sidebar
  views (Home, Reading, Want to read, All books, Finished), then libraries (top folders of the
  Books folder). Home has Continue reading cards and Recently added / Want to read shelves;
  other views are a cover grid with a sort menu. Covers show progress, a finished check and a
  format badge; books without a cover get a typographic cover. Details sit in the right sheet
  (cover, status, Read/Continue, Want to read, Download, Mark as finished, description, series
  strip, facts, Show in Files). A book opens in the middle sheet (the sidebar folds away) with
  a bar (Back, title, chapter, Find and zoom for PDFs, Aa, bookmark, contents, full screen,
  more), contents and bookmarks on the right and a footer scrubber. PDFs get a Find bar under
  the bar (count, previous/next, Escape closes; matches tinted with the accent, the current one
  outlined) and a Highlight pill under a text selection; highlights are yellow, multiplied over
  the always-white page so they read in light and dark themes, and saved to Notes; Aa holds theme (Dar, Paper, Sepia, Night),
  font, size, spacing and Pages/Scroll, saved to the account. A newer place from another device
  asks Go there / Stay here. Phones show the reader and details below the search bar with Back.
  See [Books](books.md).
- **Git:** selected [mock B](spacetimedb/git-mocks.html) over A (November 16): its own app.
  Every Files folder that keeps history is a repository. Sidebar views (Overview,
  Repositories, Agent branches), then the repositories, with an app password card at the
  bottom; New repository (right pane) creates a folder with a README and first commit, or
  keeps history on an existing folder. Overview: a year of commits per day, repository cards
  (README's first paragraph, counts, 12 weeks of activity) and recent commits labelled by
  source (Git push, Dar, Agent). A repository has a header (mark, name, Files path, Clone
  menu with the alias and permanent addresses, ZIP, bundle and app password links) and
  Code (branch or tag, path, files with their last commit, README), Commits (branch, author
  and source filters; Graph or List), Branches (behind/ahead of main, Compare, ZIP, delete
  with confirmation; Agent branches open in Agent) and Tags tabs. Commits (with Restore folder
  to this point for main), comparisons (with Merge: fast-forward or a merge commit, optionally
  deleting the branch) and files open in the right sheet. Files are not edited here. Issues and
  Pull requests (November 17, applying mock B's patterns) are tabs after Code: Open/Closed, a
  label filter, Labels and New in the toolbar; rows show a state icon, title, label chips and
  #number. An item opens in the right sheet (title, state chip, labels menu, Conversation; pull
  requests add Commits and Files and the merge box); New, Edit and Labels use the right editing
  pane with Write/Preview Markdown. Label colours are named, never colour alone. Repositories
  shared with you are a "Shared with you" sidebar section. See [Git](git.md).
- **Chat:** selected [mock A](spacetimedb/chat-mocks.html) over B, with account tabs over
  one rail app per platform (D) (November 17). A client for other platforms: WhatsApp first
  (Dar as a linked device through whatsmeow), Discord later. Account tabs under New chat
  (label on the selected tab, icon and unread count on the others, + to link) scope the whole
  app to one account; accounts never mix. WhatsApp's views are Chats, Unread, Groups and
  Archived, with the link status card at the bottom; Discord's tab has direct messages, then
  servers with channels, shown as flat rows (C). The chat list is its own sheet (avatar, name,
  time, preview with ticks, pin/mute marks, unread count) and the conversation fills the rest:
  bubbles with read ticks, replies, reactions, edits, photos, voice notes and documents, the
  composer docked below. Chat info opens as a right sheet; Link WhatsApp uses the right
  editing pane (QR code on desktop, pairing code on phones). Phones put the account tabs
  above the view chips. Dar contacts name chats by phone number (never written to
  WhatsApp); chat info offers Open or Add to Contacts. Discord follows
  [mock B](spacetimedb/discord-mocks.html) (November 18): Direct messages then servers in
  the sidebar, each opening to channels by category (mentions as counts, muted dimmed); a
  channel replaces the list sheet; messages are flat rows (avatar and name start a run);
  Link Discord shows the terms notice with the QR code. Voice: people under each voice
  channel with a speaking ring; a dock at the bottom of the sidebar (under the list on a
  phone) during a call; A's call view (tiles, round controls, the encryption badge opening
  the call code) when the voice channel is opened. See
  [Chat](chat.md).
- **Agent:** the settings sheet and editing panes sit beside the Agent's sheets
  (which make room), as in other apps; choosing and adding a project both use the
  right pane. Threads on the left, conversation and docked composer in the center,
  review on the right and collapsible output below, inside the shared frame.
  New thread is the create button; Ctrl/Cmd+P opens the command palette. Thread
  rows are one 36px line (project mark, title, age; actions on hover/focus) with
  Working and Settled shelves. On phones the thread list is home and a thread
  opens full screen with Back and Conversation/Details/Changes/Terminal tabs.
  Saving a message is distinct from running it; never show simulated output.
  A Files project whose folder keeps history commits its changes to a branch from the
  review panel and merges them into the folder; diffs use the shared `Diff` (`diff.tsx`).
  See [Agent workspace](spacetimedb/agent-workspace.md), [Codex](codex.md) and
  [Claude Code](claude-code.md). With both connected, the model menu groups models
  by provider and labels follow the selected model's provider.
- **Frame between apps:** signed-in pages keep one rail and top bar mounted (`AppFrame` in
  `shell.rs`); apps register their title and settings button through `WorkspaceHeader`, so
  switching apps replaces only the app (an app still loading shows its placeholder in the frame).
- **Sign-in:** Dar logo by default, the saved instance logo when configured, with
  a fallback to Dar if it cannot load.

## Accessibility and verification

- Keyboard use, visible focus, accessible names, reduced motion and responsive
  layouts. Ctrl/Cmd+S saves the active Notes or Files editor.
- Loading, empty, error, retry, read-only and unsaved/conflict states. Never
  silently overwrite another revision or discard a draft.
- Verify real persistence and permissions in a real browser at desktop and phone
  sizes, with isolated roots, users and browser contexts.
- Phones must stay responsive. Do not use `:has()` on `body` or large containers
  (shells, workspaces, grids): Chrome then restyles the whole page on every
  checkbox, menu or class change. Components set state classes instead
  (`has-editing-pane` on `body`, `has-calendar-main`, `is-selecting`). Format
  dates in lists with `client/dates.ts`, not per-row `toLocale…String(options)`.
- A new visual direction needs static mocks and maintainer selection. Applying an
  already-selected pattern does not.

## Decision sources

Selected mocks live in `docs/spacetimedb/*-mocks.html` (unified look, Twilight,
controls, launcher, creation/appearance, sign-in, Contacts/Calendar, Notes/Tasks,
workspace refinements, Prayer Times, Agent, Mail), `settings-revamp-mock-*.html` and `apps-revamp-mock.html`. Scope and limits per area:
[architecture](spacetimedb/architecture.md),
[Contacts and Calendar](spacetimedb/contacts-calendar.md),
[Notes and Tasks](spacetimedb/notes-tasks.md),
[identity](spacetimedb/identity-provider.md),
[app platform](spacetimedb/app-platform.md) and the
[Nextcloud reference](spacetimedb/nextcloud-reference/README.md).
