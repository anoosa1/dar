//! Module registry — imports all modules for test coverage.
//! Tests live inline in each module. This file just ensures they're compiled.

test {
    _ = @import("common/compat.zig");
    _ = @import("common/c.zig");
    _ = @import("common/common.zig");
    _ = @import("storage/user_data.zig");
    _ = @import("server/response.zig");
    _ = @import("webui/layout.zig");
    _ = @import("webui/login.zig");
    _ = @import("webui/home.zig");
    _ = @import("apps/app.zig");
    _ = @import("apps/registry.zig");
    _ = @import("apps/files/files.zig");
    _ = @import("apps/files/dav_server.zig");
    _ = @import("daemon/daemon.zig");
    _ = @import("db/db.zig");
    _ = @import("auth/users.zig");
    _ = @import("auth/sessions.zig");
    _ = @import("auth/auth.zig");
    _ = @import("server/request.zig");
    _ = @import("server/router.zig");
    _ = @import("server/middleware.zig");
}
