//! DAV-compatible HTTP server — custom Head.parse() accepting any HTTP method.
//! Wraps std.http.Server with a custom receiveHead that stores the raw method.
const std = @import("std");
const http = std.http;

pub const Request = struct {
    inner: http.Server.Request,
    raw_method: []const u8,

    pub fn respond(req: *Request, content: []const u8, options: http.Server.Request.RespondOptions) !void {
        try req.inner.respond(content, options);
    }

    pub fn readerExpectNone(req: *Request, buffer: []u8) *std.Io.Reader {
        return req.inner.readerExpectNone(buffer);
    }

    pub fn iterateHeaders(req: *const Request) http.HeaderIterator {
        return http.HeaderIterator.init(req.inner.head_buffer);
    }
};

pub const Server = struct {
    inner: http.Server,

    pub fn init(in: *std.Io.Reader, out: *std.Io.Writer) Server {
        return .{ .inner = http.Server.init(in, out) };
    }

    pub fn receiveHead(s: *Server) !Request {
        // Use the inner server's http.Reader to get raw head buffer
        const head_buffer = try s.inner.reader.receiveHead();
        // Set state to received_head so respond() works
        s.inner.reader.state = .received_head;

        // Parse method manually (accept any string)
        var it = std.mem.splitSequence(u8, head_buffer, "\r\n");
        const first_line = it.next().?;
        const method_end = std.mem.indexOfScalar(u8, first_line, ' ') orelse return error.HttpHeadersInvalid;
        const raw_method = first_line[0..method_end];

        // Try standard method enum, fall back to POST for DAV methods
        const method = std.meta.stringToEnum(http.Method, raw_method) orelse .POST;

        const version_start = std.mem.lastIndexOfScalar(u8, first_line, ' ') orelse return error.HttpHeadersInvalid;
        if (version_start == method_end) return error.HttpHeadersInvalid;
        const version_str = first_line[version_start + 1 ..];
        if (version_str.len < 8) return error.HttpHeadersInvalid;
        const version: http.Version = if (std.mem.eql(u8, version_str[0..@min(8, version_str.len)], "HTTP/1.0")) .@"HTTP/1.0"
            else if (std.mem.eql(u8, version_str[0..@min(8, version_str.len)], "HTTP/1.1")) .@"HTTP/1.1"
            else return error.HttpHeadersInvalid;

        const target = first_line[method_end + 1 .. version_start];

        // Parse headers
        var content_length: ?u64 = null;
        var content_type: ?[]const u8 = null;
        var expect: ?[]const u8 = null;
        var keep_alive: bool = version == .@"HTTP/1.1";
        var transfer_encoding: http.TransferEncoding = .none;
        var transfer_compression: http.ContentEncoding = .identity;

        while (it.next()) |line| {
            if (line.len == 0) break;
            const colon = std.mem.indexOf(u8, line, ":") orelse continue;
            const name = line[0..colon];
            const val = std.mem.trim(u8, line[colon + 1 ..], " \t");
            if (std.ascii.eqlIgnoreCase(name, "content-length")) content_length = std.fmt.parseInt(u64, val, 10) catch null
            else if (std.ascii.eqlIgnoreCase(name, "content-type")) content_type = val
            else if (std.ascii.eqlIgnoreCase(name, "expect")) expect = val
            else if (std.ascii.eqlIgnoreCase(name, "connection")) keep_alive = !std.ascii.eqlIgnoreCase(val, "close")
            else if (std.ascii.eqlIgnoreCase(name, "transfer-encoding")) {
                if (std.ascii.eqlIgnoreCase(val, "chunked")) transfer_encoding = .chunked;
            } else if (std.ascii.eqlIgnoreCase(name, "content-encoding")) {
                if (std.ascii.eqlIgnoreCase(val, "gzip")) transfer_compression = .gzip;
            }
        }

        return .{
            .raw_method = raw_method,
            .inner = .{
                .server = &s.inner,
                .head_buffer = head_buffer,
                .respond_err = null,
                .head = .{
                    .method = method,
                    .target = target,
                    .version = version,
                    .expect = expect,
                    .content_type = content_type,
                    .content_length = content_length,
                    .transfer_encoding = transfer_encoding,
                    .transfer_compression = transfer_compression,
                    .keep_alive = keep_alive,
                },
            },
        };
    }
};
