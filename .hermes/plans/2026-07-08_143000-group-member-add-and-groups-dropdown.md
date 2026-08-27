# Contacts: Group "Add Member" + Groups Multi-Select Dropdown

> **For Hermes:** Use dar-app-modification skill to implement this plan one change at a time, testing in browser after each change.

**Goal:** Add "Add member" to group 3-dot menu + replace comma-separated groups text input with a multi-select dropdown of existing groups.

**Architecture:** Both features are pure frontend changes in `internal/apps/contacts/handlers.go` — no daemon changes needed. The backend already stores groups as `CATEGORIES` in vCard and the API accepts a `groups: []string` field.

**Tech Stack:** Go `html/template`, vanilla JS, Tailwind CSS CDN, Font Awesome 6 CDN.

**Reference:** Nextcloud Contacts UI — groups are a multi-select taggable dropdown; group 3-dot menu has rename/delete; when creating a new group, Nextcloud auto-opens an "Add members" dialog.

---

## Current State (handlers.go)

- **Line 84-87:** Group 3-dot menu has Rename + Delete only
- **Line 142:** `edit-groups` is a plain `<input>` with placeholder "Groups (comma-separated)"
- **Lines 448-457:** `showCreate()` pre-fills `edit-groups` with activeGroup if filtering
- **Lines 485-497:** `editCurrent()` fills `edit-groups` with `c.groups.join(", ")`
- **Lines 508-520:** `saveContact()` splits `edit-groups` value by comma to build `data.groups`
- **Lines 298-335:** `extractGroups()` + `renderGroupList()` manages group array
- **Lines 593-650:** `toggleGroupItemMenu` / `renameGroup` / `deleteGroup` for group 3-dot menu

---

## Task 1: Add "Add member" button to group 3-dot menu

**Objective:** Add a third option in the group-item-menu dropdown to assign contacts to a group.

**Files:** Modify: `internal/apps/contacts/handlers.go:84-87`

**Step 1: Add "Add member" button to the menu HTML**

Change the group-item-menu div (lines 84-87):
```html
<div id="group-item-menu" class="hidden fixed z-50 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 min-w-[150px]">
    <button onclick="renameGroup()" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-pen w-4 text-xs"></i> Rename</button>
    <button onclick="deleteGroup()" class="w-full text-left px-3 py-2 text-sm text-error hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-trash w-4 text-xs"></i> Delete</button>
</div>
```

To:
```html
<div id="group-item-menu" class="hidden fixed z-50 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 min-w-[180px]">
    <button onclick="addMembersToGroup()" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-user-plus w-4 text-xs"></i> Add member</button>
    <hr class="border-outline-variant my-1">
    <button onclick="renameGroup()" class="w-full text-left px-3 py-2 text-sm text-on-surface hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-pen w-4 text-xs"></i> Rename</button>
    <button onclick="deleteGroup()" class="w-full text-left px-3 py-2 text-sm text-error hover:bg-surface-container-high border-0 bg-transparent cursor-pointer flex items-center gap-2"><i class="fa-solid fa-trash w-4 text-xs"></i> Delete</button>
</div>
```

**Step 2: Add the "Add Members" modal HTML**

Add after `</div>` closing the group-item-menu (after line 87):

```html
<!-- Add members to group modal -->
<div id="add-members-modal" class="hidden fixed inset-0 bg-black/30 z-50 flex items-center justify-center p-4" onclick="if(event.target===this)closeAddMembersModal()">
    <div class="bg-surface rounded-2xl w-full max-w-md shadow-2 flex flex-col max-h-[80vh]">
        <div class="flex items-center justify-between p-4 pb-2 shrink-0">
            <h2 class="text-lg font-semibold" id="add-members-title">Add members</h2>
            <button onclick="closeAddMembersModal()" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant cursor-pointer hover:bg-surface-container-highest flex items-center justify-center">
                <i class="fa-solid fa-xmark"></i>
            </button>
        </div>
        <div class="px-4 pb-2 shrink-0">
            <input id="add-members-search" type="text" placeholder="Search contacts..." 
                class="w-full h-10 px-4 rounded-full bg-surface-container text-sm border-0 outline-none focus:ring-2 focus:ring-primary/30"
                oninput="filterAddMemberList()">
        </div>
        <div id="add-members-list" class="overflow-y-auto flex-1 px-4 py-2 space-y-1"></div>
        <div class="flex justify-end gap-2 p-4 pt-3 border-t border-outline-variant shrink-0">
            <button onclick="closeAddMembersModal()" class="h-10 px-4 rounded-full text-sm font-medium text-on-surface-variant border-0 bg-transparent cursor-pointer hover:bg-surface-container-highest">Cancel</button>
            <button onclick="saveGroupMembers()" class="h-10 px-4 rounded-full bg-primary text-white text-sm font-medium border-0 cursor-pointer hover:opacity-90">Add</button>
        </div>
    </div>
</div>
```

**Step 3: Add JS functions for the modal**

Add these JS functions (place after `deleteGroup()`, around line 650):

```javascript
let addMembersGroup = null;
let addMembersChecked = {};

function addMembersToGroup() {
    addMembersGroup = activeGroupItem;
    closeGroupItemMenu();
    if (!addMembersGroup) return;
    
    document.getElementById("add-members-title").textContent = "Add members to " + addMembersGroup;
    document.getElementById("add-members-modal").classList.remove("hidden");
    document.getElementById("add-members-search").value = "";
    addMembersChecked = {};
    renderAddMembersList(contacts);
}

function closeAddMembersModal() {
    document.getElementById("add-members-modal").classList.add("hidden");
    addMembersGroup = null;
}

function renderAddMembersList(list) {
    var html = "";
    for (var i = 0; i < list.length; i++) {
        var c = list[i];
        // Skip contacts already in this group
        if (c.groups && c.groups.indexOf(addMembersGroup) !== -1) continue;
        var name = (c.first_name||"") + " " + (c.last_name||"");
        name = name.trim() || c.org || c.email || "Unnamed";
        var init = (c.first_name||name)[0];
        var checked = addMembersChecked[c.id] ? " checked" : "";
        html += '<label class="flex items-center gap-3 p-2 rounded-lg hover:bg-surface-container-high cursor-pointer">';
        html += '<input type="checkbox" value="'+c.id+'"'+checked+' onchange="addMembersChecked[\''+c.id+'\']=this.checked" class="w-4 h-4 accent-primary">';
        html += '<div class="w-8 h-8 rounded-full inline-flex items-center justify-center text-white text-xs font-bold shrink-0" style="background:'+(c.color||"#6750a4")+'">'+init+'</div>';
        html += '<div class="flex-1 min-w-0"><div class="text-sm text-on-surface truncate">'+esc(name)+'</div>';
        if (c.email) html += '<div class="text-xs text-on-surface-variant truncate">'+esc(c.email)+'</div>';
        html += '</div></label>';
    }
    if (!html) html = '<div class="text-center text-on-surface-variant py-8 text-sm">All contacts are already in this group.</div>';
    document.getElementById("add-members-list").innerHTML = html;
}

function filterAddMemberList() {
    var q = document.getElementById("add-members-search").value.toLowerCase();
    if (!q) { renderAddMembersList(contacts); return; }
    var filtered = contacts.filter(function(c) {
        return (c.first_name+" "+c.last_name+" "+c.email).toLowerCase().indexOf(q) !== -1;
    });
    renderAddMembersList(filtered);
}

function saveGroupMembers() {
    if (!addMembersGroup) return;
    var promises = [];
    for (var id in addMembersChecked) {
        if (!addMembersChecked[id]) continue;
        var c = contacts.find(function(x){ return x.id === id; });
        if (!c) continue;
        if (!c.groups) c.groups = [];
        if (c.groups.indexOf(addMembersGroup) === -1) {
            c.groups.push(addMembersGroup);
        }
        promises.push(api("POST", "/update", {id: c.id, groups: c.groups.slice()}));
    }
    Promise.all(promises).then(function() {
        closeAddMembersModal();
        loadContacts();
    });
}
```

**Step 4: Build and browser-test**

```bash
cd /home/anas/dar
nix-shell -p go --run "go build -o /tmp/dar-server ./cmd/server/"

# Kill old server, restart with full OIDC stack
pkill -f "dar-server" 2>/dev/null
pkill -f "dar-test-oidc" 2>/dev/null
pkill -f "dar-contacts" 2>/dev/null

/tmp/dar-test-oidc --port 9001 --host localhost &
sleep 1
/tmp/dar-contacts --port 8083 --data-root /tmp/dar_data &
sleep 1
env DAR_OIDC_ISSUER="http://localhost:9001" DAR_OIDC_CLIENT_ID="test-client" DAR_OIDC_CLIENT_SECRET="test-secret" DAR_OIDC_DISPLAY_NAME="TestOIDC" DAR_BASE_URL="http://localhost:8080" DAR_CONTACTS_PORT="8083" /tmp/dar-server --port 8080 --db /tmp/dar_test2.db &
```

Browser test:
1. Login at http://localhost:8080/login
2. Navigate to http://localhost:8080/contacts
3. Create a group via [+] button → name it "Test Group"
4. Hover over the group → click 3-dot ⋮
5. Verify "Add member" appears in the menu
6. Click "Add member" → modal opens
7. Verify contact list shows non-grouped contacts
8. Check a contact, click "Add"
9. Verify contact is assigned

---

## Task 2: Replace comma-separated groups input with multi-select dropdown

**Objective:** Replace the free-text `edit-groups` input with a dropdown showing all existing groups with checkboxes.

**Files:** Modify: `internal/apps/contacts/handlers.go:142`

**Step 1: Replace the input with dropdown UI**

Replace line 142:
```html
<input id="edit-groups" placeholder="Groups (comma-separated)" class="w-full h-10 px-4 rounded-lg bg-surface-container text-sm border border-outline-variant outline-none focus:ring-2 focus:ring-primary/30">
```

With:
```html
<div class="relative">
    <div id="groups-dropdown" class="w-full min-h-[40px] px-3 py-1.5 rounded-lg bg-surface-container text-sm border border-outline-variant outline-none focus-within:ring-2 focus-within:ring-primary/30 cursor-pointer flex flex-wrap gap-1 items-center" onclick="toggleGroupsDropdown(event)">
        <span id="groups-placeholder" class="text-on-surface-variant text-sm">Select groups...</span>
        <div id="groups-tags" class="flex flex-wrap gap-1"></div>
        <i class="fa-solid fa-chevron-down text-on-surface-variant text-xs ml-auto"></i>
    </div>
    <div id="groups-dropdown-menu" class="hidden absolute top-full left-0 right-0 mt-1 z-40 bg-surface rounded-xl shadow-2 border border-outline-variant py-1 max-h-[200px] overflow-y-auto">
        <div id="groups-dropdown-list"></div>
        <div id="groups-dropdown-empty" class="hidden px-3 py-2 text-xs text-on-surface-variant">No groups yet. Create one from the sidebar.</div>
    </div>
</div>
```

**Step 2: Add the JS to manage group selection state**

Add after the groups array declaration (line 211):
```javascript
let selectedGroups = [];
```

**Step 3: Add JS functions for the dropdown**

Add these functions (before `showCreate`, near line 447):

```javascript
function toggleGroupsDropdown(e) {
    e.stopPropagation();
    var menu = document.getElementById("groups-dropdown-menu");
    menu.classList.toggle("hidden");
    if (!menu.classList.contains("hidden")) renderGroupsDropdown();
}

function closeGroupsDropdown() {
    document.getElementById("groups-dropdown-menu").classList.add("hidden");
}

function renderGroupsDropdown() {
    var list = document.getElementById("groups-dropdown-list");
    var empty = document.getElementById("groups-dropdown-empty");
    if (!groups.length) {
        list.innerHTML = "";
        empty.classList.remove("hidden");
        return;
    }
    empty.classList.add("hidden");
    var html = "";
    for (var i = 0; i < groups.length; i++) {
        var g = groups[i];
        var checked = selectedGroups.indexOf(g) !== -1 ? " checked" : "";
        html += '<label class="flex items-center gap-2 px-3 py-1.5 hover:bg-surface-container-high cursor-pointer">';
        html += '<input type="checkbox" value="'+esc(g)+'"'+checked+' onchange="toggleGroupCheckbox(this)" class="w-4 h-4 accent-primary">';
        html += '<span class="text-sm text-on-surface">'+esc(g)+'</span></label>';
    }
    list.innerHTML = html;
}

window._gidx = 0;

function toggleGroupCheckbox(cb) {
    var g = cb.value;
    if (cb.checked) {
        if (selectedGroups.indexOf(g) === -1) selectedGroups.push(g);
    } else {
        selectedGroups = selectedGroups.filter(function(x){ return x !== g; });
    }
    renderGroupsTags();
}

function renderGroupsTags() {
    var tags = document.getElementById("groups-tags");
    var placeholder = document.getElementById("groups-placeholder");
    var html = "";
    for (var i = 0; i < selectedGroups.length; i++) {
        var g = selectedGroups[i];
        html += '<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-primary-container text-primary text-xs font-medium">'+esc(g);
        html += '<button onclick="event.stopPropagation();removeGroupTag(\''+esc(g)+'\')" class="w-4 h-4 rounded-full border-0 bg-transparent text-primary cursor-pointer hover:bg-primary/20 flex items-center justify-center text-[10px]"><i class="fa-solid fa-xmark"></i></button>';
        html += '</span>';
    }
    tags.innerHTML = html;
    placeholder.style.display = selectedGroups.length ? "none" : "";
}

function removeGroupTag(name) {
    selectedGroups = selectedGroups.filter(function(x){ return x !== name; });
    renderGroupsTags();
    // Update dropdown checkboxes if open
    var menu = document.getElementById("groups-dropdown-menu");
    if (!menu.classList.contains("hidden")) renderGroupsDropdown();
}

function setSelectedGroups(arr) {
    selectedGroups = (arr || []).slice();
    renderGroupsTags();
}
```

**Step 4: Update `showCreate()` to use `setSelectedGroups()`**

In `showCreate()` (around line 451), change:
```javascript
document.getElementById("edit-groups").value = defaultGroups;
```
To:
```javascript
setSelectedGroups(defaultGroups ? [defaultGroups] : []);
```

**Step 5: Update `editCurrent()` to use `setSelectedGroups()`**

In `editCurrent()` (around line 494), change:
```javascript
document.getElementById("edit-groups").value = (c.groups||[]).join(", ");
```
To:
```javascript
setSelectedGroups(c.groups || []);
```

**Step 6: Update `saveContact()` to read from `selectedGroups`**

In `saveContact()` (around line 516), change:
```javascript
groups: (document.getElementById("edit-groups").value||"").split(",").map(function(s){return s.trim()}).filter(function(s){return s!==""}),
```
To:
```javascript
groups: selectedGroups.slice(),
```

**Step 7: Add click-outside listener for groups dropdown**

Add to the existing document click listener (around lines 653-657):
```javascript
if (!e.target.closest("#groups-dropdown")) closeGroupsDropdown();
```

The full listener becomes:
```javascript
document.addEventListener("click", function(e) {
    if (!e.target.closest("#book-menu")) document.getElementById("book-menu").classList.add("hidden");
    if (!e.target.closest("#group-menu") && !e.target.closest("[onclick*='toggleGroupMenu']")) document.getElementById("group-menu").classList.add("hidden");
    if (!e.target.closest("#group-item-menu")) closeGroupItemMenu();
    if (!e.target.closest("#groups-dropdown")) closeGroupsDropdown();
});
```

**Step 8: Build and browser-test**

Same setup as Task 1. Then:
1. Navigate to /contacts
2. Click "New contact" → form opens
3. Verify groups dropdown shows "Select groups..." placeholder
4. Click dropdown → all existing groups appear with checkboxes
5. Check 1-2 groups → tags appear above dropdown
6. Fill name + email, click Save
7. Open the contact → click Edit → verify groups pre-selected
8. Uncheck a group → tag disappears
9. Save → verify groups updated in detail view
10. Check console for zero JS errors

---

## Validation

- `go build ./cmd/...` must succeed for both server and contacts daemon
- Browser DevTools console: zero JS errors
- Create contact with groups → reload → groups still present (persisted via vCard CATEGORIES)
- Add members to group via 3-dot menu → contacts appear in group filter
- Select/deselect groups in edit form → saved correctly

## Risks / Tradeoffs

1. **Search in Add Members modal:** Simple client-side filter against in-memory `contacts[]`. Works for small contact lists (<1000). For large lists, would need server-side search — out of scope.
2. **No create-group-on-the-fly in dropdown:** Unlike Nextcloud's taggable multi-select, our dropdown only shows existing groups. Users must create groups from the sidebar [+] first. This is simpler and avoids accidentally creating typos as groups.
3. **Modal uses `confirm()` pattern?** No — both modals use inline DOM elements, avoiding the browser tool blocking issue (pitfall #14 in dar-app-modification skill).
4. **Groups persist via vCard CATEGORIES:** Backend already handles this — no changes to the daemon needed. Groups are just string tags on contacts.

## Files Changed

| File | Changes |
|---|---|
| `internal/apps/contacts/handlers.go` | Add member modal HTML + JS, replace edit-groups input with dropdown, update saveContact/showCreate/editCurrent |
