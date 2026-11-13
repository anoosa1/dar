# Dar design guide

The shared design contract for every app, sign-in and settings. Read it before UI
work. The same interaction must behave the same way everywhere; reuse the shared
implementation instead of writing an app-specific version. Newer explicit
maintainer decisions win; update this guide when they change.

## Frame (unified look B, November 11)

Selected from [mock B](spacetimedb/unified-look-mock-b.html) over
[mock A](spacetimedb/unified-look-mock-a.html). Built from `next/client/frame.css`,
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
- The search covers all apps: accessible file names, contacts, calendar
  resources, Notes content, Tasks, Agent projects/threads and enabled installed
  apps' own-user data. Never another account's or a disabled app's data.
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
  choice with the menu check, not a tonal fill.

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
  Share quick actions, then Details/Versions/Sharing tabs. Moving to deleted files
  shows a toast with Undo. Storage is a meter card at the bottom of the sidebar.
  Text and Markdown files open in the Notes editor layout (type, save status,
  Write/Preview, file menu; centered column with the name as title, edit time
  and the formatting bar); plain text uses a monospace font.
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
  all-day end dates. Week start, cancelled events and ICS in settings; timezone in
  Regional settings, with per-event timezone fields.
- **Notes:** list rows show title, a plain-text excerpt, edit time and notebook.
  The note bar has Close, notebook, save status, Write/Preview, favorite and the
  note menu (details, Export Markdown); the document is a centered column with a
  quiet title and an icon formatting bar. Exact Markdown, safe preview,
  autosave, private recovery drafts and explicit conflicts. Leaving an unsaved
  note offers save/retry, keep draft and close, or discard, then resumes the
  requested navigation only after success.
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
  with Back). People/Places/Explore tabs, Shared with me, Duplicates, Locked folder
  and Trash arrive with later phases. See [Photos](photos.md).
- **Agent:** threads on the left, conversation and docked composer in the center,
  review on the right and collapsible output below, inside the shared frame.
  New thread is the create button; Ctrl/Cmd+P opens the command palette. Thread
  rows are one 36px line (project mark, title, age; actions on hover/focus) with
  Working and Settled shelves. On phones the thread list is home and a thread
  opens full screen with Back and Conversation/Details/Changes/Terminal tabs.
  Saving a message is distinct from running it; never show simulated output.
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
