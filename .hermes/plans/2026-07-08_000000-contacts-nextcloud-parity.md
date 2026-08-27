# Dar Contacts — Nextcloud Feature Parity Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Make Dar Contacts identical to Nextcloud Contacts in UX, features, and design (with Material 3 styling).

**Architecture:** All changes go to `internal/apps/contacts/handlers.go` (frontend HTML/JS) and `cmd/contacts/main.go` (daemon API). No new files, no new dependencies.

**Tech Stack:** Go 1.26+, zombiezen sqlite, vcard library, vanilla JS, Tailwind CSS (Material 3), Font Awesome 6.

---

## Sidebar & Address Books Architecture (Nextcloud)

### Sidebar structure (top to bottom):
```
+ New contact           ← primary action button at top
All contacts        3   ← count badge, always visible
Not grouped         3   ← count badge, filter for ungrouped
───────────────
Contact groups    [+]   ← collapsible section, + to create group
Teams             [+]   ← collapsible section, + to create team
  "Create your own teams for sharing..."
───────────────
[space fills]
───────────────
⚙ Contacts settings    ← PINNED TO BOTTOM as footer, always visible
```

**Key insight:** The settings button is a footer, not a list item. It stays pinned at the bottom regardless of scroll.

### Address books are in Settings modal, NOT in sidebar:
Settings modal has two tabs: **General** | **Address books**

General tab:
- Sort contacts by (dropdown)
- Update avatars from social media (toggle, weekly)
- Import contacts (button)

Address books tab (each row shows name, contact count, group count):
- [share icon] [3-dot menu ↓]
- + New address book (button at bottom)

3-dot menu per address book:
- **Copy link** (CardDAV URL)
- **Download** (export .vcf)
- **Show** (checkbox toggle — show/hide in main view)
- **Rename**
- **Delete**

### Dar's current sidebar (WRONG):
Dar shows address books INLINE in the sidebar as selectable items (like "Contacts" with count badges), and the settings button is mixed in with them. This needs to be restructured to match Nextcloud.

## Gap Analysis

### Structural Issues (sidebar/address books mismatch)
| # | Issue | Location |
|---|-------|----------|
| S1 | Settings button should be pinned footer at bottom, not mixed in sidebar items | `handlers.go` contactsHTML sidebar |
| S2 | Address books should be in Settings modal tab, NOT inline in sidebar | `handlers.go` renderBooks, settings modal |
| S3 | Sidebar should show: New contact, All contacts, Not grouped, Contact groups, Teams, Settings (footer) | `handlers.go` contactsHTML |
| S4 | Settings modal needs tab navigation (General / Address books) | `handlers.go` settings modal HTML |
| S5 | Address book 3-dot menu missing Show/Hide toggle, Download, Rename, Copy link | `handlers.go` openSettingsBookMenu |

### Bugs to Fix
| # | Issue | Location |
|---|-------|----------|
| B1 | New contact creates placeholder "New contact" in DB before form opens | `handlers.go:416` |
| B2 | After saving edit, contact list sometimes shows stale name | `handlers.go:442` (saveEdit → loadContacts race) |
| B3 | Address fields missing: Extended address, P.O. box | `handlers.go:436` and `cmd/contacts/main.go` Contact model |

### Missing Features (Nextcloud parity)
| # | Feature | Nextcloud has | Dar has |
|---|---------|--------------|---------|
| F1 | Clone contact | ✓ | ✗ |
| F2 | Generate QR Code | ✓ | ✗ |
| F3 | Favorites toggle (star) | ✓ | ✗ |
| F4 | Contact groups (assign per-contact) | ✓ | ✗ |
| F5 | "Not grouped" filter in sidebar | ✓ | ✗ |
| F6 | Copy-to-clipboard per field | ✓ | ✗ |
| F7 | Download single contact (.vcf) | ✓ | ✗ |
| F8 | Per-contact Actions menu (Download, Clone, QR, Delete) | ✓ | Only inline Delete |
| F9 | Central "Name" field (not split by default) | ✓ | Split first/last by default |
| F10 | Address extended fields (P.O. box, Extended addr) | ✓ | Missing 2 fields |
| F11 | Address book Show/Hide toggle | ✓ | Missing (hiddenBooks is broken) |
| F12 | Address book Rename in 3-dot menu | ✓ | Has it via separate prompt |
| F13 | Address book Download/export per-book | ✓ | Has global export only |
| F14 | "Recently contacted" auto-address book | ✓ | ✗ (nice-to-have) |
| F15 | Sort contacts dropdown in Settings General tab | ✓ | In settings but misplaced |
| F16 | Settings tab navigation (General/Address books) | ✓ | Flat settings modal |

---

## Implementation Tasks

### Phase 0: Sidebar & Settings Restructure (critical UX mismatch)

The biggest gap between Dar and Nextcloud Contacts is the sidebar architecture.

### Task 0.1: Move settings button to bottom as pinned footer

**Objective:** Pin the "Contacts settings" button to the bottom of the sidebar as a footer, not floating among navigation items.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (contactsHTML sidebar)

**Step 1: Restructure sidebar HTML**

The current sidebar has settings button mixed with address books. Change to:
```html
<aside class="hidden lg:flex flex-col border-r border-outline-variant bg-surface-container-low overflow-hidden">
  <!-- Scrollable top section -->
  <div class="flex-1 overflow-y-auto">
    <!-- New contact button -->
    <div class="px-4 py-3">
      <button onclick="newContact()" ...>+ New contact</button>
    </div>
    <!-- All contacts / Not grouped -->
    <div class="px-4 py-1">
      <div id="sidebar-all-contacts" ...>All contacts <span id="all-count">0</span></div>
      <div id="sidebar-not-grouped" ...>Not grouped <span id="ungrouped-count">0</span></div>
    </div>
    <!-- Contact groups section -->
    <div class="px-4 py-3">
      <div class="flex items-center justify-between mb-1">
        <span class="text-xs font-bold uppercase">Contact groups</span>
        <button onclick="addGroup()"><i class="fa-solid fa-plus text-xs"></i></button>
      </div>
      <div id="sidebar-groups"></div>
    </div>
    <!-- Teams section -->
    <div class="px-4 py-3">
      <div class="text-xs font-bold uppercase">Teams</div>
      <p class="text-xs opacity-70">Create your own teams for sharing...</p>
    </div>
  </div>
  <!-- Pinned footer -->
  <div class="border-t border-outline-variant px-4 py-3 shrink-0">
    <button onclick="openSettings()" class="w-full h-10 rounded-lg flex items-center gap-3 px-3 text-sm text-on-surface-variant hover:bg-surface-container-high">
      <i class="fa-solid fa-gear w-5 shrink-0"></i>Contacts settings
    </button>
  </div>
</aside>
```

**Step 2: Remove renderBooks() sidebar logic**

Remove renderBooks(), selectBook(), and the sidebar-books div. Load contacts for all visible books only.

**Step 3: Update loadContacts to compute count badges**

Update `#all-count` and `#ungrouped-count` after fetch.

**Step 4: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Task 0.2: Move address books into Settings modal tab

**Objective:** Remove address books from sidebar and add them as a tabbed pane in settings.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (settings modal HTML)

**Step 1: Add tab navigation to settings modal**

```html
<div class="flex border-b border-outline-variant mb-4">
  <button id="settings-tab-general" onclick="switchSettingsTab('general')"
    class="flex-1 h-10 text-sm font-medium border-b-2 border-primary text-primary">General</button>
  <button id="settings-tab-books" onclick="switchSettingsTab('books')"
    class="flex-1 h-10 text-sm font-medium border-b-2 border-transparent text-on-surface-variant">Address books</button>
</div>
```

**Step 2: Split content into General/Address books panels**

General panel: Sort dropdown, Avatar toggle, Import button.
Address books panel: Book list with share icon + 3-dot menu, + New address book.

**Step 3: Add switchSettingsTab() JS function**

**Step 4: Remove old settings book list, rebuild in address books tab**

**Step 5: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Task 0.3: Add proper address book 3-dot menu actions

**Objective:** Each address book in settings gets: Copy link, Download, Show/Hide toggle, Rename, Delete.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (settings-book-menu HTML, openSettingsBookMenu)

**Step 1: Rewrite menu HTML with all 5 actions**

```html
<div id="settings-book-menu" class="hidden absolute z-50 bg-surface-container rounded-xl shadow-lg border border-outline-variant py-1 min-w-[180px]">
  <button onclick="copyCardDAVUrl()" ...><i class="fa-solid fa-link w-4"></i>Copy link</button>
  <button onclick="downloadBook()" ...><i class="fa-solid fa-download w-4"></i>Download</button>
  <button onclick="toggleBookVisibility()" ...><i id="menu-show-icon" class="fa-solid fa-eye w-4"></i><span id="menu-show-label">Show</span></button>
  <button onclick="renameBookFromSettings(menuBook)" ...><i class="fa-solid fa-pen-to-square w-4"></i>Rename</button>
  <button onclick="deleteBookFromSettings(menuBook)" ...><i class="fa-solid fa-trash w-4"></i>Delete</button>
</div>
```

**Step 2: Add toggleBookVisibility()**

**Step 3: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Task 0.4: Fix "New contact" creation flow (no placeholder)

**Objective:** Rewrite newContact() to open the form WITHOUT creating a DB record first.

**Files:**
- Modify: `internal/apps/contacts/handlers.go:416`

**Step 1: Rewrite newContact()**

```javascript
function newContact(){
  selectedId=null;isNew=!0;
  editAvatarData=null;editPhoneIdx=editEmailIdx=editAddrIdx=0;
  document.getElementById('detail-pane').classList.remove('hidden');
  document.getElementById('detail-empty').classList.add('hidden');
  startEdit();
}
```

**Step 2: Update cancelEdit() for unsaved new contacts**

```javascript
function cancelEdit(){
  if(isNew&&!selectedId){deselect();return}
  if(isNew&&selectedId){api('POST','/delete?id='+selectedId).then(()=>{deselect();loadContacts()})}
  else if(selectedId)showView();
  else deselect();
}
```

**Step 3: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Phase 1: Remaining Bug Fixes & Field Extensions

---

### Phase 1: Remaining Bug Fixes & Field Extensions

---

### Task 1.1: Fix saveEdit race — wait for API before loadContacts

**Objective:** Ensure loadContacts() waits for the save API to complete before refreshing.

**Files:**
- Modify: `internal/apps/contacts/handlers.go:442`

**Step 1: Add await to loadContacts in saveEdit**

Line 442 already has `await api("POST",path,data);isNew=!1;await loadContacts();showView()` — verify the await chain is correct. If the API returns before the file is written on disk, add a small delay or ensure the daemon's handleCreate/handleUpdate are synchronous (they are — os.WriteFile is sync).

**Step 2: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Task 1.2: Add Extended Address and P.O. Box fields to Contact model and form

**Objective:** Add `ExtendedAddr` and `POBox` fields to the Contact struct and the edit/detail forms.

**Files:**
- Modify: `cmd/contacts/main.go` (Contact struct, parseVCard, toVCard)
- Modify: `internal/apps/contacts/handlers.go` (addAddressEdit, collectEditAddresses, showView)

**Step 1: Add fields to Contact model**

In `cmd/contacts/main.go`, add to the Address struct:
```go
type Address struct {
    Types          []string `json:"types,omitempty"`
    POBox          string   `json:"po_box,omitempty"`
    ExtendedAddr   string   `json:"extended_addr,omitempty"`
    Street         string   `json:"street,omitempty"`
    Locality       string   `json:"locality,omitempty"`
    PostalCode     string   `json:"postal_code,omitempty"`
    Country        string   `json:"country,omitempty"`
}
```

**Step 2: Update parseVCard to read the new fields**

In the vcard parsing loop in `cmd/contacts/main.go`, add parsing for:
- `vcard.FieldPostOfficeBox` → POBox
- `vcard.FieldExtendedAddress` → ExtendedAddr

**Step 3: Update toVCard to write the new fields**

**Step 4: Update addAddressEdit() in handlers.go to show new fields**

Add P.O. Box and Extended address inputs below the type selector and above Street.

**Step 5: Update collectEditAddresses() to collect the new fields**

**Step 6: Update showView() address rendering to include new fields**

**Step 7: Build and test**

```bash
go build ./cmd/...
go test ./internal/apps/contacts/... -v -count=1
```

---

### Phase 2: Missing Features

---

### Task 4: Add "Not grouped" filter to sidebar

**Objective:** Add a clickable "Not grouped" filter that shows contacts without group assignment.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (renderBooks, loadContacts, selectBook, contactsHTML)

**Step 1: Add "Not grouped" link to sidebar HTML**

In contactsHTML(), add after the "All contacts" / address books section:
```html
<div class="mt-4 px-4">
  <div class="text-xs font-bold uppercase tracking-wide text-on-surface-variant mb-2">Filter</div>
  <div id="sidebar-not-grouped" class="rounded-xl bg-transparent text-on-surface-variant hover:bg-surface-container-high cursor-pointer px-3 py-2.5 flex items-center gap-3" onclick="selectBook('__notgrouped__')">
    <i class="fa-solid fa-user-slash w-5 shrink-0"></i>
    <div class="flex-1 min-w-0"><div class="text-sm truncate">Not grouped</div></div>
  </div>
</div>
```

**Step 2: Update loadContacts() to filter when currentBook === '__notgrouped__'**

After fetching contacts, filter out contacts that have any group assignment. Since groups aren't implemented yet, this can be a simple filter. Add a `groups` field to the Contact model first (see Task 6).

**Step 3: Build and test**

---

### Task 5: Add Favorites toggle (star icon)

**Objective:** Add a star button on contact list items and detail view header.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (renderList, showView, Contacts HTML)
- Modify: `cmd/contacts/main.go` (Contact struct, handleUpdate)

**Step 1: Add `favorite` field to Contact model**

```go
type Contact struct {
    // ... existing fields
    Favorite bool `json:"favorite,omitempty"`
}
```

**Step 2: Add star toggle button to contact list items**

In renderList(), add a star icon before the contact name:
```javascript
let star=c.favorite?'fa-solid fa-star text-yellow-400':'fa-regular fa-star text-on-surface-variant opacity-40';
h+='<button onclick="event.stopPropagation();toggleFavorite(\''+c.id+'\')" class="...">'+star+'</button>';
```

**Step 3: Add toggleFavorite() function**

```javascript
async function toggleFavorite(id){
  let c=contacts.find(x=>x.id===id);
  c.favorite=!c.favorite;
  await api("POST","/update",{id:id,favorite:c.favorite});
  renderList(contacts);
}
```

**Step 4: Add star to detail view header**

In showView(), add a star next to the contact name.

**Step 5: Sort favorites to top**

In renderList(), sort contacts with favorites first.

**Step 6: Build and test**

---

### Task 6: Add Contact Groups (assign per-contact)

**Objective:** Add groups field to contacts, group dropdown in edit form, group management.

**Files:**
- Modify: `cmd/contacts/main.go` (Contact struct, handleCreate, handleUpdate, new group endpoints)
- Modify: `internal/apps/contacts/handlers.go` (saveEdit, startEdit, contactsHTML)

**Step 1: Add groups field to Contact model**

```go
type Contact struct {
    // ... existing fields
    Groups []string `json:"groups,omitempty"`
}
```

**Step 2: Add group management endpoints to daemon**

New endpoints in `cmd/contacts/main.go`:
- `GET /groups` — list all groups from a per-user JSON file
- `POST /groups/create` — create a group
- `POST /groups/delete` — delete a group
- `POST /groups/rename` — rename a group

Store groups in `data/<uid>/groups.json` as `{"groups":["Family","Work","Friends"]}`

**Step 3: Add group multi-select to edit form**

In the edit form HTML, add a group selector section below Notes:
```html
<div class="mt-4">
  <div class="text-xs font-bold uppercase tracking-wide text-on-surface-variant mb-2">Contact groups</div>
  <div id="edit-groups-container"></div>
  <button onclick="addGroupSelect()" class="...">+ Add to group</button>
</div>
```

**Step 4: Update saveEdit() to collect groups**

**Step 5: Update the sidebar "Contact groups" section to show actual groups**

**Step 6: Build and test**

---

### Task 7: Add Clone Contact action

**Objective:** Add "Clone contact" to the contact actions menu.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Step 1: Add cloneContact() function**

```javascript
async function cloneContact(){
  if(!selectedId)return;
  let c=contacts.find(x=>x.id===selectedId);
  let clone={...c};
  delete clone.id;
  let r=await api("POST","/create",clone);
  if(r.id){selectedId=r.id;await loadContacts();showView()}
}
```

**Step 2: Add clone button to detail view**

Add a clone icon/button next to Edit in the detail view header.

**Step 3: Build and test**

---

### Task 8: Add Generate QR Code action

**Objective:** Add QR code generation for contact vCard data.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Step 1: Add showQRCode() function**

Generate QR code client-side using a lightweight QR library added via CDN, or generate via API:
```javascript
function showQRCode(){
  if(!selectedId)return;
  let c=contacts.find(x=>x.id===selectedId);
  let vcard='BEGIN:VCARD\nVERSION:3.0\nFN:'+(c.first_name||'')+' '+(c.last_name||'')+'\n...';
  // Use a QR code CDN or canvas-based generator
  let qrUrl='https://api.qrserver.com/v1/create-qr-code/?size=200x200&data='+encodeURIComponent(vcard);
  // Show modal with QR image
}
```

**Step 2: Add QR button to detail view actions**

**Step 3: Build and test**

---

### Task 9: Add Copy-to-clipboard per field in detail view

**Objective:** Add copy buttons next to each field value in the detail view (matching Nextcloud).

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (showView)

**Step 1: Add copy buttons to showView() field rendering**

After each field value, add a copy icon:
```javascript
h+='<button onclick="copyText(\''+esc(p.value)+'\')" class="..."><i class="fa-regular fa-copy"></i></button>';
```

**Step 2: Add copyText() helper**

```javascript
function copyText(text){
  navigator.clipboard.writeText(text).then(()=>{
    // Brief toast/feedback
  });
}
```

**Step 3: Build and test**

---

### Task 10: Add Download single contact action

**Objective:** Add "Download" to contact actions menu that downloads the contact as .vcf.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

**Step 1: Add downloadContact() function**

```javascript
function downloadContact(){
  if(!selectedId)return;
  window.open(apiBase+'/export?id='+selectedId,'_blank');
}
```

**Step 2: Add /export?id=X endpoint to daemon**

In `cmd/contacts/main.go`, modify handleExport to support `?id=` parameter for single contact export.

**Step 3: Add Download button to detail view actions**

**Step 4: Build and test**

---

### Task 11: Switch to central "Name" field (Nextcloud-style form)

**Objective:** Make the form use a single "Name" field by default, with detailed name fields hidden behind "Show all fields" toggle.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (contactsHTML form, saveEdit, startEdit, syncFullNameEdit)

**Step 1: Reorder form fields**

Move the "Full name" field to the top of the form (it's already there). Keep Title and Company immediately below (like Nextcloud). Move Detailed Name (Prefix, First, Last, Suffix) behind the "Show all fields" toggle.

**Step 2: Update saveEdit() to parse Name field into first/last**

When saving, if the Name field has a value, split it into first_name and last_name:
```javascript
let fullName=document.getElementById("edit-fn").value.trim();
if(fullName && !document.getElementById("edit-first").value){
  let parts=fullName.split(' ');
  data.first_name=parts[0];
  data.last_name=parts.slice(1).join(' ');
}
```

**Step 3: Build and test**

---

### Task 12: Add per-contact Actions menu (3-dot menu)

**Objective:** Consolidate Clone, QR, Download, Delete into a dropdown actions menu on the detail view header.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (showView, contactsHTML)

**Step 1: Add 3-dot menu button to the detail view header HTML**

After the Edit button, add:
```html
<button onclick="openContactMenu(event)" class="..."><i class="fa-solid fa-ellipsis-vertical"></i></button>
<div id="contact-menu" class="hidden absolute ...">
  <button onclick="cloneContact()">Clone contact</button>
  <button onclick="showQRCode()">Generate QR Code</button>
  <button onclick="downloadContact()">Download</button>
  <button onclick="deleteSelected()">Delete</button>
</div>
```

**Step 2: Add openContactMenu() toggle function**

**Step 3: Build and test**

---

### Phase 3: Polish

---

### Task 13: Add "Add more info" per-field add buttons (Nextcloud pattern)

**Objective:** Replace the "Show all fields" toggle with per-section "+" add buttons that reveal additional field types.

**Files:**
- Modify: `internal/apps/contacts/handlers.go`

This is a nice-to-have UX improvement. The current "Show all fields" toggle works but Nextcloud's pattern of "Add property of this type" buttons per section is more elegant. Implement for Phone, Email, Address sections with a "+ Add more info" button that reveals Website, Birthday, Notes.

**Decision:** This can be done as a follow-up if time permits. The current toggle is functional.

---

### Task 14: Add mobile detail modal improvements

**Objective:** Improve mobile responsiveness — the detail modal should have proper close behavior and transitions.

**Files:**
- Modify: `internal/apps/contacts/handlers.go` (mobile detail modal)

The current mobile modal exists but needs polish for close-on-backdrop-click and smooth transitions.

---

### Task 15: Final verification — run all tests

```bash
go build ./cmd/...
go test ./internal/... -v -count=1
```

Verify:
- All existing contacts tests still pass
- Manual browser test: create contact, edit, clone, star, add to group, search, delete

---

## Summary

| Phase | Tasks | Effort |
|-------|-------|--------|
| Phase 0: Sidebar & Settings Restructure | Tasks 0.1-0.4 | ~2 hours |
| Phase 1: Bug Fixes & Field Extensions | Tasks 1.1-1.2 | ~30 min |
| Phase 2: Missing Features | Tasks 4-12 | ~3-4 hours |
| Phase 3: Polish | Tasks 13-15 | ~1 hour |

**Total: ~7-8 hours of implementation**

### Risk: The contacts daemon stores data as flat .vcf files per user directory. Groups and favorites are stored as contact properties in the vCard (X-DAR-GROUPS, X-DAR-FAVORITE). No DB migration needed.

### Key architectural changes from this plan:
1. Sidebar is no longer an address book switcher — it's a filter/groups/teams nav with settings footer
2. Address book management moves entirely to Settings modal under a tabbed interface
3. "New contact" no longer creates a DB record before the user fills the form
