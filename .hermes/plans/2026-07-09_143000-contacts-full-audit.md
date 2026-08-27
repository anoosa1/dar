# Contacts App Full Feature Audit

> **For Hermes:** Use browser-based testing — navigate, click every button, test every form, check console for JS errors.

**Goal:** Exercise every interactive element and feature in the Dar Contacts app and report what works vs what breaks.

**Architecture:** The Contacts app has two parts: (1) a Go daemon (`cmd/contacts/main.go`) serving a REST API on port 8083, and (2) an HTML/JS frontend embedded in `internal/apps/contacts/handlers.go` (834 lines, all in one function `contactsHTML()`). The main server proxies `/api/contacts/*` to the daemon. The UI is a three-panel layout: sidebar filters/actions, contact list, detail/edit pane.

**Tech Stack:** Go 1.26, chi router, Tailwind CSS CDN, vanilla JS, vCard file-based storage.

---

## Prerequisites: Build & Launch

### Task 0: Build and start the full stack

```bash
cd /home/anas/dar
./launch.sh --no-build  # if already built
# or without --no-build to do a fresh build
```

This starts:
- Dummy OIDC server on :9001
- Contacts daemon on :8083
- Main server on :8080

Then navigate to http://localhost:8080/login and authenticate via the dummy OIDC (click "Continue with TestOIDC" then use `test@dar.local` / authorizing).

---

## Feature Matrix to Test

### A. Left Sidebar

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| A1 | "New contact" button | Click | Opens edit form in right pane (empty fields, active group pre-selected if viewing a group) |
| A2 | "All contacts" filter | Click | Shows all contacts in middle panel, count badge updates |
| A3 | "Not grouped" filter | Click | Shows contacts with no groups, filter link visible only when count > 0 |
| A4 | Groups + button | Click | Opens group creation dropdown (input + Create button) |
| A5 | Create group (Enter) | Type name, Enter | Group appears in sidebar list, dropdown closes |
| A6 | Create group (Create btn) | Type name, click Create | Same as Enter |
| A7 | Group click | Click a group name | Filters contact list to that group's members |
| A8 | Group "..." menu | Hover group, click "..." | Opens context menu (Add member, Rename, Delete) |
| A9 | Group > Add member | Click | Opens modal with searchable contact checklist |
| A10 | Group > Rename | Click, enter new name | Prompts, updates group name across all contacts |
| A11 | Group > Delete | Click, confirm | Removes group from all contacts, deletes group |
| A12 | "Contacts settings" button | Click | Opens settings modal |

### B. Middle Panel (Contact List)

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| B1 | Search contacts | Type in search bar | Client-side filter by name/email/phone |
| B2 | Search clear | Clear search | Returns to full list |
| B3 | Alphabetical grouping | View list | Contacts grouped by first letter |
| B4 | Favorite star | Click star icon | Toggles favorite, calls API, updates UI |
| B5 | Contact click | Click a contact row | Opens detail pane with name, avatar, fields |
| B6 | Empty state | No contacts | Shows "No contacts yet" with icon |

### C. Detail Pane — View Mode

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| C1 | Avatar display | View contact | Shows initial letter with color background |
| C2 | Name display | View contact | Shows full name |
| C3 | Org/subtitle | View contact | Shows organization below name |
| C4 | Email display + copy | View + click copy | Shows email as mailto link + copy button |
| C5 | Phone display + copy | View + click copy | Shows phone as tel link + copy button |
| C6 | Organization field | View contact | Shows org text |
| C7 | Groups display | View contact | Shows comma-separated groups if any |
| C8 | Edit button | Click | Switches to edit form pre-filled with contact data |
| C9 | Clone button | Click | Creates duplicate contact via API, shows new detail |
| C10 | Download button | Click | Opens /api/contacts/export?id=X in new tab |
| C11 | Delete button | Click + confirm | Deletes via API, closes detail, reloads list |
| C12 | Empty state | No contact selected | Shows "Select a contact" placeholder |

### D. Detail Pane — Edit/Create Form

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| D1 | Avatar picker | Click camera icon | Opens file picker, shows preview in circle |
| D2 | First name field | Type text | Input works, required for save |
| D3 | Last name field | Type text | Input works |
| D4 | Email field | Type text | Input works |
| D5 | Phone field | Type text | Input works |
| D6 | Organization field | Type text | Input works |
| D7 | Groups dropdown | Click dropdown | Opens checklist of groups with checkboxes |
| D8 | Group tag removal | Click X on tag | Removes group from selection |
| D9 | Cancel button | Click | Returns to view mode (detail or empty) |
| D10 | Save button — create | Fill form, click | Creates contact via API, shows in list |
| D11 | Save button — update | Edit existing, click | Updates contact via API, refreshes detail |
| D12 | Create with group pre-selection | Click "New contact" while viewing a group | Group field pre-populated |

### E. Settings Modal

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| E1 | Settings open | Click "Contacts settings" | Modal opens with General tab active |
| E2 | Settings close (X) | Click close button | Modal closes |
| E3 | Settings close (backdrop) | Click outside modal | Modal closes |
| E4 | General tab | Default tab | Shows sort selector, avatar toggle, import button |
| E5 | Sort by dropdown | Change selection | UI updates (visual only, no persistence in daemon) |
| E6 | Avatar toggle | Click toggle | UI updates (visual only, no persistence) |
| E7 | Import contacts button | Click | Opens file picker for .vcf, calls /api/contacts/import |
| E8 | Address books tab | Click tab | Switches to address books panel |
| E9 | Address book list | View | Shows books with name, count, "..." menu |
| E10 | Book "..." menu — Copy link | Click | Copies URL to clipboard |
| E11 | Book "..." menu — Download | Click | Opens /api/contacts/export in new tab |
| E12 | Book "..." menu — Show | Click | Toggles visibility (icon changes) |
| E13 | Book "..." menu — Rename | Click | Prompts for new name |
| E14 | Book "..." menu — Delete | Click + confirm | Removes book from list |
| E15 | New address book | Click | Prompts for name, adds to list |
| E16 | Version footer | Look at bottom | Shows "Contacts 1.0" |

### F. Add Members Modal

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| F1 | Modal open | Click Add member on group | Modal shows with title, search, contact list |
| F2 | Search contacts | Type in search | Filters contact list |
| F3 | Check contacts | Click checkboxes | Toggles selection |
| F4 | Add button | Select contacts, click Add | Adds to group, closes modal, reloads |
| F5 | Cancel button | Click Cancel | Closes modal without changes |
| F6 | Close via backdrop | Click outside modal | Closes without changes |
| F7 | Already-in-group filter | View member list | Contacts already in group are excluded |

### G. Context Menus (dismissal)

| # | Feature | Interaction | Expected |
|---|---------|-------------|----------|
| G1 | Group menu auto-close | Click outside | Group creation dropdown closes |
| G2 | Group item menu auto-close | Click outside | Group "..." menu closes |
| G3 | Book menu auto-close | Click outside | Book "..." menu closes |
| G4 | Groups dropdown auto-close | Click outside | Groups multi-select dropdown closes |

### H. API Endpoints (test via curl)

| # | Endpoint | Method | Test |
|---|----------|--------|------|
| H1 | /health | GET | Returns `{"status":"ok"}` |
| H2 | /list | GET | Returns `{"contacts":[...]}` |
| H3 | /get?id=X | GET | Returns single contact |
| H4 | /create | POST | Creates contact, returns with id |
| H5 | /update | POST | Updates contact |
| H6 | /delete?id=X | POST | Deletes contact |
| H7 | /search?q=X | GET | Returns filtered results |

### I. JavaScript Console Errors

| # | Check | How |
|---|-------|-----|
| I1 | No console errors on page load | `browser_console` after navigating |
| I2 | No console errors after create | Check after clicking Save |
| I3 | No console errors after delete | Check after clicking Delete |
| I4 | No console errors after search | Check after typing in search |
| I5 | No console errors after group ops | Check after group create/rename/delete |
| I6 | No 404s on API calls | Check network via console |

---

## Known Architectural Notes

1. **Import/Export:** The JS references `/api/contacts/import` and `/api/contacts/export` but the contacts daemon has no `/import` or `/export` handlers. These will fail with 404/502.
2. **Groups are client-side only:** Groups exist as CATEGORIES in vCards and are extracted/extrapolated from contacts. No server-side groups store. Rename/delete updates all contacts via individual API calls.
3. **Address books are client-side only:** Stored in `addressBooks` JS array. No server-side persistence. Copy link uses `location.origin + "/addressbooks/" + book.id` which doesn't resolve.
4. **Settings are client-side only:** Sort order, avatar toggle are UI-only with no persistence.
5. **Favorite:** Stored as vCard custom property? No — the JS sends `favorite` in the update payload, but `toVCard` doesn't serialize it. It will be lost on refresh.
6. **Avatar picker:** UI only loads image into preview. No upload endpoint exists. The avatar data is not sent with save.
7. **Color:** Server assigns colors based on index in directory listing. Not persisted per contact.
8. **X-User-Id:** Daemon reads user from `X-User-Id` header or `user_id` query param, defaults to "default". The proxy doesn't forward auth headers — no user isolation.

---

## Testing Sequence

1. Build + launch stack (Task 0)
2. Navigate to /login, authenticate
3. Navigate to /contacts
4. Check console (I1)
5. Test A1-A12 (sidebar features)
6. Test B1-B6 (contact list features)
7. Test D1-D12 (create a test contact first)
8. Test C1-C12 (detail pane features)
9. Test E1-E16 (settings modal)
10. Test F1-F7 (add members modal - requires groups + contacts)
11. Test G1-G4 (menu dismissal)
12. Test H1-H7 (API endpoints via curl)
13. Check console (I2-I6) after each destructive operation
14. Compile findings report

---

## Risks & Tradeoffs

- **No OIDC == blocked:** The `launch.sh` with test OIDC is required. Without it, the `/contacts` page redirects to `/login`.
- **Browser tool limitations:** The browser cannot interact with native file pickers (import, avatar upload). Those features can only be tested for console errors, not full flow.
- **API gaps:** Import/export will 404. Document as known issues.
