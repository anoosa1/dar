//! HTML page shell with Material 3 via Tailwind CDN config.
const std = @import("std");
const m3 = @import("material3.zig");

const home_svg = "<svg viewBox=\"0 0 24 24\" width=\"28\" height=\"28\" aria-hidden=\"true\" xmlns=\"http://www.w3.org/2000/svg\"><path fill=\"currentColor\" d=\"M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z\"/></svg>";

pub fn page(allocator: std.mem.Allocator, title: []const u8, app_bar_html: []const u8, user_info_html: []const u8, content_html: []const u8) ![]const u8 {
    return pageWithSidebar(allocator, title, app_bar_html, user_info_html, "", content_html);
}

pub fn pageWithSidebar(allocator: std.mem.Allocator, title: []const u8, app_bar_html: []const u8, user_info_html: []const u8, sidebar_html: []const u8, content_html: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator,
        \\<!DOCTYPE html>
        \\<html lang="en">
        \\<head>
        \\  <meta charset="UTF-8">
        \\  <meta name="viewport" content="width=device-width, initial-scale=1.0">
        \\  <title>{s} — Dar</title>
        \\  {s}
        \\</head>
        \\<body class="bg-background text-on-surface min-h-screen">
        \\  <header class="bg-surface-container h-14 flex items-center px-4 shadow-1 sticky top-0 z-20">
        \\    <a href="/home" aria-label="Home" title="Home" class="w-10 h-10 rounded-full inline-flex items-center justify-center text-primary hover:bg-primary-container no-underline mr-2 shrink-0">{s}</a>
        \\    {s}
        \\    <div class="ml-auto flex items-center gap-2 text-sm">
        \\      {s}
        \\    </div>
        \\  </header>
        \\  <div class="min-h-[calc(100vh-56px)] flex">
        \\    {s}
        \\    <main class="flex-1 min-w-0 px-6 py-6">
        \\      {s}
        \\    </main>
        \\  </div>
        \\</body>
        \\</html>
    , .{ title, m3.head(), home_svg, app_bar_html, user_info_html, sidebar_html, content_html });
}
