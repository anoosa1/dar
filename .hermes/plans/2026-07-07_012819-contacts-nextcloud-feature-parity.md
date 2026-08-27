# Contacts Nextcloud Feature Parity Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Research Nextcloud Contacts features, compare them to Dar’s current Contacts app, and create a selection-ready backlog so Anas can choose what to implement or skip.

**Architecture:** Dar currently has a minimal Contacts daemon that stores one `.vcf` file per contact and a server-rendered Contacts UI with inline vanilla JS. The planned direction is to keep Dar’s zero-build, server-rendered UI constraints, expand the daemon API around vCard/CardDAV-compatible concepts, and make the Contacts main tab a full-screen app workspace instead of a narrow centered column.

**Tech Stack:** Go 1.26+, `net/http`, `html/template`, vanilla JS, Tailwind CDN classes, filesystem-backed `.vcf` storage initially, no new dependencies unless explicitly approved.

---

## Current repo context

Dar Contacts files inspected:

- `internal/apps/contacts/handlers.go`
  - Current UI wrapper is `class="max-w-3xl mx-auto"` at line 40, which squeezes the main tab in the middle.
  - Current sidebar has `All contacts` and `Groups`, but `Groups` is only a link (`/contacts?view=groups`) and has no implemented view.
  - Current UI supports creating, editing, deleting, listing, opening a modal detail view, and client-side searching contacts.
  - Current edit form fields: first name, last name, email, phone, organization.
- `cmd/contacts/main.go`
  - Current daemon endpoints: `/list`, `/get`, `/create`, `/update`, `/delete`, `/search`, `/health`.
  - Current `Contact` fields: `ID`, `FirstName`, `LastName`, `Email`, `Phone`, `Org`, `Notes`, `Color`, `UpdatedAt`.
  - Current vCard parser/writer handles only simple `FN`, `N`, `EMAIL`, `TEL`, `ORG`, `NOTE`, `REV` lines.
  - No contact tests exist yet.
- `go.mod`
  - Only Go dependencies currently present are `chi`, `oauth2`, and `zombiezen.com/go/sqlite` plus transitive dependencies.
  - AGENTS.md says no new dependencies without approval; vCard/CardDAV work should use stdlib unless Anas approves using `github.com/emersion/go-vcard` already listed in the project guide but not currently in `go.mod`.

## Research sources used

- Nextcloud Contacts app store / README: Contacts is a UI for Nextcloud’s CardDAV server, syncs contacts from devices, edits contacts online, supports vCard 3.0 and 4.0, integrates with Mail and Calendar, syncs birthdays/recurring events with Calendar, and supports address book sharing.
- Nextcloud admin manual: Nextcloud ships a CardDAV backend for address books and contacts; includes a read-only system address book, profile privacy scopes, address-book sync/change tokens, shared items, creation rate limits, address book count limits, example/default contacts, and data retention settings for sync tokens.
- Search result snippets for the current user manual indicate the Contacts app covers basic address book maintenance and is similar to mobile contacts apps but with more functionality.

## Feature inventory for Anas to choose from

Use this list as the decision checklist. Suggested labels:

- Must: implement for feature parity.
- Later: good, but not blocking.
- Skip: not needed for Dar.

### A. Layout and navigation

1. Full-screen main Contacts workspace.
   - Replace the narrow centered `max-w-3xl mx-auto` layout with a full-width/full-height app grid.
   - Required by Anas: the main tab in the UI should fill the whole screen and not be squeezed in the middle.
2. Three-pane Contacts layout.
   - Left: address books/groups/sidebar.
   - Middle: searchable/sortable contact list.
   - Right: selected contact details/editor.
   - Avoid modal-only details for desktop; keep responsive mobile fallbacks.
3. Persistent selected contact state.
   - URL/query state like `/contacts?id=...` or local UI state so reloads preserve context.
4. Empty states and onboarding.
   - Clear empty contact list state with “Create contact” and “Import contacts” actions.
5. Responsive mobile/tablet behavior.
   - Collapse panes into list/detail navigation on narrow screens.

### B. Address books

1. Multiple address books per user.
   - Create, rename, delete address books.
   - Each contact belongs to an address book.
2. Address book color/icon metadata.
   - Useful for visual grouping in the sidebar.
3. Default address book.
   - First login creates a default address book.
4. Read-only/system address book.
   - Nextcloud has a system address book exposing instance users.
   - Dar equivalent could expose Dar users as read-only contacts.
5. Address book count limits/rate limits.
   - Nextcloud limits address book creation by rate and max count.
   - Likely not needed initially unless abuse prevention matters.
6. Address book export.
   - Export one address book as `.vcf`.
7. Address book import.
   - Import `.vcf` / multi-vCard files into an address book.
8. Address book sharing.
   - Share address books with users/groups with read/edit permissions.
   - Nextcloud supports shared address books; this is a large feature because Dar needs permission/UI semantics.

### C. Contact list features

1. Server-side search.
   - Current UI does client-side filtering after loading all contacts.
   - Add `/search?q=&address_book=&group=` backed by parsed fields.
2. Search across all relevant fields.
   - Names, emails, phones, organizations, titles, nicknames, notes, addresses, URLs, IM/handles.
3. Alphabetical section headers.
   - Current UI already groups by first letter.
   - Extend to use display name sort and locale-ish fallback.
4. Sort modes.
   - First name, last name, organization, recently updated.
5. Filters.
   - By address book, group/category, favorites, missing data, birthdays.
6. Bulk selection.
   - Multi-select contacts for delete/export/move/group assignment.
7. Duplicate detection.
   - Find possible duplicates by normalized email/phone/name.
8. Merge contacts.
   - Combine duplicate vCards while preserving all fields.
9. Favorites / starred contacts.
   - Pin important contacts.
10. Recently contacted / interactions.
   - Nextcloud has Contacts Interaction as a read-only CardDAV address book.
   - Dar could later integrate with Email/Calendar activity.

### D. Contact detail fields and editing

1. Full vCard 3.0/4.0 field coverage, at least for common fields.
   - Current parser is minimal and does not handle params, escaping, folding, multiple fields, or vCard 4.0.
2. Multiple emails per contact.
   - Type labels: home, work, other.
3. Multiple phone numbers per contact.
   - Type labels: mobile, home, work, fax, pager, other.
4. Structured postal addresses.
   - Street, city, region/state, postal code, country; multiple typed addresses.
5. Organization fields.
   - Organization, department, job title, role.
6. Birthday and anniversary.
   - BDAY and ANNIVERSARY support.
   - Later integration with Calendar so birthdays appear as recurring events.
7. Contact photo/avatar.
   - Upload, display, remove photo.
   - vCard PHOTO support or Dar-managed media reference.
8. Nickname / display name.
   - NICKNAME, FN, N handling.
9. Pronouns / gender fields.
   - vCard supports GENDER in v4.0; pronouns may need notes/custom field decision.
10. Websites / URLs.
   - Multiple URL fields.
11. Social profiles / IM handles.
   - X-SOCIALPROFILE / IMPP depending compatibility target.
12. Notes.
   - Current backend supports notes but UI edit form does not expose it.
13. Custom labels and additional fields.
   - Nextcloud README discourages custom nonstandard fields; suggests Notes for custom data.
   - Dar should decide whether to stay strict vCard-compatible.
14. Field add/remove UI.
   - “Add email”, “Add phone”, “Add address”, etc.
15. Inline detail editing.
   - Edit fields in the right pane rather than opening a modal.
16. Validation.
   - Basic email/URL/date validation, not over-strict phone validation.
17. Safe escaping/unescaping.
   - Correctly handle commas, semicolons, backslashes, newlines, and folded vCard lines.
18. Revision timestamps.
   - Current writer emits `REV`; preserve and update reliably.
19. UID preservation.
   - Store/preserve vCard `UID` instead of using only filename/timestamp IDs.

### E. Groups / categories

1. Contact groups/categories.
   - Nextcloud Contacts has group-like organization in the UI; vCard often uses `CATEGORIES`.
2. Create, rename, delete groups.
3. Assign/remove contacts to groups.
4. Group filter sidebar.
5. Group bulk actions.
6. Group counts.
7. Potential compatibility decision.
   - Use vCard `CATEGORIES` for maximum sync compatibility, or maintain Dar-specific group metadata.

### F. Import / export / sync

1. Import `.vcf` files.
   - Single contact and multi-contact vCard files.
2. Export individual contact as `.vcf`.
3. Export selected contacts as `.vcf`.
4. Export full address book as `.vcf`.
5. Conflict handling during import.
   - Add all, skip duplicates, update existing, or review one by one.
6. CardDAV endpoint.
   - Nextcloud’s core Contacts value is CardDAV sync with phones/desktops.
   - Dar currently exposes REST only; CardDAV parity is a major milestone.
7. Sync tokens / change log.
   - Required for efficient CardDAV clients and differential sync.
8. ETags / If-Match conflict protection.
   - Prevent lost updates when multiple clients edit the same contact.
9. Data retention for sync tokens.
   - Nextcloud has admin-configurable sync token retention.
10. vCard version handling.
   - Read vCard 3.0 and 4.0; decide write version.

### G. Sharing and permissions

1. Share address book with another Dar user.
2. Share address book with a Dar group.
3. Permission levels.
   - Read-only vs can-edit.
4. Shared address books visible in sidebar.
5. Respect permissions in daemon endpoints.
6. System address book privacy.
   - Nextcloud exposes users based on profile scopes and sharing/autocomplete rules.
   - Dar needs an equivalent if adding a system address book.

### H. App integrations

1. Mail integration.
   - Compose email from contact detail.
   - Autocomplete recipients from contacts.
   - Add sender/recipient to contacts from Mail.
2. Calendar integration.
   - Show birthdays/anniversaries as calendar events.
   - Invite contacts to events.
3. Files integration.
   - Attach files to an email/calendar event from a contact workflow if desired.
4. Soosa tool integration.
   - Contacts daemon should register tools: search contacts, get contact, create/update contact, find duplicates, export vCard.
5. Global app search.
   - Contacts appear in Dar-wide search if that exists/is planned.

### I. Admin / settings / maintenance

1. Example/default contact on first login.
   - Nextcloud can create an example contact.
   - Dar probably should skip unless onboarding matters.
2. Configurable max address books.
3. Configurable address book creation rate limit.
4. Import default example contact from `.vcf`.
5. Backup-friendly storage.
   - Current `.vcf` files are naturally backupable.
6. Repair/reindex task.
   - Re-parse all `.vcf` files and rebuild any indexes/cache.
7. Error reporting for invalid vCards.
   - Show import parse errors and skip bad cards safely.

### J. Accessibility and polish

1. Keyboard navigation.
   - Arrow through list, Enter open, Esc close/cancel, Ctrl/Cmd+F focus search.
2. Accessible labels and focus states.
3. Loading states and error toasts.
4. Undo delete.
   - Soft-delete or short-lived undo buffer.
5. Consistent Material 3 styling with existing Dar apps.
6. No inline `style=""` attributes if avoidable.
   - Current avatar background uses inline style in JS; AGENTS.md says no inline style attributes.
   - Future UI should use safe generated classes/data attributes or controlled style strategy after deciding how strict to be.

## Discrete implementation steps after Anas selects features

This section is intentionally broken into small implementation steps so we can start, test, and stop cleanly at any point. Each step should be handled as a separate task with a focused diff.

### Step 0: Finalize selected scope

**Objective:** Convert the researched feature inventory into a concrete Must / Later / Skip checklist before code changes.

**Files:**
- Modify: `.hermes/plans/2026-07-07_012819-contacts-nextcloud-feature-parity.md`

**Actions:**
1. Anas marks each feature in the inventory as Must, Later, or Skip.
2. Move Must features into the ordered steps below.
3. Move Later features into a future backlog section.
4. Remove Skip features from the active implementation path.

**Verification:**
- The plan contains an explicit selected-feature checklist.
- No implementation starts until the Must list is clear.

### Step 1: Add Contacts daemon test harness

**Objective:** Create tests around the current daemon behavior before changing storage, parsing, or API shape.

**Files:**
- Create: `cmd/contacts/main_test.go`
- Modify: `cmd/contacts/main.go` only if minimal test seams are needed

**Test cases:**
1. `handleCreate` writes a `.vcf` for the request user.
2. `handleList` returns created contacts for the same user.
3. `handleList` does not leak contacts across users.
4. `handleUpdate` updates an existing contact.
5. `handleDelete` removes a contact.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'
```

**Expected:** PASS before feature work continues.

### Step 2: Add Contacts UI render smoke test

**Objective:** Make layout changes safer by asserting the Contacts page renders expected app containers and controls.

**Files:**
- Create: `internal/apps/contacts/handlers_test.go`
- Modify: `internal/apps/contacts/handlers.go` only if render helpers need to stay testable

**Test cases:**
1. `renderPage` includes `id="contacts-app"`.
2. Rendered HTML includes search and new-contact controls.
3. Rendered HTML does not include the narrow `max-w-3xl mx-auto` wrapper after Step 3.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts -v -count=1'
```

**Expected:** Initial test may fail on the `max-w-3xl` assertion until Step 3, then passes.

### Step 3: Make the Contacts main tab full-screen

**Objective:** Satisfy the explicit UI requirement: the Contacts main tab fills the screen and is not squeezed in the middle.

**Files:**
- Modify: `internal/apps/contacts/handlers.go:38-198`
- Test: `internal/apps/contacts/handlers_test.go`

**Implementation:**
1. Inspect the Files app layout in `internal/apps/files/handlers.go` and copy its full-width workspace conventions where appropriate.
2. Replace `class="max-w-3xl mx-auto"` with a full-width/min-width-safe Contacts workspace.
3. Keep the existing create/list/search behavior working.
4. Do not add new Contacts features in this step.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
nix-shell -p go --run 'go build ./cmd/...'
```

**Manual verification:**
1. Run `./launch.sh`.
2. Open `/contacts`.
3. Confirm the Contacts app uses the full main panel width.

### Step 4: Convert desktop UI to three-pane structure

**Objective:** Replace modal-first desktop workflow with a Nextcloud-like app workspace foundation.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`
- Test: `internal/apps/contacts/handlers_test.go`

**Implementation:**
1. Add a left pane for address books/groups placeholders.
2. Add a middle pane for the contact list and search.
3. Add a right pane for selected contact details.
4. Keep create/edit modal temporarily if needed to avoid mixing too much behavior in one step.
5. Ensure the right pane has an empty state when no contact is selected.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts -v -count=1'
```

**Manual verification:**
- Contact list remains clickable.
- Contact details render in the right pane or the previous modal still works until Step 5.

### Step 5: Move contact detail display into the right pane

**Objective:** Make selected contact details persistent in the page instead of relying on a detail modal.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Replace `showDetail` modal behavior with a function that populates the right pane.
2. Keep edit/delete actions visible in the right pane.
3. Preserve current fields: name, email, phone, organization.
4. Keep current create/edit modal for editing until Step 6.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

**Manual verification:**
- Clicking a contact updates the right pane without opening the detail modal.

### Step 6: Add notes to the edit form and detail view

**Objective:** Expose the existing backend `Notes` field in the UI before adding new vCard fields.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`
- Test: `cmd/contacts/main_test.go` if notes round-trip coverage is missing

**Implementation:**
1. Add a Notes textarea to the create/edit form.
2. Include `notes` in `saveContact`.
3. Populate notes in `editContact`.
4. Display notes in the right-pane detail view.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

**Manual verification:**
- Create a contact with notes.
- Reload Contacts.
- Notes are still shown.

### Step 7: Introduce a vCard line parser/writer test suite

**Objective:** Lock down vCard correctness before adding more fields.

**Files:**
- Modify: `cmd/contacts/main_test.go`
- Modify: `cmd/contacts/main.go`

**Test cases:**
1. Escaped commas, semicolons, backslashes, and newlines round-trip.
2. Folded vCard lines are unfolded before parsing.
3. Multiple `EMAIL` lines can be parsed once Step 8 changes the model.
4. Multiple `TEL` lines can be parsed once Step 8 changes the model.
5. `UID` is preserved when present.
6. `REV` is updated on writes.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'
```

**Expected:** Some tests fail before Step 8; commit only once implementation makes the selected tests pass.

### Step 8: Expand the Contact model for common Nextcloud fields

**Objective:** Support the selected common vCard fields without implementing CardDAV yet.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go` only for fields exposed immediately

**Candidate fields:**
1. UID.
2. Display name / nickname.
3. Multiple emails with type labels.
4. Multiple phones with type labels.
5. Organization, department, title/role.
6. Birthday and anniversary.
7. Postal addresses.
8. Websites/URLs.
9. Categories/groups.
10. Notes.

**Implementation rule:**
- Only implement fields Anas marks Must.
- Use vCard-compatible names and encoding.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'
```

### Step 9: Add dynamic field editing UI

**Objective:** Let users add/remove repeated contact fields like multiple emails, phones, addresses, and URLs.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Add “Add email” / “Add phone” controls for selected repeated fields.
2. Serialize repeated fields into the JSON shape implemented in Step 8.
3. Populate repeated fields when editing existing contacts.
4. Keep single-value backward compatibility for existing contacts.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

**Manual verification:**
- Create a contact with two emails and two phone numbers.
- Reload and edit it.
- Both values remain intact.

### Step 10: Add server-side search across selected fields

**Objective:** Move search to the daemon so it scales beyond loaded contacts and covers all selected fields.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Expand `/search?q=` to search selected fields.
2. Preserve `/list` for default list loads.
3. Debounce UI search input lightly in vanilla JS if needed.
4. Show empty search state separately from empty address book state.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

### Step 11: Add sort and filter controls

**Objective:** Add basic list controls before larger address book/group work.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Add query parameters for sort mode if server-side sorting is selected.
2. Support first name, last name, organization, and recently updated if selected.
3. Add UI dropdown/buttons for selected sort modes.
4. Keep alphabetical headers correct under the selected sort mode.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

### Step 12: Add address book model

**Objective:** Support multiple address books per user while keeping existing contacts readable.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go` only for minimal address book selector if selected

**Implementation:**
1. Add default address book creation/discovery.
2. Decide storage after user approval: filesystem metadata JSON vs SQLite metadata.
3. Add endpoints for list/create/rename/delete address books.
4. Migrate or transparently read existing contacts from `data/<uid>/contacts/*.vcf`.
5. Ensure deleting an address book is blocked or clearly destructive.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'
```

### Step 13: Add address book UI

**Objective:** Make address books visible and manageable in the left pane.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Load address books on page load.
2. Display address books in the left pane with counts if available.
3. Add create/rename/delete controls.
4. Filter contact list by selected address book.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

### Step 14: Add groups/categories backend

**Objective:** Support Nextcloud-like contact groups using vCard-compatible categories unless Anas chooses Dar-specific metadata.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`

**Implementation:**
1. Parse and write vCard `CATEGORIES`.
2. Add endpoint or query support for listing all categories/groups.
3. Add endpoint support for assigning/removing categories from contacts.
4. Ensure category changes update `REV`.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'
```

### Step 15: Add groups/categories UI

**Objective:** Make groups usable from the Contacts left pane and contact editor.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Display groups below address books in the left pane.
2. Filter contacts by selected group.
3. Add group assignment controls in the editor.
4. Show group chips in the detail pane.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

### Step 16: Add import `.vcf`

**Objective:** Import single-contact and multi-contact vCard files.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Add a daemon endpoint for import.
2. Parse multiple `BEGIN:VCARD` / `END:VCARD` blocks.
3. Return JSON with imported count, skipped count, and parse errors.
4. Add UI file picker/import action.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

**Manual verification:**
- Import a `.vcf` exported from another app.

### Step 17: Add export `.vcf`

**Objective:** Export contacts in vCard format.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Export a single contact.
2. Export selected contacts if bulk selection is selected.
3. Export a full address book if address books are selected.
4. Set `Content-Type: text/vcard; charset=utf-8` and a safe download filename.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

### Step 18: Add duplicate detection

**Objective:** Find likely duplicate contacts without merging them automatically.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Normalize emails and phone numbers.
2. Group possible duplicates by shared normalized email/phone and similar names.
3. Add a UI view listing duplicate candidates.
4. Do not merge in this step.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

### Step 19: Add merge contacts

**Objective:** Merge selected duplicate contacts while preserving all non-conflicting fields.

**Files:**
- Modify: `cmd/contacts/main.go`
- Modify: `cmd/contacts/main_test.go`
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Add merge preview response showing field conflicts.
2. Add merge endpoint that writes one contact and removes/archive duplicates.
3. Preserve UID strategy deliberately: keep primary UID unless CardDAV requires another choice.
4. Add UI confirmation.

**Run:**
```bash
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
```

### Step 20: Add bulk selection actions

**Objective:** Support multi-select workflows for selected contacts.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`
- Modify: `cmd/contacts/main.go` if bulk endpoints are needed
- Modify: `cmd/contacts/main_test.go` if backend changes are needed

**Implementation:**
1. Add list checkboxes/selection mode.
2. Add bulk delete if selected.
3. Add bulk export if selected.
4. Add bulk group assignment if groups are selected.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/apps/contacts ./cmd/contacts -v -count=1'
```

### Step 21: Add Mail integration

**Objective:** Integrate Contacts with Dar Mail if selected.

**Files:**
- Inspect first: `internal/apps/email/`, `cmd/email/`
- Modify only the traced Mail compose route/files
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Trace existing Mail compose URL/API.
2. Add “Email” action in contact detail for contacts with email addresses.
3. Add Mail recipient autocomplete from Contacts only if selected.
4. Add “Add to contacts” from Mail only if selected.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
```

### Step 22: Add Calendar birthday/anniversary integration

**Objective:** Surface contact birthdays/anniversaries in Calendar if selected.

**Files:**
- Inspect first: `internal/apps/calendar/`, `cmd/calendar/`
- Modify only traced Calendar files and Contacts birthday fields

**Implementation:**
1. Trace how Calendar stores/lists recurring events.
2. Decide whether birthdays are virtual events or materialized calendar entries.
3. Add selected implementation.
4. Ensure updating a contact updates the calendar view.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
```

### Step 23: Add Soosa Contacts tools

**Objective:** Let Soosa search/read/write Contacts if selected.

**Files:**
- Inspect first: `internal/soosa/`, daemon tool registration patterns
- Modify traced Soosa/tool files
- Modify: `cmd/contacts/main.go` only if API support is missing

**Implementation:**
1. Add tools for search contacts and get contact.
2. Add create/update tools only if write tools are selected.
3. Add export/find-duplicates tools only if selected.
4. Include permission checks consistent with existing app tools.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
```

### Step 24: Add address book sharing

**Objective:** Share address books with users/groups if selected.

**Files:**
- Inspect first: auth/user/group models and any existing sharing implementation
- Modify: `cmd/contacts/main.go`
- Modify: `internal/apps/contacts/handlers.go`
- Add tests around permission enforcement

**Implementation:**
1. Trace existing Dar users/groups and sharing conventions.
2. Choose sharing storage only after architectural approval.
3. Add read-only and can-edit permissions.
4. Enforce permissions in all daemon endpoints.
5. Show shared address books in the sidebar.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
```

### Step 25: Add system address book

**Objective:** Add a read-only address book of Dar users if selected.

**Files:**
- Inspect first: `internal/auth/` and user profile fields
- Modify: `cmd/contacts/main.go` or server-side proxy/provider after design approval
- Modify: `internal/apps/contacts/handlers.go`

**Implementation:**
1. Determine which Dar user fields are safe to expose.
2. Add read-only list/get behavior.
3. Make create/update/delete return 403 for system contacts.
4. Add UI read-only indicators.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
```

### Step 26: CardDAV investigation spike

**Objective:** Determine the smallest correct CardDAV implementation path before changing architecture.

**Files:**
- No implementation files unless Anas explicitly approves a spike branch/draft.
- Read/inspect only: existing WebDAV code under `internal/webdav/`, server routes in `cmd/server/main.go`, Contacts daemon.

**Investigation checklist:**
1. What WebDAV primitives already exist in Dar?
2. Can Contacts reuse them without a new abstraction?
3. Which routes should expose CardDAV?
4. How should auth work for external clients?
5. Where should ETags and sync tokens be stored?
6. Are we allowed to add `github.com/emersion/go-vcard`?

**Deliverable:**
- A separate CardDAV implementation plan before coding CardDAV.

### Step 27: CardDAV implementation milestone

**Objective:** Implement real CardDAV sync only after Step 26 is approved.

**Files likely to change:**
- `cmd/contacts/main.go` or a new approved internal CardDAV package
- `cmd/server/main.go` for public CardDAV routes/proxying
- Existing WebDAV/CardDAV-related files after tracing
- Tests under `cmd/contacts/` and/or `internal/carddav/`

**Implementation:**
1. Add address book discovery.
2. Add contact list/get/create/update/delete over CardDAV.
3. Add ETags and If-Match behavior.
4. Add sync token/change log support.
5. Add client compatibility verification notes.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
nix-shell -p go --run 'go build ./cmd/...'
```

**Manual verification:**
- Connect a CardDAV client and verify create/update/delete sync.

### Step 28: Final polish pass

**Objective:** Make selected feature set feel finished.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`
- Modify: `cmd/contacts/main.go` only for final bug fixes

**Implementation:**
1. Add loading states.
2. Add error toasts/messages.
3. Add keyboard shortcuts selected by Anas.
4. Add accessible labels/focus behavior.
5. Remove dead modal code if the right-pane editor replaces it.
6. Remove inline styles where practical and align with Dar UI conventions.

**Run:**
```bash
nix-shell -p go --run 'go test ./internal/... ./cmd/... -v -count=1'
nix-shell -p go --run 'go build ./cmd/...'
```

**Manual verification:**
- Full `/contacts` smoke test: list, create, edit, search, delete, and each selected Nextcloud-parity workflow.

## Files likely to change once implementation starts

- `internal/apps/contacts/handlers.go`
- `cmd/contacts/main.go`
- `cmd/contacts/main_test.go`
- `internal/apps/contacts/handlers_test.go` if UI rendering tests are added
- Possibly `cmd/server/main.go` for CardDAV routes/proxying
- Possibly `internal/soosa/` files for tool registration, after tracing current patterns
- Possibly Mail/Calendar app files for integrations

## Tests / validation plan

1. Unit tests for vCard parse/write round-trips:
   - `nix-shell -p go --run 'go test ./cmd/contacts -v -count=1'`
2. Handler/API tests for list/create/update/delete/search/import/export:
   - Use `httptest.NewRecorder` and temporary directories under `/tmp/dar_test_*`.
3. UI render smoke tests if useful:
   - Assert Contacts HTML contains the full-screen container and selected feature controls.
4. Build all binaries:
   - `nix-shell -p go --run 'go build ./cmd/...'`
5. Full internal test suite:
   - `nix-shell -p go --run 'go test ./internal/... -v -count=1'`
6. Manual verification:
   - `./launch.sh`
   - Open `/contacts`.
   - Confirm the Contacts main tab fills the screen.
   - Create/edit/delete/search contacts.
   - Verify selected Nextcloud-parity workflows.

## Risks and tradeoffs

1. CardDAV is the biggest scope multiplier.
   - UI parity is straightforward; sync parity requires WebDAV/CardDAV protocol behavior, ETags, auth, and sync tokens.
2. Current parser is too simple for real-world vCards.
   - Implementing robust vCard support by hand is error-prone.
   - The project guide lists `github.com/emersion/go-vcard`, but `go.mod` does not currently include it, so adding it needs explicit approval.
3. Filesystem storage is simple and backup-friendly but weak for search/indexing/sharing.
   - SQLite metadata may become useful, but that is an architectural decision requiring user approval.
4. Sharing address books depends on Dar-wide users/groups/permissions.
   - Need to trace existing auth/group model before implementing.
5. Full-screen UI should match Files app conventions.
   - Avoid one-off layout decisions that diverge from the rest of Dar.
6. Nextcloud-specific privacy scopes/system address book may not map cleanly to Dar yet.

## Open questions

1. Which features above are Must / Later / Skip?
2. Is CardDAV sync with phones/desktops required now, or should we start with a rich web UI and `.vcf` import/export first?
3. Are we allowed to add `github.com/emersion/go-vcard` to `go.mod`, since AGENTS.md lists it but the current module does not include it?
4. Should Contacts continue using `.vcf` files as the source of truth, or is SQLite metadata/indexing acceptable for address books, groups, sharing, search, and sync tokens?
5. Should the system address book expose Dar users to all users, or should Dar skip that Nextcloud feature?
6. Should address book sharing depend on Dar groups if group management exists, or only user-to-user sharing initially?
7. Should birthdays/anniversaries integrate with the Calendar app in the first implementation pass?
8. Should Mail recipient autocomplete and “email this contact” be included in the first implementation pass?
9. Should the UI use desktop three-pane layout as the default, with mobile collapse behavior, or something closer to the current modal style?
