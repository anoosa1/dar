# Dar design guide

The shared design contract for every app, sign-in and settings. Read it before UI
work. The same interaction must behave the same way everywhere; reuse the shared
implementation instead of writing an app-specific version. Newer explicit
maintainer decisions win; update this guide when they change.

## Frame (unified look B, November 11)

Selected from [mock B](spacetimedb/unified-look-mock-b.html) over
[mock A](spacetimedb/unified-look-mock-a.html). Built from `rust/client/styles/frame.css`,
`AppRail` (`apps.tsx`) and `WorkspaceHeader` (`shell.tsx`).

- **App rail** (72px, left): Dar mark, each enabled app as an icon pill with its
  label (tonal when current), then Settings and the account menu at the bottom.
  Administrators reach Manage apps from the account menu.
- **Top bar** (56px): app title over the sidebar, centered cross-app search,
  and the app's settings gear on the right.
- **Sheets:** content and details sit in floating sheets (16px radius, soft
  border, 8px gutter) on the soft background. The sidebar is transparent.
- **Create:** one tonal 48px button (16px radius) at the top of every sidebar.
  Files uses it as a New menu (Folder, File → Text/Markdown); the menu aligns with
  the button and is at least as wide.
- **Selection:** tonal pill for navigation and list rows; hover stays neutral.
  Sidebar rows are 36px.
- **Phones (≤800px):** the rail becomes a bottom bar with four apps and More (the
  current app always keeps a slot). Search is a full-width pill with the avatar.
  Sidebar views and collections become a horizontal chip strip, app settings is
  the last chip, and create is a floating button above the bar. Details and
  editors fill the screen below the search bar with their own close/Back control.

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

- **Files:** the breadcrumb is the title, with Upload and the list/grid switch
  beside it. Rows show a type tile, relative modified date and size; selection
  turns the toolbar into a tonal bar. Item actions in three-dot menus (Open,
  Organize, Clipboard, Delete sections); Details is its own action and does not
  repeat Rename/Move/Copy/Delete. Details: preview, Preview/Download/Favorite/
  Share quick actions, then Details/Versions/Sharing tabs; folders show History in place of
  Versions ([folder history](spacetimedb/folder-history.md)): Track history, uncommitted
  changes with Review and commit (right pane with per-file diffs) and Snapshot, agent
  branches, the commit list (a commit opens in the right pane with Restore folder to
  this point), a Git card (clone address with Copy, pushed branches and tags, Open in Git), Download
  as Git bundle and Stop keeping history. Moving to deleted files
  shows a toast with Undo. The Locked folder is a sidebar section: a password form
  (shared with Photos) until unlocked, then a normal folder with a Lock button whose
  Delete is permanent; Move to Locked folder asks for the password when needed. Storage is a meter card at the bottom of the sidebar.
  Text and Markdown files open in the Notes editor layout (type, save status,
  Write/Preview, file menu; centered column with the name as title, edit time
  and the formatting bar); plain text uses a monospace font. Other files preview
  in the same sheet (`file-preview.tsx`): images, PDFs (the browser's viewer),
  video, audio and text, fitted to the sheet, with previous/next (arrow keys),
  Download and the file menu. The selection bar offers Download (files), Move to…,
  Copy to…, Move to deleted files (Delete permanently in the Locked folder) and
  More (favorites, Locked folder); Deleted files offers Restore. The shared folder
  picker (`file-picker.tsx`) is a small browser: back, breadcrumb, filter, rows with
  type tiles (click selects, the chevron opens) and the destination below.
- **Contacts:** letter-grouped list under the name sort, with hued avatars, title ·
  organization and the address-book dot. Groups are sidebar rows. The card offers
  Email, Call, Favorite and Edit; values are links. The editor uses sections and
  opens prefix/middle/suffix/display name only when needed. vCard in settings.
- **Calendar:** Month by default; Day/Week/Month/Agenda is a segmented control. A
  sidebar mini month follows the anchor and highlights the visible range; one
  date/Today popover beside the period arrows has Go to today. Visibility boxes
  and events use the calendar color (dot for timed, tint for all-day). Week and
  Day draw duration-sized blocks in lanes with a now line; Month has six bounded
  rows with a link to overflowing days and hides times in narrow cells. Event
  cards show a readable date line; forms split date and time and show inclusive
  all-day end dates. Week start, cancelled events, tasks and ICS in settings; timezone
  in Regional settings, with per-event timezone fields. The Calendars `+` menu has
  New calendar and Subscribe by link (right pane: link, name, color, refresh);
  subscribed calendars carry a link mark (an alert mark after a failed refresh),
  Refresh now and Edit subscription, and their events are read-only. Dated tasks
  show as check chips at their due time and open in Tasks.
- **Notes:** notes are Markdown files in a Files folder (`docs/notes` by default,
  set in Notes settings); its folders are the sidebar's notebooks. List rows show
  title, a plain-text excerpt, edit time and folder, with a row menu (favorite,
  rename, move, Show in Files, delete). A note opens in the shared Markdown editor
  in the right sheet: Close, folder, save status, Write/Preview and the file menu;
  autosave, private recovery drafts and explicit conflicts. New notes take their
  first heading as the file name when closed.
- **Tasks:** quick add (title only) above the list; due-date sort groups Overdue,
  Today, Tomorrow, Upcoming, No date and Completed. Rows show a priority-colored
  check, due, repetition, list and tags. The card leads with Complete/Reopen and
  Add subtask, then properties and subtasks; Reminders and the timezone sit at the
  bottom of the sidebar. Hierarchy, recurrence scope and timezone are preserved.
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
- **Photos:** selected [mock B](spacetimedb/photos-mock-b.html) over
  [mock A](spacetimedb/photos-mock-a.html) (November 13). Sidebar views (Library,
  Favorites, Videos, Archive) then albums as collections with three-dot menus; the
  sheet header has the title, count and a zoom slider for the square, month-grouped
  grid, and turns into a tonal selection bar. A photo opens in the middle sheet with
  a filmstrip; details sit in the right sheet (phones: Info opens them full screen
  with Back). A "More" group lists Duplicates, Archive, Locked folder and Trash.
  Live Photos and stacks are one tile with a badge; the viewer plays the motion and
  shows stack members under the photo. Duplicates are cards of copies with Keep
  checkboxes. A locked view shows the shared password form in place of the grid.
  Shared with me lists albums others share as cover cards; your shared albums carry
  a share mark. Share (album header) opens the right editing pane: people with Can
  view/Can add, then links with copy, expiry, password and switches that save as
  you go. Shared-link visitors get the Photos grid and viewer in a minimal frame
  (brand bar, no app rail). Photos settings has an Immich tab: the mobile app's server
  URL and sign-in, signed-in devices, and the Immich server connection with status and
  Sync now. People/Places/Explore tabs arrive with later phases.
  See [Photos](photos.md).
- **Books:** selected [mock A](spacetimedb/books-mocks.html) over B (November 16). Sidebar
  views (Home, Reading, Want to read, All books, Finished), then libraries (top folders of the
  Books folder). Home has Continue reading cards and Recently added / Want to read shelves;
  other views are a cover grid with a sort menu. Covers show progress, a finished check and a
  format badge; books without a cover get a typographic cover. Details sit in the right sheet
  (cover, status, Read/Continue, Want to read, Download, Mark as finished, description, series
  strip, facts, Show in Files). A book opens in the middle sheet (the sidebar folds away) with
  a bar (Back, title, chapter, Aa, bookmark, contents, full screen, more), contents and
  bookmarks on the right and a footer scrubber; Aa holds theme (Dar, Paper, Sepia, Night),
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
workspace refinements, Prayer Times, Agent), `settings-revamp-mock-*.html` and `apps-revamp-mock.html`. Scope and limits per area:
[architecture](spacetimedb/architecture.md),
[Contacts and Calendar](spacetimedb/contacts-calendar.md),
[Notes and Tasks](spacetimedb/notes-tasks.md),
[identity](spacetimedb/identity-provider.md),
[app platform](spacetimedb/app-platform.md) and the
[Nextcloud reference](spacetimedb/nextcloud-reference/README.md).
