//! Login page — OAuth provider buttons. Material 3 via Tailwind.
const std = @import("std");
const layout = @import("layout.zig");

pub fn render(allocator: std.mem.Allocator, providers: []const Provider) ![]const u8 {
    var buf_list: std.ArrayList(u8) = .empty;

    try buf_list.appendSlice(allocator,
        \\<div class="max-w-sm mx-auto mt-10">
        \\  <div class="bg-surface-container rounded-md shadow-1 p-8 text-center">
        \\    <span class="material-symbols-outlined text-primary" style="font-size:48px">cloud</span>
        \\    <h1 class="text-[28px] font-normal leading-9 my-4">Welcome to Dar</h1>
        \\    <p class="text-on-surface-variant mb-8">Sign in to continue</p>
    );

    if (providers.len == 0) {
        try buf_list.appendSlice(allocator,
            \\  <div class="bg-error-container text-on-error-container rounded-sm p-3 text-sm">
            \\    No OAuth providers configured. Set DAR_OIDC_ISSUER, DAR_OIDC_CLIENT_ID, and DAR_OIDC_CLIENT_SECRET.
            \\  </div>
        );
    } else {
        try buf_list.appendSlice(allocator, "<div class=\"flex flex-col gap-2\">");
        for (providers) |p| {
            const line = try std.fmt.allocPrint(allocator,
                \\  <a href="{s}" class="border border-outline rounded-full px-6 h-10 inline-flex items-center justify-center gap-2 font-medium text-sm text-primary hover:bg-primary-container no-underline transition-colors">
                \\    <span class="material-symbols-outlined">{s}</span>
                \\    <span>Continue with {s}</span>
                \\  </a>
            , .{ p.url, p.icon, p.name });
            try buf_list.appendSlice(allocator, line);
        }
        try buf_list.appendSlice(allocator, "</div>");
    }

    try buf_list.appendSlice(allocator,
        \\  </div>
        \\</div>
    );

    const content = try buf_list.toOwnedSlice(allocator);
    return layout.page(allocator, "Login", "", "", content);
}

pub const Provider = struct {
    name: []const u8,
    icon: []const u8,
    url: []const u8,
};
