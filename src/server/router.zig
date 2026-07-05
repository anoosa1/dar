//! Path router: match method + path → handler function.
const std = @import("std");
const request_mod = @import("request.zig");

pub const Handler = *const fn (req: *std.http.Server.Request, ctx: *request_mod.RequestContext) anyerror!void;

const Route = struct {
    method: []const u8,
    path: []const u8,
    handler: Handler,
};

pub const Router = struct {
    routes: std.ArrayList(Route),
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator) Router {
        return .{ .routes = std.ArrayList(Route).empty, .allocator = allocator };
    }

    pub fn deinit(self: *Router) void {
        self.routes.deinit(self.allocator);
    }

    pub fn get(self: *Router, path: []const u8, handler: Handler) !void {
        try self.routes.append(self.allocator, .{ .method = "GET", .path = path, .handler = handler });
    }

    pub fn post(self: *Router, path: []const u8, handler: Handler) !void {
        try self.routes.append(self.allocator, .{ .method = "POST", .path = path, .handler = handler });
    }

    /// Look up a handler by method + path. Returns null if not found.
    /// Path is matched literally (no patterns yet).
    pub fn lookup(self: *const Router, method: []const u8, path: []const u8) ?Handler {
        // Strip query string from path
        const clean_path = if (std.mem.indexOf(u8, path, "?")) |qm| path[0..qm] else path;

        for (self.routes.items) |route| {
            if (std.mem.eql(u8, route.method, method) and std.mem.eql(u8, route.path, clean_path)) {
                return route.handler;
            }
        }
        return null;
    }
};

test "router match" {
    var router = Router.init(std.testing.allocator);
    defer router.deinit();

    try router.get("/home", testHandler);
    try router.post("/api/login", testHandler);

    try std.testing.expect(router.lookup("GET", "/home") != null);
    try std.testing.expect(router.lookup("POST", "/api/login") != null);
    try std.testing.expect(router.lookup("GET", "/api/login") == null); // wrong method
    try std.testing.expect(router.lookup("GET", "/nonexistent") == null);

    // Query string stripping
    try std.testing.expect(router.lookup("GET", "/home?foo=bar") != null);
}

fn testHandler(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    _ = req;
    _ = ctx;
}
