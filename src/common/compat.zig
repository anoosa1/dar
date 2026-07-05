//! Zig 0.17 compat: global single-threaded Io instance.
const std = @import("std");

var threaded: std.Io.Threaded = std.Io.Threaded.init_single_threaded;

pub fn getIo() std.Io {
    return threaded.io();
}

pub fn timestamp() i64 {
    const now = std.Io.Clock.now(std.Io.Clock.real, getIo());
    return @intCast(@divTrunc(now.nanoseconds, 1_000_000_000));
}
