//! Dummy OIDC test server for testing Dar's auth flow.
//!   ./zig-out/bin/dar-oauth-test --port 8090
const std = @import("std");

pub fn main(init: std.process.Init.Minimal) !void {
    const allocator = std.heap.page_allocator;
    const args = try init.args.toSlice(allocator);

    var port: u16 = 8090;
    var host: []const u8 = "localhost";
    var i: usize = 1;
    while (i < args.len) : (i += 1) {
        if (std.mem.eql(u8, args[i], "--port")) {
            i += 1;
            if (i < args.len) port = try std.fmt.parseInt(u16, args[i], 10);
        } else if (std.mem.eql(u8, args[i], "--host")) {
            i += 1;
            if (i < args.len) host = args[i];
        } else if (std.mem.eql(u8, args[i], "--help") or std.mem.eql(u8, args[i], "-h")) {
            std.debug.print("Usage: dar-oauth-test [--port PORT] [--host HOST]\n", .{});
            std.debug.print("  --port PORT   Port to listen on (default: 8090)\n", .{});
            std.debug.print("  --host HOST   Hostname for OIDC URLs (default: localhost)\n", .{});
            return;
        }
    }

    // Need a threaded Io for networking
    var threaded = std.Io.Threaded.init_single_threaded;
    const io = threaded.io();

    const base = try std.fmt.allocPrint(allocator, "http://{s}:{d}", .{ host, port });
    std.debug.print("Dummy OIDC server on {s}\n", .{base});

    const address = std.Io.net.IpAddress{ .ip4 = std.Io.net.Ip4Address.unspecified(port) };
    var listener = try address.listen(io, .{ .reuse_address = true });
    defer listener.deinit(io);

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

        std.debug.print("[dummy-oidc] {s} {s}\n", .{ method, path });

        if (std.mem.eql(u8, path, "/.well-known/openid-configuration")) {
            try serveDiscovery(&req, base);
        } else if (std.mem.eql(u8, path, "/authorize")) {
            try serveAuthorize(&req);
        } else if (std.mem.eql(u8, path, "/token")) {
            try serveToken(&req);
        } else if (std.mem.eql(u8, path, "/userinfo")) {
            try serveUserinfo(&req);
        } else if (std.mem.eql(u8, path, "/health")) {
            try serveHealth(&req);
        } else {
            try req.respond("Not found", .{ .status = .not_found });
        }
    }
}

fn cleanPath(target: []const u8) []const u8 {
    if (std.mem.indexOf(u8, target, "?")) |qm| return target[0..qm];
    return target;
}

fn serveDiscovery(req: *std.http.Server.Request, base: []const u8) !void {
    const allocator = std.heap.page_allocator;
    const body = try std.fmt.allocPrint(allocator,
        \\{{
        \\  "issuer": "{s}",
        \\  "authorization_endpoint": "{s}/authorize",
        \\  "token_endpoint": "{s}/token",
        \\  "userinfo_endpoint": "{s}/userinfo",
        \\  "jwks_uri": "{s}/jwks",
        \\  "scopes_supported": ["openid", "email", "profile"],
        \\  "response_types_supported": ["code"],
        \\  "grant_types_supported": ["authorization_code"],
        \\  "subject_types_supported": ["public"],
        \\  "id_token_signing_alg_values_supported": ["none"]
        \\}}
    , .{ base, base, base, base, base });
    try respondJson(req, body);
}

fn serveAuthorize(req: *std.http.Server.Request) !void {
    const allocator = std.heap.page_allocator;
    const target = req.head.target;
    const redirect_uri = extractQueryParam(target, "redirect_uri") orelse "/";
    const state = extractQueryParam(target, "state") orelse "";

    const location = if (state.len > 0)
        try std.fmt.allocPrint(allocator, "{s}?code=test-auth-code-12345&state={s}", .{ redirect_uri, state })
    else
        try std.fmt.allocPrint(allocator, "{s}?code=test-auth-code-12345", .{redirect_uri});

    try req.respond("", .{
        .status = .found,
        .extra_headers = &.{.{ .name = "location", .value = location }},
    });
}

fn serveToken(req: *std.http.Server.Request) !void {
    const body =
        \\{
        \\  "access_token": "test-access-token-abc",
        \\  "token_type": "Bearer",
        \\  "expires_in": 3600,
        \\  "id_token": "eyJhbG...IifQ.",
        \\  "scope": "openid email profile"
        \\}
    ;
    try respondJson(req, body);
}

fn serveUserinfo(req: *std.http.Server.Request) !void {
    const body =
        \\{
        \\  "sub": "test-user-123",
        \\  "email": "test@dar.local",
        \\  "name": "Test User",
        \\  "picture": "",
        \\  "email_verified": true
        \\}
    ;
    try respondJson(req, body);
}

fn serveHealth(req: *std.http.Server.Request) !void {
    try respondJson(req, "{\"status\":\"ok\"}");
}

fn respondJson(req: *std.http.Server.Request, body: []const u8) !void {
    try req.respond(body, .{
        .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }},
    });
}

fn extractQueryParam(target: []const u8, key: []const u8) ?[]const u8 {
    const qm = std.mem.indexOf(u8, target, "?") orelse return null;
    const qs = target[qm + 1 ..];
    var it = std.mem.splitScalar(u8, qs, '&');
    while (it.next()) |pair| {
        var kv = std.mem.splitScalar(u8, pair, '=');
        const k = kv.next() orelse continue;
        const v = kv.next() orelse continue;
        if (std.mem.eql(u8, k, key)) return v;
    }
    return null;
}
