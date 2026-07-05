//! HTTP client helpers using std.http.Client (Zig 0.17).
const std = @import("std");
const compat = @import("../common/compat.zig");

pub fn httpGet(allocator: std.mem.Allocator, url_str: []const u8, access_token: ?[]const u8) ![]const u8 {
    const uri = try std.Uri.parse(url_str);

    const auth_header = if (access_token) |token|
        try std.fmt.allocPrint(allocator, "Bearer {s}", .{token})
    else
        null;
    defer if (auth_header) |h| allocator.free(h);

    var extra: [1]std.http.Header = undefined;
    var extra_slice: []const std.http.Header = &.{};
    if (auth_header) |h| {
        extra[0] = .{ .name = "authorization", .value = h };
        extra_slice = &extra;
    }

    var client: std.http.Client = .{ .allocator = allocator, .io = compat.getIo() };
    defer client.deinit();

    var req = try client.request(.GET, uri, .{ .extra_headers = extra_slice });
    defer req.deinit();
    try req.sendBodiless();

    var redirect_buf: [8192]u8 = undefined;
    var resp = try req.receiveHead(&redirect_buf);
    return readAll(allocator, &resp);
}

pub fn httpPost(allocator: std.mem.Allocator, url_str: []const u8, content_type: []const u8, body: []u8) ![]const u8 {
    const uri = try std.Uri.parse(url_str);

    var client: std.http.Client = .{ .allocator = allocator, .io = compat.getIo() };
    defer client.deinit();

    var req = try client.request(.POST, uri, .{
        .extra_headers = &.{.{ .name = "content-type", .value = content_type }},
    });
    defer req.deinit();
    try req.sendBodyComplete(body);

    var redirect_buf: [8192]u8 = undefined;
    var resp = try req.receiveHead(&redirect_buf);
    return readAll(allocator, &resp);
}

fn readAll(allocator: std.mem.Allocator, resp: anytype) ![]const u8 {
    var transfer_buf: [4096]u8 = undefined;
    const reader = resp.reader(&transfer_buf);
    return reader.*.allocRemaining(allocator, .unlimited);
}
