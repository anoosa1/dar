//! User CRUD: find by email, find-or-create from OAuth profile.
const std = @import("std");
const c = @import("../common/c.zig");
const common = @import("../common/common.zig");
const compat = @import("../common/compat.zig");
const user_data = @import("../storage/user_data.zig");

var g_db: ?*c.sqlite3 = null;

pub fn init(db: *c.sqlite3) !void {
    g_db = db;
}

/// Create a new user from an OAuth profile. Returns the allocated User.
pub fn create(allocator: std.mem.Allocator, email: []const u8, display_name: []const u8, avatar_url: []const u8, provider: []const u8, provider_id: []const u8) !common.User {
    const id = try usernameFromEmail(allocator, email);
    const now = compat.timestamp();

    const sql = "INSERT INTO users (id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
    try exec(sql, .{ id, email, display_name, avatar_url, provider, provider_id, now, now });

    return common.User{
        .id = id,
        .email = try allocator.dupe(u8, email),
        .display_name = try allocator.dupe(u8, display_name),
        .avatar_url = try allocator.dupe(u8, avatar_url),
        .provider = try allocator.dupe(u8, provider),
        .provider_id = try allocator.dupe(u8, provider_id),
        .created_at = now,
        .last_login_at = now,
    };
}

/// Find a user by provider + provider_id. Returns null if not found.
/// Caller owns all string fields — must free them.
pub fn findByProvider(allocator: std.mem.Allocator, provider: []const u8, provider_id: []const u8) ?common.User {
    var stmt: ?*anyopaque = null;
    const sql = "SELECT id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at FROM users WHERE provider = ? AND provider_id = ?";
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return null;
    defer _ = c.sqlite3_finalize(stmt);

    bindArgs(stmt, .{ provider, provider_id }) catch return null;

    if (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        return common.User{
            .id = dupeCol(allocator, stmt, 0),
            .email = dupeCol(allocator, stmt, 1),
            .display_name = dupeCol(allocator, stmt, 2),
            .avatar_url = dupeCol(allocator, stmt, 3),
            .provider = dupeCol(allocator, stmt, 4),
            .provider_id = dupeCol(allocator, stmt, 5),
            .created_at = c.sqlite3_column_int64(stmt, 6),
            .last_login_at = c.sqlite3_column_int64(stmt, 7),
        };
    }
    return null;
}

/// Find-or-create: if user exists, update last_login. If not, create.
pub fn findOrCreate(allocator: std.mem.Allocator, profile: OAuthProfile) !common.User {
    // Try to find existing user
    if (findByProvider(allocator, profile.provider, profile.provider_id)) |existing| {
        defer freeUser(allocator, existing);
        const now = compat.timestamp();
        return normalizeExistingUser(allocator, existing, now);
    }
    // Try to create new user
    if (create(allocator, profile.email, profile.display_name, profile.avatar_url, profile.provider, profile.provider_id)) |user| {
        return user;
    } else |_| {
        // Create failed (likely UNIQUE constraint on email — findByProvider missed the existing user).
        // Retry the lookup one more time.
        std.debug.print("findOrCreate: create failed, retrying findByProvider\n", .{});
        if (findByProvider(allocator, profile.provider, profile.provider_id)) |existing| {
            defer freeUser(allocator, existing);
            const now = compat.timestamp();
            return normalizeExistingUser(allocator, existing, now);
        }
        return error.SqliteError;
    }
}

fn normalizeExistingUser(allocator: std.mem.Allocator, existing: common.User, last_login_at: i64) !common.User {
    const username = try usernameFromEmail(allocator, existing.email);
    defer allocator.free(username);
    if (!std.mem.eql(u8, existing.id, username)) {
        try migrateUserId(allocator, existing.id, username, last_login_at);
        return .{
            .id = try allocator.dupe(u8, username),
            .email = try allocator.dupe(u8, existing.email),
            .display_name = try allocator.dupe(u8, existing.display_name),
            .avatar_url = try allocator.dupe(u8, existing.avatar_url),
            .provider = try allocator.dupe(u8, existing.provider),
            .provider_id = try allocator.dupe(u8, existing.provider_id),
            .created_at = existing.created_at,
            .last_login_at = last_login_at,
        };
    }
    try exec("UPDATE users SET last_login_at = ? WHERE id = ?", .{ last_login_at, existing.id });
    return dupUser(allocator, existing, last_login_at);
}

fn migrateUserId(allocator: std.mem.Allocator, old_id: []const u8, username: []const u8, last_login_at: i64) !void {
    try exec("UPDATE users SET id = ?, last_login_at = ? WHERE id = ?", .{ username, last_login_at, old_id });
    try exec("UPDATE sessions SET user_id = ? WHERE user_id = ?", .{ username, old_id });
    try exec("UPDATE files SET user_id = ? WHERE user_id = ?", .{ username, old_id });
    migrateUserDirectory(allocator, old_id, username) catch |err| std.debug.print("user dir migration skipped: {}\n", .{err});
}

fn migrateUserDirectory(allocator: std.mem.Allocator, old_id: []const u8, username: []const u8) !void {
    if (std.mem.indexOfScalar(u8, old_id, '/') != null or std.mem.indexOfScalar(u8, old_id, '\\') != null or std.mem.indexOf(u8, old_id, "..") != null) return error.InvalidPath;
    const old_root = try std.fmt.allocPrint(allocator, "data/users/{s}", .{old_id});
    defer allocator.free(old_root);
    const new_root = try user_data.userRoot(allocator, username);
    defer allocator.free(new_root);
    if (!dirExists(old_root)) return;
    if (dirExists(new_root)) {
        try mergeDirectoryContents(allocator, old_root, new_root);
        return;
    }
    std.Io.Dir.rename(std.Io.Dir.cwd(), old_root, std.Io.Dir.cwd(), new_root, compat.getIo()) catch |err| switch (err) {
        error.FileNotFound => {},
        else => return err,
    };
}

fn mergeDirectoryContents(allocator: std.mem.Allocator, old_root: []const u8, new_root: []const u8) !void {
    var old_dir = try std.Io.Dir.cwd().openDir(compat.getIo(), old_root, .{ .iterate = true });
    defer old_dir.close(compat.getIo());
    var iter = old_dir.iterate();
    while (iter.next(compat.getIo()) catch null) |entry| {
        const old_child = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ old_root, entry.name });
        defer allocator.free(old_child);
        const new_child = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ new_root, entry.name });
        defer allocator.free(new_child);
        if (entry.kind == .directory and dirExists(new_child)) {
            try mergeDirectoryContents(allocator, old_child, new_child);
        } else {
            try std.Io.Dir.rename(std.Io.Dir.cwd(), old_child, std.Io.Dir.cwd(), new_child, compat.getIo());
        }
    }
    try std.Io.Dir.cwd().deleteDir(compat.getIo(), old_root);
}

fn dirExists(path: []const u8) bool {
    var dir = std.Io.Dir.cwd().openDir(compat.getIo(), path, .{}) catch return false;
    dir.close(compat.getIo());
    return true;
}

pub fn freeUser(allocator: std.mem.Allocator, user: common.User) void {
    allocator.free(user.id);
    allocator.free(user.email);
    allocator.free(user.display_name);
    allocator.free(user.avatar_url);
    allocator.free(user.provider);
    allocator.free(user.provider_id);
}

fn dupUser(allocator: std.mem.Allocator, src: common.User, last_login_at: i64) !common.User {
    return .{
        .id = try allocator.dupe(u8, src.id),
        .email = try allocator.dupe(u8, src.email),
        .display_name = try allocator.dupe(u8, src.display_name),
        .avatar_url = try allocator.dupe(u8, src.avatar_url),
        .provider = try allocator.dupe(u8, src.provider),
        .provider_id = try allocator.dupe(u8, src.provider_id),
        .created_at = src.created_at,
        .last_login_at = last_login_at,
    };
}

/// Get user by ID. Caller owns all string fields — must free them.
pub fn getById(allocator: std.mem.Allocator, id: []const u8) ?common.User {
    var stmt: ?*anyopaque = null;
    const sql = "SELECT id, email, display_name, avatar_url, provider, provider_id, created_at, last_login_at FROM users WHERE id = ?";
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return null;
    defer _ = c.sqlite3_finalize(stmt);

    _ = c.sqlite3_bind_text(stmt, 1, id.ptr, @intCast(id.len), c.SQLITE_TRANSIENT);

    if (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        return common.User{
            .id = dupeCol(allocator, stmt, 0),
            .email = dupeCol(allocator, stmt, 1),
            .display_name = dupeCol(allocator, stmt, 2),
            .avatar_url = dupeCol(allocator, stmt, 3),
            .provider = dupeCol(allocator, stmt, 4),
            .provider_id = dupeCol(allocator, stmt, 5),
            .created_at = c.sqlite3_column_int64(stmt, 6),
            .last_login_at = c.sqlite3_column_int64(stmt, 7),
        };
    }
    return null;
}

pub const OAuthProfile = struct {
    email: []const u8,
    display_name: []const u8,
    avatar_url: []const u8,
    provider: []const u8,
    provider_id: []const u8,
};

// ── Internal helpers ──

fn exec(comptime sql: [:0]const u8, args: anytype) !void {
    var stmt: ?*anyopaque = null;
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) {
        std.debug.print("DB error exec prepare (rc={d}): {s}\n", .{ rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }
    defer _ = c.sqlite3_finalize(stmt);
    try bindArgs(stmt, args);
    const step_rc = c.sqlite3_step(stmt);
    if (step_rc != c.SQLITE_DONE) {
        std.debug.print("DB error exec step (rc={d}): {s}\n", .{ step_rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }
}

fn query(comptime sql: [:0]const u8, args: anytype, comptime callback: anytype, context: anytype) void {
    var stmt: ?*anyopaque = null;
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return;
    defer _ = c.sqlite3_finalize(stmt);
    bindArgs(stmt, args) catch return;
    if (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        callback(context, stmt);
    }
    // ponytail: stmt finalized here — any slices from callback are now dangling
    // Callers must dupe data inside their callback, before this function returns
}

fn bindArgs(stmt: ?*anyopaque, args: anytype) !void {
    const info = @typeInfo(@TypeOf(args)).@"struct";
    inline for (info.field_names, 1..) |name, idx| {
        const val = @field(args, name);
        const T = @TypeOf(val);
        if (T == []const u8 or T == [:0]const u8) {
            _ = c.sqlite3_bind_text(stmt, @intCast(idx), val.ptr, @intCast(val.len), c.SQLITE_TRANSIENT);
        } else if (T == i64) {
            _ = c.sqlite3_bind_int64(stmt, @intCast(idx), val);
        } else {
            @compileError("Unsupported bind type: " ++ @typeName(T));
        }
    }
}

fn sliceFromColumn(stmt: ?*anyopaque, col: c_int) ?[]const u8 {
    const ptr = c.sqlite3_column_text(stmt, col) orelse return null;
    return std.mem.sliceTo(ptr, 0);
}

fn dupeCol(allocator: std.mem.Allocator, stmt: ?*anyopaque, col: c_int) []const u8 {
    const ptr = c.sqlite3_column_text(stmt, col) orelse return "";
    const s = std.mem.sliceTo(ptr, 0);
    return allocator.dupe(u8, s) catch "";
}

fn usernameFromEmail(allocator: std.mem.Allocator, email: []const u8) ![]const u8 {
    const at = std.mem.indexOfScalar(u8, email, '@') orelse email.len;
    const raw = email[0..at];
    var out: std.ArrayList(u8) = .empty;
    var last_sep = false;
    for (raw) |ch| {
        if (std.ascii.isAlphanumeric(ch)) {
            try out.append(allocator, std.ascii.toLower(ch));
            last_sep = false;
        } else if ((ch == '-' or ch == '_' or ch == '.' or ch == ' ' or ch == '+') and out.items.len > 0 and !last_sep) {
            try out.append(allocator, '-');
            last_sep = true;
        }
    }
    while (out.items.len > 0 and out.items[out.items.len - 1] == '-') _ = out.pop();
    if (out.items.len == 0) try out.appendSlice(allocator, "user");
    return out.toOwnedSlice(allocator);
}

test "usernameFromEmail uses readable email local-part" {
    const a = std.testing.allocator;
    const id = try usernameFromEmail(a, "Test.User+dar@example.com");
    defer a.free(id);
    try std.testing.expectEqualStrings("test-user-dar", id);
}

test "usernameFromEmail strips unknown characters and spaces" {
    const a = std.testing.allocator;
    const id = try usernameFromEmail(a, " Test 💥 User@example.com");
    defer a.free(id);
    try std.testing.expectEqualStrings("test-user", id);
}

test "users create and find" {
    const tmp_path = "/tmp/dar_test_users.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};

    var db: ?*c.sqlite3 = null;
    _ = c.sqlite3_open(@ptrCast(tmp_path), &db);
    defer _ = c.sqlite3_close(db.?);

    try @import("../db/db.zig").init(db.?);
    try init(db.?);

    const user = try create(std.testing.allocator, "test@example.com", "Test User", "", "google", "g123");
    defer {
        std.testing.allocator.free(user.id);
        std.testing.allocator.free(user.email);
        std.testing.allocator.free(user.display_name);
        std.testing.allocator.free(user.avatar_url);
        std.testing.allocator.free(user.provider);
        std.testing.allocator.free(user.provider_id);
    }

    try std.testing.expectEqualStrings("test@example.com", user.email);
    try std.testing.expectEqualStrings("Test User", user.display_name);

    const found = getById(std.testing.allocator, user.id);
    try std.testing.expect(found != null);
    defer freeUser(std.testing.allocator, found.?);
    try std.testing.expectEqualStrings(user.email, found.?.email);

    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
}

test "users findOrCreate" {
    const tmp_path = "/tmp/dar_test_users_foc.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};

    var db: ?*c.sqlite3 = null;
    _ = c.sqlite3_open(@ptrCast(tmp_path), &db);
    defer _ = c.sqlite3_close(db.?);

    try @import("../db/db.zig").init(db.?);
    try init(db.?);

    const profile = OAuthProfile{
        .email = "new@example.com",
        .display_name = "New User",
        .avatar_url = "",
        .provider = "google",
        .provider_id = "g999",
    };

    const user1 = try findOrCreate(std.testing.allocator, profile);
    defer freeUser(std.testing.allocator, user1);
    const user2 = try findOrCreate(std.testing.allocator, profile);
    defer freeUser(std.testing.allocator, user2);
    try std.testing.expectEqualStrings(user1.id, user2.id); // Same user on second call

    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
}
