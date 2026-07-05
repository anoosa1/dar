//! Home screen — app launcher grid. Material 3 via Tailwind.
const std = @import("std");
const layout = @import("layout.zig");
const registry = @import("../apps/registry.zig");

pub fn render(allocator: std.mem.Allocator, user: anytype) ![]const u8 {
    const app_bar = try registry.buildAppBar(allocator);
    const user_info = try registry.buildUserInfo(allocator, user);
    const cards = try registry.buildHomeCards(allocator);

    const content = try std.fmt.allocPrint(allocator,
        \\<div class="text-center mt-8">
        \\  <h2 class="text-[36px] font-normal leading-[44px] mb-2">Welcome to Dar</h2>
        \\  <p class="text-on-surface-variant mb-10">Your personal cloud platform.</p>
        \\  {s}
        \\</div>
    , .{cards});

    return layout.page(allocator, "Home", app_bar, user_info, content);
}
