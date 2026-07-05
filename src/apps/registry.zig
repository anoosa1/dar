//! App registry — collect apps, build app bar and home screen HTML. Material 3 via Tailwind.
const std = @import("std");
const app_mod = @import("app.zig");

var g_apps: std.ArrayList(app_mod.App) = .empty;
var g_initialized: bool = false;
var g_allocator: std.mem.Allocator = undefined;

pub fn register(allocator: std.mem.Allocator, app: app_mod.App) !void {
    if (!g_initialized) { g_allocator = allocator; g_initialized = true; }
    var pos: usize = g_apps.items.len;
    for (g_apps.items, 0..) |existing, idx| {
        if (app.app_bar_order < existing.app_bar_order) { pos = idx; break; }
    }
    try g_apps.insert(g_allocator, pos, app);
}

pub fn getApps() []const app_mod.App { return g_apps.items; }

pub fn buildAppBar(allocator: std.mem.Allocator) ![]const u8 {
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "<nav id=\"dar-app-bar\" class=\"flex items-center gap-1\">");
    for (g_apps.items) |app| {
        const link = try std.fmt.allocPrint(allocator,
            \\<a href="/{s}" aria-label="{s}" title="{s}" class="w-10 h-10 rounded-full inline-flex items-center justify-center text-[20px] text-on-surface-variant hover:bg-surface-container-highest no-underline transition-colors">
            \\  <span aria-hidden="true">{s}</span>
            \\</a>
        , .{ app.name, app.display_name, app.display_name, app.icon });
        defer allocator.free(link);
        try buf.appendSlice(allocator, link);
    }
    try buf.appendSlice(allocator, "</nav>");
    return buf.toOwnedSlice(allocator);
}

pub const SidebarItem = struct {
    label: []const u8,
    icon: []const u8,
    href: []const u8,
    data_scope: []const u8 = "",
    active: bool = false,
};

pub fn buildAppSidebar(allocator: std.mem.Allocator, items: []const SidebarItem) ![]const u8 {
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "<aside id=\"app-sidebar\" class=\"w-60 shrink-0 bg-surface border-r border-outline-variant min-h-[calc(100vh-56px)] flex flex-col\"><nav class=\"p-3 flex flex-col gap-1\">");
    for (items) |item| {
        const cls = if (item.active) "bg-primary-container text-primary" else "text-on-surface-variant hover:bg-surface-container-highest";
        const row = try std.fmt.allocPrint(allocator,
            \\<a href="{s}" data-scope="{s}" class="flex items-center gap-3 rounded-full px-4 h-10 text-sm font-medium no-underline transition-colors {s}">
            \\  <span class="w-5 text-center">{s}</span>
            \\  <span>{s}</span>
            \\</a>
        , .{ item.href, item.data_scope, cls, item.icon, item.label });
        defer allocator.free(row);
        try buf.appendSlice(allocator, row);
    }
    try buf.appendSlice(allocator, "</nav><div class=\"mt-auto p-3 border-t border-outline-variant\"><button type=\"button\" onclick=\"openSettingsDialog()\" class=\"w-full flex items-center gap-3 rounded-full px-4 h-10 text-sm font-medium text-on-surface-variant hover:bg-surface-container-highest border-0 bg-transparent cursor-pointer\"><span>⚙</span><span>Settings</span></button></div></aside>");
    return buf.toOwnedSlice(allocator);
}

pub fn buildHomeCards(allocator: std.mem.Allocator) ![]const u8 {
    if (g_apps.items.len == 0) {
        return "<p class=\"text-on-surface-variant\">No apps installed yet.</p>";
    }

    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "<div class=\"grid grid-cols-2 md:grid-cols-4 gap-4 max-w-2xl mx-auto\">");

    for (g_apps.items) |app| {
        const card = try std.fmt.allocPrint(allocator,
            \\<a href="/{s}" class="bg-surface-container rounded-md shadow-1 hover:shadow-2 transition-shadow p-6 text-center no-underline text-on-surface">
            \\  <div class="text-[40px] mb-3">{s}</div>
            \\  <div class="font-medium text-base">{s}</div>
            \\  <div class="text-on-surface-variant text-sm mt-1">{s}</div>
            \\</a>
        , .{ app.name, app.icon, app.display_name, app.description });
        try buf.appendSlice(allocator, card);
    }

    try buf.appendSlice(allocator, "</div>");
    return buf.toOwnedSlice(allocator);
}

pub fn buildUserInfo(allocator: std.mem.Allocator, user: anytype) ![]const u8 {
    return std.fmt.allocPrint(allocator,
        \\<div class="relative">
        \\  <button id="user-menu-button" type="button" onclick="toggleUserMenu()" class="w-10 h-10 rounded-full bg-primary-container text-primary inline-flex items-center justify-center font-medium border-0 cursor-pointer" aria-label="User menu">
        \\    <span aria-hidden="true">{c}</span>
        \\  </button>
        \\  <div id="user-menu" class="hidden absolute right-0 mt-2 w-48 bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30">
        \\    <div class="px-4 py-2 text-sm font-medium text-on-surface">{s}</div>
        \\    <a href="/settings" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Settings</a>
        \\    <a href="/logout" class="block px-4 py-2 text-sm text-error no-underline hover:bg-error-container">Logout</a>
        \\  </div>
        \\</div>
        \\<script>
        \\function toggleUserMenu(){{let m=document.getElementById('user-menu');if(m)m.classList.toggle('hidden')}}
        \\document.addEventListener('click',function(e){{let b=document.getElementById('user-menu-button');let m=document.getElementById('user-menu');if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add('hidden')}});
        \\</script>
    , .{ if (user.display_name.len > 0) user.display_name[0] else 'U', user.display_name });
}

test "registry insert sorted" {
    g_apps.deinit(std.testing.allocator);
    g_apps = .empty;
    g_initialized = false;
    defer {
        g_apps.deinit(std.testing.allocator);
        g_apps = .empty;
        g_initialized = false;
    }

    try register(std.testing.allocator, .{
        .name = "zulu", .display_name = "Zulu", .icon = "Z", .description = "Last",
        .routes = &.{}, .app_bar_order = 99, .daemon = null,
    });
    try register(std.testing.allocator, .{
        .name = "alpha", .display_name = "Alpha", .icon = "A", .description = "First",
        .routes = &.{}, .app_bar_order = 1, .daemon = null,
    });
    try register(std.testing.allocator, .{
        .name = "mid", .display_name = "Mid", .icon = "M", .description = "Middle",
        .routes = &.{}, .app_bar_order = 50, .daemon = null,
    });
    const apps = getApps();
    try std.testing.expectEqualStrings("alpha", apps[0].name);
    try std.testing.expectEqualStrings("mid", apps[1].name);
    try std.testing.expectEqualStrings("zulu", apps[2].name);
}
