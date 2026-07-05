//! SQLite database wrapper: open, exec, prepared statements, migrations.
const std = @import("std");
const c = @import("../common/c.zig");
const compat = @import("../common/compat.zig");

var g_db: ?*c.sqlite3 = null;

pub fn init(db: *c.sqlite3) !void {
    g_db = db;
    try exec("PRAGMA journal_mode=WAL");
    try runMigrations();
}

fn getDb() *c.sqlite3 {
    return g_db.?;
}

/// Execute a SQL string (no parameters, no return rows). Use for DDL/simple inserts.
pub fn exec(sql: [:0]const u8) !void {
    var err_msg: [*c]u8 = undefined;
    const rc = c.sqlite3_exec(getDb(), sql, null, null, &err_msg);
    if (rc != c.SQLITE_OK) {
        const msg = if (err_msg != null) std.mem.sliceTo(err_msg, 0) else "unknown error";
        std.debug.print("SQLite error: {s}\n", .{msg});
        return error.SqliteError;
    }
}

/// Prepare a statement, call `func` with the stmt, then finalize.
pub fn withStmt(sql: [:0]const u8, comptime func: fn (?*anyopaque) anyerror!void) !void {
    var stmt: ?*anyopaque = null;
    const rc = c.sqlite3_prepare_v2(getDb(), sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) return error.SqliteError;
    defer _ = c.sqlite3_finalize(stmt);
    try func(stmt);
}

/// Run migration SQL to create core tables.
fn runMigrations() !void {
    try exec(
        \\CREATE TABLE IF NOT EXISTS users (
        \\  id TEXT PRIMARY KEY,
        \\  email TEXT NOT NULL UNIQUE,
        \\  display_name TEXT NOT NULL,
        \\  avatar_url TEXT NOT NULL DEFAULT '',
        \\  provider TEXT NOT NULL,
        \\  provider_id TEXT NOT NULL,
        \\  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        \\  last_login_at INTEGER NOT NULL DEFAULT (unixepoch())
        \\)
    );
    try exec(
        \\CREATE TABLE IF NOT EXISTS sessions (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL REFERENCES users(id),
        \\  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        \\  expires_at INTEGER NOT NULL
        \\)
    );
    try exec(
        \\CREATE TABLE IF NOT EXISTS _migrations (
        \\  name TEXT PRIMARY KEY,
        \\  applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        \\)
    );
}

test "db init creates tables" {
    const tmp_path = "/tmp/dar_test_init.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};

    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);

    try init(db.?);

    // Verify users table exists
    try exec("INSERT INTO users (id, email, display_name, provider, provider_id) VALUES ('u1', 'a@b.com', 'Test', 'google', 'g1')");

    // Verify sessions table exists
    try exec("INSERT INTO sessions (id, user_id, expires_at) VALUES ('s1', 'u1', 9999999999)");

    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
}
