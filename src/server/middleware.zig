//! Middleware: session auth. Wraps handlers to require authentication.
const std = @import("std");
const request_mod = @import("request.zig");
const router_mod = @import("router.zig");
const response = @import("response.zig");

/// Wrap a handler to require authentication — comptime version for known handlers.
pub fn requireAuth(comptime handler: router_mod.Handler) router_mod.Handler {
    return struct {
        fn wrapped(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
            if (ctx.user == null) {
                try response.redirect(req, "/login");
                return;
            }
            try handler(req, ctx);
        }
    }.wrapped;
}
