//! Session management: create, validate, delete. Cookie-based.
const std = @import("std");
const c = @import("../common/c.zig");
const common = @import("../common/common.zig");
const compat = @import("../common/compat.zig");

var g_db: ?*c.sqlite3 = null;

pub fn init(db: *c.sqlite3) !void {
    g_db = db;
}

/// Create a new session for a user. Returns the session with the generated token.
pub fn create(allocator: std.mem.Allocator, user_id: []const u8) !common.Session {
    const token = try generateToken(allocator);
    const now = compat.timestamp();
    const expires = now + 2592000; // 30 days

    var stmt: ?*anyopaque = null;
    const sql = "INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)";
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) {
        std.debug.print("DB error sessions create prepare (rc={d}): {s}\n", .{ rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }
    defer _ = c.sqlite3_finalize(stmt);

    _ = c.sqlite3_bind_text(stmt, 1, token.ptr, @intCast(token.len), c.SQLITE_TRANSIENT);
    _ = c.sqlite3_bind_text(stmt, 2, user_id.ptr, @intCast(user_id.len), c.SQLITE_TRANSIENT);
    _ = c.sqlite3_bind_int64(stmt, 3, now);
    _ = c.sqlite3_bind_int64(stmt, 4, expires);

    const step_rc = c.sqlite3_step(stmt);
    if (step_rc != c.SQLITE_DONE) {
        std.debug.print("DB error sessions create step (rc={d}): {s}\n", .{ step_rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }

    return common.Session{
        .id = token,
        .user_id = try allocator.dupe(u8, user_id),
        .created_at = now,
        .expires_at = expires,
    };
}

/// Validate a session token. Returns the session if valid and not expired, null otherwise.
/// Caller owns all string fields — must free them.
pub fn validate(allocator: std.mem.Allocator, token: []const u8) ?common.Session {
    const sql = "SELECT id, user_id, created_at, expires_at FROM sessions WHERE id = ? AND expires_at > unixepoch()";

    var stmt: ?*anyopaque = null;
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return null;
    defer _ = c.sqlite3_finalize(stmt);

    _ = c.sqlite3_bind_text(stmt, 1, token.ptr, @intCast(token.len), c.SQLITE_TRANSIENT);

    if (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        const id_ptr = c.sqlite3_column_text(stmt, 0) orelse return null;
        const uid_ptr = c.sqlite3_column_text(stmt, 1) orelse return null;
        return common.Session{
            .id = tryStdAlloc(allocator, std.mem.sliceTo(id_ptr, 0)),
            .user_id = tryStdAlloc(allocator, std.mem.sliceTo(uid_ptr, 0)),
            .created_at = c.sqlite3_column_int64(stmt, 2),
            .expires_at = c.sqlite3_column_int64(stmt, 3),
        };
    }
    return null;
}

fn tryStdAlloc(allocator: std.mem.Allocator, s: []const u8) []const u8 {
    return allocator.dupe(u8, s) catch "";
}

/// Delete a session by token.
pub fn delete(token: []const u8) void {
    var stmt: ?*anyopaque = null;
    const sql = "DELETE FROM sessions WHERE id = ?";
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return;
    defer _ = c.sqlite3_finalize(stmt);
    _ = c.sqlite3_bind_text(stmt, 1, token.ptr, @intCast(token.len), c.SQLITE_TRANSIENT);
    _ = c.sqlite3_step(stmt);
}

fn generateToken(allocator: std.mem.Allocator) ![]const u8 {
    var buf: [32]u8 = undefined;
    const n = std.os.linux.getrandom(&buf, buf.len, 0);
    if (n != buf.len) return error.NoEntropy;
    // Format as hex (64 chars)
    var hex: [64]u8 = undefined;
    const hex_chars = "0123456789abcdef";
    for (buf, 0..) |byte, i| {
        hex[i * 2] = hex_chars[byte >> 4];
        hex[i * 2 + 1] = hex_chars[byte & 0xF];
    }
    return allocator.dupe(u8, &hex);
}

fn sliceOrEmpty(ptr: ?[*:0]const u8) []const u8 {
    if (ptr) |p| return std.mem.sliceTo(p, 0);
    return "";
}

test "sessions create and validate" {
    const tmp_path = "/tmp/dar_test_sessions.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};

    var db: ?*c.sqlite3 = null;
    _ = c.sqlite3_open(@ptrCast(tmp_path), &db);
    defer _ = c.sqlite3_close(db.?);

    try @import("../db/db.zig").init(db.?);
    try init(db.?);

    // Create a user first (sessions has FK)
    try @import("../db/db.zig").exec("INSERT INTO users (id, email, display_name, provider, provider_id) VALUES ('u1', 'a@b.com', 'Test', 'google', 'g1')");

    const session = try create(std.testing.allocator, "u1");
    defer {
        std.testing.allocator.free(session.id);
        std.testing.allocator.free(session.user_id);
    }

    try std.testing.expect(session.id.len == 64);

    const valid = validate(std.testing.allocator, session.id);
    try std.testing.expect(valid != null);
    defer {
        std.testing.allocator.free(valid.?.id);
        std.testing.allocator.free(valid.?.user_id);
    }
    try std.testing.expectEqualStrings("u1", valid.?.user_id);

    // Invalid token
    const invalid = validate(std.testing.allocator, "deadbeef");
    try std.testing.expect(invalid == null);

    // Delete
    delete(session.id);
    const after_delete = validate(std.testing.allocator, session.id);
    try std.testing.expect(after_delete == null);

    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
}
