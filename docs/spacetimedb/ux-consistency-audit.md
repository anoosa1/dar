# Nextcloud reference audit and shared Dar experience

Status: the maintainer selected **B — Material 3**. The shared implementation
passed full isolated acceptance on November 7, 2026; the initial five-app
reference pass is documented.
The supplied Nextcloud instance was inspected as TestUser. The maintainer chose
to mark Administration unavailable to that personal account. Observations and
coverage gaps are in `nextcloud-reference/interactions.md`; screenshots are in
`nextcloud-reference/index.html`. Findings below describe the pre-change source;
the delivery record distinguishes implemented changes from remaining parity gaps.

## Scope and evidence

Audit Files, Contacts, Calendar, Notes, Tasks, the application launcher, account
menu, personal settings and administration. Record the Nextcloud server version,
each app version, enabled integrations, user role, viewport and audit date first.
Dar's OIDC/SCIM provider settings need a Dar-specific mapping where the reference
instance has no equivalent; do not invent a Nextcloud equivalent.

For each observable interaction, record the starting state, trigger, menu items
and order, resulting panel/dialog/navigation, dismissal, keyboard behavior, focus
restoration, persisted effect, errors, permission differences and screenshot IDs.
Capture before/after states at desktop and phone sizes. Record motion separately:
screenshots cannot establish timing, easing or exit animation.

Use controlled sample data for mutations. Do not delete real records, send
invitations/messages, alter live account security or change instance-wide settings
merely to inspect their UI. Mark unexercised effects and unavailable features
explicitly. Any unsafe-to-exercise branch can be documented from its visible UI
without claiming its side effect was tested. Use test fixtures for destructive
round trips after the reference environment's test-data scope is established.

“Every possible interaction” is a coverage goal, not a finite universal claim.
Inventory visible controls and their reachable states per installed version,
including menus, secondary actions, empty/error/loading and permission states;
keep a checklist of covered, unavailable, untested and blocked branches.

## Confirmed Dar findings

| Area | Current source evidence | Required change |
| --- | --- | --- |
| Shared header | `next/client/files.tsx`, `next/client/pim/common.tsx` and `next/client/identity.tsx` each render their own header | One workspace header used by every app and global settings |
| App launcher | `next/client/apps.tsx` renders a native select | Shared themed launcher with icons, current-app state and keyboard navigation; retain access in settings |
| Account menu | Files offers Account security, admin Create user and Sign out; PIM offers Account security and Sign out; settings renders only a user-name span | One account menu and Settings entry everywhere; role-based differences only |
| Global settings | Identity settings uses a separate header and mixed Account/Applications/Directory/SCIM/Signing keys tabs | Group navigation into Personal and Administration; preserve the shared shell |
| Applications settings | One Applications tab combines personal connected apps with administrator client registrations | Separate personal connected applications from administration registrations |
| Profile settings | AccountSecurity exposes password, sessions and DAV passwords, but no dedicated personal profile page | Map supported profile fields and identity rules explicitly; do not offer controls without persistence |
| Files overflow | Both list/grid three-dot buttons call `select(entry)` and are named Details for the file | Three-dot opens an action menu; Details is a distinct menu item |
| Files actions | Rename, Move to, Copy to, version upload, download, favorite and deletion are mostly buried in details | Reuse these domain operations from the menu, with permission-aware availability; add cut/paste clipboard behavior deliberately |
| Files preferences | View switches are component state; no dedicated Files settings UI is exposed | App settings with durable user-scoped preferences and validated defaults |
| Files footer | Sidebar contains “Your files. Your place.” | Remove the promotional footer text |
| Primary creation | Files/Contacts/Calendar use `new-button`; Notes/Tasks use `primary-button workspace-new` | One shared creation-button component and placement |
| App settings | Calendar exposes a Calendar settings dialog; Contacts saves sort; Notes/Tasks save sort through PIM preferences, but expose no matching app-settings entry | Common lower-sidebar entry and common settings presentation in every app |
| Dialogs | Apps already import `next/client/dialog.tsx`, but there is no single motion contract for all overlays | Extend the shared dialog; centralize menus, drawers and reduced-motion behavior too |
| Form controls | App, sort, scope, status, permissions and settings choices use native selects at multiple call sites | Shared themed accessible choice component; choose select/combobox/menu semantics according to the interaction |
| App page markup | Contacts, Calendar and workspace apps have separate sidebar/list/editor styles | Reuse shared navigation, toolbars, list actions, empty/error states and settings layout |

The architecture already has useful shared pieces: Dialog, Icon, AccountSecurity,
PimShell, the native session boundary and revision-checked PIM preferences. Extend
those boundaries instead of introducing a second UI framework or parallel shell.

## Reference interaction checklist — coverage tracked in the interaction ledger

| Surface | Interactions/states to inspect |
| --- | --- |
| Shell | Launcher open/close, app switching, search, account menu, personal/admin Settings entry, navigation persistence, narrow viewport, keyboard/outside-click/Escape |
| Files navigation | Root/folder/breadcrumb/back, favorites, recent/shared views, trash, loading/empty/search states, list/grid and sort |
| Files creation | New menu, folder creation, upload/file chooser/drop, duplicate name, progress, cancellation and failure |
| Files item | Single/double click, checkbox/multiselect, overflow menu order, right click if available, cut/copy/paste, move/copy destination picker, rename, download, favorite, delete, restore/purge |
| Files details | Explicit Details action, preview, metadata, activity, versions, sharing, tab transitions, close/focus restoration, read-only behavior |
| Files settings | Settings entry position, control labels/defaults, view/sort/hidden-file and other available preferences, save feedback and reload persistence |
| Contacts | Address-book/group navigation, create/edit/delete contact, field add/remove/type choices, search/sort/favorites, selection/bulk actions, avatar handling if available, import/export, sharing, settings |
| Calendar | Day/week/month/agenda, previous/next/Today, date picker, create/edit/drag/resize event, all-day/timezone, attendees, recurrence and scope, reminders, calendars/sharing/import/export, settings |
| Notes | Notebook/category navigation, new/rename/delete/restore if available, source/preview/editor behavior, save/conflict states, tags/favorites/search, move/copy/export/import if exposed, attachments/links, settings |
| Tasks | List navigation, task creation/details/edit, priority/start/due/status/progress/categories, hierarchy/reorder, completion/reopen, recurrence scope if available, reminders, move/copy, delete, list sharing/import/export, settings |
| Personal settings | Profile/account identity, security/sessions/app passwords, connected apps, appearance/accessibility and navigation back into apps; record missing Dar capabilities separately |
| Administration | Users/groups/directory, applications, access/provisioning and any identity-provider integration installed; distinguish app settings from instance administration |
| Cross-cutting | Button geometry, icons, spacing, focus, menu alignment, scroll ownership, animation/reduced motion, dialog/drawer dismissal, success/error/conflict states, disabled/read-only controls |

## Selected shared implementation direction

- `AppShell`/`WorkspaceHeader`: persistent brand, themed app launcher, centered
  search and identical AccountMenu, including on global settings pages.
- `AppNavigation`: common creation action, app navigation slots, and App settings
  at a consistent lower-sidebar location. Keep storage usage separate from settings.
- `ActionMenu`: shared anchor positioning, grouping, icons, danger styling,
  keyboard navigation, outside-click/Escape and focus return. File actions call
  existing typed operations; opening the menu never opens file details.
- `Dialog`, `Drawer`, `Select`/`Combobox` and `NewItemButton`: actual shared React
  components and CSS tokens, not app-specific copies with similar class names.
- One motion contract for opening dialogs/backdrops and reduced-motion overrides.
  Verify dismissal and focus restoration. Animated exits remain a separate gap.
- `AppSettings`: common container/entry and save/error feedback; each app owns
  its actual preferences. Persist to existing SpacetimeDB ownership boundaries,
  preserving independent app preferences during concurrent edits.
- `GlobalSettings`: Personal profile/security/connected applications, and
  administrator-only directory/registered applications/provisioning/signing keys.
  Separate navigation groups, not duplicate implementations of account settings.
- No fake settings switches, client-only persistence presented as durable, or
  Nextcloud controls whose underlying behavior Dar does not implement.

## Delivery sequence

1. Obtain the selected reference URL and establish access/installed versions (done).
2. Populate the per-app interaction ledger and screenshot index with observed
   evidence; list coverage gaps instead of claiming exhaustive verification.
3. Prepare several static variants of the shared shell, Files action menu, app
   settings and Personal/Administration settings (done). The maintainer selected
   B, Material 3, as required by AGENTS.md.
4. Implement shared components first, migrate all five apps and global settings,
   then wire the requested file actions and durable app preferences.
5. Verify uniform menus/settings entries, modal motion/focus, permission handling,
   keyboard/phone behavior, preference reload persistence and existing protocols.

Static options A/B/C are in `ux-consistency-mocks.html`: Nextcloud-first (recommended),
Material 3, and Focused workspace. Each shows Files actions, global settings, and
the same app-settings container. All retain the same planned functionality.

## Additional inconsistencies found during reference inspection

1. Dar puts app-specific filtering in the centered header field; Nextcloud keeps
   a global cross-app search there and supplies local search within app navigation
   or content. Define the scope visibly instead of silently changing its meaning.
2. Dar Files currently displays a fixed Name ascending indicator while Contacts,
   Notes and Tasks expose different sort mechanisms. Sort direction, persistence
   and accessible pressed/current state need a common contract.
3. Files main content, PIM lists and identity settings have different navigation
   and loading layouts. Settings currently replaces the shell altogether.
4. Dar Notes has source/preview toggles while reference Notes has a full rich-text
   formatting toolbar and plain-text/preview modes. This is a feature gap, not
   something that can be resolved by changing a dropdown's CSS.
5. Dar's Notes store is independent of Files; reference notes and their versions,
   sharing/details actions integrate with Files. Preserve the approved authority
   boundary unless the maintainer explicitly chooses a new storage contract.
6. Reference Contacts provides per-property type controls, add/remove rows and
   Add more info in the detail editor. Dar's editors should reuse field-row and
   themed choice primitives instead of app-specific native choices.
7. Reference Tasks supports inline task creation and a right detail editor;
   Dar opens a large form dialog. Choose intentionally and consistently where
   quick creation ends and detailed editing begins.
8. Date/time controls and timezone presentation differ among Dar Calendar/Tasks
   and other date fields. Share labeled date/time, timezone and validation controls.
9. Reference Files keeps actions outside its details sidebar. Dar currently
   forces users through details to reach ordinary file operations.
10. Dar creation buttons use different classes, text treatments and icon patterns;
    even Nextcloud varies their placement. Dar should use one creation component
    and predictable placement while retaining app-specific action text.
11. Reference settings include shortcut documentation. Dar needs a coherent
    shortcut registry/help surface, with typing contexts excluded and one focus
    restoration rule after menus, pickers and overlays.
12. Native select styling alone will not supply a themed launcher or accessible
    command menu. Use a menu for commands, a select/combobox for values, and a
    date picker for dates; do not replace all three with the same HTML pattern.
13. The reference's older Tasks settings panel is itself inconsistent with the
    newer shared app-settings dialogs. Standardize Dar on one presentation rather
    than copying that inconsistency.
14. Reference Calendar failed to dismiss its detailed editor with Escape in the
    observed flow. Dar must retain its accessible dismissal/focus guarantees.
15. Dar's branded footer and per-app promotional eyebrow/empty-state copy compete
    with file/task/note operations. Remove the requested footer and use a common,
    concise empty/loading/error-state component and copy style.

Controlled reference test content was used for mutations; existing account
security and real records were not changed. This audit does not claim all listed
feature gaps are implemented.

## Material 3 delivery

- `WorkspaceHeader`, `AppSwitcher` and `ActionMenu` supply the same themed launcher
  and account menu in Files, Contacts, Calendar, Notes, Tasks and global settings.
  The account menu always exposes Settings, Account security and Sign out;
  Administration is added only for administrators. Search remains scoped to the
  current app/settings list; cross-app search is not implemented.
- `NewItemButton`, `Select`, `Dialog` and shared CSS supply common creation
  controls, themed value choices, menu keyboard navigation, dialog entry motion
  and reduced-motion handling. Dialogs use a 300 ms scale/fade and 250 ms backdrop
  fade, based on the measured reference. Dismissal restores usable focus.
- All five apps expose a lower-sidebar settings entry and the same General /
  Keyboard shortcuts dialog. Preferences are stored per user in SpacetimeDB,
  validated and revision checked; writes preserve other app preferences.
- Files settings cover list/grid, hidden files, confirmation before trash and the
  initial details tab. Contacts saves name sorting; Calendar saves timezone and
  week start; Notes saves sort/editor mode; Tasks saves sort/default view.
- Files overflow menus expose Details separately from file operations. Copy,
  Cut, Paste, Move to, Copy to, Rename, favorite, download/preview, trash,
  restore and permanent deletion reuse the existing authorization/revision
  boundaries. The account-scoped single-item clipboard is retained in this
  browser tab. Folder operations retain existing bounded recursive behavior.
  Confirmation before trash applies to the menu, toolbar and details actions.
  The promotional sidebar footer is removed.
- Global settings groups Profile, Security and Connected applications under
  Personal. Administrators additionally get Applications, Directory, SCIM and
  Signing keys. Profile changes persist names and primary email with revision
  checks; username, roles and security cannot be changed through the personal
  endpoint. Supplied email addresses remain unverified.

The personal profile API is `GET/PUT /api/v1/account/profile`. Reads return the
current user's directory profile with ETag `"<userId>:<revision>"`. PUT accepts
only `displayName`, `givenName`, `familyName` and `email`, requires that ETag in
`If-Match`, and uses the existing cookie/CSRF/origin boundary. Missing conditions
return 428; stale conditions return 412 (or a transaction conflict if raced).
Identity/privilege fields are rejected. Preferences remain at
`GET/PUT /api/v1/pim/preferences`, sharing the existing revision boundary.

Remaining reference gaps include cross-app search, Notes rich-text formatting
and Files integration, broader Files sorting, inline Tasks creation and animated
overlay exits. The reference ledger also retains unexercised branches and
unavailable Administration screens. This delivery implements the selected shared
experience and requested fixes, not complete Nextcloud feature parity.

## Verification

`nix develop ./next -c npm --prefix next run verify` passed both TypeScript
projects, 57 Node tests and five real Chromium journeys: identity, Notes/Tasks,
shared UI, Files, and Contacts/Calendar. The shared journey covers all five app
settings, persistent preferences, profile permissions, copy/cut/paste bytes,
trash-confirmation entry points, phone/tablet drawers, themed sidebar choices,
focus restoration, dialog entry motion, reduced motion and console errors.

The same run passed independent vdirsyncer round trips, module/backup upgrades,
backup/restore and forced process/database restart recovery for Files, PIM,
Notes/Tasks and identity. Calendar drag/resize uses native mouse gestures and
asserts the saved event times; it initiates dragging before destination scrolling.
Generated test artifacts remain outside the repository.
