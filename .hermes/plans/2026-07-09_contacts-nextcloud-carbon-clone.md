# Dar Contacts → Nextcloud Carbon Clone Plan

**Date:** 2026-07-09  
**Goal:** Make Dar Contacts a carbon clone of Nextcloud Contacts in features and UX (Material 3 / Tailwind, server-rendered + vanilla JS, no new deps beyond `github.com/emersion/go-vcard` already allowed).  
**Reference:** Live Nextcloud at `https://hub.asherif.xyz/apps/contacts/` (logged in as `test`).  
**Dar sources:** `cmd/contacts/main.go`, `internal/apps/contacts/handlers.go`, `internal/carddav/` (empty).

---

## Context

Dar already has a **working mini-SPA** that *looks* Nextcloud-adjacent:

| Area | Status |
|------|--------|
| 3-pane layout (sidebar / list / detail) | Done |
| New contact, edit, delete, clone, download | Done |
| Search, A–Z headers, favorites star | Done |
| Groups via `CATEGORIES` (+ not grouped, add members) | Done |
| Settings modal shell (General / Address books) | Shell only |
| Import `.vcf` | Works (raw write) |
| Export single `.vcf` | Works |
| Real multi-value vCard / address books / CardDAV / PHOTO / QR | **Missing** |

Nextcloud Contacts is a **CardDAV client UI** over full vCards. Dar is a **flat REST + simplified vCard subset**. Closing the gap is mostly:

1. **Rich contact model + real vCard I/O** (biggest data gap)
2. **Real address books** (biggest structural gap)
3. **UX polish** to match NC field editor / actions menu / detail layout
4. **CardDAV** for phone/client sync (largest protocol gap; can ship after UI parity)

Existing sample data under `data/default/addressbooks/default/*.vcf` is **real multi-field v4 cards** (DAVx5) that the current daemon **does not read**.

---

## Live Nextcloud inventory (hub.asherif.xyz)

### Layout
```
┌ Sidebar ──────────┬ List ────────────┬ Detail ─────────────────────┐
│ + New contact     │ Search contacts  │ Avatar · Name · ⋯ Actions   │
│ All contacts  (N) │ A                │ Quick actions (Talk/Mail)   │
│ Not grouped   (N) │  contact rows    │ Typed fields (Email, Phone…)│
│ Contact groups [+]│                  │ Address book picker         │
│ Teams          [+]│                  │ Shared items (empty state)  │
│ …                 │                  │ Last modified               │
│ ⚙ Contacts settings (pinned footer) │                             │
└───────────────────┴──────────────────┴─────────────────────────────┘
```

### Contact editor (new contact defaults)
- **Name** (single FN field, not split first/last by default)
- **Title**, **Company**
- **Phone** (typed: Home/Work/Cell…) + “Add property of this type”
- **Email** (typed) + add another
- **Address** (typed) with: PO box, street, extended, postal code, city, state, country
- **Notes**
- **Contact groups** (searchable multi-add)
- **Add more info** menu:
  - Phone, Email, Address, Birthday, Website, Social network, Line manager, Notes, **More fields**
- Avatar upload + crop modal
- Save / Cancel; auto-save feel with last-modified timestamp

### Detail actions menu (⋯)
- Download
- Clone contact
- Generate QR Code
- Exclude contact from Birthday Calendar
- Delete

### Sidebar extras
- **Teams** section (“Create your own teams for sharing…”) — Circles-style sharing
- Settings: sort, social avatar sync, import, address books (copy CardDAV link, download, show/hide, rename, delete, new)

### Property types NC supports (from UI + example contact Leon Green)
| Property | Types / notes |
|----------|----------------|
| FN / N (detailed name) | Name + optional detailed name (prefix/first/middle/last/suffix) |
| TITLE, ORG | Title + Company |
| TEL | Home / Work / Cell / … multi |
| EMAIL | Home / Work / Other multi |
| ADR | Full structured address multi |
| BDAY | Birthday (personal dates) |
| URL | Website |
| Social network | e.g. Nextcloud profile URL |
| NOTE | Notes |
| CATEGORIES | Contact groups |
| PHOTO | Avatar |
| Related / Line manager | Organizational |
| X- / Cloud ID | Federated Cloud ID (NC-specific) |
| Address book membership | Which book owns the contact |

---

## Gap matrix: Nextcloud vs Dar (current)

Legend: ✅ done · 🟡 partial / shell · ❌ missing

### A. Shell & navigation
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Full-width 3-pane app | ✅ | ✅ | — |
| New contact primary button | ✅ | ✅ | — |
| All contacts + count | ✅ | ✅ | — |
| Not grouped + count | ✅ | ✅ | Hide when 0 already |
| Contact groups + create/rename/delete/add members | ✅ | ✅ | Empty groups vanish on reload (no empty CATEGORIES store) |
| Teams / Circles sharing | ✅ | ❌ | Large; needs sharing model |
| Settings pinned footer | ✅ | ✅ | — |
| URL state (`/contacts/…/id`) | ✅ | ❌ | Deep-link / reload loses selection |
| Mobile list→detail stack | ✅ | 🟡 | Detail only `lg:flex`; weak mobile nav |

### B. List
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Search | ✅ | 🟡 | Client-side only; fields limited |
| A–Z section headers | ✅ | ✅ | Sort key should use FN/last per settings |
| Favorites star | ✅ | ✅ | Works via `X-DAR-FAVORITE` |
| Sort by display / last / first | ✅ | 🟡 | Select exists, **not wired** |
| Filter by team | ✅ | ❌ | Teams N/A |
| Row subtitle (email) | ✅ | 🟡 | Email only if present |

### C. Detail / actions
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Avatar + color initial | ✅ | 🟡 | Color not persisted; no PHOTO |
| Edit / Clone / Download / Delete | ✅ | 🟡 | Dar uses 4 buttons; NC uses Edit + ⋯ menu |
| QR code | ✅ | ❌ | Missing |
| Birthday calendar exclude | ✅ | ❌ | Needs Calendar integration |
| Copy field value | ✅ | ✅ | Email/phone only |
| mailto / tel links | ✅ | ✅ | — |
| Typed multi-value fields | ✅ | ❌ | Single email/phone; no ADR/URL/BDAY |
| Notes in UI | ✅ | ❌ | Backend has `Notes`; form omits it |
| Title / role | ✅ | ❌ | — |
| Last modified | ✅ | 🟡 | `UpdatedAt` exists, not shown |
| Address book on contact | ✅ | ❌ | No real books |
| Shared items section | ✅ | ❌ | Sharing N/A |

### D. Editor UX
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Single “Name” field by default | ✅ | ❌ | Split first/last only |
| Detailed name expand | ✅ | ❌ | — |
| Title + Company | ✅ | 🟡 | Org only |
| Multi phone/email/address + type chips | ✅ | ❌ | — |
| Add property / Add more info | ✅ | ❌ | Fixed form |
| Groups multi-select | ✅ | ✅ | Good enough |
| Avatar upload persist | ✅ | 🟡 | Preview only, no save |
| Crop photo | ✅ | ❌ | — |

### E. Address books & settings
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Multiple address books (server) | ✅ | ❌ | JS array only; lost on refresh |
| Create / rename / delete book | ✅ | 🟡 | Client-only |
| Show/hide book | ✅ | 🟡 | Client-only |
| Download book as `.vcf` | ✅ | ❌ | Export requires `?id=` (single contact) |
| Copy CardDAV link | ✅ | ❌ | Fake `/addressbooks/<id>` URL |
| Import contacts | ✅ | ✅ | But under-parses multi-field cards |
| Sort preference persisted | ✅ | ❌ | — |
| Social avatar sync | ✅ | 🟡 | Toggle UI only (can skip real sync) |

### F. Data / protocol
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| vCard 3/4 multi-value + params | ✅ | ❌ | Hand parser ignores `EMAIL;TYPE=…` |
| UID preserved | ✅ | ❌ | Nano filename IDs |
| Line folding / escaping | ✅ | ❌ | — |
| PHOTO binary/URI | ✅ | ❌ | — |
| Storage path | books | `data/<uid>/contacts/` | Sample data in `addressbooks/` orphaned |
| CardDAV server | ✅ | ❌ | `internal/carddav/` empty |
| User isolation via proxy | ✅ | ❌ | Proxy never sets `X-User-Id` |
| go-vcard | used | indirect unused | Should become real parser |

### G. Integrations
| Feature | NC | Dar | Gap |
|---------|----|-----|-----|
| Birthday → Calendar | ✅ | ❌ | Later |
| Mail compose from contact | ✅ | ❌ | Later |
| Soosa tools | N/A | ❌ | search/create/update |
| System / user address book | ✅ | ❌ | Later |

---

## Recommended approach (phased carbon clone)

Ship in **5 phases**. Each phase is independently demoable. Do **not** start CardDAV until the data model can round-trip real vCards (your DAVx5 sample set is the acceptance fixture).

### Phase 0 — Correctness foundations (1–2 days)
**Why first:** Everything else sits on a broken model/parser/tests.

1. **Fix tests to match reality, then grow them**  
   - `cmd/contacts/main_test.go` currently expects multi-field + `addressbooks/` path; production uses simple model + `contacts/`. Align then expand.
2. **Adopt `github.com/emersion/go-vcard` for parse/write**  
   - Promote from indirect → direct in `go.mod`.  
   - Support: folding, escaping, `UID`, multi `EMAIL`/`TEL`/`ADR`/`URL`, `BDAY`, `TITLE`, `ORG`, `NOTE`, `CATEGORIES`, `PHOTO`, `N`/`FN`, `REV`.
3. **Expand `Contact` JSON model**

```go
type TypedValue struct {
    Type  string `json:"type"`  // home, work, cell, other, …
    Value string `json:"value"`
}
type Address struct {
    Type       string `json:"type"`
    POBox      string `json:"po_box"`
    Extended   string `json:"extended"`
    Street     string `json:"street"`
    Locality   string `json:"locality"` // city
    Region     string `json:"region"`
    PostalCode string `json:"postal_code"`
    Country    string `json:"country"`
}
type Contact struct {
    ID, UID, FN, FirstName, LastName, MiddleName, Prefix, Suffix string
    Nickname, Title, Role, Org, Department, Notes string
    Birthday, Anniversary string
    Emails []TypedValue
    Phones []TypedValue
    URLs   []TypedValue
    Addresses []Address
    Groups []string
    Favorite bool
    PhotoURL string // or base64 ref
    AddressBookID string
    Color string
    UpdatedAt int64
}
```

4. **Backward compat:** map legacy single `email`/`phone` in API responses for one release *or* update UI in same PR as model (preferred: same PR).
5. **User isolation:** proxy injects `X-User-Id` from session in `cmd/server/main.go` `proxyContacts`.
6. **Storage layout migration**

```
data/<uid>/addressbooks/<book-id>/*.vcf   # canonical (matches samples + NC)
data/<uid>/contacts/                      # migrate into default book once
```

### Phase 1 — Real address books API (1–2 days)
Daemon endpoints (mirror NC settings UX):

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/books` | List books + counts |
| POST | `/books` | Create |
| POST | `/books/rename` | Rename |
| POST | `/books/delete` | Delete (move or wipe?) |
| GET | `/export?book=` | Export whole book |
| GET | `/export?id=` | Keep single contact |
| POST | `/import?book=` | Import into book |

- Default book `default` / “Contacts” auto-created.  
- Contact create/list/update take `address_book` / `book_id`.  
- Wire settings Address books tab to real API (copy link can be stub URL until CardDAV).  
- Persist **show/hide** + **sort preference** in `data/<uid>/contacts-settings.json` (or SQLite later).

### Phase 2 — Contact editor + detail carbon UX (2–3 days)
**File:** `internal/apps/contacts/handlers.go` (largest UI change).

1. **Detail header:** large avatar, name, subtitle (`Title · Org`), primary **Edit**, overflow **⋯** with Download / Clone / QR / Delete.
2. **Typed multi-field renderer** for view + edit (email/phone/url/address chips with type select + “add another”).
3. **Default form fields** match NC new-contact:
   - Name (FN), Title, Company, Phone, Email, Address (full), Notes, Groups
   - **Add more info** dropdown: Birthday, Website, Social, Detailed name, Nickname, Role, etc.
4. **Notes** always visible (already in backend).
5. **Last modified** footer.
6. **QR code:** generate client-side from `MECARD`/`vCard` string (vanilla JS, no dep) or server PNG endpoint.
7. **Photo:** `POST /photo?id=` multipart; store as `PHOTO` in vCard or sidecar `id.photo`; serve `GET /photo?id=`.
8. **URL selection state:** `?id=` + `?group=` so refresh keeps context.
9. **Sort wiring** from settings (display name / last / first).

### Phase 3 — Groups / empty groups / Teams decision (1 day + optional large)
- **Groups:** keep CATEGORIES model; for empty groups, store `data/<uid>/groups.json` list so empty groups survive reload (NC does).
- **Teams:** two options — ask before building:
  - **A)** UI placeholder only (“coming soon”) to match layout
  - **B)** Real shared address books / member lists (needs auth sharing model — large)
- Recommendation: **A for carbon visual clone**, **B later**.

### Phase 4 — CardDAV (2–4 days)
- Implement `internal/carddav/` with principal + addressbook + contact resources over the same FS tree.
- Mount under `/remote.php/dav/` or `/dav/` with session or app-password auth.
- Settings “Copy link” returns real URL.
- Acceptance: DAVx5 / iOS Contacts can sync the default book.

### Phase 5 — Integrations & polish (ongoing)
- Birthday → Calendar events  
- Mail “compose to”  
- Soosa tools: `contacts.search`, `contacts.get`, `contacts.create`  
- Duplicates / merge  
- Bulk select  
- Keyboard nav  
- System users address book  
- Social avatar sync (optional; can remain cosmetic toggle)

---

## Critical files

| File | Role |
|------|------|
| `cmd/contacts/main.go` | Daemon model, vCard I/O, REST, books, photo, export-all |
| `cmd/contacts/main_test.go` | Round-trip fixtures (use real DAVx5 samples) |
| `internal/apps/contacts/handlers.go` | Entire UI (HTML+JS) — editor, detail, settings |
| `internal/apps/contacts/handlers_test.go` | Smoke render tests (create if missing) |
| `cmd/server/main.go` | `proxyContacts` → inject `X-User-Id` |
| `internal/carddav/` | Phase 4 only |
| `data/default/addressbooks/default/*.vcf` | Golden fixtures for parser tests |

## Reuse

- Existing UI patterns: groups dropdown, settings tabs, 3-pane layout in `handlers.go`
- Existing API style: JSON REST under `/api/contacts/*`
- Allowed dep: `github.com/emersion/go-vcard` (already in `go.mod`)
- Layout shell: `internal/webui` + app bar from `server.AppBarHTML`
- Do **not** add React/Vue/HTMX/CSS build (AGENTS.md)

---

## Out of scope for “carbon clone” v1 (explicit skips)

| Item | Why |
|------|-----|
| Real Teams/Circles with email invites | Separate product surface |
| Federated Cloud ID | NC federation specific |
| Talk quick-call buttons | No Talk app |
| Social media avatar weekly sync | Network/scrape; keep toggle cosmetic |
| Birthday calendar exclude | Needs Calendar birthday feed first |
| Address book sharing ACLs | Needs multi-user sharing framework |

These can still appear as disabled/hidden UI if pure visual clone is required; prefer hiding over fake buttons that 404.

---

## Implementation task order (execute this sequence)

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 1 | Proxy `X-User-Id` | Multi-user isolation | Two users don’t see each other’s contacts |
| 2 | go-vcard parse/write + expanded model | Round-trip DAVx5 sample vCards | `go test ./cmd/contacts` |
| 3 | Migrate storage to `addressbooks/<id>/` | Default book + migrate old path | Existing contacts still list |
| 4 | Books CRUD + export-all + import-into-book | Settings Address books works after refresh | Manual + tests |
| 5 | UI multi-value fields + notes + title | Editor/detail match NC fields | Create Leon-like contact |
| 6 | Detail ⋯ menu + QR | Actions parity | QR modal shows scanable card |
| 7 | Photo upload/serve | Avatar persists | Reload keeps photo |
| 8 | Sort + URL state + last modified | Polish | Sort changes list; `?id=` works |
| 9 | Empty groups persistence | groups.json | Empty group survives reload |
| 10 | Teams placeholder | Sidebar visual match | Optional |
| 11 | CardDAV | External client sync | DAVx5 sync |
| 12 | Calendar/Mail/Soosa | Integrations | Later PRs |

---

## Verification (end-to-end)

```bash
# Unit
nix-shell -p go --run 'go test ./cmd/contacts ./internal/apps/contacts -v -count=1'
nix-shell -p go --run 'go build ./cmd/...'

# Stack
./launch.sh
# Login via dummy OIDC → /contacts

# Manual carbon checklist (mirror NC)
```

1. Sidebar: New contact, All contacts count, Not grouped, groups CRUD, settings footer.  
2. Create contact with: name, title, company, 2 phones (home/cell), 2 emails, full address, birthday, website, notes, photo, 2 groups.  
3. Reload — all fields present.  
4. Clone, download `.vcf`, open in external viewer, QR.  
5. Settings: sort by last name; create second address book; import multi-card `.vcf`; export book.  
6. Import a file from `data/default/addressbooks/default/` — all TEL/EMAIL/ADR/BDAY show.  
7. (Phase 4) DAVx5: add account, sync, edit on phone, appears in Dar.

---

## Risk notes

- **`handlers.go` is 800+ lines of embedded HTML/JS** — consider splitting JS into `//go:embed contacts.js` only if needed; not required for parity.  
- **Partial update merge** currently can’t clear fields (`if incoming.X != ""`) — multi-value model needs full replace semantics for arrays.  
- **Import raw write** without re-parse means list view loses fields until parser is fixed — Phase 0 is mandatory before claiming import parity.  
- Architectural choice for Teams / CardDAV auth should be confirmed before Phase 3B/4 (per AGENTS.md).

---

## Summary scorecard (today)

| Bucket | Rough parity |
|--------|----------------|
| Layout shell | ~80% |
| Groups UX | ~70% |
| Contact fields | ~20% |
| Address books | ~10% (UI shell only) |
| Sync (CardDAV) | 0% |
| Integrations | 0% |
| **Overall “carbon clone”** | **~35%** |

**Fastest path to feeling like Nextcloud:** Phase 0 → 2 (rich fields + editor) even before CardDAV.  
**Fastest path to being Nextcloud-compatible for phones:** Phase 0 → 1 → 4.
