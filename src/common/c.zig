//! C FFI declarations — sqlite3 only. Zig 0.17.
//! All I/O uses Zig stdlib. No ncurses (this is a web server).

pub const sqlite3 = opaque {};
pub const SQLITE_OK: c_int = 0;
pub const SQLITE_ROW: c_int = 100;
pub const SQLITE_DONE: c_int = 101;
pub const SQLITE_TRANSIENT: ?*anyopaque = @ptrFromInt(@as(usize, @bitCast(@as(isize, -1))));
pub const SQLITE_STATIC: ?*anyopaque = null;
pub const SQLITE_INTEGER: c_int = 1;
pub const SQLITE_TEXT: c_int = 3;

pub extern "c" fn sqlite3_open(filename: [*:0]const u8, ppDb: *?*sqlite3) c_int;
pub extern "c" fn sqlite3_close(db: *sqlite3) c_int;
pub extern "c" fn sqlite3_exec(db: *sqlite3, sql: [*:0]const u8, callback: ?*anyopaque, arg: ?*anyopaque, errmsg: *?[*:0]u8) c_int;
pub extern "c" fn sqlite3_prepare_v2(db: *sqlite3, sql: [*:0]const u8, nByte: c_int, ppStmt: *?*anyopaque, pzTail: ?*[*:0]const u8) c_int;
pub extern "c" fn sqlite3_step(stmt: ?*anyopaque) c_int;
pub extern "c" fn sqlite3_finalize(stmt: ?*anyopaque) c_int;
pub extern "c" fn sqlite3_column_text(stmt: ?*anyopaque, iCol: c_int) ?[*:0]const u8;
pub extern "c" fn sqlite3_column_int64(stmt: ?*anyopaque, iCol: c_int) i64;
pub extern "c" fn sqlite3_bind_text(stmt: ?*anyopaque, idx: c_int, val: [*]const u8, len: c_int, destructor: ?*anyopaque) c_int;
pub extern "c" fn sqlite3_bind_int64(stmt: ?*anyopaque, idx: c_int, val: i64) c_int;
pub extern "c" fn sqlite3_reset(stmt: ?*anyopaque) c_int;
pub extern "c" fn sqlite3_clear_bindings(stmt: ?*anyopaque) c_int;
pub extern "c" fn sqlite3_errmsg(db: *sqlite3) ?[*:0]const u8;
pub extern "c" fn sqlite3_changes(db: *sqlite3) c_int;
pub extern "c" fn sqlite3_last_insert_rowid(db: *sqlite3) i64;
