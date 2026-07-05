# Per-User Data Directories Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Give every Dar user a private directory under `data/`, make that directory the app data root for that user, and let the Files app show the user's normal files by default while also offering an All files view for the whole user directory.

**Architecture:** Add one shared storage helper module that computes and creates per-user data paths from a `common.User`/`user_id`, then update login, Files UI/API, and the DAV daemon to use that helper instead of hard-coded `data/files/<user_id>` paths. The on-disk layout is `data/users/<user_id>/`, with normal user files under `data/users/<user_id>/files/` and other app/system folders beside it, e.g. `trash/`, `calendar/`, `contacts/`. The Files app defaults to `data/users/<user_id>/files/`, but an All files tab/view exposes `data/users/<user_id>/`.

**Tech Stack:** Zig 0.17, std.Io.Dir APIs, existing SQLite user/session auth, existing Files app and DAV daemon, no new dependencies.

---

## Current context / assumptions

- `src/main.zig:224-227` already creates `data/files/<user_id>` during OIDC callback.
- `src/apps/files/files.zig:12` has `g_storage_root = "data/files"` and download paths are built as `{g_storage_root}/{user.id}{path}/{name}`.
- `src/apps/files/files.zig` still lists files from SQLite rows, not from the filesystem. Upload/mkdir/delete/move are currently stubs returning `{"status":"ok"}`.
- `src/apps/files/dav_server.zig` and `src/apps/files/dav_daemon.zig` default their root to `data/files` and expose filesystem paths under that root.
- Existing tests are mainly inline Zig tests and `test/e2e_test.sh`; there are no focused Files app integration tests yet.
- The desired layout is `data/users/<user_id>/`.
- The default Files tab shows `data/users/<user_id>/files/` only.
- An All files tab/view exposes the whole `data/users/<user_id>/` directory, so users can see folders like `files/`, `trash/`, and other app data folders when needed.

## Proposed approach

1. Create `src/storage/user_data.zig` as the single source of truth for per-user data paths.
2. Create per-user directories at login with `ensureUserDataDir(user.id)` and app folders with `ensureUserAppDir(user.id, "files")` as needed.
3. Update Files app handlers to read and write the filesystem under two scoped roots: default root `data/users/<user_id>/files/` and all-files root `data/users/<user_id>/`.
4. Keep the SQLite `files` table only if it remains useful as metadata/cache, but do not rely on it as the source of truth for listing. The default file browser should list actual filesystem contents under `files/`; the All files view should list the whole user directory.
5. Update DAV daemon root defaults from `data/files` to `data/users` and require a user prefix in DAV paths until proper DAV auth exists.
6. Add tests for path construction, traversal prevention, directory creation, and Files listing behavior.

## Files likely to change

- Create: `src/storage/user_data.zig`
- Modify: `src/lib.zig`
- Modify: `src/main.zig:212-229`
- Modify: `src/apps/files/files.zig:11-184`
- Modify: `src/apps/files/dav_server.zig:5-141`
- Modify: `src/apps/files/dav_daemon.zig:8-69` if this older daemon is still built or referenced
- Modify: `build.zig` only if `src/storage/user_data.zig` needs explicit module/test wiring beyond `src/lib.zig`
- Modify: `test/e2e_test.sh:80-120` for login directory assertions and Files root assertions
- Optional docs update: `AGENTS.md` only if the repo convention should permanently document the new `data/users/<user_id>/` layout

---

## Task 1: Add a shared per-user storage helper

**Objective:** Centralize user data path construction and directory creation so apps do not hand-build `data/...` paths.

**Files:**
- Create: `src/storage/user_data.zig`
- Modify: `src/lib.zig`

**Step 1: Write failing tests**

Create `src/storage/user_data.zig` with tests first. The tests should cover:

```zig
test "user data paths are rooted under data/users" {
    const a = std.testing.allocator;
    const p = try userRoot(a, "u123");
    defer a.free(p);
    try std.testing.expectEqualStrings("data/users/u123", p);

    const app = try userAppRoot(a, "u123", "files");
    defer a.free(app);
    try std.testing.expectEqualStrings("data/users/u123/files", app);
}

test "path segments reject traversal and separators" {
    try std.testing.expectError(error.InvalidPathSegment, validateSegment("../evil"));
    try std.testing.expectError(error.InvalidPathSegment, validateSegment("a/b"));
    try std.testing.expectError(error.InvalidPathSegment, validateSegment(""));
    try validateSegment("u123");
}
```

**Step 2: Run test to verify failure**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: FAIL because `src/storage/user_data.zig` is not imported yet, or helper functions are missing if the test file has been created with test stubs only.

**Step 3: Write minimal implementation**

Implement helpers using only Zig stdlib:

```zig
//! Per-user data directory helpers.
const std = @import("std");
const compat = @import("../common/compat.zig");

pub const root_dir: []const u8 = "data/users";

pub fn validateSegment(segment: []const u8) !void {
    if (segment.len == 0) return error.InvalidPathSegment;
    if (std.mem.indexOfScalar(u8, segment, '/') != null) return error.InvalidPathSegment;
    if (std.mem.indexOfScalar(u8, segment, '\\') != null) return error.InvalidPathSegment;
    if (std.mem.indexOf(u8, segment, "..") != null) return error.InvalidPathSegment;
}

pub fn userRoot(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    try validateSegment(user_id);
    return std.fmt.allocPrint(allocator, "{s}/{s}", .{ root_dir, user_id });
}

pub fn userAppRoot(allocator: std.mem.Allocator, user_id: []const u8, app_name: []const u8) ![]const u8 {
    try validateSegment(user_id);
    try validateSegment(app_name);
    return std.fmt.allocPrint(allocator, "{s}/{s}/{s}", .{ root_dir, user_id, app_name });
}

pub fn ensureRootDirs() !void {
    const io = compat.getIo();
    std.Io.Dir.cwd().createDir(io, "data", @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    std.Io.Dir.cwd().createDir(io, root_dir, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
}

pub fn ensureUserDataDir(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    try ensureRootDirs();
    const path = try userRoot(allocator, user_id);
    std.Io.Dir.cwd().createDir(compat.getIo(), path, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    return path;
}

pub fn ensureUserAppDir(allocator: std.mem.Allocator, user_id: []const u8, app_name: []const u8) ![]const u8 {
    const user_path = try ensureUserDataDir(allocator, user_id);
    allocator.free(user_path);
    const app_path = try userAppRoot(allocator, user_id, app_name);
    std.Io.Dir.cwd().createDir(compat.getIo(), app_path, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    return app_path;
}
```

Update `src/lib.zig` to import it for test coverage:

```zig
pub const user_data = @import("storage/user_data.zig");
```

If `src/lib.zig` uses only `_ = @import(...)` style, follow the existing style instead.

**Step 4: Run test to verify pass**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: PASS for new `user_data` tests. Existing unrelated tests should remain unchanged.

**Step 5: Commit**

```bash
git add src/storage/user_data.zig src/lib.zig
git commit -m "feat: add per-user data directory helpers"
```

---

## Task 2: Create user directories during login

**Objective:** Ensure every authenticated user has `data/users/<user_id>/` and initial app subdirectories before the session cookie is issued.

**Files:**
- Modify: `src/main.zig:212-229`
- Test: existing login flow in `test/e2e_test.sh`

**Step 1: Write failing verification in e2e test**

In `test/e2e_test.sh`, after the OAuth callback creates `SESSION_COOKIE`, add checks for the directory layout. Use the test database's known user ID by querying SQLite if the script already has `DAR_DB`, or infer from the created directory if not.

Recommended check:

```bash
USER_ID=$(sqlite3 "$DAR_DB" "SELECT id FROM users LIMIT 1;")
if [ -d "data/users/$USER_ID" ]; then
    pass_msg "OIDC login creates per-user data directory"
else
    fail_msg "OIDC login did not create data/users/$USER_ID"
fi

if [ -d "data/users/$USER_ID/files" ]; then
    pass_msg "OIDC login creates files app directory"
else
    fail_msg "OIDC login did not create data/users/$USER_ID/files"
fi
```

If `test/e2e_test.sh` currently does not keep the DB path in a variable, first add a variable near server startup and pass it to `./launch.sh --db "$DAR_DB"` or directly to `./zig-out/bin/dar --db "$DAR_DB"`, matching the existing startup pattern.

**Step 2: Run e2e to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL because login currently creates `data/files/<user_id>`, not `data/users/<user_id>` or `data/users/<user_id>/files`.

**Step 3: Update OIDC callback**

In `src/main.zig`, import the helper near existing imports:

```zig
const user_data = @import("storage/user_data.zig");
```

Replace the current hard-coded directory creation:

```zig
// Create per-user home directory under data/files/<user_id>/
const home_dir = try std.fmt.allocPrint(ctx.allocator, "data/files/{s}", .{user.id});
defer ctx.allocator.free(home_dir);
std.Io.Dir.cwd().createDir(compat.getIo(), home_dir, @enumFromInt(0o755)) catch {};
```

with:

```zig
// Create per-user data directory and the Files app directory before issuing a session.
const user_home = try user_data.ensureUserDataDir(ctx.allocator, user.id);
defer ctx.allocator.free(user_home);
const files_home = try user_data.ensureUserAppDir(ctx.allocator, user.id, "files");
defer ctx.allocator.free(files_home);
```

**Step 4: Run build and e2e**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected: build passes, unit tests pass, e2e directory checks pass.

**Step 5: Commit**

```bash
git add src/main.zig test/e2e_test.sh
git commit -m "feat: create user data directories on login"
```

---

## Task 3: Add safe relative path helpers for Files app filesystem access

**Objective:** Prevent path traversal and normalize Files app paths before using them to touch disk.

**Files:**
- Modify: `src/apps/files/files.zig`

**Step 1: Write failing tests in `src/apps/files/files.zig`**

Add tests near the bottom of the file:

```zig
test "files app rejects unsafe paths" {
    try std.testing.expectError(error.InvalidPath, cleanUserPath("../secret"));
    try std.testing.expectError(error.InvalidPath, cleanUserPath("/../../secret"));
    try std.testing.expectError(error.InvalidPath, cleanUserPath("folder/../secret"));
}

test "files app normalizes safe paths" {
    try std.testing.expectEqualStrings("/", try cleanUserPath("/"));
    try std.testing.expectEqualStrings("/docs", try cleanUserPath("/docs"));
    try std.testing.expectEqualStrings("/docs/work", try cleanUserPath("docs/work"));
}
```

**Step 2: Run tests to verify failure**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: FAIL because `cleanUserPath` does not exist yet.

**Step 3: Implement path helpers**

Add helpers in `src/apps/files/files.zig`:

```zig
fn cleanUserPath(raw: []const u8) ![]const u8 {
    if (raw.len == 0) return "/";
    if (std.mem.indexOf(u8, raw, "..") != null) return error.InvalidPath;
    if (std.mem.indexOfScalar(u8, raw, '\\') != null) return error.InvalidPath;
    return if (raw[0] == '/') raw else error.PathMustBeAbsolute;
}

const FileScope = enum { files, all };

fn joinUserDiskPath(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const user_data = @import("../../storage/user_data.zig");
    const clean = try cleanUserPath(rel_path);
    const root = switch (scope) {
        .files => try user_data.userAppRoot(allocator, user_id, "files"),
        .all => try user_data.userRoot(allocator, user_id),
    };
    defer allocator.free(root);
    if (std.mem.eql(u8, clean, "/")) return allocator.dupe(u8, root);
    return std.fmt.allocPrint(allocator, "{s}{s}", .{ root, clean });
}
```

Important: if the tests above expect `cleanUserPath("docs/work")` to normalize to `/docs/work`, either adjust the helper to prepend `/` or adjust the test. Pick one behavior and keep it consistent. Recommended behavior for HTTP query paths is to accept both `docs` and `/docs` and return a leading-slash path:

```zig
fn cleanUserPathAlloc(allocator: std.mem.Allocator, raw: []const u8) ![]const u8
```

Use an allocated helper if normalization needs to prepend `/`.

**Step 4: Run tests to verify pass**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/apps/files/files.zig
git commit -m "feat: add safe files app path helpers"
```

---

## Task 4: Make Files app list default files root and all-files root

**Objective:** `/api/files/list?path=/...` should return actual filesystem entries from `data/users/<user_id>/files/...` by default, while `/api/files/list?scope=all&path=/...` returns entries from `data/users/<user_id>/...`.

**Files:**
- Modify: `src/apps/files/files.zig:106-184`
- Test: `src/apps/files/files.zig`

**Step 1: Write failing test for directory listing JSON**

Add a unit-testable helper before wiring the HTTP handler:

```zig
test "listDiskDir returns files from default files root" {
    const a = std.testing.allocator;
    const user_id = "test-list-user";
    const root = try @import("../../storage/user_data.zig").ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    const io = compat.getIo();
    const docs_path = try std.fmt.allocPrint(a, "{s}/docs", .{root});
    defer a.free(docs_path);
    std.Io.Dir.cwd().createDir(io, docs_path, @enumFromInt(0o755)) catch {};

    const note_path = try std.fmt.allocPrint(a, "{s}/note.txt", .{root});
    defer a.free(note_path);
    var f = try std.Io.Dir.cwd().createFile(io, note_path, .{});
    _ = try f.writeStreamingAll(io, "hello");
    f.close(io);

    const json = try listDiskDir(a, user_id, .files, "/");
    defer a.free(json);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"docs\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"note.txt\"") != null);
}

test "listDiskDir all scope exposes whole user root" {
    const a = std.testing.allocator;
    const user_id = "test-list-all-user";
    const root = try @import("../../storage/user_data.zig").ensureUserDataDir(a, user_id);
    defer a.free(root);

    const trash_path = try std.fmt.allocPrint(a, "{s}/trash", .{root});
    defer a.free(trash_path);
    std.Io.Dir.cwd().createDir(compat.getIo(), trash_path, @enumFromInt(0o755)) catch {};

    const json = try listDiskDir(a, user_id, .all, "/");
    defer a.free(json);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"trash\"") != null);
}
```

**Step 2: Run test to verify failure**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: FAIL because `listDiskDir` does not exist or handler still uses SQLite.

**Step 3: Implement disk listing**

Parse the optional list scope in `handleList`:

```zig
const scope_str = extractQueryParam(req.head.target, "scope") orelse "files";
const scope: FileScope = if (std.mem.eql(u8, scope_str, "all")) .all else .files;
```

Replace `handleList`'s call from:

```zig
const rows = try listDir(ctx.allocator, user.id, dir_path);
```

To:

```zig
const rows = try listDiskDir(ctx.allocator, user.id, scope, dir_path);
```

Implement `listDiskDir` using `std.Io.Dir`:

```zig
fn listDiskDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, dir_path: []const u8) ![]const u8 {
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, dir_path);
    defer allocator.free(disk_path);

    var dir = std.Io.Dir.cwd().openDir(compat.getIo(), disk_path, .{}) catch return std.fmt.allocPrint(allocator, "{{\"files\":[]}}", .{});
    defer dir.close(compat.getIo());

    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    var it = dir.iterate();
    while (it.next(compat.getIo()) catch null) |entry| {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        const is_dir = entry.kind == .directory;
        const entry_path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ disk_path, entry.name });
        defer allocator.free(entry_path);
        const size: i64 = if (is_dir) 0 else blk: {
            var file = std.Io.Dir.cwd().openFile(compat.getIo(), entry_path, .{}) catch break :blk 0;
            defer file.close(compat.getIo());
            break :blk @intCast(file.length(compat.getIo()) catch 0);
        };
        const now = compat.timestamp();
        const item = try std.fmt.allocPrint(
            allocator,
            "{{\"id\":0,\"name\":\"{s}\",\"size\":{d},\"is_dir\":{s},\"mime_type\":\"application/octet-stream\",\"created_at\":{d},\"updated_at\":{d}}}",
            .{ entry.name, size, if (is_dir) "true" else "false", now, now },
        );
        defer allocator.free(item);
        try buf.appendSlice(allocator, item);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}
```

Important follow-up: escape JSON strings for filenames. Add a small `appendJsonString` helper rather than using raw `{s}` for names, because filenames may contain quotes or backslashes.

**Step 4: Run tests**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/apps/files/files.zig
git commit -m "feat: list files from user data directories"
```

---

## Task 5: Implement Files app upload, mkdir, delete, move against user directories

**Objective:** Make the Files app mutate the actual filesystem under the selected scoped root instead of returning stub success responses. Default mutations target `data/users/<user_id>/files/`; all-files mutations target `data/users/<user_id>/`.

**Files:**
- Modify: `src/apps/files/files.zig:117-156`
- Test: `src/apps/files/files.zig` and `test/e2e_test.sh`

**Step 1: Write failing tests for mkdir/delete/move helpers**

Before changing HTTP handlers, add helper-level tests:

```zig
test "mkdir/delete/move stay inside user root" {
    const a = std.testing.allocator;
    const user_id = "test-mutate-user";
    const root = try @import("../../storage/user_data.zig").ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try makeUserDir(a, user_id, .files, "/", "docs");
    var docs = try std.fmt.allocPrint(a, "{s}/docs", .{root});
    defer a.free(docs);
    var opened = try std.Io.Dir.cwd().openDir(compat.getIo(), docs, .{});
    opened.close(compat.getIo());

    try moveUserEntry(a, user_id, .files, "/docs", "renamed");
    const renamed = try std.fmt.allocPrint(a, "{s}/renamed", .{root});
    defer a.free(renamed);
    var opened2 = try std.Io.Dir.cwd().openDir(compat.getIo(), renamed, .{});
    opened2.close(compat.getIo());

    try deleteUserEntry(a, user_id, .files, "/renamed");
}
```

**Step 2: Run tests to verify failure**

Run:

```bash
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: FAIL because mutation helpers do not exist.

**Step 3: Implement helper functions**

Add helpers:

```zig
fn makeUserDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, parent_path: []const u8, name: []const u8) !void
fn deleteUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) !void
fn moveUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, new_name: []const u8) !void
```

Rules:
- Validate `name` with the same segment rules as `user_data.validateSegment`.
- Reject deleting or moving `/`.
- For delete, try `deleteFile`; if that fails, try `openDir` and recursively delete only if recursive deletion is explicitly desired. If no recursive helper exists, return an error for non-empty directories and let the UI show failure.
- For move, use `std.Io.Dir.renameAbsolute(old_path, new_path, compat.getIo())` per AGENTS.md.

**Step 4: Wire HTTP handlers**

Update handlers:

- `handleMkdir`: require auth, parse body JSON fields `path`, optional `scope`, and `name`, call `makeUserDir`, return JSON.
- `handleDelete`: require auth, parse either the current `id`-based payload or change the frontend to send a `path`; path-based is cleaner for filesystem source of truth.
- `handleMove`: require auth, parse `path` and `new_name`, call `moveUserEntry`.
- `handleUpload`: parse multipart enough for current browser upload, write files under the selected scoped root. By default this is `data/users/<user_id>/files/<currentPath>/<filename>`. If robust multipart parsing is too large, add a preliminary mini parser for the browser's `FormData` shape only and keep scope explicit.

Because moving from SQLite IDs to filesystem paths changes the API shape, update the inline JavaScript in `handleBrowser`:

- Add UI tabs/buttons for `Files` and `All files`. `Files` sets `scope='files'` and navigates within `data/users/<user_id>/files/`; `All files` sets `scope='all'` and navigates within `data/users/<user_id>/`.
- In `renderFiles`, compute each row's `full_path` from `currentPath` plus `f.name` and preserve the current scope.
- For download links, use `/api/files/download?path=...` instead of `id=...`.
- For rename/delete, pass the path instead of the SQLite ID.

**Step 5: Run tests and e2e**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected: build passes, unit tests pass, e2e login still passes. Add e2e checks for mkdir/upload/list/delete if the script can perform them with curl.

**Step 6: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: store files app data in user directory"
```

---

## Task 6: Update file download to use path-based filesystem access

**Objective:** Download should read from the selected scoped root and no longer depend on the SQLite `files` table. Default downloads read from `data/users/<user_id>/files/...`; `scope=all` downloads read from `data/users/<user_id>/...`.

**Files:**
- Modify: `src/apps/files/files.zig:121-144`
- Test: `src/apps/files/files.zig` or `test/e2e_test.sh`

**Step 1: Write failing verification**

Add an e2e check after creating or uploading `note.txt`:

```bash
DOWNLOAD_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" "http://localhost:$DAR_PORT/api/files/download?path=/note.txt")
if [ "$DOWNLOAD_BODY" = "hello" ]; then
    pass_msg "Files download reads from user data directory"
else
    fail_msg "Files download did not return uploaded file content"
fi
```

**Step 2: Run e2e to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL because `handleDownload` currently requires `id` and SQLite metadata.

**Step 3: Replace `handleDownload` lookup**

Use `path` query param:

```zig
const path = extractQueryParam(req.head.target, "path") orelse {
    try response.json(req, "{\"error\":\"missing path\"}");
    return;
};
const scope_str = extractQueryParam(req.head.target, "scope") orelse "files";
const scope: FileScope = if (std.mem.eql(u8, scope_str, "all")) .all else .files;
const disk_path = try joinUserDiskPath(ctx.allocator, user.id, scope, path);
defer ctx.allocator.free(disk_path);
const content_body = std.Io.Dir.cwd().readFileAlloc(compat.getIo(), disk_path, ctx.allocator, .unlimited) catch {
    try response.errorPage(req, 404, "File not on disk");
    return;
};
try req.respond(content_body, .{ .extra_headers = &.{
    .{ .name = "content-type", .value = guessMime(path) },
    .{ .name = "content-disposition", .value = try std.fmt.allocPrint(ctx.allocator, "attachment; filename=\"{s}\"", .{baseName(path)}) },
}});
```

Add simple `baseName` and `guessMime` helpers. Keep MIME guessing minimal: `.html`, `.txt`, default `application/octet-stream`.

**Step 4: Run validation**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: download files from user data directory"
```

---

## Task 7: Update DAV daemon root behavior

**Objective:** Make DAV access consistent with the new `data/users/<user_id>/` storage layout.

**Files:**
- Modify: `src/apps/files/dav_server.zig:5-141`
- Modify: `src/apps/files/dav_daemon.zig:8-69` if still built/referenced
- Possibly modify: `build.zig` or daemon spawn config if `--root data/files` is passed anywhere

**Step 1: Search for daemon root references**

Run:

```bash
rg "data/files|--root|dar-files" src build.zig test
```

Expected: find all old roots and daemon startup paths.

**Step 2: Choose DAV path convention**

Use this convention unless the user chooses otherwise:

- DAV server root is `data/users`.
- DAV URL path starts with the user ID: `http://localhost:8081/<user_id>/...`.
- Later, when DAV auth exists, the authenticated user can map directly to `data/users/<user_id>/` and hide the user ID from the URL.

**Step 3: Update defaults**

In both DAV files, change default root:

```zig
g_root = "data/users";
```

or:

```zig
var storage_root: []const u8 = "data/users";
```

Ensure all path joins still reject traversal. Add a small clean path helper like in Files app if needed.

**Step 4: Add manual DAV verification**

Run after build/server start:

```bash
mkdir -p data/users/test-user
printf 'hello' > data/users/test-user/hello.txt
./zig-out/bin/dar-files --port 8081 --root data/users &
DAV_PID=$!
curl -s http://localhost:8081/test-user/hello.txt
kill "$DAV_PID"
```

Expected: `hello`.

**Step 5: Build and test**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
```

Expected: PASS.

**Step 6: Commit**

```bash
git add src/apps/files/dav_server.zig src/apps/files/dav_daemon.zig build.zig
git commit -m "feat: point DAV storage at user data directories"
```

Only include `build.zig` if it actually changed.

---

## Task 8: Update legacy `data/files` handling and optional migration

**Objective:** Avoid losing existing local test data under `data/files/<user_id>` after switching to `data/users/<user_id>`.

**Files:**
- Optional modify: `src/storage/user_data.zig`
- Optional create: `tools/migrate-data-files.zig` or document a manual command

**Step 1: Decide migration scope**

If this repo already has real data under `data/files`, choose one:

A. No automatic migration; document that this is a fresh-dev breaking storage layout change.
B. On login, if `data/files/<user_id>` exists and `data/users/<user_id>` is empty, rename/copy it into `data/users/<user_id>/files`.
C. One-time manual migration script.

Recommended for now: B if data exists, otherwise A. Keep it simple.

**Step 2: Implement only if needed**

If using automatic migration, add helper:

```zig
pub fn migrateLegacyFilesDirIfPresent(allocator: std.mem.Allocator, user_id: []const u8) !void
```

Behavior:
- Check `data/files/<user_id>`.
- Check `data/users/<user_id>/files`.
- If legacy exists and new files dir does not exist, rename `data/files/<user_id>` to `data/users/<user_id>/files`.
- If both exist, do nothing and log a warning.

**Step 3: Add tests if implemented**

Test the rename path using temporary user IDs and cleanup.

**Step 4: Run validation**

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/storage/user_data.zig src/main.zig
git commit -m "feat: migrate legacy per-user files directories"
```

Skip this commit if no migration is implemented.

---

## Task 9: Final end-to-end validation

**Objective:** Prove that each user has an isolated data folder, Files can access it, and other users cannot see it.

**Files:**
- Modify: `test/e2e_test.sh`

**Step 1: Add scoped listing and two-user isolation e2e scenario**

Extend the dummy OAuth server/test flow if it can issue two different users. If it cannot, add a minimal helper in the test to insert a second user/session directly into SQLite.

Verify:

```bash
# User A
mkdir -p "data/users/$USER_A/files" "data/users/$USER_A/trash"
printf 'a-private' > "data/users/$USER_A/files/private.txt"
printf 'trashed' > "data/users/$USER_A/trash/old.txt"
A_LIST=$(curl -s -b "dar_session=$SESSION_A" "http://localhost:$DAR_PORT/api/files/list?path=/")
# Expected: contains private.txt and does not contain trash
A_ALL_LIST=$(curl -s -b "dar_session=$SESSION_A" "http://localhost:$DAR_PORT/api/files/list?scope=all&path=/")
# Expected: contains files and trash

# User B
mkdir -p "data/users/$USER_B"
B_LIST=$(curl -s -b "dar_session=$SESSION_B" "http://localhost:$DAR_PORT/api/files/list?path=/")
# Expected: does not contain private.txt
```

**Step 2: Run full validation**

Run:

```bash
nix develop /home/anas/soosa --command zig build
nix develop /home/anas/soosa --command zig build test --summary all
bash test/e2e_test.sh
```

Expected:
- Build succeeds.
- Unit tests pass.
- OAuth callback creates `data/users/<user_id>/`.
- Files default listing returns entries from `data/users/<user_id>/files/`.
- Files all-files listing returns entries from `data/users/<user_id>/`.
- Files download reads from that user directory.
- User B cannot see User A's files.

**Step 3: Manual server smoke test**

Run:

```bash
./launch.sh --port 9090
```

Then in another shell:

```bash
curl -s -D- http://localhost:9090/login | head
```

If login flow is available, log in and open `/files`. Expected: the default Files view shows the contents of `data/users/<logged-in-user-id>/files/`; switching to All files shows the contents of `data/users/<logged-in-user-id>/` including app/system folders.

**Step 4: Commit**

```bash
git add test/e2e_test.sh
git commit -m "test: verify per-user files isolation"
```

---

## Risks and tradeoffs

1. Path layout ambiguity: `data/users/<user_id>/` vs `data/<user_id>/`. This plan uses `data/users/<user_id>/` to avoid mixing user IDs with other top-level data directories.
2. Files app currently uses SQLite rows for listing. Switching to filesystem source-of-truth is the cleanest way to make the Files app expose both default files and all user data, but it changes API behavior from ID-based to path-based.
3. Current frontend download/delete/move actions use SQLite IDs. They need to become path-based, or the app needs a synthetic ID scheme. Path-based is simpler and avoids a metadata sync problem.
4. Multipart upload parsing may be non-trivial with only Zig stdlib. Keep the parser scoped to browser `FormData` uploads unless a general parser is actually needed.
5. DAV daemon currently has no user authentication. Rooting it at `data/users` means URLs expose user IDs until DAV auth maps a connection to one user.
6. Existing data under `data/files/<user_id>` may need a migration if it matters.
7. JSON filename escaping must be handled before filesystem names are returned to browser JavaScript. Raw `{s}` in JSON is unsafe for quotes/backslashes.

## Open questions

1. Should the on-disk layout be `data/users/<user_id>/` as this plan assumes, or exactly `data/<user_id>/`?
2. Do existing `data/files/<user_id>` directories need automatic migration into `data/users/<user_id>/files/`, or can this be a clean break for the current dev state?
3. Should DAV URLs include the user ID for now (`/:user_id/...`) until DAV auth exists, or should DAV be deferred until authenticated per-user mapping is designed?
4. Should the SQLite `files` table be removed/ignored entirely for listing, or kept as metadata/cache for future features like tags, sharing, and version history?
5. What exact behavior should delete use for directories: reject non-empty directories, or recursively delete them?
