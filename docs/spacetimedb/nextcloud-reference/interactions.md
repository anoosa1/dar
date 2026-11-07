# Nextcloud interaction ledger

Reference: the maintainer's supplied instance, Nextcloud **35.0.2**, using
TestUser / display name Test. This is a personal account. Administration screens
are **unavailable**; the maintainer explicitly chose to record that limitation.
Desktop viewport: 1440×1000. Phone viewport: 390×844. Browser: Nix Chromium.
This record covers observed interactions, not every server configuration or app
extension. App versions exposed in their settings: Contacts 8.9.1, Calendar 6.6.2,
Notes 6.1.0. Files and Tasks standalone versions were not exposed in the screens
inspected. Do not infer a Tasks version from the server version.

Screenshots and accessible-control snapshots are indexed in [screenshots.md](screenshots.md).
The [gallery](index.html) provides a browsable visual reference. A screenshot of a
control is evidence of its UI, not proof that its underlying effect was tested.

## Shared shell and settings

| Starting state / trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Dashboard → app-grid icon | Anchored themed Apps menu, icons/text, current app visible in header; Dashboard, Files, Office, Photos, Calendar, Contacts, Teams, Activity, Notes, Music, Tasks, News, Cookbook; separate Actions group includes Text document | 02 | Observed navigation to all five Dar-equivalent apps |
| App header → account avatar | Display name/status, mobile-login QR action, Set status, Appearance and accessibility, Settings, About & What's new, Help & privacy, Log out | 04 | Menu observed; no security/status changes executed |
| Global search | Header combobox says Apps, files, messages, and more; desktop Ctrl-K hint; phone reduces it to an icon/button | 02, 27 phone set | UI observed; cross-app search providers not exhaustively exercised |
| Account menu → Settings | Opens global settings route with Personal navigation, shared header, search, app launcher and avatar still present | 23 | Observed |
| Settings → app launcher | Same Apps menu is available without returning to Files first | 24 | Observed |
| Personal → Profile & contact | Profile-picture controls, full name, pronouns, location, birth date, contact channels and per-field visibility controls; no app-specific Files preferences here | 23 | UI only; identity data unchanged |
| Personal → Security | Password, 2FA, passwordless devices, devices/sessions and app-password creation on one settings surface | 25 | UI only; no password, token, factor or session changed |
| Settings at phone width | Navigation collapses behind Open navigation; account/launcher remain in header; settings content uses the main viewport | 26 | Observed |
| Administration | No Administration group or administrator controls are exposed to TestUser | 23 | Unavailable, per maintainer's chosen scope; not simulated or bypassed |

Personal navigation observed: Profile & contact, Language & locale, Security,
Notifications, Mobile & desktop, Appearance and accessibility, Sharing,
Availability, Flow, Privacy. These are reference categories, not a commitment to
add unsupported federation/workflow/2FA features to Dar during a shell refactor.

## Files

| Starting state / trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Open Files before sample creation | Local Search here in sidebar; All files/Personal files/Recent/Favorites/Shares/Tags/External storage/Folder tree/Deleted files; storage and Files settings low in sidebar | 03 | Observed |
| New | Grouped menu: Upload files/folders; New folder, file request, text document, templates; folder description | 06 | Menu observed; ordinary upload and new folder exercised |
| New → New folder | Centered Create new folder dialog, prefilled name field, Create and close icon | 07 | Created only the controlled audit folder |
| Row checkbox / Name / Modified | Selection is separate from opening; sortable column headers are actual buttons; current direction indicated | 08, 34 | UI observed; complete sort/range-selection combinations remain open |
| Folder three-dot Actions | Add to favorites, Details, Manage tags, Rename, Move or copy, Open locally, Download, separator, Delete folder | 09 | Menu and Details/Rename/Move-or-copy/Delete exercised |
| File three-dot Actions | Adds View, Set reminder and Lock file to the file-appropriate action set; no implicit opening of Details | 35 | Menu observed; external local-open, locking and reminders not executed |
| Actions → Details | Opens a right sidebar; folder details show title/size/age/owner, close and another Actions menu, Sharing and Activity tabs | 10 | Observed; no real recipient added |
| Details → Sharing | Separate internal and external recipients, existing shares, internal link, new public link and explanation controls | 10 | UI only; no link or recipient created |
| Actions → Rename | Inline name form replaces the row's name; no modal. Enter commits | 31, 32 | Audit folder renamed successfully |
| Actions → Move or copy | Common Choose destination dialog with filter, All files/Recent/Favorites, breadcrumb, New and a table of destinations | 29 | Picker navigation observed; no copy/move committed |
| Choose Notes destination | Footer changes to Copy to Notes and Move to Notes; at original root only Copy was offered | 30 | UI observed |
| Delete audit folder → Deleted files | Trash eventually shows original name/location, deleted-by, deleted time, Restore and Actions; intermediate empty state was also seen during asynchronous navigation | 33, 36 | Controlled folder deletion and restore exercised |
| Restore controlled folder | Folder returns to All files, retaining the renamed name | 37 | Observed |
| Upload controlled text file | Device chooser upload creates a normal file row, with extension visually separated from the base name | 34 | Observed |
| Delete controlled text file | File disappeared from All files; the first trash observation was ambiguous while navigation/deletion were updating | 35, 36 | Deletion triggered; this file's restore result is not verified. Do not equate the folder restore proof with file restore proof |
| Files settings | Large shared settings dialog, left section navigation, scrollable controls, top-right close | 05 | Observed |
| Show hidden files | Switch changes immediately without a Save footer; changed value survived full reload; original false value restored afterward | 38 | Persistence exercised |
| Phone Files | Sidebar collapses; filters compress; row actions remain; New moves to a lower floating position | 27-files-phone | Observed UI |

Files settings sections and controls:

- General: favorites first, folders before files, folder-tree visibility, default
  All files versus Personal files view.
- Appearance: hidden files, file-type column, extensions, crop image previews.
- Additional settings: recommendations and folder descriptions.
- Warnings: changing an extension, deleting a file.
- WebDAV: copyable connection URL and documentation link.
- Keyboard shortcuts: actions, selection, navigation and view groups. Examples:
  A actions, D details, F2 rename, Delete deletion, Ctrl-A select all, Escape
  deselect, Alt-Up parent, V grid view, ? shortcut help. This is the displayed
  shortcut reference; not all shortcuts have been functionally exercised.

Dar implication: preserve existing domain operations and permission checks, put
them behind a proper action menu, keep Details explicit, and implement an actual
preference store. Do not offer unsupported file locking, external storage,
recommendations or network integrations as decorative controls.

## Contacts

Contacts uses book/group navigation and a separate local search beside a list
and a detail/editor area. All contacts includes the read-only system Accounts
book. The system contact explicitly identifies its read-only state. Stored
screenshots use the supplied **Leon Green example contact** instead of another
person's system-account details.

| Trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Open Contacts / choose Contacts book | New contact, All contacts, Not grouped, address books, groups, counts and add/action controls; editable example record details | 11 | Observed |
| Contact → Edit | Structured fields replace the detail area: name/title/company, Save, repeated labeled phone/email/address fields, per-property add/delete, birthday, website, notes, groups, Add more info | 28 | Editor opened without saving changes to the existing example |
| Contacts settings | Shared settings modal; General and Address books sections; display-name sorting, social-avatar update switch, import, per-book actions and New address book | 12 | UI observed; no network avatar fetching enabled |
| Phone width | List and contact detail become focused panels with Go back to the list; header remains shared | 27-contacts-phone | Observed |

Dar implication: one reusable field-row/type-choice pattern, shared settings
surface, accessible read-only indicators and consistent list/detail navigation.

## Calendar

| Trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Open Calendar | Local Filter events, compact month picker, previous/next, Event creation, Today/view action, calendar list, per-calendar edit/share, trash, bottom Calendar settings | 13 | Observed month view |
| Create new event | Simple event editor: calendar, title, start/end date and time, timezone, all-day, recurrence Edit, location, description, attendees, More details and Save | 15 | Unsaved editor observed; no invitations sent |
| More details | Larger Detailed event editor adds reminder, attachments, status, shared visibility, free/busy, categories/color, forwarding and attendee scheduling | 16 | UI only; no event saved |
| Escape in detailed editor | The editor remained visible in this observed empty-title flow; first header button closed it | 16 | Observed limitation, not a Dar behavior to reproduce |
| Calendar settings | General, Appearance, Editing, Delegation, Event legend and Keyboard shortcuts in the common settings dialog | 14 | UI observed |
| Phone / Tasks integration | Calendar can show an Unscheduled tasks sidebar containing the controlled VTODO created through Tasks | 27-calendar-phone | Cross-app task visibility observed; scheduling the task was not exercised |

Settings include automatic timezone, incoming-invitation calendar, availability
link, import/CalDAV, birthdays/tasks/weekends/week numbers/month density, default
timed/all-day reminders, simple editor, attachment folder, delegates, style legend
and keyboard shortcuts. Those unavailable or not implemented in Dar remain
separate product work, not checkbox-only UI additions.

## Notes

| Trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Open empty Notes | Sidebar All notes/categories/settings; welcome explains Markdown, offers New note and Create a sample note with Markdown | 17 | Observed |
| Create a sample note | Creates Sample note with headings/lists/quotes and opens the rich-text editor; local note search and a list appear | 19 | Controlled sample creation exercised |
| Editor toolbar | Undo/redo, headings, bold/italic/underline/strike/highlight, lists/blocks/table/annotations, links, attachments/emoji, remaining actions and document save | 19 | Controls observed; all formatting combinations not exercised |
| Hover note row → Actions | Reveals a compact Actions menu: Add to favorites, Share, Versions, Details, category, Rename, Delete note | 39 | Menu observed; effect branches not all exercised |
| Notes settings | Shared modal, General/Mobile apps/Shortcuts; rich text/plain text/preview modes, new-note extension, notes folder, hidden files and startup note selection | 18 | UI observed; note-source architecture not changed |
| Notes and Files | Created sample note is visible as Notes/Sample note.md in Files | 27-files-phone, 34 | Cross-app visibility observed |

Dar implication: current source/preview modes do not match a full rich-text
editor. Treat that as a real feature gap. Nextcloud's Files-backed notes folder
also differs from Dar's approved database-owned notes; do not quietly change
authority or add a nonfunctional folder picker to imitate the screen.

## Tasks

| Trigger | Observed behavior | Evidence | Coverage |
| --- | --- | --- | --- |
| Open empty Tasks | List navigation, Add List and Trash; inline Add a task to Tasks field instead of a separate creation dialog | 20 | Observed |
| Enter sample title | Creates a task and opens its right detail sidebar automatically | 22 | Controlled task creation exercised |
| Sidebar details | Completion checkbox/title/Actions, modified time, start/due, list, Details/Notes tabs, shared visibility, status, priority/progress, location/URL, tags and reminders | 22 | UI observed; field editors and subtree operations not all exercised |
| Sidebar → Actions | Pin, Edit summary, Export, Duplicate task, Delete | 40 | Menu observed |
| Details → status | Themed choice menu: Needs action, Completed, In process, Canceled | 42 | Menu observed |
| Choose Completed | Header checkbox becomes checked, progress becomes 100%, completion time appears, Completed smart collection appears | 43 | Controlled sample exercised |
| Click completed checkbox label | Task reopens as Needs action, progress returns to 0%, All/Current smart collections return | 44 | Controlled sample exercised; original active state restored |
| Notes tab | Task description is a separate detail-panel tab | 41 | UI observed |
| Add reminder | Quick tomorrow-at-9 action and custom date/time action | 45 | Menu observed; no reminder scheduled |
| Smart collections | All and Current appeared after the first task was created; absent when empty under Automatic visibility | 20, 22 | Observed |
| Tasks settings | Expands an inline sidebar panel, rather than the newer modal used by Files/Contacts/Calendar/Notes; Default list and each smart collection's Hidden/Visible/Automatic choice | 21 | Observed; this inconsistency should not be copied into Dar |
| Phone width | Collapsed sidebar and focused task list; launcher/account remain in shared header | 27-tasks-phone | Observed |

## Motion evidence

Measured Files settings opening via browser animation effects, separately from
static screenshot capture. See [files-settings-motion.json](files-settings-motion.json).
Observed modal/backdrop/header opacity transitions: **250 ms, ease**. Inner modal
scale: **300 ms, ease, scale(0.9) to identity**. The capture also contains unrelated
100/200 ms button/navigation transitions, which are not the dialog contract.
Exit, drawer and menu motion and reduced-motion behavior are not yet measured.
Use one reusable Dar overlay implementation and one reduced-motion policy rather
than independently approximating these timings in every app.

## Test content and outstanding coverage

Created only controlled test content: the Dar UX audit folder (renamed and
restored), a small Dar UX audit sample text upload, a built-in Sample note, and
Dar UX audit sample task. Existing contact data, passwords, factors, provisioning,
account roles and instance configuration were not modified. The Files preference
changed for the reload test was returned to its original value.

Open coverage includes independent admin screens (unavailable by instruction),
permission variants needing another test account, multi-item/clipboard/range
selection, upload errors/collisions/cancellation, public-share effects,
complete contact/group/import/export workflows, calendar view/recurrence/drag
and reminder branches, note category/favorite/rename/conflict/formatting flows,
task hierarchy/recurrence/deletion/editors, exit/reduced-motion timing and offline
states. These must not be labeled exhaustively tested merely because their
entry controls appear in screenshots.
