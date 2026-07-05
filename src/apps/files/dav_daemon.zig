//! Minimal WebDAV server — handles GET, OPTIONS on a separate port.
//! Standalone binary: ./zig-out/bin/dar-files-dav --port 8081
const std = @import("std");

pub fn main(init: std.process.Init.Minimal) !void {
    const allocator = std.heap.page_allocator;
    const args = try init.args.toSlice(allocator);
    var port: u16 = 8081;
    var storage_root: []const u8 = "data/users";
    var i: usize = 1;
    while (i < args.len) : (i += 1) {
        if (std.mem.eql(u8, args[i], "--port")) { i += 1; if (i < args.len) port = try std.fmt.parseInt(u16, args[i], 10); }
        else if (std.mem.eql(u8, args[i], "--root")) { i += 1; if (i < args.len) storage_root = args[i]; }
    }

    // Minimal Io setup (no compat needed for standalone binary)
    var threaded = std.Io.Threaded.init_single_threaded;
    const io = threaded.io();

    const address = std.Io.net.IpAddress{ .ip4 = std.Io.net.Ip4Address.unspecified(port) };
    var listener = try address.listen(io, .{ .reuse_address = true });
    defer listener.deinit(io);
    std.debug.print("DAV server on http://localhost:{d} (root: {s})\n", .{ port, storage_root });

    while (true) {
        var stream = try listener.accept(io);
        defer stream.close(io);
        var rdr_buf: [8192]u8 = undefined;
        var wtr_buf: [8192]u8 = undefined;
        var rdr = stream.reader(io, &rdr_buf);
        var wtr = stream.writer(io, &wtr_buf);
        var http = std.http.Server.init(&rdr.interface, &wtr.interface);
        var req = http.receiveHead() catch continue;

        const method = @tagName(req.head.method);
        const path = cleanPath(req.head.target);

        if (std.mem.eql(u8, path, "/health")) {
            try req.respond("OK", .{ .extra_headers = &.{.{ .name = "content-type", .value = "text/plain" }} });
        } else if (std.mem.eql(u8, method, "GET")) {
            try serveFile(&req, path, storage_root, allocator);
        } else if (std.mem.eql(u8, method, "OPTIONS")) {
            try req.respond("", .{ .extra_headers = &.{
                .{ .name = "allow", .value = "OPTIONS, GET" },
                .{ .name = "dav", .value = "1" },
            }});
        } else {
            try req.respond("", .{ .status = .method_not_allowed });
        }
    }
}

fn serveFile(req: *std.http.Server.Request, path: []const u8, root: []const u8, allocator: std.mem.Allocator) !void {
    if (std.mem.indexOf(u8, path, "..") != null or std.mem.indexOfScalar(u8, path, '\\') != null) {
        try req.respond("Invalid path", .{ .status = .bad_request });
        return;
    }
    const disk_path = try std.fmt.allocPrint(allocator, "{s}{s}", .{ root, path });
    defer allocator.free(disk_path);

    // Need an Io instance for readFileAlloc
    var threaded = std.Io.Threaded.init_single_threaded;
    const io = threaded.io();

    const content = std.Io.Dir.cwd().readFileAlloc(io, disk_path, allocator, .unlimited) catch {
        try req.respond("Not found", .{ .status = .not_found });
        return;
    };
    // Guess mime type from extension
    const mime = if (std.mem.endsWith(u8, path, ".html")) "text/html"
        else if (std.mem.endsWith(u8, path, ".txt")) "text/plain"
        else "application/octet-stream";
    try req.respond(content, .{ .extra_headers = &.{.{ .name = "content-type", .value = mime }} });
}

fn cleanPath(target: []const u8) []const u8 {
    if (std.mem.indexOf(u8, target, "?")) |qm| return target[0..qm];
    return target;
}
