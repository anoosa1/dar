//! Request parsing: cookies, query params, JSON body.
const std = @import("std");

/// Per-request context passed to all handlers.
pub const RequestContext = struct {
    user: ?@import("../common/common.zig").User,
    cookies: CookieJar,
    allocator: std.mem.Allocator,

    pub fn deinit(self: *RequestContext) void {
        self.cookies.deinit(self.allocator);
    }
};

/// Simple cookie jar — parsed from the Cookie header.
pub const CookieJar = struct {
    map: std.StringHashMap([]const u8),

    pub fn init(allocator: std.mem.Allocator) CookieJar {
        return .{ .map = std.StringHashMap([]const u8).init(allocator) };
    }

    pub fn deinit(self: *CookieJar, allocator: std.mem.Allocator) void {
        var key_it = self.map.keyIterator();
        while (key_it.next()) |k| {
            allocator.free(k.*);
        }
        var it = self.map.valueIterator();
        while (it.next()) |v| {
            allocator.free(v.*);
        }
        self.map.deinit();
    }

    pub fn get(self: *const CookieJar, name: []const u8) ?[]const u8 {
        return self.map.get(name);
    }
};

/// Parse cookies from the HTTP Cookie header and return a bare RequestContext.
pub fn parseRequest(allocator: std.mem.Allocator, req: anytype) !RequestContext {
    var ctx = RequestContext{
        .user = null,
        .cookies = CookieJar.init(allocator),
        .allocator = allocator,
    };

    // Parse Cookie header
    var header_iter = req.iterateHeaders();
    while (header_iter.next()) |header| {
        if (std.ascii.eqlIgnoreCase(header.name, "cookie")) {
            parseCookieHeader(&ctx.cookies, header.value, allocator) catch {};
            break;
        }
    }

    return ctx;
}

fn parseCookieHeader(jar: *CookieJar, value: []const u8, allocator: std.mem.Allocator) !void {
    var it = std.mem.splitScalar(u8, value, ';');
    while (it.next()) |pair| {
        const trimmed = std.mem.trim(u8, pair, " ");
        var kv = std.mem.splitScalar(u8, trimmed, '=');
        const key = kv.next() orelse continue;
        const val = kv.next() orelse continue;
        try jar.map.put(
            try allocator.dupe(u8, key),
            try allocator.dupe(u8, val),
        );
    }
}

test "parseCookieHeader" {
    var jar = CookieJar.init(std.testing.allocator);
    defer jar.deinit(std.testing.allocator);

    try parseCookieHeader(&jar, "session=abc123; theme=dark", std.testing.allocator);

    try std.testing.expectEqualStrings("abc123", jar.get("session").?);
    try std.testing.expectEqualStrings("dark", jar.get("theme").?);
    try std.testing.expect(jar.get("nonexistent") == null);
}
