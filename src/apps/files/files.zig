//! Files app - file browser, upload, download, folder management. Material 3.
const std = @import("std");
const c = @import("../../common/c.zig");
const compat = @import("../../common/compat.zig");
const response = @import("../../server/response.zig");
const request_mod = @import("../../server/request.zig");
const app_mod = @import("../app.zig");
const registry = @import("../registry.zig");
const layout = @import("../../webui/layout.zig");
const user_data = @import("../../storage/user_data.zig");

var g_db: ?*c.sqlite3 = null;

const FileScope = enum { files, all };
const nextcloud_folder_svg = "<svg viewBox=\"0 0 16 16\" height=\"20\" width=\"20\" xmlns=\"http://www.w3.org/2000/svg\" xml:space=\"preserve\" style=\"fill-rule:evenodd;clip-rule:evenodd;stroke-linejoin:round;stroke-miterlimit:2\"><path d=\"M4 20c-.55 0-1.021-.196-1.412-.587A1.927 1.927 0 0 1 2 18V6c0-.55.196-1.021.588-1.413A1.926 1.926 0 0 1 4 4h6l2 2h8c.55 0 1.021.196 1.413.587.391.392.587.863.587 1.413v10a1.93 1.93 0 0 1-.587 1.413A1.93 1.93 0 0 1 20 20H4Z\" style=\"fill:#0082c9;fill-rule:nonzero\" transform=\"matrix(.7 0 0 .7 -.4 -.4)\"/></svg>";

fn envStr(ptr: ?[*:0]const u8) []const u8 {
    if (ptr) |p| return std.mem.sliceTo(p, 0);
    return "";
}

fn davBaseUrl(allocator: std.mem.Allocator) ![]const u8 {
    const raw = envStr(std.c.getenv("DAR_BASE_URL"));
    const base = if (raw.len > 0) raw else "http://localhost:8080";
    const scheme_end = std.mem.indexOf(u8, base, "://") orelse return std.fmt.allocPrint(allocator, "{s}:8081", .{base});
    const scheme = base[0..scheme_end];
    const rest = base[scheme_end + 3 ..];
    const authority_end = std.mem.indexOfScalar(u8, rest, '/') orelse rest.len;
    const authority = rest[0..authority_end];
    const host = if (authority.len > 0 and authority[0] == '[') blk: {
        const close = std.mem.indexOfScalar(u8, authority, ']') orelse authority.len - 1;
        break :blk authority[0 .. close + 1];
    } else if (std.mem.lastIndexOfScalar(u8, authority, ':')) |colon| authority[0..colon] else authority;
    return std.fmt.allocPrint(allocator, "{s}://{s}:8081", .{ scheme, host });
}

fn userDavUrl(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const base = try davBaseUrl(allocator);
    defer allocator.free(base);
    return std.fmt.allocPrint(allocator, "{s}/{s}/files", .{ base, user_id });
}

pub fn init(db: *c.sqlite3) !void {
    g_db = db;
    try exec(
        \\CREATE TABLE IF NOT EXISTS files (
        \\  id INTEGER PRIMARY KEY AUTOINCREMENT,
        \\  user_id TEXT NOT NULL,
        \\  name TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  size INTEGER NOT NULL DEFAULT 0,
        \\  mime_type TEXT NOT NULL DEFAULT 'application/octet-stream',
        \\  is_dir INTEGER NOT NULL DEFAULT 0,
        \\  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        \\  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_files_path ON files(user_id, path)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_trash (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  original_path TEXT NOT NULL,
        \\  trash_path TEXT NOT NULL,
        \\  name TEXT NOT NULL,
        \\  is_dir INTEGER NOT NULL DEFAULT 0,
        \\  size INTEGER NOT NULL DEFAULT 0,
        \\  deleted_at INTEGER NOT NULL
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_file_trash_user ON file_trash(user_id, deleted_at)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_versions (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  version_path TEXT NOT NULL,
        \\  size INTEGER NOT NULL DEFAULT 0,
        \\  created_at INTEGER NOT NULL
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_file_versions_path ON file_versions(user_id, scope, path, created_at)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_activity (
        \\  id INTEGER PRIMARY KEY AUTOINCREMENT,
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  action TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  created_at INTEGER NOT NULL
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_file_activity_user ON file_activity(user_id, created_at)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_shares (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  token TEXT NOT NULL UNIQUE,
        \\  is_dir INTEGER NOT NULL DEFAULT 0,
        \\  expires_at INTEGER NOT NULL DEFAULT 0,
        \\  created_at INTEGER NOT NULL
        \\)
    );
    exec("ALTER TABLE file_shares ADD COLUMN expires_at INTEGER NOT NULL DEFAULT 0") catch {};
    try exec("CREATE INDEX IF NOT EXISTS idx_file_shares_path ON file_shares(user_id, scope, path)");
    try exec("CREATE INDEX IF NOT EXISTS idx_file_shares_token ON file_shares(token)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_favorites (
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  created_at INTEGER NOT NULL,
        \\  PRIMARY KEY (user_id, scope, path)
        \\)
    );
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_tags (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL,
        \\  name TEXT NOT NULL,
        \\  color TEXT NOT NULL DEFAULT '#6750A4',
        \\  created_at INTEGER NOT NULL,
        \\  UNIQUE(user_id, name)
        \\)
    );
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_tag_assignments (
        \\  user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  tag_id TEXT NOT NULL,
        \\  created_at INTEGER NOT NULL,
        \\  PRIMARY KEY (user_id, scope, path, tag_id)
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_file_tag_assignments_tag ON file_tag_assignments(user_id, tag_id)");
    try exec(
        \\CREATE TABLE IF NOT EXISTS file_comments (
        \\  id TEXT PRIMARY KEY,
        \\  user_id TEXT NOT NULL,
        \\  author_user_id TEXT NOT NULL,
        \\  scope TEXT NOT NULL,
        \\  path TEXT NOT NULL,
        \\  body TEXT NOT NULL,
        \\  created_at INTEGER NOT NULL,
        \\  updated_at INTEGER NOT NULL
        \\)
    );
    try exec("CREATE INDEX IF NOT EXISTS idx_file_comments_path ON file_comments(user_id, scope, path, created_at)");
}

pub const app = app_mod.App{
    .name = "files",
    .display_name = "Files",
    .icon = nextcloud_folder_svg,
    .description = "Browse and manage your files",
    .routes = &.{
        .{ .method = "GET", .path = "/files", .handler = &handleBrowser },
        .{ .method = "GET", .path = "/api/files/list", .handler = &handleList },
        .{ .method = "GET", .path = "/api/files/search", .handler = &handleSearch },
        .{ .method = "GET", .path = "/api/files/picker", .handler = &handlePicker },
        .{ .method = "POST", .path = "/api/files/upload", .handler = &handleUpload },
        .{ .method = "GET", .path = "/api/files/download", .handler = &handleDownload },
        .{ .method = "GET", .path = "/api/files/preview", .handler = &handlePreview },
        .{ .method = "POST", .path = "/api/files/mkdir", .handler = &handleMkdir },
        .{ .method = "POST", .path = "/api/files/delete", .handler = &handleDelete },
        .{ .method = "GET", .path = "/api/files/trash/list", .handler = &handleTrashList },
        .{ .method = "POST", .path = "/api/files/trash/restore", .handler = &handleTrashRestore },
        .{ .method = "POST", .path = "/api/files/trash/purge", .handler = &handleTrashPurge },
        .{ .method = "GET", .path = "/api/files/versions/list", .handler = &handleVersionsList },
        .{ .method = "GET", .path = "/api/files/versions/download", .handler = &handleVersionDownload },
        .{ .method = "POST", .path = "/api/files/versions/restore", .handler = &handleVersionRestore },
        .{ .method = "GET", .path = "/api/files/activity/list", .handler = &handleActivityList },
        .{ .method = "POST", .path = "/api/files/shares/create", .handler = &handleShareCreate },
        .{ .method = "GET", .path = "/api/files/shares/list", .handler = &handleShareList },
        .{ .method = "POST", .path = "/api/files/shares/delete", .handler = &handleShareDelete },
        .{ .method = "GET", .path = "/api/files/dav-key", .handler = &handleDavKeyGet },
        .{ .method = "POST", .path = "/api/files/dav-key/reroll", .handler = &handleDavKeyReroll },
        .{ .method = "POST", .path = "/api/files/favorites/toggle", .handler = &handleFavoriteToggle },
        .{ .method = "GET", .path = "/api/files/favorites/list", .handler = &handleFavoritesList },
        .{ .method = "GET", .path = "/api/files/recent", .handler = &handleRecentList },
        .{ .method = "GET", .path = "/api/files/tags", .handler = &handleTagsList },
        .{ .method = "POST", .path = "/api/files/tags/create", .handler = &handleTagCreate },
        .{ .method = "POST", .path = "/api/files/tags/assign", .handler = &handleTagAssign },
        .{ .method = "POST", .path = "/api/files/tags/remove", .handler = &handleTagRemove },
        .{ .method = "GET", .path = "/api/files/tags/files", .handler = &handleTagFiles },
        .{ .method = "GET", .path = "/api/files/comments", .handler = &handleCommentsList },
        .{ .method = "POST", .path = "/api/files/comments/create", .handler = &handleCommentCreate },
        .{ .method = "POST", .path = "/api/files/comments/delete", .handler = &handleCommentDelete },
        .{ .method = "POST", .path = "/api/files/move", .handler = &handleMove },
        .{ .method = "POST", .path = "/api/files/transfer", .handler = &handleTransfer },
        .{ .method = "GET", .path = "/s", .handler = &handlePublicShare },
        .{ .method = "GET", .path = "/files/dav", .handler = &handleDavInfo },
    },
    .app_bar_order = 10,
    .daemon = null,
};

fn handleBrowser(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.redirect(req, "/login");
        return;
    };
    const app_bar = try registry.buildAppBar(ctx.allocator);
    const user_info = try registry.buildUserInfo(ctx.allocator, user);
    const sidebar = try registry.buildAppSidebar(ctx.allocator, &.{
        .{ .label = "Files", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "files", .active = true },
        .{ .label = "All files", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "all" },
        .{ .label = "Deleted files", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "trash" },
        .{ .label = "Favorites", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "favorites" },
        .{ .label = "Tags", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "tags" },
        .{ .label = "Recent", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "recent" },
        .{ .label = "Activity", .icon = nextcloud_folder_svg, .href = "#", .data_scope = "activity" },
    });
    const content: []const u8 =
        \\<div class="max-w-full">
        \\  <div class="flex items-center justify-between mb-6">
        \\    <div id="breadcrumb" class="flex items-center gap-1 text-on-surface-variant text-sm">
        \\      <a href="#" onclick="navigate('/')" class="text-primary hover:underline no-underline">Home</a>
        \\    </div>
        \\    <div class="flex gap-2 items-center">
        \\      <input id="file-search-input" oninput="searchFiles()" class="border border-outline rounded-full px-4 bg-surface text-on-surface outline-none focus:border-primary h-9 text-sm" placeholder="Search files">
        \\      <button onclick="promptMkdir()" class="border border-outline rounded-full px-4 h-9 text-xs font-medium text-primary hover:bg-primary-container">+ Folder</button>
        \\      <label class="bg-primary text-on-primary rounded-full px-4 h-9 text-xs font-medium shadow-1 hover:shadow-2 cursor-pointer inline-flex items-center">Upload<input type="file" id="upload-input" multiple onchange="uploadFiles(this)" class="hidden"></label>
        \\    </div>
        \\  </div>
        \\  <div class="flex items-stretch gap-4 h-[calc(100vh-12rem)] overflow-hidden">
        \\    <div id="drop-zone" class="flex-1 min-w-0 bg-surface-container rounded-md shadow-1 overflow-auto border-2 border-transparent">
        \\      <div id="bulk-actions" class="hidden items-center justify-between gap-3 px-4 py-2 border-b border-outline-variant bg-primary-container text-primary text-sm"><span id="selected-count">0 selected</span><button onclick="bulkDeleteSelected()" class="border border-outline rounded-full px-4 h-8 text-xs font-medium text-error hover:bg-error-container">Delete selected</button></div>
        \\      <table class="w-full border-collapse"><thead id="file-head"></thead><tbody id="file-list"><tr><td colspan="6" class="text-center text-on-surface-variant py-8">Loading...</td></tr></tbody></table>
        \\    </div>
        \\    <aside id="details-pane" class="hidden w-80 shrink-0 h-full overflow-y-auto bg-surface-container rounded-md shadow-1 p-4">
        \\      <div class="flex items-center justify-between mb-4"><h2 class="text-lg font-medium">Details</h2><button onclick="hideDetails()" class="border-0 bg-transparent text-on-surface-variant cursor-pointer">×</button></div>
        \\      <div id="details-content"></div>
        \\    </aside>
        \\  </div>
        \\  <form id="rename-form" class="hidden mt-4 flex gap-2">
        \\    <input id="rename-input" class="flex-1 border border-outline rounded-sm p-2 bg-surface text-on-surface outline-none focus:border-primary h-10" placeholder="New name">
        \\    <button type="submit" class="bg-primary text-on-primary rounded-full px-6 h-10 text-sm font-medium shadow-1 hover:shadow-2 border-0 cursor-pointer">Rename</button>
        \\    <button type="button" onclick="hideRename()" class="border border-outline rounded-full px-6 h-10 text-sm font-medium text-primary hover:bg-primary-container">Cancel</button>
        \\  </form>
        \\  <div id="file-action-menu" class="hidden fixed bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30 min-w-40"></div>
        \\  <div id="dar-dialog" class="hidden fixed inset-0 bg-black/30 z-50 items-center justify-center" onclick="closeDarDialog(null)">
        \\    <div class="bg-surface rounded-xl shadow-3 border border-outline-variant w-full max-w-md p-6" onclick="event.stopPropagation()">
        \\      <h2 id="dar-dialog-title" class="text-xl font-medium mb-3"></h2>
        \\      <p id="dar-dialog-message" class="text-sm text-on-surface-variant mb-4"></p>
        \\      <input id="dar-dialog-input" class="hidden w-full border border-outline rounded-sm px-3 bg-surface text-on-surface h-10 mb-5">
        \\      <div class="flex justify-end gap-2"><button id="dar-dialog-cancel" class="border border-outline rounded-full px-5 h-10 text-sm font-medium text-primary hover:bg-primary-container">Cancel</button><button id="dar-dialog-ok" class="bg-primary text-on-primary rounded-full px-5 h-10 text-sm font-medium shadow-1 hover:shadow-2 border-0 cursor-pointer">OK</button></div>
        \\    </div>
        \\  </div>
        \\  <div id="file-picker-dialog" class="hidden fixed inset-0 bg-black/30 z-40 items-center justify-center" onclick="closeFilePicker()">
        \\    <div class="bg-surface rounded-xl shadow-3 border border-outline-variant w-full max-w-2xl max-h-[80vh] flex flex-col" onclick="event.stopPropagation()">
        \\      <div class="flex items-center justify-between p-4 border-b border-outline-variant"><div><h2 class="text-lg font-medium">Choose folder</h2><div id="file-picker-breadcrumb" class="text-sm text-on-surface-variant mt-1"></div></div><button onclick="closeFilePicker()" class="border-0 bg-transparent text-on-surface-variant cursor-pointer text-xl">×</button></div>
        \\      <div id="file-picker-list" class="overflow-auto min-h-64 p-2"></div>
        \\      <div class="flex justify-end gap-2 p-4 border-t border-outline-variant"><button onclick="closeFilePicker()" class="border border-outline rounded-full px-5 h-10 text-sm font-medium text-primary hover:bg-primary-container">Cancel</button><button onclick="confirmFilePicker('move')" class="bg-primary text-on-primary rounded-full px-5 h-10 text-sm font-medium shadow-1 hover:shadow-2 border-0 cursor-pointer">Move here</button><button onclick="confirmFilePicker('copy')" class="border border-outline rounded-full px-5 h-10 text-sm font-medium text-primary hover:bg-primary-container">Copy here</button></div>
        \\    </div>
        \\  </div>
        \\  <div id="settings-dialog" class="hidden fixed inset-0 bg-black/30 z-40 items-center justify-center" onclick="closeSettingsDialog()">
        \\    <div class="bg-surface-container rounded-xl shadow-3 border border-outline-variant w-full max-w-md p-6" onclick="event.stopPropagation()">
        \\      <div class="flex items-center justify-between mb-5"><h2 class="text-xl font-medium">Files settings</h2><button onclick="closeSettingsDialog()" class="border-0 bg-transparent text-on-surface-variant cursor-pointer text-xl">×</button></div>
        \\      <div class="space-y-4">
        \\        <label class="flex items-center justify-between gap-4 text-sm"><span>Show hidden files</span><input id="toggle-hidden" type="checkbox" onchange="setHidden(this.checked)"></label>
        \\        <label class="flex items-center justify-between gap-4 text-sm"><span>Show file extensions</span><input id="toggle-extensions" type="checkbox" checked onchange="setExtensions(this.checked)"></label>
        \\        <div id="columns-menu" class="border-t border-outline-variant pt-4">
        \\          <div class="text-sm font-medium mb-2">Columns</div>
        \\          <label class="flex items-center justify-between gap-4 text-sm py-1"><span>Size</span><input type="checkbox" checked onchange="setColumn('size',this.checked)"></label>
        \\          <label class="flex items-center justify-between gap-4 text-sm py-1"><span>Modified</span><input type="checkbox" checked onchange="setColumn('modified',this.checked)"></label>
        \\        </div>
        \\        <div class="border-t border-outline-variant pt-4">
        \\          <label for="dav-url-field" class="block text-sm font-medium mb-2">WebDAV URL</label>
        \\          <div class="flex gap-2">
        \\            <input id="dav-url-field" readonly value="DAR_DAV_URL_PLACEHOLDER" class="flex-1 border border-outline rounded-sm px-3 bg-surface text-on-surface text-sm h-10">
        \\            <button type="button" onclick="copyDavUrl()" class="border border-outline rounded-full px-4 h-10 text-sm font-medium text-primary hover:bg-primary-container">Copy link</button>
        \\          </div>
        \\          <label for="dav-api-key-field" class="block text-sm font-medium mt-4 mb-2">WebDAV API key</label>
        \\          <div class="flex gap-2">
        \\            <input id="dav-api-key-field" readonly value="DAR_DAV_API_KEY_PLACEHOLDER" class="flex-1 border border-outline rounded-sm px-3 bg-surface text-on-surface text-sm h-10 font-mono">
        \\            <button type="button" onclick="rerollDavApiKey()" class="border border-outline rounded-full px-4 h-10 text-sm font-medium text-primary hover:bg-primary-container">Reroll</button>
        \\          </div>
        \\          <p class="text-xs text-on-surface-variant mt-2">Use your username and this key as the WebDAV password. Rerolling immediately invalidates the old key.</p>
        \\        </div>
        \\      </div>
        \\    </div>
        \\  </div>
        \\</div>
        \\<script>
        \\let currentPath='/';let currentScope='files';let renameTarget=null;let pickerTarget=null;let pickerPath='/';let currentFiles=[];let showHidden=false;let showExtensions=true;let visibleColumns={size:true,modified:true};let sortKey='name';let sortDir=1;let selectedPaths=new Set();let dialogResolve=null;let searchTimer=null;
        \\function scopeParam(){return '&scope='+encodeURIComponent(currentScope)}
        \\function joinPath(a,b){return a==='/'?'/'+b:a+'/'+b}
        \\function setScope(s){currentScope=s;currentPath='/';selectedPaths.clear();document.getElementById('file-search-input').value='';document.querySelectorAll('#app-sidebar [data-scope]').forEach(a=>{let on=a.dataset.scope===s;a.className='flex items-center gap-3 rounded-full px-4 h-10 text-sm font-medium no-underline transition-colors '+(on?'bg-primary-container text-primary':'text-on-surface-variant hover:bg-surface-container-highest')});navigate('/')}
        \\async function navigate(p){currentPath=p;selectedPaths.clear();let url=currentScope==='activity'?'/api/files/activity/list':(currentScope==='trash'?'/api/files/trash/list':(currentScope==='favorites'?'/api/files/favorites/list':(currentScope==='recent'?'/api/files/recent':(currentScope==='tags'?'/api/files/tags':'/api/files/list?path='+encodeURIComponent(p)+scopeParam()))));let r=await fetch(url);let d=await r.json();renderBreadcrumb(p);if(currentScope==='activity')renderActivity(d);else if(currentScope==='tags')renderTags(d);else renderFiles(d)}
        \\function searchFiles(){clearTimeout(searchTimer);searchTimer=setTimeout(async()=>{let q=document.getElementById('file-search-input').value.trim();if(!q){navigate(currentPath);return}let r=await fetch('/api/files/search?path='+encodeURIComponent(currentPath)+scopeParam()+'&q='+encodeURIComponent(q));let d=await r.json();selectedPaths.clear();renderFiles(d)},200)}
        \\function renderBreadcrumb(p){let root=currentScope==='activity'?'Activity':(currentScope==='trash'?'Deleted files':(currentScope==='favorites'?'Favorites':(currentScope==='tags'?'Tags':(currentScope==='recent'?'Recent':(currentScope==='all'?'All files':'Files')))));let parts=p.split('/').filter(x=>x);let h='<a href="#" onclick="navigate(\'/\')" class="text-primary no-underline">'+root+'</a>';let a='';if(!['trash','activity','favorites','tags','recent'].includes(currentScope))parts.forEach((x,i)=>{a+='/'+x;h+='<span class="mx-1 text-on-surface-variant">/</span>';h+=i===parts.length-1?'<span class="font-medium">'+e(x)+'</span>':'<a href="#" onclick="navigate(\''+a+'\')" class="text-primary no-underline">'+e(x)+'</a>'});document.getElementById('breadcrumb').innerHTML=h}
        \\function renderHead(){let h='<tr><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant w-10"><input id="select-all-files" type="checkbox" onchange="toggleAllSelection(this.checked)"></th><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant"></th><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant"><button onclick="sortBy(\'name\')" class="border-0 bg-transparent text-inherit cursor-pointer">Name</button></th>';if(visibleColumns.size)h+='<th class="text-right p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant w-20"><button onclick="sortBy(\'size\')" class="border-0 bg-transparent text-inherit cursor-pointer">Size</button></th>';if(visibleColumns.modified)h+='<th class="text-right p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant w-36"><button onclick="sortBy(\'updated_at\')" class="border-0 bg-transparent text-inherit cursor-pointer">Modified</button></th>';h+='<th class="text-right p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant w-16">Actions</th></tr>';document.getElementById('file-head').innerHTML=h}
        \\function renderFiles(d){if(d.files)currentFiles=d.files;renderHead();let files=currentFiles.filter(f=>showHidden||!f.name.startsWith('.'));let t=document.getElementById('file-list');if(!files.length){t.innerHTML='<tr><td colspan="6" class="text-center text-on-surface-variant py-8">Empty folder</td></tr>';updateBulkBar();return}files.sort(cmp);let h='';files.forEach(f=>{let full=currentScope==='trash'?f.id:(f.path||joinPath(currentPath,f.name));let rowClass='border-b border-outline-variant hover:bg-surface-container-high '+(f.is_dir&&currentScope!=='trash'?'cursor-pointer':'');let rowClick=(f.is_dir&&currentScope!=='trash')?' onclick="navigate(\''+full+'\')"':' onclick="showDetails(\''+full+'\',findFile(\''+full+'\'))"';h+='<tr class="'+rowClass+'"'+rowClick+'>';h+='<td class="p-3 w-10"><input type="checkbox" '+(selectedPaths.has(full)?'checked':'')+' onclick="event.stopPropagation()" onchange="toggleSelection(\''+full+'\',this.checked)"></td>';h+='<td class="p-3 w-10">'+entryIcon(f)+'</td>';h+='<td class="p-3">'+(f.is_dir?'<span class="text-primary font-medium">'+e(displayName(f.name))+'</span>':e(displayName(f.name)))+(currentScope==='trash'?'<div class="text-xs text-on-surface-variant">'+e(f.original_path||'')+'</div>':(f.path&&f.path!==full?'<div class="text-xs text-on-surface-variant">'+e(f.path)+'</div>':''))+'</td>';if(visibleColumns.size)h+='<td class="p-3 text-right text-on-surface-variant">'+(f.is_dir?'-':fs(f.size))+'</td>';if(visibleColumns.modified)h+='<td class="p-3 text-right text-on-surface-variant">'+fd(f.updated_at||f.deleted_at)+'</td>';h+='<td class="p-3 text-right"><button aria-label="File actions" onclick="event.stopPropagation();openActionMenu(event,\''+full+'\')" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant hover:bg-surface-container-highest cursor-pointer">⋮</button></td></tr>'});t.innerHTML=h;updateBulkBar()}
        \\function renderActivity(d){document.getElementById('file-head').innerHTML='<tr><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant">Action</th><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant">Path</th><th class="text-right p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant">When</th></tr>';let rows=d.activities||[];let t=document.getElementById('file-list');if(!rows.length){t.innerHTML='<tr><td colspan="3" class="text-center text-on-surface-variant py-8">No activity yet</td></tr>';return}let h='';rows.forEach(a=>{h+='<tr class="border-b border-outline-variant"><td class="p-3 font-medium">'+e(a.action)+'</td><td class="p-3 break-all">'+e(a.path)+'</td><td class="p-3 text-right text-on-surface-variant">'+fd(a.created_at)+'</td></tr>'});t.innerHTML=h}
        \\function renderTags(d){document.getElementById('file-head').innerHTML='<tr><th class="text-left p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant">Tag</th><th class="text-right p-3 text-xs font-medium text-on-surface-variant border-b border-outline-variant">Created</th></tr>';let rows=d.tags||[];let t=document.getElementById('file-list');if(!rows.length){t.innerHTML='<tr><td colspan="2" class="text-center text-on-surface-variant py-8">No tags yet</td></tr>';return}let h='';rows.forEach(tag=>{h+='<tr class="border-b border-outline-variant cursor-pointer" onclick="loadTagFiles(\''+tag.id+'\')"><td class="p-3"><span class="inline-block w-3 h-3 rounded-full mr-2" style="background:'+e(tag.color)+'"></span>'+e(tag.name)+'</td><td class="p-3 text-right text-on-surface-variant">'+fd(tag.created_at)+'</td></tr>'});t.innerHTML=h}
        \\async function uploadFiles(i){await uploadFileList(i.files);i.value=''}
        \\async function uploadFileList(files){for(let f of files){let fd=new FormData();fd.append('file',f);let url='/api/files/upload?path='+encodeURIComponent(currentPath)+scopeParam();let r=await fetch(url,{method:'POST',body:fd});if(r.status===409){let replace=await showConfirmDialog(f.name+' already exists','Replace it? Choose Cancel to keep both files.');let fd2=new FormData();fd2.append('file',f);await fetch(url+'&conflict='+(replace?'overwrite':'rename'),{method:'POST',body:fd2})}}navigate(currentPath)}
        \\async function promptMkdir(){let n=await showPromptDialog('New folder','Folder name:','');if(!n)return;await fetch('/api/files/mkdir',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:currentPath,name:n})});navigate(currentPath)}
        \\function sr(p,n){renameTarget=p;document.getElementById('rename-input').value=n;document.getElementById('rename-form').classList.remove('hidden')}
        \\function hideRename(){renameTarget=null;document.getElementById('rename-form').classList.add('hidden')}
        \\function startRename(p){let f=findFile(p);sr(p,f?f.name:nameOfPath(p));document.getElementById('file-action-menu').classList.add('hidden')}
        \\document.getElementById('rename-form').onsubmit=async function(ev){ev.preventDefault();let nn=document.getElementById('rename-input').value;if(!nn||!renameTarget)return;let old=nameOfPath(renameTarget);if(extOf(old)!==extOf(nn)&&!await showConfirmDialog('This changes the file extension','The file may stop working. Continue?'))return;await fetch('/api/files/move',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:renameTarget,new_name:nn})});hideRename();navigate(currentPath)}
        \\async function deleteWithConfirm(p){let f=findFile(p);if(!await showConfirmDialog('Move to deleted files?','Delete '+(f?f.name:p)+'?'))return;await fetch('/api/files/delete',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p})});selectedPaths.delete(p);navigate(currentPath)}
        \\async function restoreTrash(id){await fetch('/api/files/trash/restore',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:id})});navigate('/')}
        \\async function purgeTrash(id){if(!await showConfirmDialog('Delete permanently?','This cannot be undone.'))return;await fetch('/api/files/trash/purge',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:id})});navigate('/')}
        \\async function startMoveCopy(p){pickerTarget=p;pickerPath='/';document.getElementById('file-action-menu').classList.add('hidden');await renderFilePicker('/')}
        \\function openActionMenu(ev,p){let f=findFile(p);if(!f)return;let m=document.getElementById('file-action-menu');if(currentScope==='trash'){m.innerHTML='<button onclick="showDetails(\''+p+'\',findFile(\''+p+'\'))" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Details</button><button onclick="restoreTrash(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Restore</button><button onclick="purgeTrash(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm text-error hover:bg-error-container border-0 bg-transparent">Delete permanently</button>'}else{let dl=f.is_dir?'':'<a href="/api/files/download?path='+encodeURIComponent(p)+scopeParam()+'" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Download</a>';let fav='<button onclick="toggleFavoritePath(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">'+(f.favorite?'Unfavorite':'Favorite')+'</button>';m.innerHTML='<button onclick="showDetails(\''+p+'\',findFile(\''+p+'\'))" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Details</button>'+fav+'<button onclick="startRename(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Rename</button><button onclick="startMoveCopy(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Move or copy</button>'+dl+'<button onclick="deleteWithConfirm(\''+p+'\')" class="block w-full text-left px-4 py-2 text-sm text-error hover:bg-error-container border-0 bg-transparent">Delete</button>'}m.classList.remove('hidden');let r=m.getBoundingClientRect();let x=Math.max(8,Math.min(ev.clientX,window.innerWidth-r.width-8));let y=Math.max(8,Math.min(ev.clientY,window.innerHeight-r.height-8));m.style.left=x+'px';m.style.top=y+'px'}
        \\function showDetails(p,f){if(!f)return;let pane=document.getElementById('details-pane');let c=document.getElementById('details-content');pane.classList.remove('hidden');let preview=(currentScope!=='trash'&&!f.is_dir)?'<div class="border-t border-outline-variant mt-4 pt-4"><div class="font-medium text-sm mb-2">Preview</div><div id="preview-content" class="text-sm text-on-surface-variant">Loading...</div></div>':'';let versions=(currentScope!=='trash'&&!f.is_dir)?'<div class="border-t border-outline-variant mt-4 pt-4"><div class="font-medium text-sm mb-2">Versions</div><div id="versions-list" class="text-sm text-on-surface-variant">Loading...</div></div>':'';let sharing=(currentScope!=='trash'&&currentScope!=='activity')?'<div class="border-t border-outline-variant mt-4 pt-4"><div class="font-medium text-sm mb-2">Sharing</div><label class="block text-xs text-on-surface-variant mb-1" for="share-expiration">Expires</label><input id="share-expiration" type="date" class="w-full border border-outline rounded-sm px-2 bg-surface text-on-surface text-sm h-9 mb-2"><button onclick="createShare(\''+p+'\')" class="bg-primary text-on-primary rounded-full px-4 h-9 text-xs font-medium shadow-1 hover:shadow-2 border-0 cursor-pointer mb-3">Create public link</button><div id="sharing-list" class="text-sm text-on-surface-variant">Loading...</div></div>':'';let tags=(currentScope!=='trash'&&currentScope!=='activity')?'<div class="border-t border-outline-variant mt-4 pt-4"><div class="font-medium text-sm mb-2">Tags</div><div id="tags-list" class="text-sm text-on-surface-variant mb-2">Loading...</div><button onclick="createAndAssignTag(\''+p+'\')" class="border border-outline rounded-full px-4 h-8 text-xs font-medium text-primary hover:bg-primary-container">Add tag</button></div>':'';let comments=(currentScope!=='trash'&&currentScope!=='activity')?'<div class="border-t border-outline-variant mt-4 pt-4"><div class="font-medium text-sm mb-2">Comments</div><div id="comments-list" class="text-sm text-on-surface-variant mb-2">Loading...</div><button onclick="addComment(\''+p+'\')" class="border border-outline rounded-full px-4 h-8 text-xs font-medium text-primary hover:bg-primary-container">Add comment</button></div>':'';c.innerHTML='<div class="mb-3">'+entryIcon(f)+'</div><div class="font-medium mb-1">'+e(f.name)+'</div><button onclick="toggleFavoritePath(\''+p+'\')" class="border-0 bg-transparent text-primary cursor-pointer mb-3">'+(f.favorite?'★ Favorited':'☆ Add favorite')+'</button><dl class="text-sm space-y-2"><div><dt class="text-on-surface-variant">Type</dt><dd>'+(f.is_dir?'Folder':'File')+'</dd></div><div><dt class="text-on-surface-variant">Size</dt><dd>'+(f.is_dir?'-':fs(f.size))+'</dd></div><div><dt class="text-on-surface-variant">Modified</dt><dd>'+fd(f.updated_at)+'</dd></div><div><dt class="text-on-surface-variant">Path</dt><dd class="break-all">'+e(p)+'</dd></div></dl>'+preview+sharing+versions+tags+comments;document.getElementById('file-action-menu').classList.add('hidden');if(preview)loadPreview(p,f);if(sharing)loadShares(p);if(versions)loadVersions(p);if(tags)loadTagsForPath(p);if(comments)loadComments(p)}
        \\async function loadPreview(p,f){let box=document.getElementById('preview-content');if(!box)return;let url='/api/files/preview?path='+encodeURIComponent(p)+scopeParam();let ext=extOf(f.name);if(['png','jpg','jpeg','gif','webp','svg','avif'].includes(ext)){box.innerHTML='<img src="'+url+'" class="max-w-full rounded-md border border-outline-variant" alt="Preview">';return}if(ext==='pdf'){box.innerHTML='<iframe src="'+url+'" class="w-full h-64 rounded-md border border-outline-variant"></iframe>';return}let r=await fetch(url);if(!r.ok){box.textContent='Preview unavailable';return}let text=await r.text();box.innerHTML='<pre class="max-h-64 overflow-auto whitespace-pre-wrap break-words bg-surface rounded-sm p-3 border border-outline-variant">'+e(text)+'</pre>'}
        \\async function loadVersions(p){let box=document.getElementById('versions-list');if(!box)return;let r=await fetch('/api/files/versions/list?path='+encodeURIComponent(p)+scopeParam());let d=await r.json();let versions=d.versions||[];if(!versions.length){box.textContent='No versions yet';return}let h='';versions.forEach(v=>{h+='<div class="flex items-center justify-between gap-2 py-2 border-b border-outline-variant"><div><div>'+fd(v.created_at)+'</div><div class="text-xs">'+fs(v.size)+'</div></div><div class="flex gap-2"><a href="/api/files/versions/download?id='+encodeURIComponent(v.id)+'" class="text-primary no-underline">Download</a><button onclick="restoreVersion(\''+v.id+'\')" class="border-0 bg-transparent text-primary cursor-pointer">Restore</button></div></div>'});box.innerHTML=h}
        \\async function loadShares(p){let box=document.getElementById('sharing-list');if(!box)return;let r=await fetch('/api/files/shares/list?path='+encodeURIComponent(p)+scopeParam());let d=await r.json();let shares=d.shares||[];if(!shares.length){box.textContent='No public links yet';return}let h='';shares.forEach(s=>{let exp=s.expires_at?'<div class="text-xs text-on-surface-variant">Expires '+fd(s.expires_at)+'</div>':'';h+='<div class="flex items-center justify-between gap-2 py-2 border-b border-outline-variant"><div><a href="'+e(s.url)+'" class="text-primary no-underline break-all">'+e(s.url)+'</a>'+exp+'</div><button onclick="copyShareLink(\''+s.url+'\')" class="border-0 bg-transparent text-primary cursor-pointer">Copy</button></div>'});box.innerHTML=h}
        \\async function toggleFavoritePath(p){await fetch('/api/files/favorites/toggle',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p})});navigate(currentPath)}
        \\async function loadTagFiles(id){let r=await fetch('/api/files/tags/files?tag_id='+encodeURIComponent(id));let d=await r.json();currentScope='favorites';renderBreadcrumb('/');renderFiles(d)}
        \\async function loadTagsForPath(p){let box=document.getElementById('tags-list');if(!box)return;let r=await fetch('/api/files/tags?path='+encodeURIComponent(p)+scopeParam());let d=await r.json();let tags=d.tags||[];box.innerHTML=tags.length?tags.map(t=>'<span class="inline-flex items-center rounded-full px-2 h-7 text-xs mr-1 mb-1 bg-primary-container text-primary">'+e(t.name)+'</span>').join(''):'No tags yet'}
        \\async function createAndAssignTag(p){let n=await showPromptDialog('New tag','Tag name:','');if(!n)return;let r=await fetch('/api/files/tags/create',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})});let t=await r.json();await fetch('/api/files/tags/assign',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p,tag_id:t.id})});loadTagsForPath(p)}
        \\async function loadComments(p){let box=document.getElementById('comments-list');if(!box)return;let r=await fetch('/api/files/comments?path='+encodeURIComponent(p)+scopeParam());let d=await r.json();let comments=d.comments||[];box.innerHTML=comments.length?comments.map(c=>'<div class="border-b border-outline-variant py-2"><div class="text-xs text-on-surface-variant">'+e(c.author_user_id)+' · '+fd(c.created_at)+'</div><div>'+e(c.body)+'</div></div>').join(''):'No comments yet'}
        \\async function addComment(p){let b=await showPromptDialog('Add comment','Comment:','');if(!b)return;await fetch('/api/files/comments/create',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p,body:b})});loadComments(p)}
        \\async function createShare(p){let payload={scope:currentScope,path:p};let ex=document.getElementById('share-expiration');if(ex&&ex.value)payload.expires_at=Math.floor(new Date(ex.value+'T23:59:59').getTime()/1000);let r=await fetch('/api/files/shares/create',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});await r.json();loadShares(p)}
        \\async function copyShareLink(u){let full=window.location.origin+u;if(navigator.clipboard)await navigator.clipboard.writeText(full)}
        \\async function restoreVersion(id){await fetch('/api/files/versions/restore',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:id})});navigate(currentPath);hideDetails()}
        \\function hideDetails(){document.getElementById('details-pane').classList.add('hidden')}
        \\function setHidden(v){showHidden=v;renderFiles({files:currentFiles})}
        \\function setExtensions(v){showExtensions=v;renderFiles({files:currentFiles})}
        \\function openSettingsDialog(){let d=document.getElementById('settings-dialog');if(d){d.classList.remove('hidden');d.classList.add('flex')}}
        \\function closeSettingsDialog(){let d=document.getElementById('settings-dialog');if(d){d.classList.add('hidden');d.classList.remove('flex')}}
        \\async function copyDavUrl(){let f=document.getElementById('dav-url-field');if(!f)return;if(navigator.clipboard)await navigator.clipboard.writeText(f.value);else{f.select();document.execCommand('copy')}}
        \\async function rerollDavApiKey(){if(!await showConfirmDialog('Reroll WebDAV API key?','The old key will stop working immediately.'))return;let r=await fetch('/api/files/dav-key/reroll',{method:'POST'});let d=await r.json();let f=document.getElementById('dav-api-key-field');if(f&&d.key)f.value=d.key}
        \\async function renderFilePicker(p){pickerPath=p;let d=document.getElementById('file-picker-dialog');d.classList.remove('hidden');d.classList.add('flex');document.getElementById('file-picker-breadcrumb').textContent=(currentScope==='all'?'All files':'Files')+' '+p;let r=await fetch('/api/files/picker?mode=folder&path='+encodeURIComponent(p)+scopeParam());let data=await r.json();let folders=(data.files||[]).filter(f=>f.is_dir);let h=p==='/'?'':'<button onclick="renderFilePicker(\''+parentOf(p)+'\')" class="w-full text-left flex items-center gap-3 px-3 py-3 rounded-md hover:bg-surface-container-highest border-0 bg-transparent cursor-pointer">'+folderIcon()+'<span>..</span></button>';folders.forEach(f=>{let full=joinPath(p,f.name);h+='<button onclick="renderFilePicker(\''+full+'\')" class="w-full text-left flex items-center gap-3 px-3 py-3 rounded-md hover:bg-surface-container-highest border-0 bg-transparent cursor-pointer">'+folderIcon()+'<span>'+e(f.name)+'</span></button>'});if(!h)h='<div class="text-center text-on-surface-variant py-8">No folders</div>';document.getElementById('file-picker-list').innerHTML=h}
        \\function closeFilePicker(){let d=document.getElementById('file-picker-dialog');d.classList.add('hidden');d.classList.remove('flex');pickerTarget=null}
        \\async function confirmFilePicker(mode){if(!pickerTarget)return;await fetch('/api/files/transfer',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:pickerTarget,target_path:pickerPath,mode:mode})});closeFilePicker();navigate(currentPath)}
        \\function setColumn(k,v){visibleColumns[k]=v;renderFiles({files:currentFiles})}
        \\function sortBy(k){if(sortKey===k)sortDir*=-1;else{sortKey=k;sortDir=1}renderFiles({files:currentFiles})}
        \\function cmp(a,b){if(a.is_dir&&!b.is_dir)return -1;if(!a.is_dir&&b.is_dir)return 1;let av=a[sortKey],bv=b[sortKey];if(sortKey==='name'){av=a.name.toLowerCase();bv=b.name.toLowerCase()}if(av<bv)return -1*sortDir;if(av>bv)return 1*sortDir;return 0}
        \\function displayName(n){if(showExtensions)return n;let i=n.lastIndexOf('.');return i<=0?n:n.slice(0,i)}
        \\function findFile(p){return currentScope==='trash'?currentFiles.find(f=>f.id===p):currentFiles.find(f=>(f.path||joinPath(currentPath,f.name))===p)}
        \\function extOf(n){let i=n.lastIndexOf('.');return i>0?n.slice(i+1).toLowerCase():''}
        \\function nameOfPath(p){let i=p.lastIndexOf('/');return i>=0?p.slice(i+1):p}
        \\function parentOf(p){let i=p.lastIndexOf('/');return i<=0?'/':p.slice(0,i)}
        \\function folderIcon(){return '<svg viewBox="0 0 16 16" height="20" width="20" xmlns="http://www.w3.org/2000/svg" xml:space="preserve" style="fill-rule:evenodd;clip-rule:evenodd;stroke-linejoin:round;stroke-miterlimit:2"><path d="M4 20c-.55 0-1.021-.196-1.412-.587A1.927 1.927 0 0 1 2 18V6c0-.55.196-1.021.588-1.413A1.926 1.926 0 0 1 4 4h6l2 2h8c.55 0 1.021.196 1.413.587.391.392.587.863.587 1.413v10a1.93 1.93 0 0 1-.587 1.413A1.93 1.93 0 0 1 20 20H4Z" style="fill:#0082c9;fill-rule:nonzero" transform="matrix(.7 0 0 .7 -.4 -.4)"/></svg>'}
        \\function fileIconSvg(fill,label){return '<svg viewBox="0 0 24 24" height="22" width="22" aria-label="'+label+'" xmlns="http://www.w3.org/2000/svg"><path d="M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zm-1 7V3.5L18.5 9H13z" fill="'+fill+'"/></svg>'}
        \\function pdfIcon(){return '<svg width="22" height="22" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M19 3H5C3.9 3 3 3.9 3 5V19C3 20.1 3.9 21 5 21H19C20.1 21 21 20.1 21 19V5C21 3.9 20.1 3 19 3ZM9.5 11.5C9.5 12.3 8.8 13 8 13H7V15H5.5V9H8C8.8 9 9.5 9.7 9.5 10.5V11.5ZM14.5 13.5C14.5 14.3 13.8 15 13 15H10.5V9H13C13.8 9 14.5 9.7 14.5 10.5V13.5ZM18.5 10.5H17V11.5H18.5V13H17V15H15.5V9H18.5V10.5ZM12 10.5H13V13.5H12V10.5ZM7 10.5H8V11.5H7V10.5Z" fill="#DC5047"/></svg>'}
        \\function textIcon(){return '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24"><path fill="#969696" d="M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-5 14H7v-2h7v2zm3-4H7v-2h10v2zm0-4H7V7h10v2z"/></svg>'}
        \\function imageIcon(){return '<svg viewBox="0 0 24 24" height="22" width="22" xmlns="http://www.w3.org/2000/svg"><path fill="#2aa198" d="M5 21c-.55 0-1.02-.2-1.41-.59A1.93 1.93 0 0 1 3 19V5c0-.55.2-1.02.59-1.41C3.98 3.2 4.45 3 5 3h14c.55 0 1.02.2 1.41.59.39.39.59.86.59 1.41v14c0 .55-.2 1.02-.59 1.41-.39.39-.86.59-1.41.59H5zm1.5-4h11l-3.45-4.6-2.8 3.6-1.95-2.6L6.5 17z"/></svg>'}
        \\function audioIcon(){return '<svg viewBox="0 0 24 24" height="22" width="22" xmlns="http://www.w3.org/2000/svg"><path fill="#8e44ad" d="M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z"/></svg>'}
        \\function videoIcon(){return '<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 24 24"><path fill="#969696" d="M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z"/></svg>'}
        \\function officeIcon(kind){if(kind==='sheet')return fileIconSvg('#9abd4e','spreadsheet');if(kind==='slide')return fileIconSvg('#f0965f','presentation');return fileIconSvg('#49abea','document')}
        \\function fileIcon(n){let x=extOf(n);if(x==='pdf')return pdfIcon();if(['txt','md','csv','log','json','xml','html','css','js','ts','zig','py','rs','go','c','h'].includes(x))return textIcon();if(['png','jpg','jpeg','gif','webp','svg','heic','avif'].includes(x))return imageIcon();if(['mp3','wav','flac','ogg','m4a'].includes(x))return audioIcon();if(['mp4','mov','mkv','webm','avi'].includes(x))return videoIcon();if(['doc','docx','odt','rtf'].includes(x))return officeIcon('doc');if(['xls','xlsx','ods'].includes(x))return officeIcon('sheet');if(['ppt','pptx','odp'].includes(x))return officeIcon('slide');return fileIconSvg('#8a8a8a','file')}
        \\function entryIcon(f){return f.is_dir?folderIcon():fileIcon(f.name)}
        \\function fs(b){if(b<1024)return b+' B';if(b<1048576)return(b/1024).toFixed(1)+' KB';if(b<1073741824)return(b/1048576).toFixed(1)+' MB';return(b/1073741824).toFixed(1)+' GB'}
        \\function fd(t){return new Date(t*1000).toLocaleDateString()}
        \\function e(s){return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;')}
        \\function toggleSelection(p,on){if(on)selectedPaths.add(p);else selectedPaths.delete(p);updateBulkBar()}
        \\function toggleAllSelection(on){currentFiles.forEach(f=>{let p=currentScope==='trash'?f.id:(f.path||joinPath(currentPath,f.name));if(on)selectedPaths.add(p);else selectedPaths.delete(p)});renderFiles({files:currentFiles})}
        \\function updateBulkBar(){let b=document.getElementById('bulk-actions');let c=document.getElementById('selected-count');if(!b||!c)return;c.textContent=selectedPaths.size+' selected';if(selectedPaths.size>0&&currentScope!=='activity'){b.classList.remove('hidden');b.classList.add('flex')}else{b.classList.add('hidden');b.classList.remove('flex')}}
        \\async function bulkDeleteSelected(){let paths=Array.from(selectedPaths);if(!paths.length)return;if(!await showConfirmDialog('Move selected to deleted files?','Delete '+paths.length+' selected item(s)?'))return;for(let p of paths){if(currentScope==='trash')await fetch('/api/files/trash/purge',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:p})});else await fetch('/api/files/delete',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p})})}selectedPaths.clear();navigate(currentPath)}
        \\function closeDarDialog(value){let d=document.getElementById('dar-dialog');d.classList.add('hidden');d.classList.remove('flex');if(dialogResolve){let r=dialogResolve;dialogResolve=null;r(value)}}
        \\function showConfirmDialog(title,msg){return new Promise(resolve=>{dialogResolve=resolve;let d=document.getElementById('dar-dialog');document.getElementById('dar-dialog-title').textContent=title;document.getElementById('dar-dialog-message').textContent=msg;document.getElementById('dar-dialog-input').classList.add('hidden');document.getElementById('dar-dialog-cancel').onclick=()=>closeDarDialog(false);document.getElementById('dar-dialog-ok').onclick=()=>closeDarDialog(true);d.classList.remove('hidden');d.classList.add('flex')})}
        \\function showPromptDialog(title,msg,value){return new Promise(resolve=>{dialogResolve=resolve;let d=document.getElementById('dar-dialog');let input=document.getElementById('dar-dialog-input');document.getElementById('dar-dialog-title').textContent=title;document.getElementById('dar-dialog-message').textContent=msg;input.value=value||'';input.classList.remove('hidden');document.getElementById('dar-dialog-cancel').onclick=()=>closeDarDialog(null);document.getElementById('dar-dialog-ok').onclick=()=>closeDarDialog(input.value);d.classList.remove('hidden');d.classList.add('flex');input.focus()})}
        \\async function handleDrop(ev){ev.preventDefault();document.getElementById('drop-zone').classList.remove('border-primary');if(ev.dataTransfer&&ev.dataTransfer.files&&ev.dataTransfer.files.length)await uploadFileList(ev.dataTransfer.files)}
        \\let dz=document.getElementById('drop-zone');dz.addEventListener('dragover',ev=>{ev.preventDefault();dz.classList.add('border-primary')});dz.addEventListener('dragleave',()=>dz.classList.remove('border-primary'));dz.addEventListener('drop',handleDrop);
        \\document.querySelectorAll('#app-sidebar [data-scope]').forEach(a=>a.addEventListener('click',ev=>{ev.preventDefault();setScope(a.dataset.scope)}));
        \\document.addEventListener('click',()=>document.getElementById('file-action-menu')?.classList.add('hidden'));
        \\navigate('/');
        \\</script>
    ;
    const dav_url = try userDavUrl(ctx.allocator, user.id);
    defer ctx.allocator.free(dav_url);
    const dav_key = try ensureDavApiKey(ctx.allocator, user.id);
    defer ctx.allocator.free(dav_key);
    const with_url_len = std.mem.replacementSize(u8, content, "DAR_DAV_URL_PLACEHOLDER", dav_url);
    const with_url = try ctx.allocator.alloc(u8, with_url_len);
    _ = std.mem.replace(u8, content, "DAR_DAV_URL_PLACEHOLDER", dav_url, with_url);
    defer ctx.allocator.free(with_url);
    const with_key_len = std.mem.replacementSize(u8, with_url, "DAR_DAV_API_KEY_PLACEHOLDER", dav_key);
    const with_key = try ctx.allocator.alloc(u8, with_key_len);
    _ = std.mem.replace(u8, with_url, "DAR_DAV_API_KEY_PLACEHOLDER", dav_key, with_key);
    defer ctx.allocator.free(with_key);
    const html = try layout.pageWithSidebar(ctx.allocator, "Files", app_bar, user_info, sidebar, with_key);
    try response.html(req, html);
}

fn handleList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const dir_path_raw = extractQueryParam(req.head.target, "path") orelse "/";
    const dir_path = try urlDecodeAlloc(ctx.allocator, dir_path_raw);
    defer ctx.allocator.free(dir_path);
    const rows = try listDiskDir(ctx.allocator, user.id, scope, dir_path);
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handleSearch(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const dir_path_raw = extractQueryParam(req.head.target, "path") orelse "/";
    const query_raw = extractQueryParam(req.head.target, "q") orelse "";
    const recursive_raw = extractQueryParam(req.head.target, "recursive") orelse "0";
    const dir_path = try urlDecodeAlloc(ctx.allocator, dir_path_raw);
    defer ctx.allocator.free(dir_path);
    const query = try urlDecodeAlloc(ctx.allocator, query_raw);
    defer ctx.allocator.free(query);
    const recursive = std.mem.eql(u8, recursive_raw, "1") or std.mem.eql(u8, recursive_raw, "true");
    const rows = searchDiskDir(ctx.allocator, user.id, scope, dir_path, query, recursive) catch |err| {
        if (err == error.InvalidPath) {
            try req.respond("{\"error\":\"invalid path\"}", .{ .status = .bad_request, .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
            return;
        }
        return err;
    };
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handlePicker(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const dir_path_raw = extractQueryParam(req.head.target, "path") orelse "/";
    const dir_path = try urlDecodeAlloc(ctx.allocator, dir_path_raw);
    defer ctx.allocator.free(dir_path);
    const rows = try listDiskDir(ctx.allocator, user.id, scope, dir_path);
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handleUpload(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const dir_path_raw = extractQueryParam(req.head.target, "path") orelse "/";
    const conflict = extractQueryParam(req.head.target, "conflict") orelse "";
    const dir_path = try urlDecodeAlloc(ctx.allocator, dir_path_raw);
    defer ctx.allocator.free(dir_path);

    var boundary: ?[]const u8 = null;
    var it = req.iterateHeaders();
    while (it.next()) |hdr| {
        if (std.ascii.eqlIgnoreCase(hdr.name, "content-type")) {
            if (std.mem.indexOf(u8, hdr.value, "boundary=")) |idx| boundary = hdr.value[idx + 9 ..];
        }
    }
    const body = try readBodyLimit(req, ctx.allocator, 1024 * 1024 * 1024);
    defer ctx.allocator.free(body);
    const b = boundary orelse {
        try response.json(req, "{\"error\":\"missing boundary\"}");
        return;
    };
    const filename = extractMultipartFilename(body) orelse {
        try response.json(req, "{\"error\":\"missing file\"}");
        return;
    };
    const file_body = extractMultipartFileBody(body, b) orelse {
        try response.json(req, "{\"error\":\"missing body\"}");
        return;
    };
    var upload_name = filename;
    var owned_name: ?[]const u8 = null;
    defer if (owned_name) |n| ctx.allocator.free(n);
    if (try userEntryExists(ctx.allocator, user.id, scope, dir_path, filename)) {
        if (std.mem.eql(u8, conflict, "rename")) {
            owned_name = try nonConflictingName(ctx.allocator, user.id, scope, dir_path, filename);
            upload_name = owned_name.?;
        } else if (!std.mem.eql(u8, conflict, "overwrite")) {
            try req.respond("{\"error\":\"conflict\"}", .{ .status = .conflict, .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
            return;
        }
    }
    try writeUserFile(ctx.allocator, user.id, scope, dir_path, upload_name, file_body);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleDownload(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const path_raw = extractQueryParam(req.head.target, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const path = try urlDecodeAlloc(ctx.allocator, path_raw);
    defer ctx.allocator.free(path);
    const disk_path = try joinUserDiskPath(ctx.allocator, user.id, scope, path);
    defer ctx.allocator.free(disk_path);
    const content_body = std.Io.Dir.cwd().readFileAlloc(compat.getIo(), disk_path, ctx.allocator, .unlimited) catch {
        try response.errorPage(req, 404, "File not on disk");
        return;
    };
    try req.respond(content_body, .{ .extra_headers = &.{
        .{ .name = "content-type", .value = guessMime(path) },
        .{ .name = "content-disposition", .value = try std.fmt.allocPrint(ctx.allocator, "attachment; filename=\"{s}\"", .{baseName(path)}) },
    }});
}

fn handlePreview(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const path_raw = extractQueryParam(req.head.target, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const path = try urlDecodeAlloc(ctx.allocator, path_raw);
    defer ctx.allocator.free(path);
    const mime = guessMime(path);
    if (!isPreviewableMime(mime)) {
        try req.respond("{\"error\":\"preview unavailable\"}", .{ .status = .unsupported_media_type, .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
        return;
    }
    const disk_path = joinUserDiskPath(ctx.allocator, user.id, scope, path) catch |err| {
        if (err == error.InvalidPath) {
            try req.respond("{\"error\":\"invalid path\"}", .{ .status = .bad_request, .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
            return;
        }
        return err;
    };
    defer ctx.allocator.free(disk_path);
    if (isDiskDir(disk_path)) {
        try req.respond("{\"error\":\"preview unavailable\"}", .{ .status = .unsupported_media_type, .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
        return;
    }
    const content_body = std.Io.Dir.cwd().readFileAlloc(compat.getIo(), disk_path, ctx.allocator, .unlimited) catch {
        try response.errorPage(req, 404, "File not on disk");
        return;
    };
    try req.respond(content_body, .{ .extra_headers = &.{
        .{ .name = "content-type", .value = mime },
        .{ .name = "content-disposition", .value = "inline" },
    }});
}

fn handleMkdir(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const parent_path = extractJsonString(body, "path") orelse "/";
    const name = extractJsonString(body, "name") orelse {
        try response.json(req, "{\"error\":\"missing name\"}");
        return;
    };
    try makeUserDir(ctx.allocator, user.id, scope, parent_path, name);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleDelete(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    deleteUserEntry(ctx.allocator, user.id, scope, path) catch |err| {
        std.debug.print("delete failed: {}\n", .{err});
        try response.json(req, "{\"error\":\"delete failed\"}");
        return;
    };
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleTrashList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const rows = try listTrash(ctx.allocator, user.id);
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handleTrashRestore(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const id = extractJsonString(body, "id") orelse {
        try response.json(req, "{\"error\":\"missing id\"}");
        return;
    };
    try restoreTrashEntry(ctx.allocator, user.id, id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleTrashPurge(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const id = extractJsonString(body, "id") orelse {
        try response.json(req, "{\"error\":\"missing id\"}");
        return;
    };
    try purgeTrashEntry(ctx.allocator, user.id, id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleVersionsList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const path_raw = extractQueryParam(req.head.target, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const path = try urlDecodeAlloc(ctx.allocator, path_raw);
    defer ctx.allocator.free(path);
    const rows = try listVersions(ctx.allocator, user.id, scope, path);
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handleVersionDownload(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const id = extractQueryParam(req.head.target, "id") orelse {
        try response.json(req, "{\"error\":\"missing id\"}");
        return;
    };
    const bytes = try readVersionBytes(ctx.allocator, user.id, id);
    try req.respond(bytes, .{ .extra_headers = &.{.{ .name = "content-type", .value = "application/octet-stream" }} });
}

fn handleVersionRestore(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const id = extractJsonString(body, "id") orelse {
        try response.json(req, "{\"error\":\"missing id\"}");
        return;
    };
    try restoreVersion(ctx.allocator, user.id, id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleActivityList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const rows = try listActivity(ctx.allocator, user.id);
    defer ctx.allocator.free(rows);
    try response.json(req, rows);
}

fn handleShareCreate(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const expires_at = extractJsonInt(body, "expires_at") orelse 0;
    const json = try createPublicShare(ctx.allocator, user.id, scope, path, expires_at);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleShareList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const path_raw = extractQueryParam(req.head.target, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const path = try urlDecodeAlloc(ctx.allocator, path_raw);
    defer ctx.allocator.free(path);
    const json = try listShares(ctx.allocator, user.id, scope, path);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleShareDelete(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const token = extractJsonString(body, "token") orelse {
        try response.json(req, "{\"error\":\"missing token\"}");
        return;
    };
    try deleteShare(user.id, token);
    try response.json(req, "{\"status\":\"ok\"}");
}


fn handleDavKeyGet(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const key = try ensureDavApiKey(ctx.allocator, user.id);
    defer ctx.allocator.free(key);
    const json = try std.fmt.allocPrint(ctx.allocator, "{{\"key\":\"{s}\"}}", .{key});
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleDavKeyReroll(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    _ = req.readerExpectNone(&[_]u8{});
    const key = try rerollDavApiKey(ctx.allocator, user.id);
    defer ctx.allocator.free(key);
    const json = try std.fmt.allocPrint(ctx.allocator, "{{\"status\":\"ok\",\"key\":\"{s}\"}}", .{key});
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleFavoriteToggle(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse { try response.json(req, "{\"error\":\"missing path\"}"); return; };
    const favorite = try toggleFavorite(user.id, scope, path);
    try req.respond(if (favorite) "{\"status\":\"ok\",\"favorite\":true}" else "{\"status\":\"ok\",\"favorite\":false}", .{ .extra_headers = &.{.{ .name = "content-type", .value = "application/json" }} });
}

fn handleFavoritesList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const json = try listFavorites(ctx.allocator, user.id);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleRecentList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const json = try listRecent(ctx.allocator, user.id);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleTagsList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const path_raw = extractQueryParam(req.head.target, "path");
    const path = if (path_raw) |p| try urlDecodeAlloc(ctx.allocator, p) else null;
    defer if (path) |p| ctx.allocator.free(p);
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const json = if (path) |p| try listTagsForPath(ctx.allocator, user.id, scope, p) else try listTags(ctx.allocator, user.id);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleTagCreate(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const name = extractJsonString(body, "name") orelse { try response.json(req, "{\"error\":\"missing name\"}"); return; };
    const color = extractJsonString(body, "color") orelse "#6750A4";
    const json = try createTag(ctx.allocator, user.id, name, color);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleTagAssign(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse { try response.json(req, "{\"error\":\"missing path\"}"); return; };
    const tag_id = extractJsonString(body, "tag_id") orelse { try response.json(req, "{\"error\":\"missing tag_id\"}"); return; };
    try assignTag(user.id, scope, path, tag_id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleTagRemove(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse { try response.json(req, "{\"error\":\"missing path\"}"); return; };
    const tag_id = extractJsonString(body, "tag_id") orelse { try response.json(req, "{\"error\":\"missing tag_id\"}"); return; };
    try removeTag(user.id, scope, path, tag_id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleTagFiles(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const tag_id = extractQueryParam(req.head.target, "tag_id") orelse { try response.json(req, "{\"error\":\"missing tag_id\"}"); return; };
    const json = try listFilesForTag(ctx.allocator, user.id, tag_id);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleCommentsList(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const scope = parseScope(extractQueryParam(req.head.target, "scope"));
    const path_raw = extractQueryParam(req.head.target, "path") orelse { try response.json(req, "{\"error\":\"missing path\"}"); return; };
    const path = try urlDecodeAlloc(ctx.allocator, path_raw);
    defer ctx.allocator.free(path);
    const json = try listComments(ctx.allocator, user.id, scope, path);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleCommentCreate(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse { try response.json(req, "{\"error\":\"missing path\"}"); return; };
    const comment_body = extractJsonString(body, "body") orelse { try response.json(req, "{\"error\":\"missing body\"}"); return; };
    const json = try createComment(ctx.allocator, user.id, scope, path, comment_body);
    defer ctx.allocator.free(json);
    try response.json(req, json);
}

fn handleCommentDelete(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse { _ = req.readerExpectNone(&[_]u8{}); try response.json(req, "{\"error\":\"unauthorized\"}"); return; };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const id = extractJsonString(body, "id") orelse { try response.json(req, "{\"error\":\"missing id\"}"); return; };
    try deleteComment(user.id, id);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handlePublicShare(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const token = extractQueryParam(req.head.target, "token") orelse {
        try response.errorPage(req, 404, "Share not found");
        return;
    };
    const entry = getShareByToken(ctx.allocator, token) catch {
        try response.errorPage(req, 404, "Share not found");
        return;
    };
    defer freeShareEntry(ctx.allocator, entry);
    const child_raw = extractQueryParam(req.head.target, "path") orelse "/";
    const child = try urlDecodeAlloc(ctx.allocator, child_raw);
    defer ctx.allocator.free(child);
    const target_path = try shareTargetPath(ctx.allocator, entry.path, child, entry.is_dir);
    defer ctx.allocator.free(target_path);
    const disk_path = try joinUserDiskPath(ctx.allocator, entry.user_id, scopeFromName(entry.scope), target_path);
    defer ctx.allocator.free(disk_path);
    if (isDiskDir(disk_path)) {
        const html = try renderPublicFolder(ctx.allocator, token, target_path, disk_path);
        defer ctx.allocator.free(html);
        try response.html(req, html);
        return;
    }
    const bytes = std.Io.Dir.cwd().readFileAlloc(compat.getIo(), disk_path, ctx.allocator, .unlimited) catch {
        try response.errorPage(req, 404, "Shared file not found");
        return;
    };
    try req.respond(bytes, .{ .extra_headers = &.{
        .{ .name = "content-type", .value = guessMime(target_path) },
        .{ .name = "content-disposition", .value = try std.fmt.allocPrint(ctx.allocator, "attachment; filename=\"{s}\"", .{baseName(target_path)}) },
    }});
}

fn handleMove(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const new_name = extractJsonString(body, "new_name") orelse {
        try response.json(req, "{\"error\":\"missing new_name\"}");
        return;
    };
    try moveUserEntry(ctx.allocator, user.id, scope, path, new_name);
    try response.json(req, "{\"status\":\"ok\"}");
}

fn handleTransfer(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const path = extractJsonString(body, "path") orelse {
        try response.json(req, "{\"error\":\"missing path\"}");
        return;
    };
    const target_path = extractJsonString(body, "target_path") orelse {
        try response.json(req, "{\"error\":\"missing target_path\"}");
        return;
    };
    const mode = extractJsonString(body, "mode") orelse "move";
    if (std.mem.eql(u8, mode, "copy")) {
        try copyUserEntry(ctx.allocator, user.id, scope, path, target_path);
    } else {
        try moveUserEntryToDir(ctx.allocator, user.id, scope, path, target_path);
    }
    try response.json(req, "{\"status\":\"ok\"}");
}

fn listDiskDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, dir_path: []const u8) ![]const u8 {
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, dir_path);
    defer allocator.free(disk_path);
    var threaded = std.Io.Threaded.init_single_threaded;
    const io = threaded.io();

    var dir = std.Io.Dir.cwd().openDir(io, disk_path, .{ .iterate = true }) catch return std.fmt.allocPrint(allocator, "{{\"files\":[]}}", .{});
    defer dir.close(io);

    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    var iter = dir.iterate();
    while (iter.next(io) catch null) |entry| {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        const is_dir = entry.kind == .directory;
        const entry_path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ disk_path, entry.name });
        defer allocator.free(entry_path);
        const size: i64 = if (is_dir) 0 else blk: {
            var file = std.Io.Dir.cwd().openFile(io, entry_path, .{}) catch break :blk 0;
            defer file.close(io);
            break :blk @intCast(file.length(io) catch 0);
        };
        const now = compat.timestamp();
        try buf.appendSlice(allocator, "{\"id\":0,\"name\":");
        try appendJsonString(&buf, allocator, entry.name);
        const rel_path = try childUserPath(allocator, dir_path, entry.name);
        defer allocator.free(rel_path);
        const suffix = try std.fmt.allocPrint(allocator, ",\"size\":{d},\"is_dir\":{s},\"favorite\":{s},\"mime_type\":\"application/octet-stream\",\"created_at\":{d},\"updated_at\":{d}}}", .{ size, if (is_dir) "true" else "false", if (try isFavorite(user_id, scope, rel_path)) "true" else "false", now, now });
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn searchDiskDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, dir_path: []const u8, query: []const u8, recursive: bool) ![]const u8 {
    _ = try cleanUserPath(dir_path);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    if (query.len > 0) {
        const disk_path = try joinUserDiskPath(allocator, user_id, scope, dir_path);
        defer allocator.free(disk_path);
        try searchDiskDirInto(allocator, user_id, scope, disk_path, dir_path, query, recursive, &buf, &first);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn searchDiskDirInto(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, disk_path: []const u8, rel_dir: []const u8, query: []const u8, recursive: bool, buf: *std.ArrayList(u8), first: *bool) !void {
    var dir = std.Io.Dir.cwd().openDir(compat.getIo(), disk_path, .{ .iterate = true }) catch return;
    defer dir.close(compat.getIo());
    var iter = dir.iterate();
    while (iter.next(compat.getIo()) catch null) |entry| {
        const child_rel = try childUserPath(allocator, rel_dir, entry.name);
        defer allocator.free(child_rel);
        const child_disk = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ disk_path, entry.name });
        defer allocator.free(child_disk);
        const is_dir = entry.kind == .directory;
        if (containsIgnoreCase(entry.name, query)) {
            if (!first.*) try buf.appendSlice(allocator, ",");
            first.* = false;
            const size: i64 = if (is_dir) 0 else fileSize(child_disk);
            const now = compat.timestamp();
            try buf.appendSlice(allocator, "{\"id\":0,\"name\":");
            try appendJsonString(buf, allocator, entry.name);
            try buf.appendSlice(allocator, ",\"path\":");
            try appendJsonString(buf, allocator, child_rel);
            const suffix = try std.fmt.allocPrint(allocator, ",\"size\":{d},\"is_dir\":{s},\"favorite\":{s},\"mime_type\":\"{s}\",\"created_at\":{d},\"updated_at\":{d}}}", .{ size, if (is_dir) "true" else "false", if (try isFavorite(user_id, scope, child_rel)) "true" else "false", if (is_dir) "inode/directory" else guessMime(entry.name), now, now });
            defer allocator.free(suffix);
            try buf.appendSlice(allocator, suffix);
        }
        if (recursive and is_dir) try searchDiskDirInto(allocator, user_id, scope, child_disk, child_rel, query, recursive, buf, first);
    }
}

fn containsIgnoreCase(haystack: []const u8, needle: []const u8) bool {
    if (needle.len == 0) return true;
    if (needle.len > haystack.len) return false;
    var i: usize = 0;
    while (i + needle.len <= haystack.len) : (i += 1) {
        var ok = true;
        for (needle, 0..) |ch, j| {
            if (std.ascii.toLower(haystack[i + j]) != std.ascii.toLower(ch)) {
                ok = false;
                break;
            }
        }
        if (ok) return true;
    }
    return false;
}

fn cleanUserPathAlloc(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    if (raw.len == 0) return allocator.dupe(u8, "/");
    if (std.mem.indexOf(u8, raw, "..") != null) return error.InvalidPath;
    if (std.mem.indexOfScalar(u8, raw, '\\') != null) return error.InvalidPath;
    if (raw[0] == '/') return allocator.dupe(u8, raw);
    return std.fmt.allocPrint(allocator, "/{s}", .{raw});
}

fn cleanUserPath(raw: []const u8) ![]const u8 {
    if (raw.len == 0) return "/";
    if (std.mem.indexOf(u8, raw, "..") != null) return error.InvalidPath;
    if (std.mem.indexOfScalar(u8, raw, '\\') != null) return error.InvalidPath;
    return if (raw[0] == '/') raw else error.PathMustBeAbsolute;
}

fn joinUserDiskPath(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPathAlloc(allocator, rel_path);
    defer allocator.free(clean);
    const root = switch (scope) {
        .files => try user_data.userAppRoot(allocator, user_id, "files"),
        .all => try user_data.userRoot(allocator, user_id),
    };
    defer allocator.free(root);
    if (std.mem.eql(u8, clean, "/")) return allocator.dupe(u8, root);
    return std.fmt.allocPrint(allocator, "{s}{s}", .{ root, clean });
}

fn ensureScopeRoot(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope) !void {
    const root = switch (scope) {
        .files => try user_data.ensureUserAppDir(allocator, user_id, "files"),
        .all => try user_data.ensureUserDataDir(allocator, user_id),
    };
    allocator.free(root);
}

fn userEntryExists(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, parent_path: []const u8, name: []const u8) !bool {
    const rel_path = try childUserPath(allocator, parent_path, name);
    defer allocator.free(rel_path);
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, rel_path);
    defer allocator.free(disk_path);
    var file = std.Io.Dir.cwd().openFile(compat.getIo(), disk_path, .{}) catch |err| switch (err) {
        error.FileNotFound => return false,
        error.IsDir => return true,
        else => return err,
    };
    file.close(compat.getIo());
    return true;
}

fn nonConflictingName(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, parent_path: []const u8, name: []const u8) ![]const u8 {
    var dot = std.mem.lastIndexOfScalar(u8, name, '.') orelse name.len;
    if (dot == 0) dot = name.len;
    const stem = name[0..dot];
    const ext = name[dot..];
    var i: usize = 2;
    while (i < 10_000) : (i += 1) {
        const candidate = try std.fmt.allocPrint(allocator, "{s} ({d}){s}", .{ stem, i, ext });
        if (!try userEntryExists(allocator, user_id, scope, parent_path, candidate)) return candidate;
        allocator.free(candidate);
    }
    return error.PathAlreadyExists;
}

fn makeUserDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, parent_path: []const u8, name: []const u8) !void {
    try user_data.validateSegment(name);
    try ensureScopeRoot(allocator, user_id, scope);
    const parent = try joinUserDiskPath(allocator, user_id, scope, parent_path);
    defer allocator.free(parent);
    const path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ parent, name });
    defer allocator.free(path);
    std.Io.Dir.cwd().createDir(compat.getIo(), path, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    const rel = try childUserPath(allocator, parent_path, name);
    defer allocator.free(rel);
    try logActivity(user_id, scope, "created folder", rel);
}

fn deleteUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    if (std.mem.eql(u8, clean, "/")) return error.InvalidPath;
    const path = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(path);
    const trash_dir = try ensureTrashFilesDir(allocator, user_id);
    defer allocator.free(trash_dir);
    const trash_id = try makeTrashId(allocator);
    defer allocator.free(trash_id);
    const trash_path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ trash_dir, trash_id });
    defer allocator.free(trash_path);
    const is_dir = isDiskDir(path);
    const size = if (is_dir) 0 else fileSize(path);
    try std.Io.Dir.rename(std.Io.Dir.cwd(), path, std.Io.Dir.cwd(), trash_path, compat.getIo());
    try insertTrashEntry(allocator, trash_id, user_id, scope, clean, trash_path, baseName(clean), is_dir, size);
    try logActivity(user_id, scope, "deleted", clean);
}

fn ensureTrashFilesDir(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const user_root = try user_data.ensureUserDataDir(allocator, user_id);
    defer allocator.free(user_root);
    const trash_root = try user_data.userAppRoot(allocator, user_id, "trash");
    std.Io.Dir.cwd().createDir(compat.getIo(), trash_root, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    const trash_files = try std.fmt.allocPrint(allocator, "{s}/files", .{trash_root});
    allocator.free(trash_root);
    std.Io.Dir.cwd().createDir(compat.getIo(), trash_files, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    return trash_files;
}

fn makeTrashId(allocator: std.mem.Allocator) ![]const u8 {
    var bytes: [16]u8 = undefined;
    const n = std.os.linux.getrandom(&bytes, bytes.len, 0);
    if (n != bytes.len) return error.NoEntropy;
    var hex: [32]u8 = undefined;
    const chars = "0123456789abcdef";
    for (bytes, 0..) |byte, i| {
        hex[i * 2] = chars[byte >> 4];
        hex[i * 2 + 1] = chars[byte & 0xF];
    }
    return allocator.dupe(u8, &hex);
}


fn makeDavApiKey(allocator: std.mem.Allocator) ![]const u8 {
    var bytes: [32]u8 = undefined;
    const n = std.os.linux.getrandom(&bytes, bytes.len, 0);
    if (n != bytes.len) return error.NoEntropy;
    var hex: [64]u8 = undefined;
    const chars = "0123456789abcdef";
    for (bytes, 0..) |byte, i| {
        hex[i * 2] = chars[byte >> 4];
        hex[i * 2 + 1] = chars[byte & 0xF];
    }
    return allocator.dupe(u8, &hex);
}

fn davApiKeyPath(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const root = try user_data.ensureUserDataDir(allocator, user_id);
    defer allocator.free(root);
    return std.fmt.allocPrint(allocator, "{s}/.dav_api_key", .{root});
}

fn ensureDavApiKey(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const path = try davApiKeyPath(allocator, user_id);
    defer allocator.free(path);
    if (std.Io.Dir.cwd().readFileAlloc(compat.getIo(), path, allocator, std.Io.Limit.limited(256))) |key| {
        if (key.len == 64) return key;
        allocator.free(key);
    } else |_| {}
    const key = try makeDavApiKey(allocator);
    var file = try std.Io.Dir.cwd().createFile(compat.getIo(), path, .{});
    defer file.close(compat.getIo());
    _ = try file.writeStreamingAll(compat.getIo(), key);
    return key;
}

fn rerollDavApiKey(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const path = try davApiKeyPath(allocator, user_id);
    defer allocator.free(path);
    const key = try makeDavApiKey(allocator);
    var file = try std.Io.Dir.cwd().createFile(compat.getIo(), path, .{});
    defer file.close(compat.getIo());
    _ = try file.writeStreamingAll(compat.getIo(), key);
    return key;
}

fn isDiskDir(path: []const u8) bool {
    var dir = std.Io.Dir.cwd().openDir(compat.getIo(), path, .{}) catch return false;
    dir.close(compat.getIo());
    return true;
}

fn fileSize(path: []const u8) i64 {
    var file = std.Io.Dir.cwd().openFile(compat.getIo(), path, .{}) catch return 0;
    defer file.close(compat.getIo());
    return @intCast(file.length(compat.getIo()) catch 0);
}

fn scopeName(scope: FileScope) []const u8 {
    return switch (scope) { .files => "files", .all => "all" };
}

fn scopeFromName(name: []const u8) FileScope {
    if (std.mem.eql(u8, name, "all")) return .all;
    return .files;
}

fn prepare(sql: [:0]const u8) !?*anyopaque {
    var stmt: ?*anyopaque = null;
    const rc = c.sqlite3_prepare_v2(g_db.?, sql, -1, &stmt, null);
    if (rc != c.SQLITE_OK) {
        std.debug.print("DB prepare failed (rc={d}): {s}\n", .{ rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }
    return stmt;
}

fn bindText(stmt: ?*anyopaque, idx: c_int, text: []const u8) !void {
    if (c.sqlite3_bind_text(stmt, idx, text.ptr, @intCast(text.len), c.SQLITE_TRANSIENT) != c.SQLITE_OK) return error.SqliteError;
}

fn stepDone(stmt: ?*anyopaque) !void {
    const rc = c.sqlite3_step(stmt);
    if (rc != c.SQLITE_DONE) {
        std.debug.print("DB step failed (rc={d}): {s}\n", .{ rc, std.mem.sliceTo(c.sqlite3_errmsg(g_db.?) orelse &[_:0]u8{0}, 0) });
        return error.SqliteError;
    }
}

fn insertTrashEntry(allocator: std.mem.Allocator, id: []const u8, user_id: []const u8, scope: FileScope, original_path: []const u8, trash_path: []const u8, name: []const u8, is_dir: bool, size: i64) !void {
    _ = allocator;
    const stmt = try prepare("INSERT INTO file_trash (id,user_id,scope,original_path,trash_path,name,is_dir,size,deleted_at) VALUES (?,?,?,?,?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, id);
    try bindText(stmt, 2, user_id);
    try bindText(stmt, 3, scopeName(scope));
    try bindText(stmt, 4, original_path);
    try bindText(stmt, 5, trash_path);
    try bindText(stmt, 6, name);
    if (c.sqlite3_bind_int64(stmt, 7, if (is_dir) 1 else 0) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 8, size) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 9, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
}

const TrashEntry = struct {
    id: []const u8,
    scope: []const u8,
    original_path: []const u8,
    trash_path: []const u8,
    name: []const u8,
    is_dir: bool,
};

fn freeTrashEntry(allocator: std.mem.Allocator, entry: TrashEntry) void {
    allocator.free(entry.id);
    allocator.free(entry.scope);
    allocator.free(entry.original_path);
    allocator.free(entry.trash_path);
    allocator.free(entry.name);
}

fn dupeCol(allocator: std.mem.Allocator, stmt: ?*anyopaque, col: c_int) ![]const u8 {
    const p = c.sqlite3_column_text(stmt, col) orelse return allocator.dupe(u8, "");
    return allocator.dupe(u8, std.mem.sliceTo(p, 0));
}

fn getTrashEntry(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) !TrashEntry {
    const stmt = try prepare("SELECT id,scope,original_path,trash_path,name,is_dir FROM file_trash WHERE user_id=? AND id=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, id);
    if (c.sqlite3_step(stmt) != c.SQLITE_ROW) return error.NotFound;
    return .{
        .id = try dupeCol(allocator, stmt, 0),
        .scope = try dupeCol(allocator, stmt, 1),
        .original_path = try dupeCol(allocator, stmt, 2),
        .trash_path = try dupeCol(allocator, stmt, 3),
        .name = try dupeCol(allocator, stmt, 4),
        .is_dir = c.sqlite3_column_int64(stmt, 5) != 0,
    };
}

fn deleteTrashMetadata(user_id: []const u8, id: []const u8) !void {
    const stmt = try prepare("DELETE FROM file_trash WHERE user_id=? AND id=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, id);
    try stepDone(stmt);
}

fn restoreTrashEntry(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) !void {
    const entry = try getTrashEntry(allocator, user_id, id);
    defer freeTrashEntry(allocator, entry);
    const dst = try joinUserDiskPath(allocator, user_id, scopeFromName(entry.scope), entry.original_path);
    defer allocator.free(dst);
    try std.Io.Dir.rename(std.Io.Dir.cwd(), entry.trash_path, std.Io.Dir.cwd(), dst, compat.getIo());
    try deleteTrashMetadata(user_id, id);
    try logActivity(user_id, scopeFromName(entry.scope), "restored from trash", entry.original_path);
}

fn purgeTrashEntry(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) !void {
    const entry = try getTrashEntry(allocator, user_id, id);
    defer freeTrashEntry(allocator, entry);
    if (entry.is_dir) {
        try deleteDirRecursive(allocator, entry.trash_path);
    } else {
        try std.Io.Dir.cwd().deleteFile(compat.getIo(), entry.trash_path);
    }
    try deleteTrashMetadata(user_id, id);
    try logActivity(user_id, scopeFromName(entry.scope), "purged", entry.original_path);
}

fn deleteDirRecursive(allocator: std.mem.Allocator, path: []const u8) !void {
    var dir = try std.Io.Dir.cwd().openDir(compat.getIo(), path, .{ .iterate = true });
    defer dir.close(compat.getIo());
    var iter = dir.iterate();
    while (iter.next(compat.getIo()) catch null) |entry| {
        const child = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ path, entry.name });
        defer allocator.free(child);
        if (entry.kind == .directory) try deleteDirRecursive(allocator, child) else try std.Io.Dir.cwd().deleteFile(compat.getIo(), child);
    }
    try std.Io.Dir.cwd().deleteDir(compat.getIo(), path);
}

fn listTrash(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT id,original_path,name,is_dir,size,deleted_at FROM file_trash WHERE user_id=? ORDER BY deleted_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"id\":");
        const id = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(id, 0));
        try buf.appendSlice(allocator, ",\"original_path\":");
        const original_path = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(original_path, 0));
        try buf.appendSlice(allocator, ",\"name\":");
        const name = c.sqlite3_column_text(stmt, 2) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(name, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"is_dir\":{s},\"size\":{d},\"mime_type\":\"application/octet-stream\",\"created_at\":{d},\"updated_at\":{d},\"deleted_at\":{d}}}", .{
            if (c.sqlite3_column_int64(stmt, 3) != 0) "true" else "false",
            c.sqlite3_column_int64(stmt, 4),
            c.sqlite3_column_int64(stmt, 5),
            c.sqlite3_column_int64(stmt, 5),
            c.sqlite3_column_int64(stmt, 5),
        });
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn firstTrashId(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT id FROM file_trash WHERE user_id=? ORDER BY deleted_at DESC LIMIT 1");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    if (c.sqlite3_step(stmt) != c.SQLITE_ROW) return error.NotFound;
    return dupeCol(allocator, stmt, 0);
}

fn ensureVersionFilesDir(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const user_root = try user_data.ensureUserDataDir(allocator, user_id);
    defer allocator.free(user_root);
    const versions_root = try user_data.userAppRoot(allocator, user_id, "versions");
    std.Io.Dir.cwd().createDir(compat.getIo(), versions_root, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    const versions_files = try std.fmt.allocPrint(allocator, "{s}/files", .{versions_root});
    allocator.free(versions_root);
    std.Io.Dir.cwd().createDir(compat.getIo(), versions_files, @enumFromInt(0o755)) catch |err| switch (err) {
        error.PathAlreadyExists => {},
        else => return err,
    };
    return versions_files;
}

fn insertVersionEntry(id: []const u8, user_id: []const u8, scope: FileScope, path: []const u8, version_path: []const u8, size: i64) !void {
    const stmt = try prepare("INSERT INTO file_versions (id,user_id,scope,path,version_path,size,created_at) VALUES (?,?,?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, id);
    try bindText(stmt, 2, user_id);
    try bindText(stmt, 3, scopeName(scope));
    try bindText(stmt, 4, path);
    try bindText(stmt, 5, version_path);
    if (c.sqlite3_bind_int64(stmt, 6, size) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 7, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
}

fn saveVersionIfExists(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    if (std.mem.eql(u8, clean, "/")) return;
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(disk_path);
    if (isDiskDir(disk_path)) return;
    const content = std.Io.Dir.cwd().readFileAlloc(compat.getIo(), disk_path, allocator, .unlimited) catch return;
    defer allocator.free(content);
    const versions_dir = try ensureVersionFilesDir(allocator, user_id);
    defer allocator.free(versions_dir);
    const version_id = try makeTrashId(allocator);
    defer allocator.free(version_id);
    const version_path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ versions_dir, version_id });
    defer allocator.free(version_path);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), version_path, .{});
    defer f.close(compat.getIo());
    _ = try f.writeStreamingAll(compat.getIo(), content);
    try insertVersionEntry(version_id, user_id, scope, clean, version_path, @intCast(content.len));
    try logActivity(user_id, scope, "versioned", clean);
}

const VersionEntry = struct {
    id: []const u8,
    scope: []const u8,
    path: []const u8,
    version_path: []const u8,
};

fn freeVersionEntry(allocator: std.mem.Allocator, entry: VersionEntry) void {
    allocator.free(entry.id);
    allocator.free(entry.scope);
    allocator.free(entry.path);
    allocator.free(entry.version_path);
}

fn getVersionEntry(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) !VersionEntry {
    const stmt = try prepare("SELECT id,scope,path,version_path FROM file_versions WHERE user_id=? AND id=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, id);
    if (c.sqlite3_step(stmt) != c.SQLITE_ROW) return error.NotFound;
    return .{
        .id = try dupeCol(allocator, stmt, 0),
        .scope = try dupeCol(allocator, stmt, 1),
        .path = try dupeCol(allocator, stmt, 2),
        .version_path = try dupeCol(allocator, stmt, 3),
    };
}

fn readVersionBytes(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) ![]const u8 {
    const entry = try getVersionEntry(allocator, user_id, id);
    defer freeVersionEntry(allocator, entry);
    return std.Io.Dir.cwd().readFileAlloc(compat.getIo(), entry.version_path, allocator, .unlimited);
}

fn restoreVersion(allocator: std.mem.Allocator, user_id: []const u8, id: []const u8) !void {
    const entry = try getVersionEntry(allocator, user_id, id);
    defer freeVersionEntry(allocator, entry);
    const bytes = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), entry.version_path, allocator, .unlimited);
    defer allocator.free(bytes);
    const dst = try joinUserDiskPath(allocator, user_id, scopeFromName(entry.scope), entry.path);
    defer allocator.free(dst);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), dst, .{});
    defer f.close(compat.getIo());
    _ = try f.writeStreamingAll(compat.getIo(), bytes);
    try logActivity(user_id, scopeFromName(entry.scope), "restored version", entry.path);
}

fn listVersions(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("SELECT id,path,size,created_at FROM file_versions WHERE user_id=? AND scope=? AND path=? ORDER BY created_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"versions\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"id\":");
        const id = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(id, 0));
        try buf.appendSlice(allocator, ",\"path\":");
        const path = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(path, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"size\":{d},\"created_at\":{d}}}", .{ c.sqlite3_column_int64(stmt, 2), c.sqlite3_column_int64(stmt, 3) });
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn firstVersionId(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("SELECT id FROM file_versions WHERE user_id=? AND scope=? AND path=? ORDER BY created_at DESC LIMIT 1");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    if (c.sqlite3_step(stmt) != c.SQLITE_ROW) return error.NotFound;
    return dupeCol(allocator, stmt, 0);
}

fn logActivity(user_id: []const u8, scope: FileScope, action: []const u8, path: []const u8) !void {
    const stmt = try prepare("INSERT INTO file_activity (user_id,scope,action,path,created_at) VALUES (?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, action);
    try bindText(stmt, 4, path);
    if (c.sqlite3_bind_int64(stmt, 5, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
}

fn listActivity(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT id,scope,action,path,created_at FROM file_activity WHERE user_id=? ORDER BY created_at DESC, id DESC LIMIT 100");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"activities\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        const prefix = try std.fmt.allocPrint(allocator, "{{\"id\":{d},\"scope\":", .{c.sqlite3_column_int64(stmt, 0)});
        defer allocator.free(prefix);
        try buf.appendSlice(allocator, prefix);
        const scope = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(scope, 0));
        try buf.appendSlice(allocator, ",\"action\":");
        const action = c.sqlite3_column_text(stmt, 2) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(action, 0));
        try buf.appendSlice(allocator, ",\"path\":");
        const path = c.sqlite3_column_text(stmt, 3) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(path, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"created_at\":{d}}}", .{c.sqlite3_column_int64(stmt, 4)});
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn isFavorite(user_id: []const u8, scope: FileScope, rel_path: []const u8) !bool {
    if (user_id.len == 0) return false;
    if (g_db == null) return false;
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("SELECT 1 FROM file_favorites WHERE user_id=? AND scope=? AND path=? LIMIT 1");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    return c.sqlite3_step(stmt) == c.SQLITE_ROW;
}

fn toggleFavorite(user_id: []const u8, scope: FileScope, rel_path: []const u8) !bool {
    const clean = try cleanUserPath(rel_path);
    if (try isFavorite(user_id, scope, clean)) {
        const stmt = try prepare("DELETE FROM file_favorites WHERE user_id=? AND scope=? AND path=?");
        defer _ = c.sqlite3_finalize(stmt);
        try bindText(stmt, 1, user_id);
        try bindText(stmt, 2, scopeName(scope));
        try bindText(stmt, 3, clean);
        try stepDone(stmt);
        try logActivity(user_id, scope, "unfavorited", clean);
        return false;
    }
    const stmt = try prepare("INSERT INTO file_favorites (user_id,scope,path,created_at) VALUES (?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    if (c.sqlite3_bind_int64(stmt, 4, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
    try logActivity(user_id, scope, "favorited", clean);
    return true;
}

fn appendFileJsonForPath(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, when: i64) !void {
    const clean = try cleanUserPath(rel_path);
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(disk_path);
    const is_dir = isDiskDir(disk_path);
    if (!is_dir) {
        var f = std.Io.Dir.cwd().openFile(compat.getIo(), disk_path, .{}) catch return error.NotFound;
        f.close(compat.getIo());
    }
    try buf.appendSlice(allocator, "{\"id\":0,\"name\":");
    try appendJsonString(buf, allocator, baseName(clean));
    try buf.appendSlice(allocator, ",\"path\":");
    try appendJsonString(buf, allocator, clean);
    const suffix = try std.fmt.allocPrint(allocator, ",\"size\":{d},\"is_dir\":{s},\"favorite\":{s},\"mime_type\":\"{s}\",\"created_at\":{d},\"updated_at\":{d}}}", .{ if (is_dir) 0 else fileSize(disk_path), if (is_dir) "true" else "false", if (try isFavorite(user_id, scope, clean)) "true" else "false", if (is_dir) "inode/directory" else guessMime(clean), when, when });
    defer allocator.free(suffix);
    try buf.appendSlice(allocator, suffix);
}

fn listFavorites(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT scope,path,created_at FROM file_favorites WHERE user_id=? ORDER BY created_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        const scope_col = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        const path_col = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        if (!first) try buf.appendSlice(allocator, ",");
        appendFileJsonForPath(&buf, allocator, user_id, scopeFromName(std.mem.sliceTo(scope_col, 0)), std.mem.sliceTo(path_col, 0), c.sqlite3_column_int64(stmt, 2)) catch continue;
        first = false;
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn listRecent(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT scope,path,MAX(created_at) FROM file_activity WHERE user_id=? AND path <> '/' GROUP BY scope,path ORDER BY MAX(created_at) DESC LIMIT 50");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        const scope_col = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        const path_col = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        if (!first) try buf.appendSlice(allocator, ",");
        appendFileJsonForPath(&buf, allocator, user_id, scopeFromName(std.mem.sliceTo(scope_col, 0)), std.mem.sliceTo(path_col, 0), c.sqlite3_column_int64(stmt, 2)) catch continue;
        first = false;
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn createTag(allocator: std.mem.Allocator, user_id: []const u8, name: []const u8, color: []const u8) ![]const u8 {
    const id = try makeTrashId(allocator);
    defer allocator.free(id);
    const stmt = try prepare("INSERT INTO file_tags (id,user_id,name,color,created_at) VALUES (?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, id);
    try bindText(stmt, 2, user_id);
    try bindText(stmt, 3, name);
    try bindText(stmt, 4, color);
    if (c.sqlite3_bind_int64(stmt, 5, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
    try logActivity(user_id, .files, "created tag", name);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"status\":\"ok\",\"id\":");
    try appendJsonString(&buf, allocator, id);
    try buf.appendSlice(allocator, ",\"name\":");
    try appendJsonString(&buf, allocator, name);
    try buf.appendSlice(allocator, ",\"color\":");
    try appendJsonString(&buf, allocator, color);
    try buf.appendSlice(allocator, "}");
    return buf.toOwnedSlice(allocator);
}

fn listTags(allocator: std.mem.Allocator, user_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT id,name,color,created_at FROM file_tags WHERE user_id=? ORDER BY name");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"tags\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"id\":");
        const id = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(id, 0));
        try buf.appendSlice(allocator, ",\"name\":");
        const name = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(name, 0));
        try buf.appendSlice(allocator, ",\"color\":");
        const color = c.sqlite3_column_text(stmt, 2) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(color, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"created_at\":{d}}}", .{c.sqlite3_column_int64(stmt, 3)});
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn listTagsForPath(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("SELECT t.id,t.name,t.color,t.created_at FROM file_tags t JOIN file_tag_assignments a ON a.tag_id=t.id WHERE a.user_id=? AND a.scope=? AND a.path=? ORDER BY t.name");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"tags\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"id\":");
        const id = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(id, 0));
        try buf.appendSlice(allocator, ",\"name\":");
        const name = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(name, 0));
        try buf.appendSlice(allocator, ",\"color\":");
        const color = c.sqlite3_column_text(stmt, 2) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(color, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"created_at\":{d}}}", .{c.sqlite3_column_int64(stmt, 3)});
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn assignTag(user_id: []const u8, scope: FileScope, rel_path: []const u8, tag_id: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("INSERT OR IGNORE INTO file_tag_assignments (user_id,scope,path,tag_id,created_at) VALUES (?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    try bindText(stmt, 4, tag_id);
    if (c.sqlite3_bind_int64(stmt, 5, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
    try logActivity(user_id, scope, "tagged", clean);
}

fn removeTag(user_id: []const u8, scope: FileScope, rel_path: []const u8, tag_id: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("DELETE FROM file_tag_assignments WHERE user_id=? AND scope=? AND path=? AND tag_id=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    try bindText(stmt, 4, tag_id);
    try stepDone(stmt);
    try logActivity(user_id, scope, "untagged", clean);
}

fn listFilesForTag(allocator: std.mem.Allocator, user_id: []const u8, tag_id: []const u8) ![]const u8 {
    const stmt = try prepare("SELECT scope,path,created_at FROM file_tag_assignments WHERE user_id=? AND tag_id=? ORDER BY created_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, tag_id);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"files\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        const scope_col = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        const path_col = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        if (!first) try buf.appendSlice(allocator, ",");
        appendFileJsonForPath(&buf, allocator, user_id, scopeFromName(std.mem.sliceTo(scope_col, 0)), std.mem.sliceTo(path_col, 0), c.sqlite3_column_int64(stmt, 2)) catch continue;
        first = false;
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn createComment(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, body: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const id = try makeTrashId(allocator);
    defer allocator.free(id);
    const now = compat.timestamp();
    const stmt = try prepare("INSERT INTO file_comments (id,user_id,author_user_id,scope,path,body,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, id);
    try bindText(stmt, 2, user_id);
    try bindText(stmt, 3, user_id);
    try bindText(stmt, 4, scopeName(scope));
    try bindText(stmt, 5, clean);
    try bindText(stmt, 6, body);
    if (c.sqlite3_bind_int64(stmt, 7, now) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 8, now) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
    try logActivity(user_id, scope, "commented", clean);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"status\":\"ok\",\"id\":");
    try appendJsonString(&buf, allocator, id);
    try buf.appendSlice(allocator, "}");
    return buf.toOwnedSlice(allocator);
}

fn listComments(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const stmt = try prepare("SELECT id,author_user_id,body,created_at,updated_at FROM file_comments WHERE user_id=? AND scope=? AND path=? ORDER BY created_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"comments\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"id\":");
        const id = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(id, 0));
        try buf.appendSlice(allocator, ",\"author_user_id\":");
        const author = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(author, 0));
        try buf.appendSlice(allocator, ",\"body\":");
        const comment_body = c.sqlite3_column_text(stmt, 2) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(comment_body, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"created_at\":{d},\"updated_at\":{d}}}", .{ c.sqlite3_column_int64(stmt, 3), c.sqlite3_column_int64(stmt, 4) });
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn deleteComment(user_id: []const u8, id: []const u8) !void {
    const stmt = try prepare("DELETE FROM file_comments WHERE user_id=? AND id=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, id);
    try stepDone(stmt);
}

fn updateMetadataPath(user_id: []const u8, scope: FileScope, old_path: []const u8, new_path: []const u8) !void {
    const stmts = [_][:0]const u8{
        "UPDATE file_favorites SET path=? WHERE user_id=? AND scope=? AND path=?",
        "UPDATE file_tag_assignments SET path=? WHERE user_id=? AND scope=? AND path=?",
        "UPDATE file_comments SET path=? WHERE user_id=? AND scope=? AND path=?",
    };
    for (stmts) |sql| {
        const stmt = try prepare(sql);
        defer _ = c.sqlite3_finalize(stmt);
        try bindText(stmt, 1, new_path);
        try bindText(stmt, 2, user_id);
        try bindText(stmt, 3, scopeName(scope));
        try bindText(stmt, 4, old_path);
        try stepDone(stmt);
    }
}

const ShareEntry = struct {
    id: []const u8,
    user_id: []const u8,
    scope: []const u8,
    path: []const u8,
    token: []const u8,
    is_dir: bool,
    expires_at: i64,
};

fn freeShareEntry(allocator: std.mem.Allocator, entry: ShareEntry) void {
    allocator.free(entry.id);
    allocator.free(entry.user_id);
    allocator.free(entry.scope);
    allocator.free(entry.path);
    allocator.free(entry.token);
}

fn createPublicShare(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, expires_at: i64) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    const disk_path = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(disk_path);
    const is_dir = isDiskDir(disk_path);
    if (!is_dir) {
        var file = std.Io.Dir.cwd().openFile(compat.getIo(), disk_path, .{}) catch return error.NotFound;
        file.close(compat.getIo());
    }
    const id = try makeTrashId(allocator);
    defer allocator.free(id);
    const token = try makeTrashId(allocator);
    defer allocator.free(token);
    const stmt = try prepare("INSERT INTO file_shares (id,user_id,scope,path,token,is_dir,expires_at,created_at) VALUES (?,?,?,?,?,?,?,?)");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, id);
    try bindText(stmt, 2, user_id);
    try bindText(stmt, 3, scopeName(scope));
    try bindText(stmt, 4, clean);
    try bindText(stmt, 5, token);
    if (c.sqlite3_bind_int64(stmt, 6, if (is_dir) 1 else 0) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 7, expires_at) != c.SQLITE_OK) return error.SqliteError;
    if (c.sqlite3_bind_int64(stmt, 8, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
    try logActivity(user_id, scope, "shared", clean);
    return std.fmt.allocPrint(allocator, "{{\"status\":\"ok\",\"token\":\"{s}\",\"url\":\"/s?token={s}\",\"expires_at\":{d}}}", .{ token, token, expires_at });
}

fn listShares(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8) ![]const u8 {
    const clean = try cleanUserPath(rel_path);
    try deleteExpiredShares();
    const stmt = try prepare("SELECT token,path,is_dir,expires_at,created_at FROM file_shares WHERE user_id=? AND scope=? AND path=? ORDER BY created_at DESC");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, scopeName(scope));
    try bindText(stmt, 3, clean);
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator, "{\"shares\":[");
    var first = true;
    while (c.sqlite3_step(stmt) == c.SQLITE_ROW) {
        if (!first) try buf.appendSlice(allocator, ",");
        first = false;
        try buf.appendSlice(allocator, "{\"token\":");
        const token = c.sqlite3_column_text(stmt, 0) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(token, 0));
        try buf.appendSlice(allocator, ",\"path\":");
        const path = c.sqlite3_column_text(stmt, 1) orelse &[_:0]u8{0};
        try appendJsonString(&buf, allocator, std.mem.sliceTo(path, 0));
        const suffix = try std.fmt.allocPrint(allocator, ",\"is_dir\":{s},\"expires_at\":{d},\"created_at\":{d},\"url\":\"/s?token={s}\"}}", .{
            if (c.sqlite3_column_int64(stmt, 2) != 0) "true" else "false",
            c.sqlite3_column_int64(stmt, 3),
            c.sqlite3_column_int64(stmt, 4),
            std.mem.sliceTo(token, 0),
        });
        defer allocator.free(suffix);
        try buf.appendSlice(allocator, suffix);
    }
    try buf.appendSlice(allocator, "]}");
    return buf.toOwnedSlice(allocator);
}

fn deleteShare(user_id: []const u8, token: []const u8) !void {
    const stmt = try prepare("DELETE FROM file_shares WHERE user_id=? AND token=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, user_id);
    try bindText(stmt, 2, token);
    try stepDone(stmt);
}

fn deleteExpiredShares() !void {
    const stmt = try prepare("DELETE FROM file_shares WHERE expires_at > 0 AND expires_at <= ?");
    defer _ = c.sqlite3_finalize(stmt);
    if (c.sqlite3_bind_int64(stmt, 1, compat.timestamp()) != c.SQLITE_OK) return error.SqliteError;
    try stepDone(stmt);
}

fn getShareByToken(allocator: std.mem.Allocator, token: []const u8) !ShareEntry {
    try deleteExpiredShares();
    const stmt = try prepare("SELECT id,user_id,scope,path,token,is_dir,expires_at FROM file_shares WHERE token=?");
    defer _ = c.sqlite3_finalize(stmt);
    try bindText(stmt, 1, token);
    if (c.sqlite3_step(stmt) != c.SQLITE_ROW) return error.NotFound;
    return .{
        .id = try dupeCol(allocator, stmt, 0),
        .user_id = try dupeCol(allocator, stmt, 1),
        .scope = try dupeCol(allocator, stmt, 2),
        .path = try dupeCol(allocator, stmt, 3),
        .token = try dupeCol(allocator, stmt, 4),
        .is_dir = c.sqlite3_column_int64(stmt, 5) != 0,
        .expires_at = c.sqlite3_column_int64(stmt, 6),
    };
}

fn shareTargetPath(allocator: std.mem.Allocator, share_path: []const u8, child_raw: []const u8, share_is_dir: bool) ![]const u8 {
    const child = try cleanUserPathAlloc(allocator, child_raw);
    defer allocator.free(child);
    if (!share_is_dir) {
        if (!std.mem.eql(u8, child, "/")) return error.InvalidPath;
        return allocator.dupe(u8, share_path);
    }
    if (std.mem.eql(u8, child, "/")) return allocator.dupe(u8, share_path);
    if (std.mem.eql(u8, share_path, "/")) return allocator.dupe(u8, child);
    return std.fmt.allocPrint(allocator, "{s}{s}", .{ share_path, child });
}

fn renderPublicFolder(allocator: std.mem.Allocator, token: []const u8, rel_path: []const u8, disk_path: []const u8) ![]const u8 {
    var dir = std.Io.Dir.cwd().openDir(compat.getIo(), disk_path, .{ .iterate = true }) catch return error.NotFound;
    defer dir.close(compat.getIo());
    var rows: std.ArrayList(u8) = .empty;
    var iter = dir.iterate();
    while (iter.next(compat.getIo()) catch null) |entry| {
        const child_public_path = try childUserPath(allocator, if (std.mem.eql(u8, rel_path, "/")) "/" else rel_path, entry.name);
        defer allocator.free(child_public_path);
        const link = try std.fmt.allocPrint(allocator, "<li class=\"py-2 border-b border-outline-variant\"><a class=\"text-primary no-underline\" href=\"/s?token={s}&path={s}\">{s}</a></li>", .{ token, child_public_path, entry.name });
        defer allocator.free(link);
        try rows.appendSlice(allocator, link);
    }
    const rows_html = try rows.toOwnedSlice(allocator);
    defer allocator.free(rows_html);
    return std.fmt.allocPrint(allocator,
        \\<!DOCTYPE html><html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0"><title>Shared folder — Dar</title></head>
        \\<body><main><h1>Shared folder</h1><ul>{s}</ul></main></body></html>
    , .{rows_html});
}

fn moveUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, new_name: []const u8) !void {
    try user_data.validateSegment(new_name);
    const clean = try cleanUserPath(rel_path);
    if (std.mem.eql(u8, clean, "/")) return error.InvalidPath;
    const old_path = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(old_path);
    const parent_rel = parentPath(clean);
    const parent_disk = try joinUserDiskPath(allocator, user_id, scope, parent_rel);
    defer allocator.free(parent_disk);
    const new_path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ parent_disk, new_name });
    defer allocator.free(new_path);
    try std.Io.Dir.rename(std.Io.Dir.cwd(), old_path, std.Io.Dir.cwd(), new_path, compat.getIo());
    const new_rel = try childUserPath(allocator, parent_rel, new_name);
    defer allocator.free(new_rel);
    try updateMetadataPath(user_id, scope, clean, new_rel);
    try logActivity(user_id, scope, "renamed", new_rel);
}

fn moveUserEntryToDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, target_dir_rel: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    if (std.mem.eql(u8, clean, "/")) return error.InvalidPath;
    const src = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(src);
    const target_dir = try joinUserDiskPath(allocator, user_id, scope, target_dir_rel);
    defer allocator.free(target_dir);
    const dst = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ target_dir, baseName(clean) });
    defer allocator.free(dst);
    try std.Io.Dir.rename(std.Io.Dir.cwd(), src, std.Io.Dir.cwd(), dst, compat.getIo());
    const dst_rel = try childUserPath(allocator, target_dir_rel, baseName(clean));
    defer allocator.free(dst_rel);
    try updateMetadataPath(user_id, scope, clean, dst_rel);
    try logActivity(user_id, scope, "moved", dst_rel);
}

fn copyUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, target_dir_rel: []const u8) !void {
    const clean = try cleanUserPath(rel_path);
    if (std.mem.eql(u8, clean, "/")) return error.InvalidPath;
    const src = try joinUserDiskPath(allocator, user_id, scope, clean);
    defer allocator.free(src);
    const target_dir = try joinUserDiskPath(allocator, user_id, scope, target_dir_rel);
    defer allocator.free(target_dir);
    const dst = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ target_dir, baseName(clean) });
    defer allocator.free(dst);
    const dst_rel = try childUserPath(allocator, target_dir_rel, baseName(clean));
    defer allocator.free(dst_rel);
    try saveVersionIfExists(allocator, user_id, scope, dst_rel);
    const content = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), src, allocator, .unlimited);
    defer allocator.free(content);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), dst, .{});
    defer f.close(compat.getIo());
    _ = try f.writeStreamingAll(compat.getIo(), content);
    try logActivity(user_id, scope, "copied", dst_rel);
}

fn writeUserFile(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, parent_path: []const u8, name: []const u8, bytes: []const u8) !void {
    try user_data.validateSegment(name);
    try ensureScopeRoot(allocator, user_id, scope);
    const parent = try joinUserDiskPath(allocator, user_id, scope, parent_path);
    defer allocator.free(parent);
    const path = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ parent, name });
    defer allocator.free(path);
    const rel_path = try childUserPath(allocator, parent_path, name);
    defer allocator.free(rel_path);
    try saveVersionIfExists(allocator, user_id, scope, rel_path);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), path, .{});
    defer f.close(compat.getIo());
    _ = try f.writeStreamingAll(compat.getIo(), bytes);
    try logActivity(user_id, scope, "uploaded", rel_path);
}

fn readBody(req: *std.http.Server.Request, allocator: std.mem.Allocator) ![]const u8 {
    return readBodyLimit(req, allocator, 16 * 1024 * 1024);
}

fn readBodyLimit(req: *std.http.Server.Request, allocator: std.mem.Allocator, limit: usize) ![]const u8 {
    var body_buf: [8192]u8 = undefined;
    const body_reader = if (req.head.expect != null) try req.readerExpectContinue(&body_buf) else req.readerExpectNone(&body_buf);
    return body_reader.allocRemaining(allocator, std.Io.Limit.limited(limit));
}

fn parseScope(value: ?[]const u8) FileScope {
    if (value) |v| if (std.mem.eql(u8, v, "all")) return .all;
    return .files;
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

fn urlDecodeAlloc(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    var i: usize = 0;
    while (i < raw.len) : (i += 1) {
        if (raw[i] == '%' and i + 2 < raw.len) {
            const byte = std.fmt.parseInt(u8, raw[i + 1 .. i + 3], 16) catch raw[i];
            try out.append(allocator, byte);
            i += 2;
        } else if (raw[i] == '+') {
            try out.append(allocator, ' ');
        } else {
            try out.append(allocator, raw[i]);
        }
    }
    return out.toOwnedSlice(allocator);
}

fn extractJsonString(json: []const u8, key: []const u8) ?[]const u8 {
    const key_start = std.mem.indexOf(u8, json, key) orelse return null;
    if (key_start == 0 or json[key_start - 1] != '"') return null;
    const key_end = key_start + key.len;
    if (key_end >= json.len or json[key_end] != '"') return null;
    var pos = key_end + 1;
    while (pos < json.len and (json[pos] == ' ' or json[pos] == ':')) : (pos += 1) {}
    if (pos >= json.len or json[pos] != '"') return null;
    pos += 1;
    const val_start = pos;
    while (pos < json.len and json[pos] != '"') : (pos += 1) {
        if (json[pos] == '\\') pos += 1;
    }
    return json[val_start..pos];
}

fn extractJsonInt(json: []const u8, key: []const u8) ?i64 {
    const key_start = std.mem.indexOf(u8, json, key) orelse return null;
    if (key_start == 0 or json[key_start - 1] != '"') return null;
    const key_end = key_start + key.len;
    if (key_end >= json.len or json[key_end] != '"') return null;
    var pos = key_end + 1;
    while (pos < json.len and (json[pos] == ' ' or json[pos] == ':')) : (pos += 1) {}
    const val_start = pos;
    while (pos < json.len and json[pos] >= '0' and json[pos] <= '9') : (pos += 1) {}
    if (pos == val_start) return null;
    return std.fmt.parseInt(i64, json[val_start..pos], 10) catch null;
}

fn extractMultipartFilename(body: []const u8) ?[]const u8 {
    const key = "filename=\"";
    const start = std.mem.indexOf(u8, body, key) orelse return null;
    const name_start = start + key.len;
    const rest = body[name_start..];
    const end = std.mem.indexOfScalar(u8, rest, '"') orelse return null;
    return rest[0..end];
}

fn extractMultipartFileBody(body: []const u8, boundary: []const u8) ?[]const u8 {
    _ = boundary;
    const head_end = std.mem.indexOf(u8, body, "\r\n\r\n") orelse return null;
    const start = head_end + 4;
    const rest = body[start..];
    const end = std.mem.indexOf(u8, rest, "\r\n--") orelse return null;
    return rest[0..end];
}

fn appendJsonString(buf: *std.ArrayList(u8), allocator: std.mem.Allocator, s: []const u8) !void {
    try buf.append(allocator, '"');
    for (s) |ch| switch (ch) {
        '"' => try buf.appendSlice(allocator, "\\\""),
        '\\' => try buf.appendSlice(allocator, "\\\\"),
        '\n' => try buf.appendSlice(allocator, "\\n"),
        '\r' => try buf.appendSlice(allocator, "\\r"),
        '\t' => try buf.appendSlice(allocator, "\\t"),
        else => try buf.append(allocator, ch),
    };
    try buf.append(allocator, '"');
}

fn parentPath(path: []const u8) []const u8 {
    const slash = std.mem.lastIndexOfScalar(u8, path, '/') orelse return "/";
    if (slash == 0) return "/";
    return path[0..slash];
}

fn baseName(path: []const u8) []const u8 {
    const slash = std.mem.lastIndexOfScalar(u8, path, '/') orelse return path;
    return path[slash + 1 ..];
}

fn childUserPath(allocator: std.mem.Allocator, parent: []const u8, name: []const u8) ![]const u8 {
    if (std.mem.eql(u8, parent, "/")) return std.fmt.allocPrint(allocator, "/{s}", .{name});
    return std.fmt.allocPrint(allocator, "{s}/{s}", .{ parent, name });
}

fn guessMime(path: []const u8) []const u8 {
    if (std.mem.endsWith(u8, path, ".html")) return "text/html";
    if (std.mem.endsWith(u8, path, ".txt")) return "text/plain";
    if (std.mem.endsWith(u8, path, ".md")) return "text/markdown";
    if (std.mem.endsWith(u8, path, ".csv")) return "text/csv";
    if (std.mem.endsWith(u8, path, ".json")) return "application/json";
    if (std.mem.endsWith(u8, path, ".pdf")) return "application/pdf";
    if (std.mem.endsWith(u8, path, ".png")) return "image/png";
    if (std.mem.endsWith(u8, path, ".jpg") or std.mem.endsWith(u8, path, ".jpeg")) return "image/jpeg";
    if (std.mem.endsWith(u8, path, ".gif")) return "image/gif";
    if (std.mem.endsWith(u8, path, ".webp")) return "image/webp";
    if (std.mem.endsWith(u8, path, ".svg")) return "image/svg+xml";
    if (std.mem.endsWith(u8, path, ".avif")) return "image/avif";
    return "application/octet-stream";
}

fn isPreviewableMime(mime: []const u8) bool {
    return std.mem.startsWith(u8, mime, "text/") or
        std.mem.startsWith(u8, mime, "image/") or
        std.mem.eql(u8, mime, "application/json") or
        std.mem.eql(u8, mime, "application/pdf");
}

fn exec(sql: [:0]const u8) !void {
    var err_msg: [*c]u8 = undefined;
    if (c.sqlite3_exec(g_db.?, sql, null, null, &err_msg) != c.SQLITE_OK) return error.SqliteError;
}

fn handleDavInfo(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        try response.redirect(req, "/login");
        return;
    };
    const app_bar = try registry.buildAppBar(ctx.allocator);
    const user_info = try registry.buildUserInfo(ctx.allocator, user);
    const dav_url = try userDavUrl(ctx.allocator, user.id);
    defer ctx.allocator.free(dav_url);
    const content = try std.fmt.allocPrint(ctx.allocator,
        \\<div class="max-w-lg mx-auto mt-10">
        \\  <div class="bg-surface-container rounded-md shadow-1 p-6">
        \\    <h2 class="text-[28px] font-normal mb-4">WebDAV Access</h2>
        \\    <p class="text-on-surface-variant mb-6">Connect to your files via WebDAV using the following URL:</p>
        \\    <div class="bg-surface-container-highest rounded-sm p-3 font-mono text-sm mb-4">
        \\      <a href="{s}" class="text-primary no-underline">{s}</a>
        \\    </div>
        \\    <p class="text-on-surface-variant text-sm">The DAV server exposes your normal files folder at this user-specific URL.</p>
        \\    <a href="/files" class="inline-flex items-center text-primary rounded-full px-3 h-10 mt-4 text-sm font-medium hover:bg-primary-container no-underline transition-colors">Back to Files</a>
        \\  </div>
        \\</div>
    , .{ dav_url, dav_url });
    const html = try layout.page(ctx.allocator, "DAV Access", app_bar, user_info, content);
    try response.html(req, html);
}

test "files app rejects unsafe paths" {
    try std.testing.expectError(error.InvalidPath, cleanUserPath("../secret"));
    try std.testing.expectError(error.InvalidPath, cleanUserPath("/../../secret"));
    try std.testing.expectError(error.InvalidPath, cleanUserPath("folder/../secret"));
}

test "files app normalizes safe paths" {
    const a = std.testing.allocator;
    const root = try cleanUserPathAlloc(a, "/");
    defer a.free(root);
    try std.testing.expectEqualStrings("/", root);
    const docs = try cleanUserPathAlloc(a, "/docs");
    defer a.free(docs);
    try std.testing.expectEqualStrings("/docs", docs);
    const nested = try cleanUserPathAlloc(a, "docs/work");
    defer a.free(nested);
    try std.testing.expectEqualStrings("/docs/work", nested);
}

test "listDiskDir returns files from default files root" {
    const a = std.testing.allocator;
    const user_id = "test-list-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    const io = compat.getIo();
    const docs_path = try std.fmt.allocPrint(a, "{s}/docs", .{root});
    defer a.free(docs_path);
    std.Io.Dir.cwd().createDir(io, docs_path, @enumFromInt(0o755)) catch {};

    const note_path = try std.fmt.allocPrint(a, "{s}/note.txt", .{root});
    defer a.free(note_path);
    var f = try std.Io.Dir.cwd().createFile(io, note_path, .{});
    _ = try f.writeStreamingAll(io, "hello");
    f.close(io);

    const json = try listDiskDir(a, user_id, .files, "/");
    defer a.free(json);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"docs\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"note.txt\"") != null);
}

test "listDiskDir all scope exposes whole user root" {
    const a = std.testing.allocator;
    const user_id = "test-list-all-user";
    const root = try user_data.ensureUserDataDir(a, user_id);
    defer a.free(root);

    const trash_path = try std.fmt.allocPrint(a, "{s}/trash", .{root});
    defer a.free(trash_path);
    std.Io.Dir.cwd().createDir(compat.getIo(), trash_path, @enumFromInt(0o755)) catch {};

    const json = try listDiskDir(a, user_id, .all, "/");
    defer a.free(json);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"name\":\"trash\"") != null);
}

test "mkdir/delete/move stay inside user root" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_mutate.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-mutate-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try makeUserDir(a, user_id, .files, "/", "docs");
    const docs = try std.fmt.allocPrint(a, "{s}/docs", .{root});
    defer a.free(docs);
    var opened = try std.Io.Dir.cwd().openDir(compat.getIo(), docs, .{});
    opened.close(compat.getIo());

    try moveUserEntry(a, user_id, .files, "/docs", "renamed");
    const renamed = try std.fmt.allocPrint(a, "{s}/renamed", .{root});
    defer a.free(renamed);
    var opened2 = try std.Io.Dir.cwd().openDir(compat.getIo(), renamed, .{});
    opened2.close(compat.getIo());

    try deleteUserEntry(a, user_id, .files, "/renamed");
}

test "mkdir and upload recreate missing files app root" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_missing_root.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-missing-files-root-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};

    try makeUserDir(a, user_id, .files, "/", "docs");
    const docs = try std.fmt.allocPrint(a, "{s}/files/docs", .{user_root});
    defer a.free(docs);
    var opened = try std.Io.Dir.cwd().openDir(compat.getIo(), docs, .{});
    opened.close(compat.getIo());

    try writeUserFile(a, user_id, .files, "/", "note.txt", "hello");
    const note = try std.fmt.allocPrint(a, "{s}/files/note.txt", .{user_root});
    defer a.free(note);
    const bytes = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), note, a, .unlimited);
    defer a.free(bytes);
    try std.testing.expectEqualStrings("hello", bytes);
}

test "delete moves file to trash and restore returns it to original path" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_trash.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-trash-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);
    const note = try std.fmt.allocPrint(a, "{s}/note.txt", .{root});
    defer a.free(note);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), note, .{});
    _ = try f.writeStreamingAll(compat.getIo(), "trash me");
    f.close(compat.getIo());

    try deleteUserEntry(a, user_id, .files, "/note.txt");
    try std.testing.expectError(error.FileNotFound, std.Io.Dir.cwd().openFile(compat.getIo(), note, .{}));

    const trash_json = try listTrash(a, user_id);
    defer a.free(trash_json);
    try std.testing.expect(std.mem.indexOf(u8, trash_json, "\"name\":\"note.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, trash_json, "\"original_path\":\"/note.txt\"") != null);

    const trash_id = try firstTrashId(a, user_id);
    defer a.free(trash_id);
    try restoreTrashEntry(a, user_id, trash_id);
    var restored = try std.Io.Dir.cwd().openFile(compat.getIo(), note, .{});
    restored.close(compat.getIo());
}

test "trash purge permanently removes deleted file" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_trash_purge.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-trash-purge-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);
    const note = try std.fmt.allocPrint(a, "{s}/purge.txt", .{root});
    defer a.free(note);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), note, .{});
    _ = try f.writeStreamingAll(compat.getIo(), "purge me");
    f.close(compat.getIo());

    try deleteUserEntry(a, user_id, .files, "/purge.txt");
    const trash_id = try firstTrashId(a, user_id);
    defer a.free(trash_id);
    try purgeTrashEntry(a, user_id, trash_id);
    const trash_json = try listTrash(a, user_id);
    defer a.free(trash_json);
    try std.testing.expect(std.mem.indexOf(u8, trash_json, "purge.txt") == null);
}

test "overwrite creates version and restore returns previous bytes" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_versions.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-version-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try writeUserFile(a, user_id, .files, "/", "note.txt", "first");
    try writeUserFile(a, user_id, .files, "/", "note.txt", "second");

    const versions_json = try listVersions(a, user_id, .files, "/note.txt");
    defer a.free(versions_json);
    try std.testing.expect(std.mem.indexOf(u8, versions_json, "\"path\":\"/note.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, versions_json, "\"size\":5") != null);

    const version_id = try firstVersionId(a, user_id, .files, "/note.txt");
    defer a.free(version_id);
    const version_bytes = try readVersionBytes(a, user_id, version_id);
    defer a.free(version_bytes);
    try std.testing.expectEqualStrings("first", version_bytes);

    try restoreVersion(a, user_id, version_id);
    const note = try std.fmt.allocPrint(a, "{s}/note.txt", .{root});
    defer a.free(note);
    const restored = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), note, a, .unlimited);
    defer a.free(restored);
    try std.testing.expectEqualStrings("first", restored);
}

test "file activity records upload mkdir rename and delete" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_activity.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-activity-user";
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try writeUserFile(a, user_id, .files, "/", "note.txt", "hello");
    try makeUserDir(a, user_id, .files, "/", "docs");
    try moveUserEntry(a, user_id, .files, "/note.txt", "renamed.txt");
    try deleteUserEntry(a, user_id, .files, "/renamed.txt");

    const activity_json = try listActivity(a, user_id);
    defer a.free(activity_json);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"action\":\"uploaded\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"path\":\"/note.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"action\":\"created folder\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"path\":\"/docs\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"action\":\"renamed\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"path\":\"/renamed.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, activity_json, "\"action\":\"deleted\"") != null);
}

test "public shares create token and list metadata" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_shares.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-share-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try writeUserFile(a, user_id, .files, "/", "note.txt", "shared");
    const share_json = try createPublicShare(a, user_id, .files, "/note.txt", 0);
    defer a.free(share_json);
    try std.testing.expect(std.mem.indexOf(u8, share_json, "\"token\":") != null);
    try std.testing.expect(std.mem.indexOf(u8, share_json, "\"url\":\"/s?token=") != null);

    const shares_json = try listShares(a, user_id, .files, "/note.txt");
    defer a.free(shares_json);
    try std.testing.expect(std.mem.indexOf(u8, shares_json, "\"path\":\"/note.txt\"") != null);
}

test "upload conflict helpers keep original and generate renamed copy" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_conflict.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-conflict-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try writeUserFile(a, user_id, .files, "/", "note.txt", "one");
    try std.testing.expect(try userEntryExists(a, user_id, .files, "/", "note.txt"));

    const renamed = try nonConflictingName(a, user_id, .files, "/", "note.txt");
    defer a.free(renamed);
    try std.testing.expectEqualStrings("note (2).txt", renamed);
    try writeUserFile(a, user_id, .files, "/", renamed, "two");

    const original_path = try std.fmt.allocPrint(a, "{s}/note.txt", .{root});
    defer a.free(original_path);
    const original = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), original_path, a, .unlimited);
    defer a.free(original);
    try std.testing.expectEqualStrings("one", original);

    const renamed_path = try std.fmt.allocPrint(a, "{s}/{s}", .{ root, renamed });
    defer a.free(renamed_path);
    const renamed_bytes = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), renamed_path, a, .unlimited);
    defer a.free(renamed_bytes);
    try std.testing.expectEqualStrings("two", renamed_bytes);
}

test "file search returns matching names and rejects traversal" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_search.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);

    const user_id = "test-search-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    const root = try user_data.ensureUserAppDir(a, user_id, "files");
    defer a.free(root);

    try writeUserFile(a, user_id, .files, "/", "note.txt", "hello");
    try writeUserFile(a, user_id, .files, "/", "photo.jpg", "jpg");
    try makeUserDir(a, user_id, .files, "/", "docs");
    try writeUserFile(a, user_id, .files, "/docs", "meeting-notes.md", "notes");

    const shallow = try searchDiskDir(a, user_id, .files, "/", "note", false);
    defer a.free(shallow);
    try std.testing.expect(std.mem.indexOf(u8, shallow, "\"name\":\"note.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, shallow, "meeting-notes.md") == null);

    const recursive = try searchDiskDir(a, user_id, .files, "/", "note", true);
    defer a.free(recursive);
    try std.testing.expect(std.mem.indexOf(u8, recursive, "\"name\":\"note.txt\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, recursive, "\"path\":\"/docs/meeting-notes.md\"") != null);

    try std.testing.expectError(error.InvalidPath, searchDiskDir(a, user_id, .files, "../secret", "note", false));
}

test "preview mime helper covers browser-previewable files" {
    try std.testing.expectEqualStrings("text/plain", guessMime("/notes.txt"));
    try std.testing.expectEqualStrings("text/markdown", guessMime("/readme.md"));
    try std.testing.expectEqualStrings("image/jpeg", guessMime("/photo.jpg"));
    try std.testing.expectEqualStrings("image/png", guessMime("/photo.png"));
    try std.testing.expectEqualStrings("application/pdf", guessMime("/paper.pdf"));
    try std.testing.expect(isPreviewableMime(guessMime("/notes.txt")));
    try std.testing.expect(isPreviewableMime(guessMime("/photo.jpg")));
    try std.testing.expect(isPreviewableMime(guessMime("/paper.pdf")));
    try std.testing.expect(!isPreviewableMime(guessMime("/archive.bin")));
}

test "favorites list view marks files and toggles off" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_favorites.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);
    const user_id = "test-favorite-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    try writeUserFile(a, user_id, .files, "/", "favorite.txt", "fav");
    try std.testing.expect(try toggleFavorite(user_id, .files, "/favorite.txt"));
    const listed = try listDiskDir(a, user_id, .files, "/");
    defer a.free(listed);
    try std.testing.expect(std.mem.indexOf(u8, listed, "\"favorite\":true") != null);
    const favorites = try listFavorites(a, user_id);
    defer a.free(favorites);
    try std.testing.expect(std.mem.indexOf(u8, favorites, "\"path\":\"/favorite.txt\"") != null);
    try std.testing.expect(!(try toggleFavorite(user_id, .files, "/favorite.txt")));
    const empty = try listFavorites(a, user_id);
    defer a.free(empty);
    try std.testing.expect(std.mem.indexOf(u8, empty, "favorite.txt") == null);
}

test "tags assign files and metadata follows rename" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_tags.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);
    const user_id = "test-tags-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    try writeUserFile(a, user_id, .files, "/", "tagged.txt", "tagged");
    _ = try toggleFavorite(user_id, .files, "/tagged.txt");
    const tag_json = try createTag(a, user_id, "work", "#6750A4");
    defer a.free(tag_json);
    const tag_id_key = "\"id\":\"";
    const tag_start = (std.mem.indexOf(u8, tag_json, tag_id_key) orelse return error.TestUnexpectedResult) + tag_id_key.len;
    const tag_rest = tag_json[tag_start..];
    const tag_end = std.mem.indexOfScalar(u8, tag_rest, '"') orelse return error.TestUnexpectedResult;
    const tag_id = tag_rest[0..tag_end];
    try assignTag(user_id, .files, "/tagged.txt", tag_id);
    const tags = try listTagsForPath(a, user_id, .files, "/tagged.txt");
    defer a.free(tags);
    try std.testing.expect(std.mem.indexOf(u8, tags, "\"name\":\"work\"") != null);
    try moveUserEntry(a, user_id, .files, "/tagged.txt", "renamed.txt");
    const favorites = try listFavorites(a, user_id);
    defer a.free(favorites);
    try std.testing.expect(std.mem.indexOf(u8, favorites, "\"path\":\"/renamed.txt\"") != null);
    const tag_files = try listFilesForTag(a, user_id, tag_id);
    defer a.free(tag_files);
    try std.testing.expect(std.mem.indexOf(u8, tag_files, "\"path\":\"/renamed.txt\"") != null);
}

test "comments create list delete and recent view includes file" {
    const a = std.testing.allocator;
    const tmp_path = "/tmp/dar_test_files_comments.sqlite";
    _ = std.Io.Dir.cwd().deleteFile(compat.getIo(), tmp_path) catch {};
    var db: ?*c.sqlite3 = null;
    const rc = c.sqlite3_open(@ptrCast(tmp_path), &db);
    try std.testing.expectEqual(c.SQLITE_OK, rc);
    defer _ = c.sqlite3_close(db.?);
    try init(db.?);
    const user_id = "test-comments-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};
    try writeUserFile(a, user_id, .files, "/", "commented.txt", "hello");
    const comment_json = try createComment(a, user_id, .files, "/commented.txt", "looks good");
    defer a.free(comment_json);
    try std.testing.expect(std.mem.indexOf(u8, comment_json, "\"status\":\"ok\"") != null);
    const comments = try listComments(a, user_id, .files, "/commented.txt");
    defer a.free(comments);
    try std.testing.expect(std.mem.indexOf(u8, comments, "looks good") != null);
    const recent = try listRecent(a, user_id);
    defer a.free(recent);
    try std.testing.expect(std.mem.indexOf(u8, recent, "\"path\":\"/commented.txt\"") != null);
}

test "DAV API key is stable until rerolled" {
    const a = std.testing.allocator;
    const user_id = "test-dav-key-user";
    const user_root = try user_data.userRoot(a, user_id);
    defer a.free(user_root);
    deleteDirRecursive(a, user_root) catch {};

    const first = try ensureDavApiKey(a, user_id);
    defer a.free(first);
    try std.testing.expectEqual(@as(usize, 64), first.len);

    const second = try ensureDavApiKey(a, user_id);
    defer a.free(second);
    try std.testing.expectEqualStrings(first, second);

    const rerolled = try rerollDavApiKey(a, user_id);
    defer a.free(rerolled);
    try std.testing.expectEqual(@as(usize, 64), rerolled.len);
    try std.testing.expect(!std.mem.eql(u8, first, rerolled));

    const persisted = try ensureDavApiKey(a, user_id);
    defer a.free(persisted);
    try std.testing.expectEqualStrings(rerolled, persisted);
}
