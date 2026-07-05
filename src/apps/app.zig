//! App interface — each Dar app implements this struct.
const std = @import("std");

/// HTTP route definition for an app.
pub const Route = struct {
    method: []const u8, // "GET", "POST", etc.
    path: []const u8,
    handler: *const fn (*std.http.Server.Request, *@import("../server/request.zig").RequestContext) anyerror!void,
};

/// Daemon config — if set, the app spawns a daemon process on startup.
pub const DaemonConfig = struct {
    command: []const u8,
    args: []const []const u8,
    port: u16,
    health_path: []const u8,
    startup_timeout_ms: u32,
};

/// An app that appears in the Dar app bar and home screen.
pub const App = struct {
    name: []const u8,           // "files" — used in URLs like /files
    display_name: []const u8,   // "Files" — shown in UI
    icon: []const u8,           // SVG or emoji for the app bar icon
    description: []const u8,    // Shown on the home screen card
    routes: []const Route,      // Web UI routes served by the main server
    app_bar_order: u8,          // Position in top app bar (lower = leftmost)
    daemon: ?DaemonConfig,      // If set, spawn this daemon on startup
};
