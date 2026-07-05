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
