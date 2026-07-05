# Nextcloud Files Feature Parity Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Compare Nextcloud Files features against Dar's current Files app, create a clear feature gap matrix, and prepare the app so Anas can choose which missing features to add next.

**Architecture:** Do not implement features yet. First create a repo-grounded parity audit from `src/apps/files/files.zig`, `src/apps/files/dav_server.zig`, `src/apps/files/dav_http.zig`, `src/storage/user_data.zig`, and `test/e2e_test.sh`, then use that audit to drive small TDD implementation plans for whichever features the user selects. Any feature that requires a durable data model or new API surface must be confirmed with the user before implementation because `AGENTS.md` requires asking before architectural decisions.

**Tech Stack:** Zig 0.17, `std.http.Server`, sqlite3 via `src/common/c.zig`, filesystem storage under `data/users/<username>/`, Tailwind CDN with Material 3 tokens, vanilla JavaScript, no new external dependencies.

---

## Current repo context

Dar's Files app currently lives mostly in one file:

- Main Files web app and JSON API: `src/apps/files/files.zig`
- WebDAV daemon binary: `src/apps/files/dav_server.zig`
- WebDAV method parser wrapper: `src/apps/files/dav_http.zig`
- Per-user storage helpers: `src/storage/user_data.zig`
- Existing e2e assertions: `test/e2e_test.sh`
- Test coverage entry point: `src/lib.zig`

Current implemented Dar Files features observed in the repo:

1. File browser page at `GET /files`.
2. Default `files` scope rooted at `data/users/<username>/files`.
3. `all` scope rooted at `data/users/<username>/`.
4. Directory listing API at `GET /api/files/list?path=...&scope=...`.
5. Folder picker API at `GET /api/files/picker?mode=folder&path=...&scope=...`.
6. Upload via multipart `POST /api/files/upload`.
7. Download via `GET /api/files/download?path=...` with attachment filename.
8. Create folder via `POST /api/files/mkdir`.
9. Delete via `POST /api/files/delete`; currently permanent delete, not trash.
10. Same-directory rename via `POST /api/files/move`.
11. Move/copy-to-folder via `POST /api/files/transfer`.
12. File picker dialog for move/copy.
13. Details pane with type, size, modified date, and path.
14. Sortable table headers for name, size, modified.
15. Client-side hidden-file filtering.
16. Client-side show/hide file extensions.
17. Client-side column visibility for size and modified.
18. Three-dot action menu clamped to viewport.
19. SVG icons for folders and common file types.
20. WebDAV URL in Files settings dialog.
21. DAV daemon supports `OPTIONS`, `GET`, `HEAD`, `PUT`, `DELETE`, `PROPFIND`, `MKCOL`, `LOCK`, and `UNLOCK`.
22. Unit tests for path normalization and basic filesystem operations in `src/apps/files/files.zig`.
23. E2E tests for shell/sidebar/settings/DAV URL/icons/list/upload/download/mkdir/rename/delete/move/copy in `test/e2e_test.sh`.

Nextcloud Files features found from public Nextcloud docs/search snippets:

1. Access files in web UI.
2. Create, preview, edit, delete, share, and re-share files.
3. Details view with Activities, Sharing, and Versions information.
4. Show/hide hidden files.
5. Restore older versions of files.
6. Tag and comment on files.
7. Password-protected public links.
8. File Drop links so external people can upload into a folder.
9. Share permission management.
10. WebDAV access for desktop/mobile clients.
11. Deleted files / trash retention and restore.
12. Federated / external sharing may exist, but should be treated as later scope unless explicitly requested.
13. Admin file workflows such as automated tagging / classification exist, but should be out of scope until the core Files app is solid.

## Feature gap matrix

| Area | Nextcloud behavior | Dar status | Likely priority | Notes |
| --- | --- | --- | --- | --- |
| Browse files | Web UI file list | Present | Done | `GET /files`, `/api/files/list` |
| Create folders | Create folders from UI | Present | Done | Prompt-based; could improve to dialog later |
| Upload | Upload files from UI | Present | Done/basic | Multipart parser is minimal; no chunking/resume |
| Download | Download files | Present | Done/basic | Uses `attachment` disposition |
| Rename | Rename entries | Present | Done/basic | Warns on extension changes |
| Move/copy | Move/copy entries | Present | Done/basic | Copy only reads files into memory; directory copy not supported |
| Delete | Delete entries | Partial | High | Permanent delete; Nextcloud has trash/deleted files restore |
| Details pane | Metadata/details | Partial | Medium | Has basic details; missing activity, comments, sharing, versions tabs |
| Activity | Per-file activity history | Missing | Medium | Needs DB table and write hooks around all mutations |
| Comments | File comments | Missing | Medium | Needs DB table and details-pane tab |
| Versions | File version history and restore | Missing | High | Needs version storage and save-before-overwrite hooks |
| Sharing | Internal/public shares | Missing | High | Needs share table, permission model, public token routes |
| Re-sharing | Share a received share | Missing | Low/medium | Depends on sharing model |
| File Drop | External upload-only link | Missing | High if sharing selected | Could be a constrained share type |
| Tags/favorites | Tag/comment/favorite files | Missing | Medium | Favorites can be small first step; tags need UX/API |
| Preview | Preview files in browser | Missing | Medium | Text/image/PDF preview first; no external deps |
| Edit | Edit text documents in browser | Missing | Medium | Text/code only first; rich docs are out of scope without deps |
| Search | Search/filter files | Missing | Medium | Filename search simple; content search out of scope initially |
| Multi-select/bulk actions | Bulk delete/move/download | Missing | Medium | UI-only plus batch APIs |
| Drag/drop upload | Drag files into browser | Missing | Low/medium | UI enhancement using existing upload API |
| Conflict handling | Upload overwrite/conflict choices | Missing | Medium | Needs API behavior choice |
| WebDAV | Desktop/mobile filesystem access | Partial | Medium | Core methods present; likely missing DAV `MOVE`, `COPY`, `Depth` handling, auth, mtime/etag fidelity |
| Quotas | User storage limits | Missing | Low/medium | Needs user/app settings model |
| External storage | Mount external backends | Missing | Low | Likely out of scope due no deps and architecture decision |
| Workflow automation | Automated tags/classification | Missing | Low | Out of scope until metadata/tags exist |

## Proposed feature priority bundles

These are not implementation decisions. They are selectable bundles to discuss with the user.

### Bundle A: Core safety and data durability

Best first if Dar should feel trustworthy before adding collaboration.

1. Trash/deleted files with restore.
2. File versions on overwrite and explicit restore.
3. Activity log covering upload, mkdir, rename, move, copy, delete, restore, and download.
4. Better upload conflict behavior.

### Bundle B: Sharing and collaboration

Best first if Dar should become useful for other people.

1. Public share links for files/folders.
2. Password-protected public links.
3. Expiration date and permission flags.
4. File Drop upload-only links for folders.
5. Details-pane Sharing tab.

### Bundle C: Finder-quality file manager UX

Best first if the browser app should feel polished.

1. Multi-select rows and bulk actions.
2. Drag/drop upload.
3. Search/filter.
4. Preview pane for images/text/PDF.
5. Better dialogs replacing prompt/confirm.

### Bundle D: Metadata and organization

Best first if users will manage many files.

1. Favorites.
2. Tags.
3. Comments.
4. Activity tab.
5. Saved views for Favorites/Tags/Recent.

### Bundle E: WebDAV completeness

Best first if desktop/mobile sync compatibility matters.

1. Add DAV `MOVE` and `COPY`.
2. Improve `PROPFIND` depth handling and directory children listing.
3. Add stable etags/last-modified properties.
4. Add auth/session or app-password story for DAV.
5. Add e2e DAV tests with `curl -X PROPFIND`, `PUT`, `MKCOL`, `MOVE`, `COPY`, `DELETE`.

## Step-by-step plan

### Task 1: Save a feature parity audit document

**Objective:** Create a durable repo document that records the Nextcloud-vs-Dar comparison so future implementation work starts from the same baseline.

**Files:**
- Create: `docs/files-feature-parity.md`

**Step 1: Draft the document**

Write sections:

1. `# Files Feature Parity`
2. `## Current Dar Files features`
3. `## Nextcloud Files reference features`
4. `## Gap matrix`
5. `## Suggested implementation bundles`
6. `## Out of scope for now`

Use the feature matrix above as the initial content.

**Step 2: Verify the document is accurate against repo files**

Run:

```bash
rg -n "api/files/(list|picker|upload|download|mkdir|delete|move|transfer)|function (showDetails|sortBy|openActionMenu|renderFilePicker)|WebDAV|PROPFIND|MKCOL|LOCK|UNLOCK" src/apps/files test/e2e_test.sh
```

Expected: results show each claimed existing feature in `src/apps/files/files.zig`, `src/apps/files/dav_server.zig`, or `test/e2e_test.sh`.

**Step 3: Commit**

Only if the user asks to commit:

```bash
git add docs/files-feature-parity.md
git commit -m "docs: add files feature parity audit"
```

### Task 2: Ask user to select the first implementation bundle

**Objective:** Avoid making architecture decisions silently and let Anas pick what to add first.

**Files:**
- No code changes.

**Step 1: Present the five bundles**

Ask the user to pick one:

1. Core safety and data durability: trash, versions, activity, conflict handling.
2. Sharing and collaboration: public links, passwords, expiration, file drop.
3. Finder-quality file manager UX: multi-select, drag/drop, search, previews.
4. Metadata and organization: favorites, tags, comments, saved views.
5. WebDAV completeness: MOVE/COPY, PROPFIND depth, etags, auth/app-passwords.

**Step 2: Confirm architecture-sensitive choices**

Before implementing any selected bundle, ask questions listed in the `Open questions` section below.

### Task 3: If Bundle A is selected, implement trash/deleted files first

**Objective:** Change delete from permanent removal to recoverable trash, matching the highest-value Nextcloud safety behavior.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`
- Possibly create: `src/apps/files/trash.zig` only if the user approves splitting the module.

**Step 1: Write failing unit tests**

Add tests in `src/apps/files/files.zig` for:

1. Deleting `/note.txt` moves it into a per-user trash location.
2. Listing trash returns the deleted entry.
3. Restoring the deleted entry moves it back to the original path.
4. Purging a trash entry permanently removes it.
5. Deleting `/` is rejected.
6. Deleting `../secret` is rejected.

**Step 2: Run tests to verify failure**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: FAIL because trash helpers/routes do not exist yet.

**Step 3: Add minimal storage model**

Preferred low-dependency storage shape, if approved by user:

- Move deleted files into `data/users/<username>/trash/files/<trash_id>`.
- Store metadata in sqlite table `file_trash`:
  - `id TEXT PRIMARY KEY`
  - `user_id TEXT NOT NULL`
  - `scope TEXT NOT NULL`
  - `original_path TEXT NOT NULL`
  - `trash_path TEXT NOT NULL`
  - `name TEXT NOT NULL`
  - `is_dir INTEGER NOT NULL`
  - `size INTEGER NOT NULL DEFAULT 0`
  - `deleted_at INTEGER NOT NULL`

Add routes:

- `GET /api/files/trash/list`
- `POST /api/files/trash/restore`
- `POST /api/files/trash/purge`

**Step 4: Update UI**

Add a sidebar item below `All files`:

- `Deleted files`, backed by a new client-side scope/view.

For normal files:

- `Delete` action moves to trash and updates copy to `Move to deleted files?`.

For trash view:

- Actions are `Restore` and `Delete permanently`.

**Step 5: Add e2e coverage**

In `test/e2e_test.sh`, after current delete test, add:

1. Create `trashme.txt`.
2. Delete via `/api/files/delete`.
3. Assert original file path no longer exists.
4. Assert `/api/files/trash/list` contains `trashme.txt` and original path.
5. Restore via `/api/files/trash/restore`.
6. Assert original file path exists again.
7. Delete again then purge.
8. Assert no restored file and no trash file remain.

**Step 6: Verify**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected: build succeeds, unit tests pass, e2e reports all PASS.

### Task 4: If Bundle A continues, implement file versions

**Objective:** Preserve previous versions before overwrites and allow restore/download from the details pane.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`
- Possibly create: `src/apps/files/versions.zig` only if the user approves splitting the module.

**Step 1: Write failing tests**

Add tests for:

1. Uploading over an existing file stores the previous bytes as a version.
2. `GET /api/files/versions?path=/note.txt` returns versions newest-first.
3. `POST /api/files/versions/restore` restores selected bytes to the live file.
4. Deleting a file does not orphan versions unexpectedly, or deliberately moves versions with trash if that behavior is chosen.

**Step 2: Implement minimal version storage**

Proposed storage, pending user approval:

- Store bytes under `data/users/<username>/versions/<version_id>`.
- Store metadata in sqlite table `file_versions`:
  - `id TEXT PRIMARY KEY`
  - `user_id TEXT NOT NULL`
  - `scope TEXT NOT NULL`
  - `path TEXT NOT NULL`
  - `version_path TEXT NOT NULL`
  - `size INTEGER NOT NULL`
  - `created_at INTEGER NOT NULL`

Hook version creation before overwrites in:

- `writeUserFile(...)`
- `copyUserEntry(...)` if destination exists
- future text editor save route if added
- DAV `PUT` in `src/apps/files/dav_server.zig` if DAV versioning should match web uploads

**Step 3: UI**

Add a `Versions` tab/section inside `#details-pane` with:

- timestamp
- size
- Download
- Restore

**Step 4: Verify**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

### Task 5: If Bundle B is selected, implement public sharing MVP

**Objective:** Add public file/folder share links with optional password and expiration.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `src/lib.zig` if a new module is created
- Modify: `test/e2e_test.sh`
- Possibly create: `src/apps/files/shares.zig` only if the user approves splitting the module.

**Step 1: Write failing tests**

Add unit/API tests for:

1. Create public share for `/note.txt`.
2. Token route downloads file without a session.
3. Disabled/expired share returns 404 or 403.
4. Password-protected share rejects missing/wrong password.
5. Folder share lists folder contents without exposing parent paths.
6. Share tokens reject path traversal.

**Step 2: Proposed sqlite model**

Pending user approval:

```sql
CREATE TABLE IF NOT EXISTS file_shares (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'files',
  path TEXT NOT NULL,
  token TEXT NOT NULL UNIQUE,
  password_hash TEXT,
  expires_at INTEGER,
  permissions TEXT NOT NULL DEFAULT 'read',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
```

Important: do not store plaintext passwords. Need an approved hashing approach using Zig stdlib only.

**Step 3: API routes**

Add:

- `POST /api/files/shares/create`
- `GET /api/files/shares/list?path=...`
- `POST /api/files/shares/update`
- `POST /api/files/shares/delete`
- `GET /s/<token>` public route
- `POST /s/<token>/auth` if password-protected shares are selected

**Step 4: UI**

Add a `Sharing` section in details pane:

- create link
- copy link
- password checkbox/field
- expiration field
- permission selector for file/folder, initially read-only/download-only
- delete share

**Step 5: Verify**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Then manually curl a public token URL without `dar_session` and verify it works only as permitted.

### Task 6: If Bundle B continues, implement File Drop links

**Objective:** Add upload-only public links for folders, similar to Nextcloud File Drop.

**Files:**
- Modify: `src/apps/files/files.zig` or approved `src/apps/files/shares.zig`
- Modify: `test/e2e_test.sh`

**Step 1: Write failing tests**

Test:

1. Create file-drop share for a folder.
2. Public user can upload to that folder.
3. Public user cannot list existing folder contents unless permission says so.
4. Public user cannot overwrite files unless conflict behavior is approved.

**Step 2: Implement as share permission**

Use `permissions = 'drop'` or equivalent in `file_shares`.

**Step 3: UI**

In Sharing tab:

- toggle `Allow uploads only (File Drop)` for folders.
- show public drop page with upload control.

**Step 4: Verify**

Use curl multipart upload against the public drop route without a session.

### Task 7: If Bundle C is selected, implement search/filter first

**Objective:** Add filename search in current folder and optional recursive search.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`

**Step 1: Write failing tests**

Test:

1. `GET /api/files/search?q=note&path=/&scope=files` returns matching names.
2. Search rejects traversal in `path`.
3. Search only returns files inside current user's root.
4. Empty query returns `400` or an empty list, whichever the user chooses.

**Step 2: Implement filename-only search**

Use Zig directory iteration only. Do not add FTS/content indexing yet.

**Step 3: UI**

Add search field in Files toolbar. Debounce client-side calls with vanilla JS.

**Step 4: Verify**

Run build, unit tests, e2e, and manual curl search.

### Task 8: If Bundle C continues, implement previews

**Objective:** Add a safe browser preview for common types without new dependencies.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`

**Step 1: Start with text and images**

Routes:

- `GET /api/files/preview?path=...`

Behavior:

- images: serve inline with detected MIME.
- text/code/json/markdown: serve escaped text or raw with `text/plain`.
- pdf: browser inline route can reuse download data with `application/pdf`, if MIME helper is extended.
- everything else: return unsupported.

**Step 2: UI**

Details pane gets `Preview` action or preview block.

**Step 3: Verify**

Curl headers and HTML markers from e2e.

### Task 9: If Bundle D is selected, implement favorites first

**Objective:** Add a low-risk metadata feature before full tags/comments.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`

**Step 1: Write failing tests**

Test:

1. `POST /api/files/favorites/toggle` marks file favorite.
2. List response includes `favorite:true`.
3. Favorites view lists all favorite files.
4. Favorite metadata follows rename/move, or define path-based limitation clearly.

**Step 2: Proposed sqlite table**

Pending user approval:

```sql
CREATE TABLE IF NOT EXISTS file_favorites (
  user_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  path TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, scope, path)
);
```

**Step 3: UI**

Add star action in row/actions and sidebar item `Favorites`.

**Step 4: Verify**

Run standard build/test/e2e.

### Task 10: If Bundle D continues, implement tags and comments

**Objective:** Match Nextcloud's tag/comment organization features.

**Files:**
- Modify: `src/apps/files/files.zig`
- Modify: `test/e2e_test.sh`
- Possibly create approved metadata module.

**Step 1: Tags**

Proposed tables:

- `file_tags(id, user_id, name, color, created_at)`
- `file_tag_assignments(user_id, scope, path, tag_id, created_at)`

Add API:

- `GET /api/files/tags`
- `POST /api/files/tags/create`
- `POST /api/files/tags/assign`
- `POST /api/files/tags/remove`

**Step 2: Comments**

Proposed table:

- `file_comments(id, user_id, author_user_id, scope, path, body, created_at, updated_at)`

Add API:

- `GET /api/files/comments?path=...`
- `POST /api/files/comments/create`
- `POST /api/files/comments/delete`

**Step 3: UI**

Details pane tabs:

- Details
- Activity
- Sharing
- Versions
- Comments
- Tags

Only show tabs for implemented features.

**Step 4: Verify**

Run standard build/test/e2e and focused curl tests for comments/tags.

### Task 11: If Bundle E is selected, improve WebDAV compatibility

**Objective:** Bring Dar's DAV daemon closer to actual desktop/mobile client expectations.

**Files:**
- Modify: `src/apps/files/dav_server.zig`
- Modify: `src/apps/files/dav_http.zig` only if parser/header support is insufficient
- Modify: `test/e2e_test.sh` or create a focused `test/dav_e2e_test.sh` if approved

**Step 1: Write failing DAV tests**

Add curl-based checks for:

```bash
curl -i -X OPTIONS http://localhost:8081/test/files
curl -i -X MKCOL http://localhost:8081/test/files/davdir
curl -i -X PUT --data 'hello' http://localhost:8081/test/files/davdir/a.txt
curl -i -X PROPFIND -H 'Depth: 1' http://localhost:8081/test/files/davdir
curl -i -X COPY -H 'Destination: http://localhost:8081/test/files/davdir/b.txt' http://localhost:8081/test/files/davdir/a.txt
curl -i -X MOVE -H 'Destination: http://localhost:8081/test/files/davdir/c.txt' http://localhost:8081/test/files/davdir/b.txt
curl -i -X DELETE http://localhost:8081/test/files/davdir/c.txt
```

Expected before implementation: COPY/MOVE fail; PROPFIND only returns the requested path, not children.

**Step 2: Implement PROPFIND Depth: 1**

Read `Depth` header from `req.iterateHeaders()` before consuming body. For directories, include child responses with `resourcetype`, `getcontentlength`, and stable hrefs.

**Step 3: Implement COPY and MOVE**

Parse `Destination` header, validate it remains under `g_root`, then use filesystem copy/rename helpers.

**Step 4: Add etag/last-modified**

Add stable-ish properties from file size and modification time if Zig 0.17 APIs are available. If mtime API is not available, ask before using another approach.

**Step 5: Verify**

Run:

```bash
nix develop /home/anas/soosa --command zig build
./zig-out/bin/dar-files --port 18081 --root data/users
```

In another terminal or background script:

```bash
curl -i -X OPTIONS http://localhost:18081/test/files
curl -i -X MKCOL http://localhost:18081/test/files/davdir
curl -i -X PUT --data 'hello' http://localhost:18081/test/files/davdir/a.txt
curl -i -X PROPFIND -H 'Depth: 1' http://localhost:18081/test/files/davdir
curl -i -X COPY -H 'Destination: http://localhost:18081/test/files/davdir/b.txt' http://localhost:18081/test/files/davdir/a.txt
curl -i -X MOVE -H 'Destination: http://localhost:18081/test/files/davdir/c.txt' http://localhost:18081/test/files/davdir/b.txt
curl -i -X DELETE http://localhost:18081/test/files/davdir/c.txt
```

Expected: all operations return 2xx/201/204/207 as appropriate and filesystem state matches.

## Files likely to change after feature selection

Core likely files:

- `src/apps/files/files.zig`
- `src/apps/files/dav_server.zig`
- `src/apps/files/dav_http.zig`
- `src/storage/user_data.zig`
- `src/lib.zig`
- `test/e2e_test.sh`

Possible new files, only after user approves module split:

- `src/apps/files/trash.zig`
- `src/apps/files/versions.zig`
- `src/apps/files/shares.zig`
- `src/apps/files/activity.zig`
- `src/apps/files/metadata.zig`
- `test/dav_e2e_test.sh`
- `docs/files-feature-parity.md`

## Testing / validation baseline for any selected feature

After each implementation task:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

For focused filesystem/API verification, use a temporary script under `/tmp/hermes-verify-files-*.sh` that:

1. Builds Dar.
2. Starts dummy OAuth and Dar on random ports.
3. Creates/uses a test session.
4. Calls the new API with `curl`.
5. Verifies actual files under `data/users/<username>/...`.
6. Cleans up ports, DBs, data, and temporary files.

For WebDAV-specific features, additionally run `dar-files` and probe with `curl -X PROPFIND`, `PUT`, `MKCOL`, `MOVE`, `COPY`, and `DELETE`.

## Risks and tradeoffs

1. `src/apps/files/files.zig` is already large. Splitting into modules would improve maintainability, but that is an architectural decision and must be approved before doing it.
2. Many missing Nextcloud features need persistent metadata. Path-based metadata is simplest but can break on rename/move unless every mutation updates metadata. Stable file IDs are more robust but require a larger model change.
3. Trash and versions can consume disk quickly. Retention/cleanup policy should be decided before or during implementation.
4. Sharing/passwords require secure token generation and password hashing without adding dependencies. Need a Zig-stdlib-only approach or a user-approved compromise.
5. Current upload and copy paths read whole files into memory. Large-file support will require streaming work before chunked/resumable uploads.
6. DAV compatibility can be deceptively broad. Desktop clients may require more than simple method support: auth, etags, locks, last-modified, depth handling, URL encoding, and good status XML.
7. Public sharing and File Drop introduce unauthenticated routes. These need careful path validation and negative tests.
8. Nextcloud has many app/plugin-level features. Dar should prioritize core file-manager behavior and avoid recreating admin workflow automation until primitives exist.

## Open questions

1. Which bundle should be implemented first: A) safety/durability, B) sharing/collaboration, C) polished file manager UX, D) metadata/organization, or E) WebDAV completeness?
2. Are we allowed to split `src/apps/files/files.zig` into smaller modules, or should the next feature stay in the existing file?
3. For deleted files, should Dar behave like Nextcloud trash by default, or keep permanent delete with trash as a separate action/view?
4. What retention policy should trash and versions use: never auto-delete, size-based cleanup, age-based cleanup, or configurable later?
5. Should metadata track files by path only, or should Dar introduce stable file IDs for robust rename/move/version/share behavior?
6. For public sharing, are password-protected links required in the first pass, and what password hashing approach is acceptable with Zig stdlib only?
7. Should public shares support folders in the first pass, or files only?
8. Should File Drop be part of the first sharing milestone, or a follow-up?
9. Should DAV access require Dar session/app-password authentication before more compatibility work, or is LAN-local/no-auth acceptable during development?
10. Should previews include only images/text/PDF first, or should editing text files be part of the same milestone?
11. Should search be filename-only first, or should content search be planned despite the no-extra-dependencies constraint?
12. Should external storage/federated sharing/admin workflows be explicitly out of scope for Dar's first full-featured Files app?
