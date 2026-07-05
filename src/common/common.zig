//! Shared types for the Dar application.
const std = @import("std");

pub const User = struct {
    id: []const u8,
    email: []const u8,
    display_name: []const u8,
    avatar_url: []const u8,
    provider: []const u8,
    provider_id: []const u8,
    created_at: i64,
    last_login_at: i64,
};

pub const Session = struct {
    id: []const u8,
    user_id: []const u8,
    created_at: i64,
    expires_at: i64,
};

pub const RequestContext = struct {
    user: ?User,
    allocator: std.mem.Allocator,
};
