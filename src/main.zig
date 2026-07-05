//! Dar — self-hosted Nextcloud-like platform. Entry point.
const std = @import("std");
const compat = @import("common/compat.zig");
const c = @import("common/c.zig");
const response = @import("server/response.zig");
const login_page = @import("webui/login.zig");
const home_page = @import("webui/home.zig");
const db_mod = @import("db/db.zig");
const users_mod = @import("auth/users.zig");
const sessions_mod = @import("auth/sessions.zig");
const auth_mod = @import("auth/auth.zig");
const request_mod = @import("server/request.zig");
const router_mod = @import("server/router.zig");
const middleware = @import("server/middleware.zig");
const app_registry = @import("apps/registry.zig");
const app_mod = @import("apps/app.zig");
const daemon_mod = @import("daemon/daemon.zig");
const files_mod = @import("apps/files/files.zig");
const user_data = @import("storage/user_data.zig");

const allocator = std.heap.page_allocator;

pub fn main(init: std.process.Init.Minimal) !void {
    const args = try init.args.toSlice(allocator);

    var port: u16 = 8080;
    var db_path: [:0]const u8 = "dar.db";

    var i: usize = 1;
    while (i < args.len) : (i += 1) {
        if (std.mem.eql(u8, args[i], "--port")) {
            i += 1;
            if (i < args.len) port = try std.fmt.parseInt(u16, args[i], 10);
        } else if (std.mem.eql(u8, args[i], "--db")) {
            i += 1;
            if (i < args.len) db_path = args[i];
        } else if (std.mem.eql(u8, args[i], "--help") or std.mem.eql(u8, args[i], "-h")) {
            printHelp();
            return;
        } else {
            std.debug.print("Unknown flag: {s}\n", .{args[i]});
            printHelp();
            return;
        }
    }

    // Init database
    var db: ?*c.sqlite3 = null;
    const open_rc = c.sqlite3_open(db_path.ptr, &db);
    if (open_rc != c.SQLITE_OK or db == null) {
        std.debug.print("Failed to open database: {s}\n", .{db_path});
        return error.DatabaseError;
    }
    defer _ = c.sqlite3_close(db.?);
    try db_mod.init(db.?);
    try users_mod.init(db.?);
    try sessions_mod.init(db.?);
    try files_mod.init(db.?);

    // Configure OIDC
    const oidc_issuer = envStr(std.c.getenv("DAR_OIDC_ISSUER"));
    const oidc_client_id = envStr(std.c.getenv("DAR_OIDC_CLIENT_ID"));
    const oidc_client_secret = envStr(std.c.getenv("DAR_OIDC_CLIENT_SECRET"));
    const oidc_display_name = blk: {
        const raw = envStr(std.c.getenv("DAR_OIDC_DISPLAY_NAME"));
        break :blk if (raw.len > 0) raw else "OIDC";
    };
    const base_url = blk: {
        const raw = envStr(std.c.getenv("DAR_BASE_URL"));
        break :blk if (raw.len > 0) raw else "http://localhost:8080";
    };

    var has_oidc = oidc_issuer.len > 0 and oidc_client_id.len > 0 and oidc_client_secret.len > 0;
    if (has_oidc) {
        const redirect_uri = try std.fmt.allocPrint(allocator, "{s}/auth/oidc/callback", .{base_url});
        auth_mod.configure(.{
            .issuer = oidc_issuer,
            .client_id = oidc_client_id,
            .client_secret = oidc_client_secret,
            .redirect_uri = redirect_uri,
            .display_name = oidc_display_name,
        }, allocator) catch {
            std.debug.print("Warning: OIDC discovery failed, continuing without auth\n", .{});
            has_oidc = false;
        };
    }

    // Register apps
    try app_registry.register(allocator, files_mod.app);

    // Init daemon manager (skeleton — used in Phase 3+)
    var daemon_mgr = daemon_mod.DaemonManager.init();
    _ = &daemon_mgr; // ponytail: used in Phase 3+

    // Build router
    var router = router_mod.Router.init(allocator);
    try router.get("/", handleIndex);
    try router.get("/login", handleLogin);
    try router.get("/auth/oidc/callback", handleOidcCallback);
    try router.get("/home", middleware.requireAuth(handleHome));
    try router.get("/api/me", middleware.requireAuth(handleApiMe));
    try router.get("/logout", handleLogout);

    // Register app routes directly (main loop already resolves user from session)
    for (app_registry.getApps()) |app| {
        for (app.routes) |route| {
            if (std.mem.eql(u8, route.method, "GET")) {
                try router.get(route.path, route.handler);
            } else if (std.mem.eql(u8, route.method, "POST")) {
                try router.post(route.path, route.handler);
            }
        }
    }

    // Start server (Zig 0.17 std.Io.net API)
    const io = compat.getIo();
    const address = std.Io.net.IpAddress{ .ip4 = std.Io.net.Ip4Address.unspecified(port) };
    var listener = address.listen(io, .{ .reuse_address = true }) catch |err| {
        if (err == error.AddressInUse) {
            std.debug.print("Error: port {d} is already in use. Use --port to pick another.\n", .{port});
        } else {
            std.debug.print("Error: failed to start server: {}\n", .{err});
        }
        std.process.exit(1);
    };
    defer listener.deinit(io);

    std.debug.print("Dar server listening on http://localhost:{d}\n", .{port});
    std.debug.print("Database: {s}\n", .{db_path});

    while (true) {
        var stream = try listener.accept(io);
        defer stream.close(io);

        var rdr_buf: [8192]u8 = undefined;
        var wtr_buf: [8192]u8 = undefined;
        var rdr = stream.reader(io, &rdr_buf);
        var wtr = stream.writer(io, &wtr_buf);

        var http = std.http.Server.init(&rdr.interface, &wtr.interface);
        var req = http.receiveHead() catch continue;

        const method = @tagName(req.head.method);
        const path = req.head.target;
        var log_path_buf: [2048]u8 = undefined;
        const log_path_len = @min(path.len, log_path_buf.len);
        @memcpy(log_path_buf[0..log_path_len], path[0..log_path_len]);
        const log_path = log_path_buf[0..log_path_len];

        var req_ctx = request_mod.parseRequest(allocator, &req) catch request_mod.RequestContext{
            .user = null,
            .cookies = request_mod.CookieJar.init(allocator),
            .allocator = allocator,
        };
        defer req_ctx.deinit();

        if (req_ctx.cookies.get("dar_session")) |session_id| {
            if (sessions_mod.validate(req_ctx.allocator, session_id)) |session| {
                if (users_mod.getById(req_ctx.allocator, session.user_id)) |user| {
                    req_ctx.user = user;
                }
            }
        }

        const handler = router.lookup(method, path);
        if (handler) |h| {
            h(&req, &req_ctx) catch |err| {
                std.debug.print("Handler error [{s} {s}]: {}\n", .{ method, log_path, err });
                response.errorPage(&req, 500, "Internal server error") catch {};
            };
        } else {
            response.errorPage(&req, 404, "Not found") catch {};
        }
    }
}

// ── Handlers ──

fn handleIndex(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    if (ctx.user != null) {
        try response.redirect(req, "/home");
    } else {
        try response.redirect(req, "/login");
    }
}

fn handleLogin(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    var providers: std.ArrayList(login_page.Provider) = .empty;
    if (auth_mod.isConfigured()) {
        const url = auth_mod.getAuthUrl(ctx.allocator) orelse "";
        if (url.len > 0) {
            try providers.append(ctx.allocator, .{ .name = auth_mod.getDisplayName(), .icon = "🔑", .url = url });
        }
    }
    const html = try login_page.render(ctx.allocator, providers.items);
    try response.html(req, html);
}

fn handleOidcCallback(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const code = extractQueryParam(req.head.target, "code") orelse {
        try response.errorPage(req, 400, "Missing authorization code");
        return;
    };
    const profile = auth_mod.exchangeCode(code, ctx.allocator) catch |err| {
        std.debug.print("OIDC exchange failed: {}\n", .{err});
        try response.errorPage(req, 500, "Failed to authenticate");
        return;
    };
    const user = try users_mod.findOrCreate(ctx.allocator, profile);

    // Create per-user data directory and the Files app directory before issuing a session.
    const user_home = try user_data.ensureUserDataDir(ctx.allocator, user.id);
    defer ctx.allocator.free(user_home);
    const files_home = try user_data.ensureUserAppDir(ctx.allocator, user.id, "files");
    defer ctx.allocator.free(files_home);

    const session = try sessions_mod.create(ctx.allocator, user.id);
    const cookie = try std.fmt.allocPrint(ctx.allocator,
        "dar_session={s}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000", .{session.id});

    try req.respond("", .{
        .status = .see_other,
        .extra_headers = &.{
            .{ .name = "location", .value = "/home" },
            .{ .name = "set-cookie", .value = cookie },
        },
    });
}

fn handleHome(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user.?;
    const html = try home_page.render(ctx.allocator, user);
    try response.html(req, html);
}

fn handleApiMe(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user.?;
    const json = try std.fmt.allocPrint(ctx.allocator,
        \\{{"email":"{s}","display_name":"{s}","avatar_url":"{s}","provider":"{s}"}}
    , .{ user.email, user.display_name, user.avatar_url, user.provider });
    try response.json(req, json);
}

fn handleLogout(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    if (ctx.cookies.get("dar_session")) |session_id| {
        sessions_mod.delete(session_id);
    }
    try req.respond("", .{
        .status = .see_other,
        .extra_headers = &.{
            .{ .name = "location", .value = "/login" },
            .{ .name = "set-cookie", .value = "dar_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0" },
        },
    });
}

fn extractQueryParam(target: []const u8, key: []const u8) ?[]const u8 {
    const qm = std.mem.indexOf(u8, target, "?") orelse return null;
    const qs = target[qm + 1 ..];
    var it = std.mem.splitScalar(u8, qs, '&');
    while (it.next()) |pair| {
        var kv = std.mem.splitScalar(u8, pair, '=');
        const k = kv.next() orelse continue;
        const v = kv.next() orelse continue;
        if (std.mem.eql(u8, k, key)) return v;
    }
    return null;
}

fn envStr(ptr: ?[*:0]const u8) []const u8 {
    if (ptr) |p| return std.mem.sliceTo(p, 0);
    return "";
}

fn printHelp() void {
    std.debug.print(
        \\Dar — self-hosted Nextcloud-like platform
        \\
        \\Usage: dar [flags]
        \\
        \\Flags:
        \\  --port <port>   HTTP port (default: 8080)
        \\  --db <path>     SQLite database path (default: dar.db)
        \\  --help, -h      Show this help
        \\
        \\Environment:
        \\  DAR_OIDC_ISSUER        OIDC provider URL (e.g. https://accounts.google.com)
        \\  DAR_OIDC_CLIENT_ID     OIDC client ID
        \\  DAR_OIDC_CLIENT_SECRET OIDC client secret
        \\  DAR_OIDC_DISPLAY_NAME  Name shown on login button (default: OIDC)
        \\  DAR_BASE_URL           Public URL of this server (default: http://localhost:8080)
        \\
    , .{});
}
