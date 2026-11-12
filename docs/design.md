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

## Search

- The top-bar search covers all apps: accessible file names, contacts, calendar
  resources, Notes content, Tasks, Agent projects/threads and enabled installed
  apps' own-user data. Never another account's or a disabled app's data.
- Group results by app with keyboard selection and loading, empty, partial-error
  and retry states. Bound scans and say when results are truncated.
- Ctrl/Cmd+K focuses it outside open editors/dialogs; Escape clears it. Apps do
  not add their own duplicate search or tag/category filter fields.

## Collections and menus

- Address books, calendars, notebooks and task lists share one pattern: select
  the row; a separate three-dot menu offers permission-aware Edit, Share and
  Delete for that row, even when another collection is selected.
- Collection lists page on desktop and show every collection as a chip on phones.
- Import/export lives in app settings with an explicit target collection
  (vCard in Contacts, ICS in Calendar, notebooks/task lists in Notes/Tasks).
- Destructive actions use the shared confirmation, naming the target and effect.
- Menus fit the viewport, open in the right direction, scroll internally and use
  shared keyboard, Escape, outside-click and focus-return behavior.

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
- Administration → Apps enables/disables Notes, Tasks, Agent and installed apps
  instance-wide, retaining data. Files, Contacts and Calendar have no toggle.
- App passwords use an app × Read/Write matrix. Keep old grants; write-only never
  exposes contents. Show the Files DAV address when issuing a Files password.
- Preferences save with account scope and revision checks, preserve other apps'
  values, and must restore after reload.

## Apps

- **Files:** item actions in three-dot menus; Details is its own action and does
  not repeat Rename/Move/Copy/Delete. Preview/download, versions, favorites,
  trash/restore and revision-safe text/Markdown editing. Storage usage sits at
  the bottom of the sidebar.
- **Contacts:** list plus right details/editor. Name sort and vCard transfers in
  settings. Email, phone and website values are links.
- **Calendar:** Month by default. One date/Today control beside the period arrows
  opens a themed month-grid popover with Go to today. Day/week/agenda scroll
  internally; Month has six bounded rows with a link to overflowing days. Week
  start, cancelled events and ICS in settings; timezone in Regional settings,
  with per-event timezone fields. Forms show inclusive all-day end dates.
- **Notes:** clean document surface with a quiet title, compact formatting bar and
  small saved/error status; metadata in a note menu. Exact Markdown, safe preview,
  autosave, private recovery drafts and explicit conflicts. Leaving an unsaved
  note offers save/retry, keep draft and close, or discard, then resumes the
  requested navigation only after success.
- **Tasks:** list plus details with readable status, priority, dates and
  repetition, and a specific Complete/Reopen command. Hierarchy, recurrence
  scope and timezone behavior are preserved.
- **Scheduling forms:** common repetition, priority and reminder choices, with
  advanced controls for exact imported values. Never simplify an unknown rule.
- **Prayer Times:** a today band (next prayer, countdown and a timeline of the
  day with a now marker) above a compact week where every row lists its times;
  a day opens the right daily timetable, phones open it full width with Back to
  week and restored focus. Settings use the settings sheet and mark the times a
  change affected. See [app platform](spacetimedb/app-platform.md).
- **Agent:** threads on the left, conversation and docked composer in the center,
  review on the right and collapsible output below, inside the shared frame.
  New thread is the create button; Ctrl/Cmd+P opens the command palette. Thread
  rows are one 36px line (project mark, title, age; actions on hover/focus) with
  Working and Settled shelves. On phones the thread list is home and a thread
  opens full screen with Back and Conversation/Details/Changes/Terminal tabs.
  Saving a message is distinct from running it; never show simulated output.
  See [Agent workspace](spacetimedb/agent-workspace.md) and [Codex](codex.md).
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
workspace refinements, Prayer Times, Agent) and `settings-revamp-mock-*.html`. Scope and limits per area:
[architecture](spacetimedb/architecture.md),
[Contacts and Calendar](spacetimedb/contacts-calendar.md),
[Notes and Tasks](spacetimedb/notes-tasks.md),
[identity](spacetimedb/identity-provider.md),
[app platform](spacetimedb/app-platform.md) and the
[Nextcloud reference](spacetimedb/nextcloud-reference/README.md).
