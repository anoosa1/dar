# Contacts App — Full Feature Test Plan

> **For Hermes:** Execute this systematically — one feature, browser-test, verify, then next.

**Goal:** Test every button, link, modal, dropdown, and form in the Dar Contacts app, cataloging what works and what's broken.

**Architecture:** The Contacts app has a Go daemon on port 8083 serving a REST API (vCard file storage), and a Go handler (`internal/apps/contacts/handlers.go`) rendering an SPA-like HTML page with embedded vanilla JS. The main server proxies `/api/contacts/*` to the daemon and serves the page at `/contacts` behind OIDC auth.

**Tech Stack:** Go 1.26+, chi router, vanilla JS, Tailwind CSS CDN, Font Awesome CDN

---

## Prerequisites

Before testing, ensure:
1. OIDC env vars are set (`DAR_OIDC_ISSUER`, `DAR_OIDC_CLIENT_ID`, `DAR_OIDC_CLIENT_SECRET`) so you can log in
2. Build both binaries: `go build ./cmd/server/ ./cmd/contacts/`
3. Start the contacts daemon first: `./contacts --port 8083 --data-root /tmp/dar_test_data`
4. Start the main server: `./dar-server --port 8080 --db /tmp/dar_test.db`
5. Navigate to `http://localhost:8080/contacts` and authenticate

**Data directory:** `/tmp/dar_test_data/default/contacts/` — pre-populate with test .vcf files to test existing contacts, or start empty and create them through the UI.

---

## Feature Inventory

### Sidebar (left panel)

| # | Feature | Trigger | Expected Behavior |
|---|---------|---------|-------------------|
| S1 | "New contact" button | Click | Opens edit form in detail pane (empty fields, "Save" visible) |
| S2 | "All contacts" filter | Click link | Shows all contacts in list, badge shows count |
| S3 | "Not grouped" filter | Click link | Shows only contacts with no groups, badge shows count |
| S4 | "+" create group button | Click "+" icon | Opens inline dropdown with name input + "Create" button |
| S5 | Create group (Enter key) | Type name, press Enter | Group added to sidebar list, dropdown closes |
| S6 | Create group (button) | Type name, click "Create" | Group added to sidebar list, dropdown closes |
| S7 | Group list item click | Click group name | Filters contact list to that group's members |
| S8 | Group "..." hover menu | Hover group, click "..." | Shows "Add member", "Rename", "Delete" options |
| S9 | "Add member" (group menu) | Click "Add member" in group menu | Opens modal with searchable contact checklist |
| S10 | Add members modal search | Type in search field | Filters contacts in the checklist |
| S11 | Add members modal check+save | Check contacts, click "Add" | Adds selected contacts to group, modal closes |
| S12 | Add members modal cancel | Click "Cancel" or backdrop | Modal closes, no changes |
| S13 | "Rename" group | Click "Rename", enter new name | Group renamed in sidebar and on all member contacts |
| S14 | "Delete" group | Click "Delete", confirm | Group removed from sidebar and all member contacts |
| S15 | "Contacts settings" button | Click gear button at bottom | Opens settings modal |

### Main Contact List (center panel)

| # | Feature | Trigger | Expected Behavior |
|---|---------|---------|-------------------|
| L1 | Search bar (empty) | Clear search field | Shows all contacts alphabetically grouped |
| L2 | Search bar (typing) | Type in search field | Client-side filters by name/email/phone |
| L3 | Alphabetical grouping | N/A | Contacts grouped by first letter with letter headers |
| L4 | Star/favorite toggle | Click star icon on contact row | Toggles filled yellow star, persists via API call |
| L5 | Contact row click | Click contact row | Opens detail pane with full contact info |
| L6 | Contact row highlight | Click contact row | Selected row gets ring+background highlight |

### Detail Pane — View Mode (right panel)

| # | Feature | Trigger | Expected Behavior |
|---|---------|---------|-------------------|
| D1 | Empty state | No contact selected | Shows "Select a contact" placeholder |
| D2 | Avatar display | Select contact | Shows colored circle with first letter initial |
| D3 | Name display | Select contact | Shows first + last name centered |
| D4 | Org/subtitle | Select contact | Shows organization below name |
| D5 | Email field | Select contact with email | Shows "Email" label + mailto link + copy button |
| D6 | Phone field | Select contact with phone | Shows "Phone" label + tel link + copy button |
| D7 | Organization field | Select contact with org | Shows "Organization" label + value |
| D8 | Groups field | Select contact with groups | Shows "Contact groups" label + comma-separated list |
| D9 | Email copy button | Click copy icon next to email | Copies email to clipboard |
| D10 | Phone copy button | Click copy icon next to phone | Copies phone to clipboard |
| D11 | "Edit" button | Click "Edit" | Switches detail pane to edit mode, populates fields |
| D12 | "Clone" button | Click "Clone" | Creates duplicate contact, shows it in detail |
| D13 | "Download" button | Click "Download" | Opens `/api/contacts/export?id=...` in new tab |
| D14 | "Delete" button | Click "Delete", confirm | Deletes contact, closes detail pane, refreshes list |

### Detail Pane — Edit Mode

| # | Feature | Trigger | Expected Behavior |
|---|---------|---------|-------------------|
| E1 | Avatar picker | Click camera icon on avatar | Opens file picker for image, shows preview |
| E2 | First name input | Type/edit text | Works as normal input |
| E3 | Last name input | Type/edit text | Works as normal input |
| E4 | Email input | Type/edit text | Works as normal input |
| E5 | Phone input | Type/edit text | Works as normal input |
| E6 | Organization input | Type/edit text | Works as normal input |
| E7 | Groups dropdown toggle | Click dropdown | Opens checkbox list of available groups |
| E8 | Groups checkbox check | Check a group | Shows tag below dropdown, updates selectedGroups |
| E9 | Groups tag remove | Click "x" on group tag | Removes group from selection, updates checkbox |
| E10 | Groups dropdown close (click outside) | Click anywhere outside | Closes dropdown |
| E11 | "Cancel" button | Click "Cancel" | Returns to view mode (or closes pane if creating new) |
| E12 | "Save" button (create) | Fill fields, click "Save" | Creates contact, closes edit, shows in list |
| E13 | "Save" button (update) | Edit fields, click "Save" | Updates contact, returns to view mode with new data |

### Settings Modal

| # | Feature | Trigger | Expected Behavior |
|---|---------|---------|-------------------|
| M1 | Open settings | Click "Contacts settings" in sidebar | Shows modal with "General" and "Address books" tabs |
| M2 | Close settings (X button) | Click "X" button | Closes modal |
| M3 | Close settings (backdrop) | Click backdrop outside modal | Closes modal |
| M4 | General tab (default) | Open settings | Shows sort dropdown, avatar toggle, import button |
| M5 | Sort by dropdown | Change sort option | UI control (sorting likely not wired to backend) |
| M6 | Avatar toggle | Click toggle switch | UI control (toggle not wired to backend) |
| M7 | "Import contacts" button | Click "Import contacts" | Opens file picker for .vcf/.vcard files |
| M8 | Import .vcf file | Select valid .vcf file | Uploads to `/api/contacts/import`, shows alert with count |
| M9 | Import invalid file | Select non-vcf file | Shows "No contacts found" or error alert |
| M10 | Address books tab | Click "Address books" tab | Shows list of address books + "New address book" button |
| M11 | Address book "..." menu | Click "..." on an address book | Shows Copy link, Download, Show, Rename, Delete |
| M12 | "Copy link" (book menu) | Click "Copy link" | Copies `/addressbooks/<id>` URL to clipboard |
| M13 | "Download" (book menu) | Click "Download" | Opens `/api/contacts/export` in new tab |
| M14 | "Show" toggle (book menu) | Click "Show" | Toggles address book visibility (visual feedback on icon) |
| M15 | "Rename" (book menu) | Click "Rename", enter name | Renames address book (client-side only, no API) |
| M16 | "Delete" (book menu) | Click "Delete", confirm | Removes address book (client-side only, no API) |
| M17 | "New address book" button | Click button, enter name | Adds book to list (client-side only, no API) |
| M18 | Version footer | Open settings | Shows "Contacts 1.0" |

### API Endpoints (curl-based verification)

| # | Endpoint | Method | Test |
|---|----------|--------|------|
| A1 | `/list` | GET | Returns JSON `{"contacts": [...]}` with all contacts |
| A2 | `/get?id=<id>` | GET | Returns single contact JSON |
| A3 | `/get?id=nonexistent` | GET | Returns 404 |
| A4 | `/create` | POST | Creates .vcf file, returns contact with generated id |
| A5 | `/update` | POST | Updates .vcf file, returns updated contact |
| A6 | `/update` (no id) | POST | Returns 400 "missing id" |
| A7 | `/delete?id=<id>` | POST | Deletes .vcf file, returns `{"status":"ok"}` |
| A8 | `/search?q=<term>` | GET | Returns filtered contacts |
| A9 | `/health` | GET | Returns `{"status":"ok"}` |

---

## Step-by-Step Testing Plan

### Phase 0: Setup

**Step 0.1: Build binaries**

```bash
cd /home/anas/dar
go build -o dar-server ./cmd/server/
go build -o contacts ./cmd/contacts/
```

**Step 0.2: Start contacts daemon**

```bash
./contacts --port 8083 --data-root /tmp/dar_test_data &
```

**Step 0.3: Start main server (with OIDC env vars)**

```bash
export DAR_OIDC_ISSUER=...
export DAR_OIDC_CLIENT_ID=...
export DAR_OIDC_CLIENT_SECRET=...
./dar-server --port 8080 --db /tmp/dar_test.db &
```

**Step 0.4: Navigate to /contacts, log in, verify page loads**

- `browser_navigate("http://localhost:8080/contacts")`
- Complete OIDC flow
- Verify the contacts page renders with sidebar, search bar, empty contact list, and detail pane showing "Select a contact"

### Phase 1: Create Test Data

We'll create contacts through the UI and verify they appear.

**Task 1.1: Create first contact**
- Click "New contact" in sidebar
- Enter: First=Alice, Last=Smith, Email=alice@example.com, Phone=555-0100, Org=Acme Corp
- Click "Save"
- Verify: Alice appears in contact list under "A", detail pane shows her info
- Use `browser_console` to check for JS errors

**Task 1.2: Create second contact**
- "New contact" → First=Bob, Last=Jones, Email=bob@example.com
- Save → verify appears under "B"

**Task 1.3: Create third contact (no group)**
- "New contact" → First=Charlie, Last=Brown
- Save → verify appears under "C", "Not grouped" count = 3 (assuming no groups assigned yet)

### Phase 2: Sidebar Features

**Task 2.1: "New contact" button (S1)**
- Click "New contact"
- Verify: detail pane switches to edit form, all fields empty, Save button visible

**Task 2.2: "All contacts" filter (S2)**
- After selecting a group or filter, click "All contacts"
- Verify: badge shows correct count (3), all 3 contacts visible

**Task 2.3: "Not grouped" filter (S3)**
- Click "Not grouped"
- Verify: shows only contacts with no groups, badge count matches

**Task 2.4: Create group via "+" button (S4, S5, S6)**
- Click "+" next to "Contact groups"
- Verify: inline dropdown appears with input field and "Create" button

**Task 2.5: Create group — Enter key (S5)**
- Type "Friends" in the input
- Press Enter
- Verify: "Friends" appears in group list, dropdown closes

**Task 2.6: Create group — button (S6)**
- Click "+" again
- Type "Work"
- Click "Create"
- Verify: "Work" appears in group list

**Task 2.7: Group list click (S7)**
- Click "Friends" group
- Verify: contact list is filtered (should be empty since no contacts have this group yet)

**Task 2.8: Group "..." menu (S8)**
- Hover over "Friends" group, click "..." icon
- Verify: dropdown menu appears with "Add member", "Rename", "Delete"

**Task 2.9: Add member to group (S9-S12)**
- Click "Add member" on Friends group
- Verify: modal opens with "Add members to Friends" title, searchable list of contacts not yet in group
- Search for "Alice"
- Check Alice's checkbox
- Click "Add"
- Verify: Alice now listed under Friends when clicking that group filter

**Task 2.10: Rename group (S13)**
- Open "..." on "Friends", click "Rename"
- Enter "Besties" in prompt
- Verify: group renamed to "Besties" in sidebar, Alice's groups updated

**Task 2.11: Delete group (S14)**
- Open "..." on "Besties", click "Delete"
- Confirm in dialog
- Verify: group removed from sidebar, Alice no longer has the group

**Task 2.12: Settings button (S15)**
- Click "Contacts settings" at bottom of sidebar
- Verify: settings modal opens

### Phase 3: Contact List Features

**Task 3.1: Empty search / full list (L1)**
- Ensure search bar is empty
- Verify: all 3 contacts visible, grouped by A, B, C letter headers

**Task 3.2: Search filtering (L2)**
- Type "ali" in search bar
- Verify: only Alice appears
- Type "555"
- Verify: only Alice appears (phone match)
- Clear search
- Verify: all contacts return

**Task 3.3: Alphabetical grouping (L3)**
- Verify: each letter has a header div above the contacts for that letter

**Task 3.4: Favorite toggle (L4)**
- Click the star next to Alice
- Verify: star becomes filled yellow, persists after page interactions
- Click Alice's row to show detail → star state should be consistent

**Task 3.5: Contact row click (L5)**
- Click Bob's row
- Verify: detail pane shows Bob's avatar, name, email

**Task 3.6: Row highlight (L6)**
- Verify: selected row (Bob) has ring/background highlight

### Phase 4: Detail Pane — View Mode

**Task 4.1: Empty state (D1)**
- Somehow deselect or refresh
- Verify: "Select a contact" placeholder shown

**Task 4.2-4.8: Contact detail display (D2-D8)**
- Select Alice (who has email, phone, org)
- Verify: avatar with "A", "Alice Smith" as name, "Acme Corp" subtitle
- Verify: Email section with mailto link and copy button
- Verify: Phone section with tel link and copy button
- Verify: Organization section with "Acme Corp"

**Task 4.9-4.10: Copy buttons (D9-D10)**
- Click copy icon next to Alice's email
- Verify: email copied to clipboard (`browser_console` check)

**Task 4.11: Edit button (D11)**
- Select Alice, click "Edit"
- Verify: form fields populated with Alice's data
- Verify: groups dropdown reflects Alice's groups

**Task 4.12: Clone button (D12)**
- Select Alice, click "Clone"
- Verify: new contact appears in list (Alice clone), detail pane shows the clone
- Verify: clone has same first/last/email/phone/org

**Task 4.13: Download button (D13)**
- Select Alice, click "Download"
- Verify: new tab opens with vCard content (or download prompt)
- `browser_console` to check for errors

**Task 4.14: Delete button (D14)**
- Select Charlie, click "Delete"
- Confirm dialog
- Verify: Charlie removed from list, detail pane shows empty state
- Verify: count badges update

### Phase 5: Detail Pane — Edit Mode

**Task 5.1: Avatar picker (E1)**
- Click "New contact", click camera icon on avatar
- Verify: file picker opens (check console for errors)

**Task 5.2-5.6: Input fields (E2-E6)**
- Fill all text inputs
- Verify: typing works, no JS errors in console

**Task 5.7: Groups dropdown (E7)**
- Click "Select groups..." dropdown in edit form
- Verify: checkbox list of existing groups appears

**Task 5.8-5.10: Groups selection (E8-E10)**
- Check "Work" group
- Verify: "Work" tag appears above dropdown
- Click "x" on "Work" tag
- Verify: tag removed, checkbox unchecked
- Click outside dropdown
- Verify: dropdown closes

**Task 5.11: Cancel edit (E11)**
- Click "New contact", enter some data, click "Cancel"
- Verify: detailed pane shows empty state (since it was a new contact)

**Task 5.12: Save new contact (E12)**
- Click "New contact", enter: First=Dave, Last=Wilson, Email=dave@example.com
- Click "Save"
- Verify: Dave appears in list, detail pane shows Dave's info

**Task 5.13: Save edit (E13)**
- Select Bob, click "Edit"
- Change email to "bob.updated@example.com"
- Click "Save"
- Verify: Bob's detail shows updated email

### Phase 6: Settings Modal

**Task 6.1: Open/close settings (M1-M3)**
- Click "Contacts settings"
- Verify: modal visible with General tab active
- Click X button → modal closes
- Reopen, click backdrop → modal closes

**Task 6.4: General tab (M4)**
- Open settings → verify "General" tab is active
- Verify: sort dropdown, avatar toggle, import button all visible

**Task 6.5: Sort dropdown (M5)**
- Change sort to "Last name"
- Check: dropdown value changes (though sorting may not be wired)

**Task 6.6: Avatar toggle (M6)**
- Click toggle switch
- Check: visual change (toggle not wired to backend)

**Task 6.7-6.9: Import contacts (M7-M9)**
- Click "Import contacts"
- Verify: file picker opens with .vcf/.vcard filter
- (To test fully: select a valid .vcf file, verify import alert)

**Task 6.10: Address books tab (M10)**
- Click "Address books" tab
- Verify: tab switches, shows address books list + "New address book" button

**Task 6.11-6.17: Address book management (M11-M17)**
- Click "..." on the default address book
- Verify: menu with Copy link, Download, Show, Rename, Delete
- Test "Show" toggle → icon should change (filled vs outline)
- Test "Rename" → prompt appears, rename works
- Test "New address book" → prompt, new book appears in list
- Test "Delete" → confirm, book removed
- Test "Copy link" → copies URL to clipboard

**Task 6.18: Version footer (M18)**
- Verify: "Contacts 1.0" visible at bottom of settings modal

### Phase 7: API Endpoint Verification (curl)

**Task 7.1: Health endpoint**
```bash
curl -s http://localhost:8083/health | jq .
# Expected: {"status":"ok"}
```

**Task 7.2: List endpoint**
```bash
curl -s http://localhost:8083/list | jq .
# Expected: {"contacts": [...]} with all created contacts
```

**Task 7.3: Get endpoint**
```bash
# Get a specific contact by ID (from list output)
curl -s "http://localhost:8083/get?id=<id>" | jq .
# Expected: single contact JSON
```

**Task 7.4: Search endpoint**
```bash
curl -s "http://localhost:8083/search?q=alice" | jq .
# Expected: contacts matching "alice"
```

**Task 7.5: Create, update, delete via curl**
```bash
# Create
curl -s -X POST http://localhost:8083/create \
  -H "Content-Type: application/json" \
  -d '{"first_name":"Eve","last_name":"Test","email":"eve@test.com"}' | jq .

# Update (use returned id)
curl -s -X POST http://localhost:8083/update \
  -H "Content-Type: application/json" \
  -d '{"id":"<id>","first_name":"Eve","last_name":"Updated","email":"eve@test.com"}' | jq .

# Delete
curl -s -X POST "http://localhost:8083/delete?id=<id>" | jq .
# Expected: {"status":"ok"}
```

### Phase 8: Edge Cases & Error Handling

**Task 8.1: Duplicate group name**
- Create group "Work" → create another "Work"
- Verify: second creation silently ignored (no duplicate entry)

**Task 8.2: Empty contact save**
- Click "New contact" → "Save" without filling anything
- Check: what happens? Error? Empty contact created?

**Task 8.3: Very long text inputs**
- Enter very long strings in all fields → Save
- Verify: no crash, data preserved

**Task 8.4: Special characters**
- Enter names with <, >, &, " → Save
- Verify: escaped properly in display (XSS prevention)

**Task 8.5: Rapid clicks**
- Click "New contact", "Save", "Delete" rapidly
- Verify: no JS errors, no duplicate data

**Task 8.6: Browser console throughout**
- Keep browser_console open during all tests
- Check for: uncaught exceptions, failed fetch calls, undefined references

---

## Known Architecture Notes (from code review)

1. **No backend auth on daemon**: The contacts daemon trusts `X-User-Id` header or `user_id` query param, falls back to "default". All users share `data/default/contacts/` directory.

2. **No `/export` endpoint**: The JS calls `apiBase + "/export"` for download but there's no `/export` handler in the daemon. This will return 404.

3. **No `/import` endpoint**: The JS calls `apiBase + "/import"` for import but there's no `/import` handler. This will return 404.

4. **Address books are client-side only**: `addressBooks` array is JavaScript state (never persisted to server). Create/rename/delete operations only modify the JS array.

5. **Groups are stored in vCard CATEGORIES**: Group data persists because it's embedded in each contact's .vcf file as `CATEGORIES:group1,group2`.

6. **Favorite (`favorite` field)**: Toggled client-side and sent via `/update`, but the backend doesn't parse/store a `favorite` field from vCard — it will be silently dropped.

7. **Avatar upload**: The picker reads a file as data URL for preview but there's no POST to save the avatar image anywhere — it's client-side only.

8. **Color assignment**: `contactColor(idx)` uses index in directory listing, so colors may shift if contacts are created or deleted.

9. **Search is client-side**: Both the contact list search and add-members search filter in JavaScript — no server round-trip.

10. **Sort dropdown**: Present in settings UI but no `sort` functionality is wired in JavaScript.

---

## Expected Failures (from code review)

| # | Feature | Expected Outcome |
|---|---------|-----------------|
| F1 | "Download" button (detail pane) | 404 — `/export` endpoint doesn't exist in daemon |
| F2 | "Download" button (address book menu) | 404 — same missing `/export` endpoint |
| F3 | "Import contacts" button | 404 — `/import` endpoint doesn't exist |
| F4 | Avatar persistence | Avatar preview works but isn't saved anywhere |
| F5 | Favorite persistence | Star toggles visually but `favorite` field not parsed from vCard |
| F6 | Sort by dropdown | UI control present but not wired to any JS |
| F7 | Avatar toggle in settings | UI control present but not wired to any JS |

---

## Verification Commands

```bash
# After all tests, verify data directory
ls -la /tmp/dar_test_data/default/contacts/
cat /tmp/dar_test_data/default/contacts/*.vcf

# Check server logs for any errors
# Check browser console for accumulated JS errors
```

---

## Test Completion Criteria

- [ ] All S1-S15 sidebar features tested and documented
- [ ] All L1-L6 contact list features tested and documented
- [ ] All D1-D14 detail view features tested and documented
- [ ] All E1-E13 edit mode features tested and documented
- [ ] All M1-M18 settings modal features tested and documented
- [ ] All A1-A9 API endpoints verified via curl
- [ ] All edge cases tested (Phase 8)
- [ ] Browser console checked for JS errors throughout
- [ ] Results cataloged: working ✓, broken ✗, works with caveats ⚠
