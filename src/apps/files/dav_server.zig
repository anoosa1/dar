//! Files DAV daemon - custom DAV-compatible Server for all HTTP methods.
//! Binary: ./zig-out/bin/dar-files --port 8081
const std = @import("std");
const DavServer = @import("dav_http.zig");
var g_root: []const u8 = undefined;

pub fn main(init: std.process.Init.Minimal) !void {
    const al = std.heap.page_allocator;
    const args = try init.args.toSlice(al);
    var port: u16 = 8081;
    g_root = "data/users";
    var i: usize = 1;
    while (i < args.len) : (i += 1) {
        if (std.mem.eql(u8, args[i], "--port")) { i += 1; if (i < args.len) port = try std.fmt.parseInt(u16, args[i], 10); }
        else if (std.mem.eql(u8, args[i], "--root")) { i += 1; if (i < args.len) g_root = args[i]; }
    }
    var th = std.Io.Threaded.init_single_threaded;
    const io = th.io();
    const addr = std.Io.net.IpAddress{ .ip4 = std.Io.net.Ip4Address.unspecified(port) };
    var ln = try addr.listen(io, .{ .reuse_address = true });
    defer ln.deinit(io);
    while (true) {
        var s = try ln.accept(io);
        defer s.close(io);
        var rbuf: [8192]u8 = undefined;
        var wbuf: [8192]u8 = undefined;
        var rdr = s.reader(io, &rbuf);
        var wtr = s.writer(io, &wbuf);
        var h = DavServer.Server.init(&rdr.interface, &wtr.interface);
        var req = h.receiveHead() catch {
            var raw_wtr = s.writer(io, &wbuf);
            try rawResp(&raw_wtr, "400", "", "", "");
            continue;
        };
        try handle(&req, al);
    }
}

fn rawResp(wtr: anytype, st: []const u8, ct: []const u8, extra: []const u8, body: []const u8) !void {
    const al = std.heap.page_allocator;
    var o: std.ArrayList(u8) = .empty;
    try o.appendSlice(al, "HTTP/1.1 "); try o.appendSlice(al, st); try o.appendSlice(al, "\r\n");
    if (ct.len > 0) { try o.appendSlice(al, "Content-Type: "); try o.appendSlice(al, ct); try o.appendSlice(al, "\r\n"); }
    if (extra.len > 0) { try o.appendSlice(al, extra); if (!std.mem.endsWith(u8, extra, "\r\n")) try o.appendSlice(al, "\r\n"); }
    try o.appendSlice(al, try std.fmt.allocPrint(al, "Content-Length: {d}\r\n\r\n", .{body.len}));
    try o.appendSlice(al, body);
    try wtr.interface.writeAll(o.items);
}

fn handle(req: *DavServer.Request, al: std.mem.Allocator) !void {
    const method = req.raw_method;
    const path = cleanPath(req.inner.head.target);

    if (std.mem.eql(u8, path, "/health"))
        try req.inner.respond("OK", .{ .extra_headers = &.{.{ .name = "content-type", .value = "text/plain" }} })
    else if (!try isAuthorized(req, al, path)) {
        _ = req.inner.readerExpectNone(&[_]u8{});
        try req.inner.respond("Unauthorized", .{ .status = .unauthorized, .extra_headers = &.{
            .{ .name = "www-authenticate", .value = "Basic realm=\"Dar Files WebDAV\"" },
            .{ .name = "content-type", .value = "text/plain" },
        } });
    } else if (std.mem.eql(u8, method, "OPTIONS"))
        try req.inner.respond("", .{ .extra_headers = &.{
            .{ .name = "allow", .value = "OPTIONS, GET, PUT, DELETE, MKCOL, PROPFIND, COPY, MOVE, LOCK, UNLOCK" },
            .{ .name = "dav", .value = "1" },
        }})
    else if (std.mem.eql(u8, method, "GET") or std.mem.eql(u8, method, "HEAD"))
        try serveFile(req, path, al, std.mem.eql(u8, method, "HEAD"))
    else if (std.mem.eql(u8, method, "PUT"))
        try servePut(req, path, al)
    else if (std.mem.eql(u8, method, "DELETE"))
        try serveDelete(req, path, al)
    else if (std.mem.eql(u8, method, "PROPFIND")) {
        const depth = headerValue(req, "depth") orelse "infinity";
        _ = req.inner.readerExpectNone(&[_]u8{});
        try servePropfind(req, path, depth, al);
    } else if (std.mem.eql(u8, method, "MKCOL"))
        try serveMkcol(req, path, al)
    else if (std.mem.eql(u8, method, "COPY"))
        try serveCopyMove(req, path, al, false)
    else if (std.mem.eql(u8, method, "MOVE"))
        try serveCopyMove(req, path, al, true)
    else if (std.mem.eql(u8, method, "LOCK")) {
        _ = req.inner.readerExpectNone(&[_]u8{});
        try req.inner.respond("<?xml version=\"1.0\"?><D:prop xmlns:D=\"DAV:\"><D:lockdiscovery/></D:prop>", .{ .extra_headers = &.{.{ .name = "content-type", .value = "application/xml" }} });
    } else if (std.mem.eql(u8, method, "UNLOCK")) {
        _ = req.inner.readerExpectNone(&[_]u8{});
        try req.inner.respond("", .{ .status = .no_content });
    } else {
        _ = req.inner.readerExpectNone(&[_]u8{});
        try req.inner.respond("", .{ .status = .method_not_allowed });
    }
}

fn cleanPath(t: []const u8) []const u8 { return if (std.mem.indexOf(u8, t, "?")) |qm| t[0..qm] else t; }

fn safePath(path: []const u8) !void {
    if (std.mem.indexOf(u8, path, "..") != null) return error.InvalidPath;
    if (std.mem.indexOfScalar(u8, path, '\\') != null) return error.InvalidPath;
}

fn headerValue(req: *DavServer.Request, name: []const u8) ?[]const u8 {
    var it = req.inner.iterateHeaders();
    while (it.next()) |h| {
        if (std.ascii.eqlIgnoreCase(h.name, name)) return h.value;
    }
    return null;
}

fn diskPath(al: std.mem.Allocator, path: []const u8) ![]const u8 {
    return std.fmt.allocPrint(al, "{s}{s}", .{ g_root, path });
}

fn userFromDavPath(path: []const u8) ?[]const u8 {
    if (path.len < 2 or path[0] != '/') return null;
    const rest = path[1..];
    const slash = std.mem.indexOfScalar(u8, rest, '/') orelse return null;
    if (slash == 0) return null;
    return rest[0..slash];
}

fn davKeyPath(al: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    return std.fmt.allocPrint(al, "{s}/{s}/.dav_api_key", .{ g_root, user_id });
}

fn isAuthorized(req: *DavServer.Request, al: std.mem.Allocator, path: []const u8) !bool {
    const user_id = userFromDavPath(path) orelse return false;
    const key_path = try davKeyPath(al, user_id);
    defer al.free(key_path);
    var th = std.Io.Threaded.init_single_threaded;
    const key = std.Io.Dir.cwd().readFileAlloc(th.io(), key_path, al, std.Io.Limit.limited(256)) catch return false;
    defer al.free(key);
    if (key.len == 0) return false;
    if (headerValue(req, "x-dar-dav-key")) |provided| {
        if (std.mem.eql(u8, provided, key)) return true;
    }
    const auth = headerValue(req, "authorization") orelse return false;
    if (std.mem.startsWith(u8, auth, "Bearer ")) return std.mem.eql(u8, auth[7..], key);
    if (!std.mem.startsWith(u8, auth, "Basic ")) return false;
    const encoded = auth[6..];
    const size = std.base64.standard.Decoder.calcSizeForSlice(encoded) catch return false;
    const decoded = try al.alloc(u8, size);
    defer al.free(decoded);
    std.base64.standard.Decoder.decode(decoded, encoded) catch return false;
    const colon = std.mem.indexOfScalar(u8, decoded, ':') orelse return false;
    return std.mem.eql(u8, decoded[0..colon], user_id) and std.mem.eql(u8, decoded[colon + 1 ..], key);
}

fn serveFile(req: *DavServer.Request, path: []const u8, al: std.mem.Allocator, head: bool) !void {
    safePath(path) catch { try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    const d = try diskPath(al, path); defer al.free(d);
    const is_dir = std.Io.Dir.cwd().openDir(io2, d, .{});
    if (is_dir) |*dir| { dir.close(io2);
        try req.inner.respond("", .{ .extra_headers = &.{.{ .name = "content-type", .value = "text/html" }} });
        return;
    } else |_| {}
    const data = std.Io.Dir.cwd().readFileAlloc(io2, d, al, .unlimited) catch { try req.inner.respond("Not found", .{ .status = .not_found }); return; };
    const mime = if (std.mem.endsWith(u8, path, ".html")) "text/html" else if (std.mem.endsWith(u8, path, ".txt")) "text/plain" else "application/octet-stream";
    if (head) { try req.inner.respond("", .{ .extra_headers = &.{.{ .name = "content-type", .value = mime }} }); } else { try req.inner.respond(data, .{ .extra_headers = &.{.{ .name = "content-type", .value = mime }} }); }
}

fn servePut(req: *DavServer.Request, path: []const u8, al: std.mem.Allocator) !void {
    safePath(path) catch { _ = req.inner.readerExpectNone(&[_]u8{}); try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    const d = try diskPath(al, path); defer al.free(d);
    var f = try std.Io.Dir.cwd().createFile(io2, d, .{});
    var cl_buf: [4096]u8 = undefined; const cl_reader = req.inner.readerExpectNone(&cl_buf);
    var rb: [4096]u8 = undefined;
    while (true) { var rv: [1][]u8 = undefined; rv[0] = rb[0..]; const n = cl_reader.readVec(&rv) catch break; if (n == 0) break; _ = try f.writeStreamingAll(io2, rb[0..n]); }
    f.close(io2); try req.inner.respond("", .{ .status = .created });
}

fn serveDelete(req: *DavServer.Request, path: []const u8, al: std.mem.Allocator) !void {
    safePath(path) catch { try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    const d = try diskPath(al, path); defer al.free(d);
    std.Io.Dir.cwd().deleteFile(io2, d) catch {};
    try req.inner.respond("", .{ .status = .no_content });
}

fn servePropfind(req: *DavServer.Request, path: []const u8, depth: []const u8, al: std.mem.Allocator) !void {
    safePath(path) catch { try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    const d = try diskPath(al, path); defer al.free(d);
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    var x: std.ArrayList(u8) = .empty;
    try x.appendSlice(al, "<?xml version=\"1.0\"?>\r\n<D:multistatus xmlns:D=\"DAV:\">\r\n");
    const dir_check = std.Io.Dir.cwd().openDir(io2, d, .{ .iterate = true });
    if (dir_check) |*dir| {
        try appendPropResponse(al, &x, io2, path, d, true);
        if (!std.mem.eql(u8, depth, "0")) {
            var iter = dir.iterate();
            while (iter.next(io2) catch null) |entry| {
                const child_href = if (std.mem.endsWith(u8, path, "/"))
                    try std.fmt.allocPrint(al, "{s}{s}", .{ path, entry.name })
                else
                    try std.fmt.allocPrint(al, "{s}/{s}", .{ path, entry.name });
                const child_disk = try diskPath(al, child_href);
                try appendPropResponse(al, &x, io2, child_href, child_disk, entry.kind == .directory);
            }
        }
        dir.close(io2);
    } else |_| {
        const is_file = std.Io.Dir.cwd().openFile(io2, d, .{});
        if (is_file) |f| { f.close(io2); try appendPropResponse(al, &x, io2, path, d, false); }
        else |_| { try req.inner.respond("Not found", .{ .status = .not_found }); return; }
    }
    try x.appendSlice(al, "</D:multistatus>\r\n");
    try req.inner.respond(x.items, .{ .status = .multi_status, .extra_headers = &.{.{ .name = "content-type", .value = "application/xml" }} });
}

fn appendPropResponse(al: std.mem.Allocator, x: *std.ArrayList(u8), io: std.Io, href: []const u8, path: []const u8, is_dir: bool) !void {
    try x.appendSlice(al, "<D:response><D:href>");
    try x.appendSlice(al, href);
    try x.appendSlice(al, "</D:href><D:propstat><D:prop>");
    if (is_dir) {
        try x.appendSlice(al, "<D:resourcetype><D:collection/></D:resourcetype><D:getcontentlength>0</D:getcontentlength>");
        try x.appendSlice(al, "<D:getetag>\"dir\"</D:getetag><D:getlastmodified>Thu, 01 Jan 1970 00:00:00 GMT</D:getlastmodified>");
    } else {
        var file = try std.Io.Dir.cwd().openFile(io, path, .{});
        defer file.close(io);
        const st = file.stat(io) catch null;
        const size = if (st) |s| s.size else (file.length(io) catch 0);
        const mtime_ns: i128 = if (st) |s| s.mtime.nanoseconds else 0;
        try x.appendSlice(al, "<D:resourcetype/>");
        try x.appendSlice(al, try std.fmt.allocPrint(al, "<D:getcontentlength>{d}</D:getcontentlength><D:getetag>\"{d}-{d}\"</D:getetag><D:getlastmodified>{d}</D:getlastmodified>", .{ size, size, mtime_ns, @divTrunc(mtime_ns, 1_000_000_000) }));
    }
    try x.appendSlice(al, "</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>\r\n");
}

fn serveMkcol(req: *DavServer.Request, path: []const u8, al: std.mem.Allocator) !void {
    _ = req.inner.readerExpectNone(&[_]u8{});
    safePath(path) catch { try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    const d = try diskPath(al, path); defer al.free(d);
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    std.Io.Dir.cwd().createDir(io2, d, @enumFromInt(0o755)) catch { try req.inner.respond("", .{ .status = .method_not_allowed }); return; };
    try req.inner.respond("", .{ .status = .created });
}

fn serveCopyMove(req: *DavServer.Request, path: []const u8, al: std.mem.Allocator, move: bool) !void {
    const destination = headerValue(req, "destination");
    _ = req.inner.readerExpectNone(&[_]u8{});
    safePath(path) catch { try req.inner.respond("Invalid path", .{ .status = .bad_request }); return; };
    const dest_path = parseDestinationPath(destination orelse "") catch { try req.inner.respond("Invalid destination", .{ .status = .bad_request }); return; };
    safePath(dest_path) catch { try req.inner.respond("Invalid destination", .{ .status = .bad_request }); return; };
    const src_disk = try diskPath(al, path); defer al.free(src_disk);
    const dest_disk = try diskPath(al, dest_path); defer al.free(dest_disk);
    var th2 = std.Io.Threaded.init_single_threaded; const io2 = th2.io();
    if (move) {
        std.Io.Dir.rename(std.Io.Dir.cwd(), src_disk, std.Io.Dir.cwd(), dest_disk, io2) catch { try req.inner.respond("", .{ .status = .not_found }); return; };
    } else {
        const bytes = std.Io.Dir.cwd().readFileAlloc(io2, src_disk, al, .unlimited) catch { try req.inner.respond("", .{ .status = .not_found }); return; };
        var f = std.Io.Dir.cwd().createFile(io2, dest_disk, .{}) catch { try req.inner.respond("", .{ .status = .method_not_allowed }); return; };
        defer f.close(io2);
        try f.writeStreamingAll(io2, bytes);
    }
    try req.inner.respond("", .{ .status = .created });
}

fn parseDestinationPath(dest: []const u8) ![]const u8 {
    if (dest.len == 0) return error.InvalidDestination;
    const path = if (std.mem.indexOf(u8, dest, "://")) |scheme_end| blk: {
        const after_host = std.mem.indexOfScalarPos(u8, dest, scheme_end + 3, '/') orelse return error.InvalidDestination;
        break :blk dest[after_host..];
    } else dest;
    if (path.len == 0 or path[0] != '/') return error.InvalidDestination;
    return cleanPath(path);
}

test "DAV destination parser accepts absolute and origin-form paths" {
    try std.testing.expectEqualStrings("/test/files/b.txt", try parseDestinationPath("http://localhost:18081/test/files/b.txt"));
    try std.testing.expectEqualStrings("/test/files/c.txt", try parseDestinationPath("/test/files/c.txt"));
    try std.testing.expectError(error.InvalidDestination, parseDestinationPath("http://localhost:18081"));
}
