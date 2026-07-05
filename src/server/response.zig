//! HTTP response builders.
const std = @import("std");

pub const Header = struct { name: []const u8, value: []const u8 };

pub fn html(req: anytype, body: []const u8) !void {
    try respond(req, body, "text/html; charset=utf-8", null);
}

pub fn json(req: anytype, body: []const u8) !void {
    try respond(req, body, "application/json", null);
}

pub fn redirect(req: anytype, location: []const u8) !void {
    try req.respond("", .{
        .status = .found,
        .extra_headers = &.{
            .{ .name = "location", .value = location },
        },
    });
}

pub fn seeOther(req: anytype, location: []const u8) !void {
    try req.respond("", .{
        .status = .see_other,
        .extra_headers = &.{
            .{ .name = "location", .value = location },
        },
    });
}

pub fn errorPage(req: anytype, status_code: u16, message: []const u8) !void {
    const body = std.fmt.allocPrint(
        std.heap.page_allocator,
        "<!DOCTYPE html><html><head><meta charset=\"UTF-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Error {d} — Dar</title><style>body{{font:400 14px/20px Roboto,system-ui,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#FEF7FF;color:#1D1B20}}main{{text-align:center;padding:48px}}h1{{font:400 72px/1 Roboto,sans-serif;color:#6750A4;margin:0}}p{{color:#49454F;margin:16px 0}}a{{color:#6750A4;text-decoration:none}}a:hover{{text-decoration:underline}}</style></head><body><main><h1>{d}</h1><p>{s}</p><a href=\"/home\">Back to home</a></main></body></html>",
        .{ status_code, status_code, message },
    ) catch "Error";
    // ponytail: page_allocator.free() corrupts shared pages. Don't free.
    try respond(req, body, "text/html; charset=utf-8", status_code);
}

pub fn noContent(req: anytype) !void {
    try req.respond("", .{ .status = .no_content });
}

fn respond(req: anytype, body: []const u8, content_type: []const u8, status: ?u16) !void {
    var opts = std.http.Server.Request.RespondOptions{
        .extra_headers = &.{
            .{ .name = "content-type", .value = content_type },
        },
    };
    if (status) |s| {
        opts.status = @enumFromInt(s);
    }
    try req.respond(body, opts);
}
