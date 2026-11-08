# Dar design guide

This is the shared design contract for Files, Contacts, Calendar, Notes, Tasks,
sign-in, personal settings and administration. Read it before building or changing
UI. The same interaction must work the same way throughout the apps. Reuse the
shared implementation instead of inventing an app-specific version.

This guide consolidates the maintainer's instructions in the current conversation
and the decisions recorded in the repository. It does not claim access to
unrecorded conversations. New explicit maintainer instructions take precedence;
update this guide when a design decision changes.

## Shared visual direction

- Use the selected compact Material 3 workspace with Catppuccin Latte for light
  mode and Mocha for dark mode. Support the saved System appearance preference.
- Keep one shared header, centered persistent search, themed app launcher and
  account menu across every app and settings. Keep the launcher usable in settings.
- Use shared buttons, menus, form controls, panes, dialogs, empty states and error
  states. A control's spacing, behavior and placement must not vary arbitrarily
  between apps. No inline product styles or browser prompt/confirm dialogs.
- Keep primary creation actions in the same place. Files uses a New menu with
  Folder and File, including Text/Markdown creation. Its menu matches the trigger
  width and alignment. Dark creation buttons share an accent outline and tonal fill.
- Avoid promotional app labels, subtitles and footer copy. Keep useful permission,
  safety and recovery explanations.

## Workspace and scrolling

- The desktop workspace has left navigation, middle content and a focused right
  details/editor pane. Creation and editing use the right pane.
- Panes are independent. Scrolling the middle list must not move the header,
  navigation or right pane. The right pane scrolls its own content.
- In Contacts, only the middle contact list scrolls; the page and open contact on
  the right must not move with it. Keep list controls outside that scrolling list.
- Fit the workspace to the available viewport. Use bounded layout regions with
  internal overflow, rather than letting content make the document taller.
- Keep detail tabs in a stable frame with a stable scroll gutter. Switching tabs
  must not resize the entire pane.
- On phones, use the same complete forms at full width, accessible navigation and
  explicit return/close controls. Avoid horizontal overflow or compressed desktop
  panes. Preserve unsaved changes and offer explicit discard.

## Collection navigation and action menus

- Address books, calendars, notebooks and task lists follow the same collection
  interaction pattern wherever their operations apply.
- Each collection row has a three-dot action menu. Selecting the row and opening
  its menu are separate actions.
- Put Edit, Share and Delete in that collection's menu, with permission-aware
  availability. The menu targets the row that opened it, even if another collection
  is currently selected.
- Do not display a permanent row of Edit/Share/Delete/Import/Export buttons under
  an address book. Contacts must follow Calendar's existing collection menu.
- Import and export belong in app settings, with an explicit collection target.
  Contacts owns vCard transfers there; Calendar owns ICS transfers there.
- Destructive actions use the shared confirmation UI and describe their actual
  target and consequences. Read-only collections do not offer write operations.
- Menus fit within the viewport, open in an appropriate direction and scroll
  internally when needed. Use shared keyboard, Escape, outside-click and focus
  restoration behavior.
- Keep app settings at the bottom of the left sidebar. Contacts and Calendar use
  compact, paged collection lists so their settings footers stay accessible. Files storage usage
  sits immediately above Files settings.

## App settings and global settings

- All apps use the common app-settings presentation, with General and Keyboard
  shortcuts where applicable. App preferences persist per user.
- Keep account-wide preferences in Personal settings and instance-wide controls
  in Administration. Do not scatter the same preference across apps.
- Personal settings include Profile, Appearance, Security, Regional settings and
  Connected applications. Administration includes Appearance, registered
  Applications, Directory, SCIM and Signing keys as permissions allow.
- Personal Appearance selects System/Latte/Mocha. Administration Appearance
  customizes the public sign-in logo, with preview and reset to the Dar logo.
- App passwords use a compact app/Read/Write matrix. Users may select multiple
  apps and Read, Write or both for each. Display the saved grants clearly.
  Keep old passwords' existing permissions. Most synchronization clients need
  both permissions; write-only must not expose resource contents.
- Save preferences with explicit account scope and revision checks. Preserve
  other apps' preferences when saving one setting. Every visible setting must
  actually persist and restore after reload.

## App-specific decisions

- Files: keep item actions in three-dot menus; Details is a separate menu action.
  Do not duplicate Rename/Move/Copy/Delete in Details. Preserve permission-aware
  preview/download, versions, favorites, trash/restore and revision-safe editing.
- Contacts: use a contact list and independent right details/editor pane. Address
  books use the shared collection menu. Put name sorting and vCard import/export
  in Contacts settings.
- Calendar: default to Month. Place one date/Today control beside period arrows;
  it shows the selected date away from today and always returns to today.
  Day/week/agenda scroll internally. Month uses six bounded rows with a link to
  a day when events overflow. Put week start, cancelled-event visibility and ICS
  transfers in Calendar settings. Put the user's timezone in Personal → Regional
  settings; retain per-event/task timezone fields.
- Calendar shows inclusive all-day ending dates in forms and details, translating
  them to the existing exclusive end in stored/exported calendar data.
- Notes: preserve exact Markdown, safe preview, autosave/manual save, private
  recovery drafts and explicit conflict handling.
- Notes and Files share source-preserving Markdown formatting controls. Writing
  uses the normal text color; accent colors identify actions and selection.
- Block formatting applies at line boundaries. Closing an unsaved note offers
  save/retry, keep a private recovery draft and close, or explicit discard.
  Switching notes, views or notebooks uses that same decision and resumes the
  requested destination only after it succeeds. Keep editing cancels navigation;
  unavailable draft storage keeps the editor open. Dialog actions wrap to fit.
  Cancelling departure invalidates an in-flight save's close action. Transient
  refresh failures retain the editor and offer retry; confirmed access loss closes it.
  Selected-item recovery actions belong in the visible details pane on phones.
- Notes and Tasks use the same paged collection rows and owner action menus as
  Contacts and Calendar. Their collection import/export lives in app settings,
  with an explicit notebook or task-list target.
- Notes and Tasks put item commands in row action menus. Task details show
  readable status, priority, dates and repetition, with a specific Complete or
  Reopen command. Contact email, phone and supported website values are links.
- Tasks: preserve the shared list/details pattern, hierarchy, completion,
  recurrence-slot editing and date/timezone behavior.
- Prayer Times: layout B was selected November 8. Use a week list and
  focused right daily timetable; settings editing replaces that pane. Phones open
  a full-width day with an explicit return to the week and restored row focus.
  See [the app platform](spacetimedb/app-platform.md). Calculations use Dar's own
  dependency-free library with explicit method/high-latitude settings and labelled
  unavailable solar events. It ships as a separate trusted app package.
- Scheduling forms offer common repetition, priority and reminder choices.
  Exact imported values and custom rules remain available in advanced controls;
  summaries must not simplify schedules they do not understand.
- Sign-in: show the Dar logo by default and use the saved instance logo when
  configured. Fall back to Dar when the custom image cannot load.

## Accessibility, state and verification

- Preserve keyboard use, visible focus, accessible names and responsive layouts.
  Respect reduced motion. Reuse the shared dialog/menu/pane motion behavior.
- Ctrl/Cmd+K focuses the current app's search outside open editors/dialogs;
  Escape clears that search. Ctrl/Cmd+S saves the active Notes or Files editor.
- Phone details move focus into the opened panel and return it to the originating
  row on close, with a visible fallback if removed. Navigation exposes expanded
  state, focuses its controls, and restores its opener when dismissed with Escape.
- Include loading, empty, error, retry, read-only and unsaved/conflict states.
  Do not silently overwrite another revision or discard a draft.
- Verify actual persistence, permissions and resulting data, not just a click or
  screenshot. Browser tests use isolated roots, users and browser contexts.
- Check desktop and phone layouts, independent scroll regions, action targets,
  keyboard/focus behavior, overflow and console errors.
- New visual directions require distinct static mocks and maintainer selection.
  Fixing an app to follow an already selected shared pattern does not require a
  new direction. Do not invent a new design merely because adding a feature.

## DAV setup and error guidance

The maintainer reported an empty Files view at `/dav/`, and “no DAV at this URL”
at `/dav/files/anas`. The source lacked the Files `DAV` response header and the
generic discovery root did not expose Files. Advertise `DAV: 1` on Files
responses, including OPTIONS. The root lists a Files home only when that
credential has Files Read access. Show the explicit per-user Files address when
issuing a mixed-app password; do not make users guess the path.

DAVx⁵ Files mounts are separate from Contacts/Calendar synchronization. Document
the mount workflow and required grants in [DAV setup](davx5.md). Do not assume
Contacts/Calendar synchronization proves Files support, or claim Android-device
verification from server tests alone.

## Decision sources

- Root and [application agent guidance](../next/AGENTS.md).
- [Shared UX decisions and follow-ups](spacetimedb/ux-consistency-audit.md),
  selected [Material 3 mocks](spacetimedb/ux-consistency-mocks.html) and
  [reference interaction ledger](spacetimedb/nextcloud-reference/interactions.md).
- [Calendar refinement](spacetimedb/calendar-workspace.md).
- Selected [creation/appearance mocks](spacetimedb/creation-appearance-mocks.html)
  and [sign-in/app-password mock A](spacetimedb/signin-app-password-mocks.html).
- [Contacts/Calendar scope](spacetimedb/contacts-calendar.md) and
  [Notes/Tasks scope](spacetimedb/notes-tasks.md).
- Current maintainer instructions: consistent behavior across apps, Contacts
  three-dot address-book menus, vCard transfers in settings and independent
  middle/right scrolling. These refine older documents where they disagree.
