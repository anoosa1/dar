# CardDAV Implementation Plan (final)

**Date:** 2026-07-09  
**Status:** Final — ready for implementation  
**Goal:** Phone/desktop clients (DAVx5, Thunderbird, Apple Contacts) sync Dar address books via CardDAV using app passwords.

**Out of scope:** CalDAV, shared books/ACLs, SQLite contact store, replacing REST for the web UI, CardDAV proxy to external servers.

---

## Locked decisions

| ID | Choice |
|----|--------|
| **D1** Location | `internal/carddav` + thin FS backend in contacts daemon |
| **D2** URLs | Hybrid: `/.well-known/carddav` → `/dav/…` |
| **D3** Path user | Internal user ID |
| **D4** MKCOL in v1 | No (UI-only book creation) |

---

## Current state

| Piece | Status |
|-------|--------|
| vCard storage | ✅ `data/<uid>/addressbooks/<book>/*.vcf` + `meta.json` |
| REST API | ✅ list/get/create/update/delete, books, import/export, `/me` |
| Multi-value vCard | ✅ go-vcard |
| App passwords + Basic Auth | ✅ `RequireAuthAPI` on `/api/*` |
| Proxy injects `X-User-Id` | ✅ `proxyContacts` |
| CardDAV protocol | ❌ empty `internal/carddav/` |
| Web UI | ✅ Contacts app |

**Do not re-store contacts in SQLite.** CardDAV reads/writes the same `.vcf` files as REST/UI.

---

## Target clients

| Client | Priority |
|--------|----------|
| DAVx5 (Android) | P0 |
| Thunderbird | P0 |
| Apple Contacts / iOS | P1 |
| curl smoke | P0 |

**Success:** UI create → phone after sync; phone edit → UI after refresh; no duplicates on re-sync.

---

## Public URL tree

```
/.well-known/carddav                          → 301 to /dav/
/dav/                                         → root (current-user-principal)
/dav/principals/<uid>/                        → principal + addressbook-home-set
/dav/addressbooks/<uid>/                      → home (list books)
/dav/addressbooks/<uid>/<book>/               → address book collection
/dav/addressbooks/<uid>/<book>/<id>.vcf       → contact resource
```

- Clients hit **main server** only (auth + proxy), not raw `:8083`.
- Path `uid` must equal authenticated `X-User-Id` → else **404**.
- Basic auth: **email** + **app password**.

### Path ↔ storage

| DAV | FS |
|-----|-----|
| `/dav/addressbooks/<uid>/` | `data/<uid>/addressbooks/` |
| `…/<book>/` | `…/<book>/` |
| `…/<book>/<id>.vcf` | `…/<book>/<id>.vcf` |

Skip `meta.json`, `groups.json`, non-`.vcf`. Me contact (`X-DAR-ME`) is a normal card.

---

## Protocol v1 (DAVx5-ready)

| Method | Behavior |
|--------|----------|
| `OPTIONS` | `DAV: 1, 3, addressbook` |
| `PROPFIND` | Depth 0/1 on principal, home, book, card |
| `REPORT` | `addressbook-multiget` (P0); `addressbook-query` = all cards in v1 |
| `GET` | Raw vCard + ETag; `Content-Type: text/vcard` |
| `PUT` | Create/replace; validate parseable vCard; If-Match → 412 |
| `DELETE` | 204 |

**Props:** `current-user-principal`, `addressbook-home-set`, `resourcetype` (+ `addressbook`), `displayname`, `CS:getctag`, `getetag`, `address-data` (REPORT).

**ETag:** `"mtime-size"`. **CTag:** aggregate of card etags (or max mtime + count).  
**PUT:** go-vcard parse; empty/invalid → 400; size cap ~1 MiB → 403.  
**Canonical id:** filename stem (not free-form vCard UID alone).

**v2 later:** `sync-collection`, MKCOL, PHOTO edge cases, ACLs, rate limits.

---

## Architecture

```
Client ──Basic(email, app_password)──► Server RequireAuthAPI
                                          │ X-User-Id
                                          ▼
                                       Proxy /dav/* + /.well-known/carddav
                                          ▼
                                       contacts :8083
                                          │
                                       internal/carddav.Handler
                                          │ Backend
                                       FSBackend → bookDir / .vcf I/O
```

```go
// internal/carddav Backend
ListBooks(uid) ([]BookInfo, error)
BookMeta(uid, bookID) (BookInfo, error)
ListCards(uid, bookID) ([]CardInfo, error)
GetCard(uid, bookID, id) (vcf []byte, etag string, err error)
PutCard(uid, bookID, id string, vcf []byte, ifMatch string) (etag string, created bool, err error)
DeleteCard(uid, bookID, id string) error
```

No new dependencies (`encoding/xml` + existing `go-vcard`).  
Chi: method-agnostic `Handle` so PROPFIND/REPORT are not dropped.

### Security

1. Path uid ≠ `X-User-Id` → **404**  
2. Prefer `X-User-Id` header over query `user_id` for DAV  
3. No unauthenticated CardDAV routes  

---

## File map

| File | Change |
|------|--------|
| `internal/carddav/*.go` | Protocol handler, XML, REPORT |
| `internal/carddav/*_test.go` | Multistatus fixtures, path isolation |
| `cmd/contacts/carddav_backend.go` | FSBackend adapter |
| `cmd/contacts/main.go` | Mount `/dav/` |
| `cmd/server/main.go` | Auth + proxy for well-known + `/dav/*` |
| `internal/apps/contacts/handlers.go` | CardDAV URL copy field |
| `cmd/contacts/main_test.go` | REST ↔ CardDAV cross tests |

---

## Implementation order

### PR 1 — Discovery + CRUD (~2–3 days)

1. `internal/carddav` skeleton: OPTIONS, path parse, multistatus  
2. Server proxy (all methods) + `X-User-Id`  
3. PROPFIND principal / home-set / books / cards  
4. GET / PUT / DELETE on same FS as REST  
5. Tests: uid mismatch 404; PUT → REST list; REST create → PROPFIND  

### PR 2 — REPORT + clients (~2 days)

1. `addressbook-multiget` + simple `addressbook-query`  
2. `getctag` on books  
3. UI: CardDAV base URL + email/app-password hint  
4. curl smoke + DAVx5 / Thunderbird  
5. Fix client XML quirks  

---

## Testing

1. Unit: REPORT parse, multistatus shape, path isolation  
2. Daemon httptest: PUT → GET → DELETE; PROPFIND books  
3. Cross-API: REST create ↔ CardDAV list; CardDAV PUT ↔ REST get  
4. Auth: no creds → 401; wrong path uid → 404  

```bash
curl -u 'user@example.com:dar_…' -X PROPFIND -H 'Depth: 0' http://localhost:8080/dav/
curl -u '…' -T contact.vcf \
  http://localhost:8080/dav/addressbooks/<uid>/default/abc.vcf
nix-shell -p go --run 'go test ./internal/carddav ./cmd/contacts -count=1'
```

### Client settings

| Field | Value |
|-------|--------|
| Base URL | `https://<host>/dav/` or `/.well-known/carddav` |
| Username | Account email |
| Password | App password (Settings → Security) |

---

## Risks

| Risk | Mitigation |
|------|------------|
| Client XML namespaces | Golden tests; DAVx5 logs |
| Proxy drops PROPFIND | `Handle` not GET-only; curl verify |
| Filename vs vCard UID | Canonical id = filename stem |
| Me contact client edits | Allowed; Settings `/me` may overwrite |

---

## Effort

~4–5 days for v1 (two PRs).
