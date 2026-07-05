//! OIDC (OpenID Connect) client: auto-discovers endpoints from .well-known/openid-configuration.
const std = @import("std");
const compat = @import("../common/compat.zig");
const http = @import("http.zig");
const users_mod = @import("users.zig");

pub const OidcConfig = struct {
    issuer: []const u8,
    client_id: []const u8,
    client_secret: []const u8,
    redirect_uri: []const u8,
    display_name: []const u8,
};

const OidcEndpoints = struct {
    authorization_endpoint: []const u8,
    token_endpoint: []const u8,
    userinfo_endpoint: []const u8,
};

var g_config: ?OidcConfig = null;
var g_endpoints: ?OidcEndpoints = null;
var g_configured: bool = false;

pub fn isConfigured() bool {
    return g_configured;
}

pub fn configure(config: OidcConfig, allocator: std.mem.Allocator) !void {
    g_config = config;

    const discovery_url = try std.fmt.allocPrint(allocator, "{s}/.well-known/openid-configuration", .{config.issuer});
    defer allocator.free(discovery_url);

    const discovery_json = try http.httpGet(allocator, discovery_url, null);
    defer allocator.free(discovery_json);

    const auth_ep = extractJsonString(discovery_json, "authorization_endpoint") orelse return error.OidcDiscoveryFailed;
    const token_ep = extractJsonString(discovery_json, "token_endpoint") orelse return error.OidcDiscoveryFailed;
    const userinfo_ep = extractJsonString(discovery_json, "userinfo_endpoint") orelse return error.OidcDiscoveryFailed;

    g_endpoints = OidcEndpoints{
        .authorization_endpoint = try allocator.dupe(u8, auth_ep),
        .token_endpoint = try allocator.dupe(u8, token_ep),
        .userinfo_endpoint = try allocator.dupe(u8, userinfo_ep),
    };
    g_configured = true;
    std.debug.print("OIDC provider configured: {s}\n", .{config.issuer});
}

pub fn getAuthUrl(allocator: std.mem.Allocator) ?[]const u8 {
    const config = g_config orelse return null;
    const eps = g_endpoints orelse return null;
    return std.fmt.allocPrint(allocator,
        "{s}?client_id={s}&redirect_uri={s}&response_type=code&scope=openid%20email%20profile",
        .{ eps.authorization_endpoint, config.client_id, config.redirect_uri },
    ) catch return null;
}

pub fn exchangeCode(code: []const u8, allocator: std.mem.Allocator) !users_mod.OAuthProfile {
    const config = g_config orelse return error.ProviderNotConfigured;
    const eps = g_endpoints orelse return error.ProviderNotConfigured;

    const body = try std.fmt.allocPrint(allocator,
        "code={s}&client_id={s}&client_secret={s}&redirect_uri={s}&grant_type=authorization_code",
        .{ code, config.client_id, config.client_secret, config.redirect_uri },
    );
    defer allocator.free(body);

    const token_response = try http.httpPost(allocator, eps.token_endpoint, "application/x-www-form-urlencoded", @constCast(body));
    defer allocator.free(token_response);

    const access_token = extractJsonString(token_response, "access_token") orelse return error.TokenExchangeFailed;

    const userinfo_response = try http.httpGet(allocator, eps.userinfo_endpoint, access_token);
    defer allocator.free(userinfo_response);

    const email = extractJsonString(userinfo_response, "email") orelse "unknown@oidc";
    const name = extractJsonString(userinfo_response, "name") orelse "OIDC User";
    const picture = extractJsonString(userinfo_response, "picture") orelse "";
    const sub = extractJsonString(userinfo_response, "sub") orelse "unknown";

    return users_mod.OAuthProfile{
        .email = try allocator.dupe(u8, email),
        .display_name = try allocator.dupe(u8, name),
        .avatar_url = try allocator.dupe(u8, picture),
        .provider = try allocator.dupe(u8, config.issuer),
        .provider_id = try allocator.dupe(u8, sub),
    };
}

pub fn getDisplayName() []const u8 {
    if (g_config) |c| return c.display_name;
    return "OIDC";
}

fn extractJsonString(json: []const u8, key: []const u8) ?[]const u8 {
    const search = std.fmt.allocPrint(std.heap.page_allocator, "\"{s}\"", .{key}) catch return null;
    defer std.heap.page_allocator.free(search);
    const start = std.mem.indexOf(u8, json, search) orelse return null;
    var pos = start + search.len;
    while (pos < json.len and (json[pos] == ' ' or json[pos] == ':')) : (pos += 1) {}
    if (pos >= json.len or json[pos] != '"') return null;
    pos += 1;
    const val_start = pos;
    while (pos < json.len and json[pos] != '"') : (pos += 1) {
        if (json[pos] == '\\') pos += 1;
    }
    return json[val_start..pos];
}

test "extractJsonString" {
    const json = "{\"access_token\":\"abc123\",\"token_type\":\"Bearer\"}";
    const token = extractJsonString(json, "access_token");
    try std.testing.expect(token != null);
    try std.testing.expectEqualStrings("abc123", token.?);
    const missing = extractJsonString(json, "refresh_token");
    try std.testing.expect(missing == null);
}

test "auth not configured" {
    try std.testing.expect(!isConfigured());
}
