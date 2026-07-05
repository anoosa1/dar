const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // ── Main binary: dar ──────────────────────────────────────────────
    const main_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    const main_exe = b.addExecutable(.{ .name = "dar", .root_module = main_mod });
    main_exe.root_module.linkSystemLibrary("sqlite3", .{});
    b.installArtifact(main_exe);

    // ── Dummy OAuth test server ───────────────────────────────────────
    const oauth_mod = b.createModule(.{
        .root_source_file = b.path("src/test_oauth_server.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    const oauth_exe = b.addExecutable(.{ .name = "dar-oauth-test", .root_module = oauth_mod });
    b.installArtifact(oauth_exe);

    // ── Files DAV server (full WebDAV) ────────────────────────────────
    const dav_mod = b.createModule(.{
        .root_source_file = b.path("src/apps/files/dav_server.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    const dav_exe = b.addExecutable(.{ .name = "dar-files", .root_module = dav_mod });
    b.installArtifact(dav_exe);

    // ── Tests ─────────────────────────────────────────────────────────
    const test_mod = b.createModule(.{
        .root_source_file = b.path("src/lib.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    const tests = b.addTest(.{ .root_module = test_mod });
    tests.root_module.linkSystemLibrary("sqlite3", .{});
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_tests.step);
}
