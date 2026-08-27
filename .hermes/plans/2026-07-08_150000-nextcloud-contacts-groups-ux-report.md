# Nextcloud Contacts App — Groups UX Report

> **Goal:** Document exactly how Nextcloud's Contacts app handles "Contact Groups" at the UX level, as a reference for Dar's contacts group implementation.

**Source:** Nextcloud Contacts app (main branch, GitHub) + official Nextcloud 34 user manual.

---

## 1. Sidebar Navigation: Groups as First-Class Filters

The left sidebar has this vertical structure, top to bottom:

```
┌──────────────────────────┐
│  [+ New contact] button  │
├──────────────────────────┤
│  👤 All contacts  (42)   │  <-- always visible, active by default
│  📊 Organization chart   │  <-- shown only if org data exists
│  👤 Not grouped   (7)    │  <-- shown only if count > 0
│  🕐 Recently contacted   │  <-- shown if contacts interaction enabled
├──────────────────────────┤
│  CONTACT GROUPS  [+]     │  <-- section caption with + create button
│    ├ Family              │  <-- each group is a clickable nav item
│    ├ Work                │
│    └ Sports              │
├──────────────────────────┤
│  ⚙ Contacts settings     │  <-- bottom footer, gear icon
└──────────────────────────┘
```

**Key observations:**

- "All contacts" is always present and is the default view.
- "Not grouped" only appears when there are ungrouped contacts (count > 0). This is a dynamically-filtered view, not a persistent "group."
- "Recently contacted" is only shown if the contacts interaction app is enabled.
- The "Contact groups" section header is always visible, even with zero groups.
- Individual groups are listed alphabetically under the header.
- If there are more than 5 groups, a "Show all groups" / "Show less" toggle collapses the list.
- Each group item shows the group name — no count badge. (Count/contact badges are only on "All contacts" and "Not grouped".)
- Clicking any group filters the main contact list to show only contacts in that group.
- The URL updates to reflect the selected group: `/apps/contacts/Family` or `/apps/contacts/Family/contactKey`

---

## 2. Creating a Group (Sidebar)

**Workflow:**

1. Click the `[+]` icon next to the "Contact groups" section caption.
2. An inline input field slides open below the caption:
   - Placeholder text: "Contact group name"
   - A help line above it: "Create a new contact group"
   - An error icon shows if creation fails
3. Type a name and press Enter.
4. The group appears in the sidebar list (alphabetically sorted).
5. The inline input closes.

**Important UX detail:** A group with zero members does NOT persist. Groups are derived/aggregated from the vCard `CATEGORIES` property of contacts. If all contacts are removed from a group, the group disappears from the sidebar.

---

## 3. Group Management in Contact Details (Read-Only Mode)

When viewing a contact's details:

- Groups are listed in a dedicated "Contact groups" section.
- **Read-only display:** Group names shown as comma-separated text: `Family, Work, Sports`
- If no groups: shows "None"
- The section only appears if the contact has groups (read-only mode) OR is always visible in edit mode.

---

## 4. Group Management in Contact Edit Mode

When editing a contact:

- The "Contact groups" field is a **multi-select taggable dropdown** (NcSelect component).
- **Adding groups:**
  - Click the field, a dropdown opens showing all existing groups (alphabetically sorted).
  - Select existing groups by clicking/checking them.
  - **Create new groups on the fly:** Type a name not in the list and it offers to create it as a tag (taggable mode).
  - Deselect groups by clicking the X on their tag or unchecking in the dropdown.
- **Visual display:**
  - Selected groups appear as tags/chips inside the input field.
  - If more than 3 groups are selected, shows `+N` with a tooltip listing the overflow group names.
  - The dropdown stays open after selection for multiple picks (`keep-open`).
- **Automatic group assignment:**
  - When creating a **new contact** while a group filter is active in the sidebar, the new contact is automatically assigned to that group.

---

## 5. Group as vCard Data (`CATEGORIES` Property)

Groups are **not separate entities** — they're stored as the vCard `CATEGORIES` property on each contact.

```javascript
// Contact model (from contact.js)
get groups() {
  const groupsProp = this.vCard.getFirstProperty('categories')
  if (groupsProp) {
    return groupsProp.getValues()
      .filter(g => typeof g === 'string')
      .filter(g => g.trim() !== '')
  }
  return []
}

set groups(groups) {
  if (isEmpty(groups)) {
    this.vCard.removeProperty('categories')
    return
  }
  let property = this.vCard.getFirstProperty('categories')
  if (!property) {
    property = this.vCard.addPropertyWithValue('categories', '')
  }
  property.setValues(groups)
}
```

**Implications:**
- Groups sync via CardDAV as part of each contact.
- Deleting a contact removes it from its groups automatically.
- There is no server-side "groups table" — groups are a client-side aggregation.

---

## 6. Store Layer (Vuex `groups.js`)

The client-side store maintains a computed group list:

```javascript
state: {
  groups: [
    { name: "Family", contacts: ["key1", "key2"] },
    { name: "Work",   contacts: ["key3"] }
  ]
}
```

**Key operations:**

| Operation | Description |
|---|---|
| `extractGroupsFromContacts(contacts)` | On initial load, scans all contacts' CATEGORIES to build group list |
| `addContactToGroups({groupNames, contact})` | Adds contact to specified groups; creates group if new |
| `removeContactFromGroup({groupName, contact})` | Removes contact from one group |
| `removeContactFromGroups(contact)` | Removes contact from all groups (used on delete) |
| `renameGroup({oldGroupName, newGroupName})` | Renames a group across all contacts (mutates vCard CATEGORIES) |
| `addGroup(groupName)` | Creates empty group `{ name, contacts: [] }` |
| `removeGroup(groupName)` | Deletes group from store (does not touch contacts) |

**When a contact's CATEGORIES change** (user edits groups in the detail panel):
1. The updated vCard is saved to the CardDAV server.
2. The store dispatches `updateContactGroups` which removes the contact from all groups, then re-adds it to the new set.
3. The sidebar re-renders to reflect the change.

---

## 7. Filtering & Routing

The URL structure encodes the active group:

```
/:selectedGroup                  →  Contacts.vue, list filtered by group
/:selectedGroup/:selectedContact →  Contacts.vue, list + detail pane
```

Special group constants:
- `GROUP_ALL_CONTACTS` = "All contacts" (i18n key)
- `GROUP_NO_GROUP_CONTACTS` = "Not grouped" (i18n key)
- `GROUP_RECENTLY_CONTACTED` = "Recently contacted"

Filtering is computed client-side:
```javascript
contactsList() {
  if (this.selectedGroup === GROUP_ALL_CONTACTS)      return this.sortedContacts
  if (this.selectedGroup === GROUP_NO_GROUP_CONTACTS)  return this.ungroupedContacts
  const group = this.groups.find(g => g.name === this.selectedGroup)
  if (group) {
    return this.sortedContacts.filter(c => group.contacts.includes(c.key))
  }
  return []
}
```

---

## 8. UX Behaviors Summary

| Behavior | Detail |
|---|---|
| **Group persistence** | Groups exist only through contact data (vCard CATEGORIES). Empty groups vanish. |
| **Creating a group** | Two ways: (a) `[+]` in sidebar creates an empty group placeholder, (b) tagging during contact edit creates group + assigns contact in one step. |
| **Renaming a group** | The GroupNavigationItem has a 3-dot menu with rename option. Renaming updates all contacts' CATEGORIES for that group. |
| **Deleting a group** | GroupNavigationItem 3-dot menu has delete. Removes the group name from the store. Does NOT modify contacts. |
| **Group count badge** | NOT shown on individual groups in sidebar — only on "All contacts" and "Not grouped." |
| **Auto-assignment** | Creating a new contact while a group is selected auto-assigns it to that group. |
| **Multi-select in edit** | The contact edit form uses a taggable multi-select: pick existing groups or type-to-create. |
| **Read-only address books** | Contacts from read-only address books (e.g., system address book) cannot be added to groups. |
| **Mobile** | Groups are collapsed behind a navigation toggle. |

---

## 9. Dar Current State vs Nextcloud (Gap Analysis)

Dar has a bare-bones group implementation in `internal/apps/contacts/handlers.go`:

### What Dar Has:
- Sidebar with "All contacts", "Not grouped", and "Contact groups" section header
- `[+]` button next to "Contact groups" to create via inline input
- Groups stored as a JS array in memory (`let groups = []`)
- `selectGroup(name)` and `selectFilter(f)` for basic filtering
- `renderGroupList()` renders groups in sidebar
- Contact data has a `groups` array field (from API)

### What Dar Is Missing:
1. **Groups in contact detail view** — Groups not shown when viewing a contact
2. **Groups in contact edit/create form** — No way to assign groups when editing/creating
3. **Multi-select taggable group picker** — No taggable dropdown for group assignment
4. **Group auto-assignment** — Creating a contact while group is selected doesn't auto-assign
5. **Group renaming** — No way to rename a group
6. **Group deletion** — No way to delete a group
7. **Group count in "Not grouped"** — Counter always shows total contacts, not actual ungrouped count
8. **Persistence** — Groups are ephemeral JS array, not synced server-side
9. **"Not grouped" only shows when count > 0** — Dar always shows it
10. **Group nav item has no 3-dot menu** — No rename/delete actions
11. **Group links don't update URL** — No route-based filtering
12. **vCard CATEGORIES roundtrip** — API doesn't appear to persist groups to vCard

---

## 10. Recommended Implementation Order for Dar

1. **Persist groups in vCard CATEGORIES** — back the `groups` field with actual CardDAV data
2. **Show groups in contact detail view** — readonly display, comma-separated
3. **Add groups field to edit/create form** — simple text input at first, evolve to taggable multi-select
4. **Auto-assign group when creating from filtered view**
5. **Group rename and delete via 3-dot menu** on sidebar items
6. **Hide "Not grouped" when count is 0**
7. **Fix ungrouped counter** to show actual ungrouped contacts
8. **URL-based routing** for group selection
