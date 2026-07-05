//! Daemon manager skeleton — spawn, health-check, restart, kill app daemons.
//! Full implementation in Phase 3+ when apps with daemons are added.
const std = @import("std");
const app_mod = @import("../apps/app.zig");

/// Placeholder — will manage daemon subprocesses in later phases.
pub const DaemonManager = struct {
    pub fn init() DaemonManager {
        return .{};
    }

    pub fn spawn(_: *DaemonManager, _: app_mod.DaemonConfig) !void {
        // ponytail: spawn daemon subprocess will be implemented when apps need it
    }

    pub fn shutdown(_: *DaemonManager) void {
        // ponytail: kill all daemons on shutdown
    }
};
