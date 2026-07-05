# Nextcloud-Style App Shell and Files Controls Implementation Plan

> **For Hermes:** Use subagent-driven-development skill to implement this plan task-by-task.

**Goal:** Rework Dar’s global UI shell to look more like Nextcloud/Google Photos, with icon-only top app navigation, a left app sidebar, avatar dropdown, and richer Files controls.

**Architecture:** Put shared navigation, avatar menu, and app sidebar primitives in the global web UI layer so future apps can reuse the same shell instead of rebuilding it inside each app. Keep Files-specific views in `src/apps/files/files.zig`, but make them consume the shared shell/sidebar pattern and expose new filesystem operations through scoped API endpoints.

**Tech Stack:** Zig 0.17, `std.http.Server`, sqlite3 FFI only, server-rendered HTML via `std.fmt`, Tailwind CDN with existing Material 3 theme tokens, vanilla JavaScript only.

---

## Current context / assumptions

- Global page chrome is currently centralized in `src/webui/layout.zig` via `layout.page(...)`.
- App navigation and user display are built in `src/apps/registry.zig` via:
  - `buildAppBar(...)`
  - `buildUserInfo(...)`
- Current global top bar centers content in `max-w-7xl`; the `Dar` title is not hard-left.
- Current app bar shows icon plus text. User wants icon-only app nav starting immediately after `Dar`.
- Current user info is display-name text plus Logout link. User wants a round blank avatar button with dropdown: username text, Settings, Logout.
- Files app currently owns its own Files/All files tab row inside `src/apps/files/files.zig`.
- Files storage already has scoped roots:
  - `scope=files` -> `data/users/<user>/files/`
  - `scope=all` -> `data/users/<user>/`
- Files table currently supports list, upload, download, mkdir, delete, move/rename, and basic row-click browsing.
- User wants the left tab/sidebar style to be applicable to all apps in the future, not just Files.
- User specifically requested UI/visual changes, so after implementation, do not treat tests as final approval for visual design. Build/e2e should run when code is ready, but final visual acceptance should come from the user.

## Proposed approach

1. Add a reusable app-shell concept to `layout.zig`: keep `layout.page(...)` compatible, but add a new overload/helper that accepts optional left sidebar HTML.
2. Update registry-generated app bar to be Nextcloud-like:
   - `Dar` hard-left in the top bar.
   - App links are icon-only, directly after `Dar`.
   - Avatar is a blank round button on the far right.
   - Dropdown menu is implemented once in registry/global layout JavaScript.
3. Add a reusable left-sidebar builder for app-local navigation:
   - Primary nav items at top: Files, All files.
   - Settings pinned to bottom.
   - Use the same markup/style pattern so Calendar/Contacts/etc. can adopt it later.
4. Refactor Files page to use the sidebar and move Files/All files out of the content toolbar.
5. Add missing Files features incrementally with simple API/UI state, no new dependencies:
   - show hidden toggle
   - configurable columns
   - show/hide file extensions
   - warn when changing extension
   - confirm before delete
   - sort by columns
   - right-side file details pane
   - 3-dot action menu
   - copy/move files
   - download files from the menu and/or direct action

## Files likely to change

- `src/webui/layout.zig`
  - Global top bar layout, left-sidebar capable page helper.
- `src/apps/registry.zig`
  - Icon-only app bar, avatar dropdown, reusable app sidebar builder.
- `src/apps/files/files.zig`
  - Files UI layout, JS state, column visibility, sorting, details pane, 3-dot menu, copy/move APIs.
- `test/e2e_test.sh`
  - Focused assertions for generated shell markup and Files API behavior.
- Optional if helper extraction becomes worthwhile:
  - Create `src/webui/sidebar.zig`
  - Create `src/webui/user_menu.zig`
  - Only do this if it keeps `layout.zig`/`registry.zig` smaller. Otherwise avoid new abstractions.

## Design constraints

- Use Tailwind utility classes and existing Material 3 theme tokens only.
- No custom CSS files.
- No NPM, no frontend build step, no framework.
- Vanilla JS only.
- Keep app route handlers directly registered; do not wrap app routes in middleware.
- Keep app handlers guarded with `ctx.user orelse`, never `ctx.user.?`.
- Avoid broad architectural rewrites. The shared shell should be a small extension to the current layout/registry pattern.

---

## Task 1: Add icon-only app bar and avatar dropdown to the shared shell

**Objective:** Make the global top bar Nextcloud-like: `Dar` hard-left, app icons immediately to the right, avatar dropdown on the far right.

**Files:**
- Modify: `src/webui/layout.zig:5-32`
- Modify: `src/apps/registry.zig:20-34`
- Modify: `src/apps/registry.zig:59-64`
- Test: `test/e2e_test.sh`

**Step 1: Write failing e2e markup checks**

Add checks after authenticated `/home` fetch or create a small authenticated page fetch against `/files`:

```bash
HOME_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/home")
if echo "$HOME_HTML" | grep -q 'id="dar-app-bar"' && echo "$HOME_HTML" | grep -q 'aria-label="Files"'; then
    pass_msg "Top bar renders icon-only app nav"
else
    fail_msg "Top bar renders icon-only app nav"
fi

if echo "$HOME_HTML" | grep -q 'id="user-menu-button"' && echo "$HOME_HTML" | grep -q 'id="user-menu"'; then
    pass_msg "Top bar renders avatar user menu"
else
    fail_msg "Top bar renders avatar user menu"
fi
```

**Step 2: Run test to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL on new top bar checks because the current markup lacks `dar-app-bar`, `user-menu-button`, and icon-only app labels.

**Step 3: Update `layout.page` top bar**

Replace the centered wrapper with full-width left/right layout. Keep the same function signature for compatibility:

```zig
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
        \\    <a href="/home" class="text-[22px] font-medium text-on-surface no-underline mr-4 shrink-0">Dar</a>
        \\    {s}
        \\    <div class="ml-auto flex items-center gap-2">
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
    , .{ title, m3.head(), app_bar_html, user_info_html, sidebar_html, content_html });
}
```

Notes:
- Preserve `layout.page(...)` so Home, Calendar, Contacts, etc. do not break.
- `pageWithSidebar(...)` is the shared future-app hook.

**Step 4: Update app bar to icon-only**

In `src/apps/registry.zig`, change `buildAppBar` to render only icons, with accessible labels/tooltips:

```zig
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
```

**Step 5: Update user avatar dropdown**

In `buildUserInfo`, replace text+logout with blank avatar and dropdown:

```zig
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
        \\function toggleUserMenu(){let m=document.getElementById('user-menu');if(m)m.classList.toggle('hidden')}
        \\document.addEventListener('click',function(e){let b=document.getElementById('user-menu-button');let m=document.getElementById('user-menu');if(b&&m&&!b.contains(e.target)&&!m.contains(e.target))m.classList.add('hidden')});
        \\</script>
    , .{ if (user.display_name.len > 0) user.display_name[0] else 'U', user.display_name });
}
```

**Step 6: Run test to verify pass**

Run:

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

Expected:
- Build passes.
- New top-bar checks pass.
- Existing e2e remains green.

**Step 7: Commit**

```bash
git add src/webui/layout.zig src/apps/registry.zig test/e2e_test.sh
git commit -m "feat: add nextcloud-style top bar"
```

---

## Task 2: Add reusable left sidebar helper for app-local navigation

**Objective:** Provide a shared left sidebar pattern that Files can use now and other apps can adopt later.

**Files:**
- Modify: `src/apps/registry.zig`
- Modify: `src/webui/layout.zig`
- Test: `test/e2e_test.sh`

**Step 1: Write failing e2e check for Files sidebar**

Add after authenticated `/files` request:

```bash
FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'id="app-sidebar"' && echo "$FILES_HTML" | grep -q 'data-scope="files"' && echo "$FILES_HTML" | grep -q 'data-scope="all"'; then
    pass_msg "Files page renders shared left sidebar"
else
    fail_msg "Files page renders shared left sidebar"
fi

if echo "$FILES_HTML" | grep -q 'href="/settings"'; then
    pass_msg "App sidebar includes bottom Settings item"
else
    fail_msg "App sidebar includes bottom Settings item"
fi
```

**Step 2: Run test to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL because Files still uses top tabs.

**Step 3: Add a sidebar builder**

Add a small shared function in `src/apps/registry.zig` or a new `src/webui/sidebar.zig`. Prefer `registry.zig` initially to avoid abstraction churn:

```zig
pub const SidebarItem = struct {
    label: []const u8,
    icon: []const u8,
    href: []const u8,
    data_scope: []const u8 = "",
    active: bool = false,
};

pub fn buildAppSidebar(allocator: std.mem.Allocator, items: []const SidebarItem) ![]const u8 {
    var buf: std.ArrayList(u8) = .empty;
    try buf.appendSlice(allocator,
        "<aside id=\"app-sidebar\" class=\"w-60 shrink-0 bg-surface border-r border-outline-variant min-h-[calc(100vh-56px)] flex flex-col\">" ++
        "<nav class=\"p-3 flex flex-col gap-1\">"
    );
    for (items) |item| {
        const cls = if (item.active)
            "bg-primary-container text-primary"
        else
            "text-on-surface-variant hover:bg-surface-container-highest";
        const row = try std.fmt.allocPrint(allocator,
            \\<a href="{s}" data-scope="{s}" class="flex items-center gap-3 rounded-full px-4 h-10 text-sm font-medium no-underline transition-colors {s}">
            \\  <span class="w-5 text-center">{s}</span>
            \\  <span>{s}</span>
            \\</a>
        , .{ item.href, item.data_scope, cls, item.icon, item.label });
        defer allocator.free(row);
        try buf.appendSlice(allocator, row);
    }
    try buf.appendSlice(allocator,
        "</nav>" ++
        "<div class=\"mt-auto p-3 border-t border-outline-variant\">" ++
        "<a href=\"/settings\" class=\"flex items-center gap-3 rounded-full px-4 h-10 text-sm font-medium text-on-surface-variant hover:bg-surface-container-highest no-underline\"><span>⚙</span><span>Settings</span></a>" ++
        "</div>" ++
        "</aside>"
    );
    return buf.toOwnedSlice(allocator);
}
```

**Step 4: Use `layout.pageWithSidebar` from Files**

In `src/apps/files/files.zig`, build sidebar:

```zig
const sidebar = try registry.buildAppSidebar(ctx.allocator, &.{
    .{ .label = "Files", .icon = "📁", .href = "#", .data_scope = "files", .active = true },
    .{ .label = "All files", .icon = "🗂", .href = "#", .data_scope = "all" },
});
const html = try layout.pageWithSidebar(ctx.allocator, "Files", app_bar, user_info, sidebar, content);
```

**Step 5: Wire sidebar clicks to Files scope**

In Files inline JS, add event binding after functions:

```js
document.querySelectorAll('#app-sidebar [data-scope]').forEach(a => {
  a.addEventListener('click', ev => {
    ev.preventDefault();
    setScope(a.dataset.scope);
  });
});
```

Also update active classes inside `setScope` by toggling sidebar links instead of old top buttons.

**Step 6: Remove old top Files/All files tab markup**

Delete the old button row from `content`:

```html
<button id="tab-files" ...>Files</button>
<button id="tab-all" ...>All files</button>
```

Keep the toolbar for upload/folder actions.

**Step 7: Run test to verify pass**

Run:

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

Expected: sidebar checks pass.

**Step 8: Commit**

```bash
git add src/apps/registry.zig src/webui/layout.zig src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: add reusable app sidebar"
```

---

## Task 3: Add Files view preferences: hidden files, extension display, and column visibility

**Objective:** Let the user toggle hidden files, file extensions, and visible columns client-side without changing storage.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing API/e2e fixture assertions**

Because the behavior is UI-side, use generated HTML checks and API setup:

```bash
printf 'hidden' > "data/users/$USER_ID/files/.secret"
FILES_HIDDEN_OFF=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/list?path=/")
if echo "$FILES_HIDDEN_OFF" | grep -q '"name":".secret"'; then
    pass_msg "Files API returns hidden files for client-side filtering"
else
    fail_msg "Files API returns hidden files for client-side filtering"
fi

FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'id="toggle-hidden"' && echo "$FILES_HTML" | grep -q 'id="toggle-extensions"' && echo "$FILES_HTML" | grep -q 'id="columns-menu"'; then
    pass_msg "Files page renders view preference controls"
else
    fail_msg "Files page renders view preference controls"
fi
```

**Step 2: Run test to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL due to missing controls.

**Step 3: Add state variables**

In Files JS:

```js
let showHidden = false;
let showExtensions = true;
let visibleColumns = {size:true, modified:true};
let currentFiles = [];
```

**Step 4: Add toolbar controls**

Add controls near Upload/+Folder:

```html
<button id="toggle-hidden" onclick="toggleHidden()" class="border border-outline rounded-full px-4 h-9 text-xs font-medium text-primary hover:bg-primary-container">Show hidden</button>
<button id="toggle-extensions" onclick="toggleExtensions()" class="border border-outline rounded-full px-4 h-9 text-xs font-medium text-primary hover:bg-primary-container">Extensions</button>
<div class="relative">
  <button onclick="toggleColumnsMenu()" class="border border-outline rounded-full px-4 h-9 text-xs font-medium text-primary hover:bg-primary-container">Columns</button>
  <div id="columns-menu" class="hidden absolute right-0 mt-2 bg-surface-container rounded-md shadow-2 border border-outline-variant p-2 z-20">
    <label class="block px-2 py-1 text-sm"><input type="checkbox" checked onchange="setColumn('size',this.checked)"> Size</label>
    <label class="block px-2 py-1 text-sm"><input type="checkbox" checked onchange="setColumn('modified',this.checked)"> Modified</label>
  </div>
</div>
```

**Step 5: Apply filtering/rendering**

Add helpers:

```js
function displayName(name){
  if(showExtensions) return name;
  let i=name.lastIndexOf('.');
  if(i<=0) return name;
  return name.slice(0,i);
}
function visibleList(files){
  return files.filter(f => showHidden || !f.name.startsWith('.'));
}
function toggleHidden(){showHidden=!showHidden;renderFiles({files:currentFiles})}
function toggleExtensions(){showExtensions=!showExtensions;renderFiles({files:currentFiles})}
function setColumn(k,v){visibleColumns[k]=v;renderFiles({files:currentFiles})}
function toggleColumnsMenu(){document.getElementById('columns-menu').classList.toggle('hidden')}
```

In `renderFiles`, set:

```js
currentFiles = d.files || currentFiles;
let files = visibleList(currentFiles);
```

Only render Size/Modified cells if enabled. Keep table headers in sync by re-rendering header or hiding cells via classes.

**Step 6: Run test to verify pass**

Run:

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

Expected: build/e2e pass.

**Step 7: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: add files view preferences"
```

---

## Task 4: Add sortable columns

**Objective:** Let the user sort Files by name, size, and modified date by clicking column headers.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing markup check**

```bash
FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'onclick="sortBy(\'name\')"' && echo "$FILES_HTML" | grep -q 'onclick="sortBy(\'size\')"'; then
    pass_msg "Files table headers are sortable"
else
    fail_msg "Files table headers are sortable"
fi
```

If shell escaping is awkward, check for `sortBy('name')` and `sortBy('size')` separately.

**Step 2: Run test to verify failure**

Run:

```bash
bash test/e2e_test.sh
```

Expected: FAIL.

**Step 3: Add sort state and function**

```js
let sortKey = 'name';
let sortDir = 1;
function sortBy(k){
  if(sortKey===k) sortDir *= -1;
  else { sortKey=k; sortDir=1; }
  renderFiles({files:currentFiles});
}
function cmp(a,b){
  if(a.is_dir&&!b.is_dir) return -1;
  if(!a.is_dir&&b.is_dir) return 1;
  let av=a[sortKey], bv=b[sortKey];
  if(sortKey==='name'){av=a.name.toLowerCase();bv=b.name.toLowerCase();}
  if(av<bv) return -1*sortDir;
  if(av>bv) return 1*sortDir;
  return 0;
}
```

Replace current sort with:

```js
files.sort(cmp);
```

**Step 4: Make headers clickable**

Render headers with buttons:

```html
<th class="..."><button onclick="sortBy('name')" class="...">Name</button></th>
<th class="..."><button onclick="sortBy('size')" class="...">Size</button></th>
<th class="..."><button onclick="sortBy('updated_at')" class="...">Modified</button></th>
```

**Step 5: Run test to verify pass**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 6: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: sort files by table columns"
```

---

## Task 5: Add right-side file details pane

**Objective:** Clicking/selecting a file or folder opens a right pane with details, like Nextcloud.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing markup check**

```bash
FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'id="details-pane"' && echo "$FILES_HTML" | grep -q 'function showDetails'; then
    pass_msg "Files page renders details pane"
else
    fail_msg "Files page renders details pane"
fi
```

**Step 2: Run test to verify failure**

```bash
bash test/e2e_test.sh
```

Expected: FAIL.

**Step 3: Add details pane markup**

Wrap table and pane in flex:

```html
<div class="flex gap-4">
  <div class="flex-1 min-w-0 bg-surface-container rounded-md shadow-1">...table...</div>
  <aside id="details-pane" class="hidden w-80 shrink-0 bg-surface-container rounded-md shadow-1 p-4">
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-lg font-medium">Details</h2>
      <button onclick="hideDetails()" class="...">×</button>
    </div>
    <div id="details-content"></div>
  </aside>
</div>
```

**Step 4: Add JS to open/close details**

```js
let selectedFile = null;
function showDetails(path, file){
  selectedFile = file;
  let p=document.getElementById('details-pane');
  let c=document.getElementById('details-content');
  p.classList.remove('hidden');
  c.innerHTML='<div class="text-[40px] mb-3">'+(file.is_dir?'FOLDER':'FILE')+'</div>'+
    '<div class="font-medium mb-4">'+e(file.name)+'</div>'+
    '<dl class="text-sm space-y-2">'+
    '<div><dt class="text-on-surface-variant">Type</dt><dd>'+(file.is_dir?'Folder':'File')+'</dd></div>'+
    '<div><dt class="text-on-surface-variant">Size</dt><dd>'+(file.is_dir?'-':fs(file.size))+'</dd></div>'+
    '<div><dt class="text-on-surface-variant">Modified</dt><dd>'+fd(file.updated_at)+'</dd></div>'+
    '<div><dt class="text-on-surface-variant">Path</dt><dd class="break-all">'+e(path)+'</dd></div>'+
    '</dl>';
}
function hideDetails(){document.getElementById('details-pane').classList.add('hidden')}
```

**Step 5: Wire row click behavior carefully**

- Directory row click should still browse.
- Add a dedicated “details” action in the 3-dot menu in Task 6, or use single click for details and double click for directory navigation. To avoid behavior surprise, keep directory row click for navigation and add `Details` in the action menu in Task 6.
- For now, add a small temporary details button or wire file row click to details only for files.

**Step 6: Run test to verify pass**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 7: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: add files details pane"
```

---

## Task 6: Move all file actions into a 3-dot menu

**Objective:** Replace inline DL/E/X actions with a single 3-dot menu per row containing Details, Rename, Move/Copy, Download, Delete.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing markup check**

```bash
FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'function openActionMenu' && echo "$FILES_HTML" | grep -q 'aria-label="File actions"'; then
    pass_msg "Files rows use 3-dot action menus"
else
    fail_msg "Files rows use 3-dot action menus"
fi
```

**Step 2: Run test to verify failure**

```bash
bash test/e2e_test.sh
```

Expected: FAIL.

**Step 3: Add per-row action menu button**

Replace inline actions with:

```js
h+='<button aria-label="File actions" onclick="event.stopPropagation();openActionMenu(event,\''+full+'\')" class="w-8 h-8 rounded-full border-0 bg-transparent text-on-surface-variant hover:bg-surface-container-highest cursor-pointer">⋮</button>';
```

**Step 4: Add one floating menu container**

Add to HTML:

```html
<div id="file-action-menu" class="hidden fixed bg-surface-container rounded-md shadow-2 border border-outline-variant py-2 z-30 min-w-40"></div>
```

**Step 5: Add menu JS**

```js
function findFile(path){return currentFiles.find(f=>joinPath(currentPath,f.name)===path)}
function openActionMenu(ev,path){
  let f=findFile(path); if(!f)return;
  let m=document.getElementById('file-action-menu');
  let download = f.is_dir ? '' : '<a href="/api/files/download?path='+encodeURIComponent(path)+scopeParam()+'" class="block px-4 py-2 text-sm text-on-surface no-underline hover:bg-surface-container-highest">Download</a>';
  m.innerHTML =
    '<button onclick="showDetails(\''+path+'\',findFile(\''+path+'\'))" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Details</button>'+
    '<button onclick="startRename(\''+path+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Rename</button>'+
    '<button onclick="startMoveCopy(\''+path+'\')" class="block w-full text-left px-4 py-2 text-sm hover:bg-surface-container-highest border-0 bg-transparent">Move or copy</button>'+
    download+
    '<button onclick="deleteWithConfirm(\''+path+'\')" class="block w-full text-left px-4 py-2 text-sm text-error hover:bg-error-container border-0 bg-transparent">Delete</button>';
  m.style.left = ev.clientX + 'px';
  m.style.top = ev.clientY + 'px';
  m.classList.remove('hidden');
}
document.addEventListener('click',()=>document.getElementById('file-action-menu')?.classList.add('hidden'));
```

**Step 6: Keep delete warning**

Replace `df(...)` with a named warning function:

```js
async function deleteWithConfirm(p){
  let f=findFile(p);
  if(!confirm('Delete '+(f?f.name:p)+'? This cannot be undone.')) return;
  await fetch('/api/files/delete',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:p})});
  navigate(currentPath);
}
```

**Step 7: Run test to verify pass**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 8: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: add files action menu"
```

---

## Task 7: Warn when renaming changes file extension

**Objective:** Show a browser confirmation when a rename changes a file extension.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing markup check**

```bash
FILES_HTML=$(curl -s -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/files")
if echo "$FILES_HTML" | grep -q 'function extOf' && echo "$FILES_HTML" | grep -q 'changes the file extension'; then
    pass_msg "Files rename warns on extension changes"
else
    fail_msg "Files rename warns on extension changes"
fi
```

**Step 2: Run test to verify failure**

```bash
bash test/e2e_test.sh
```

Expected: FAIL.

**Step 3: Add extension helpers**

```js
function extOf(name){let i=name.lastIndexOf('.');return i>0?name.slice(i+1).toLowerCase():''}
function nameOfPath(p){let i=p.lastIndexOf('/');return i>=0?p.slice(i+1):p}
```

**Step 4: Add warning in rename submit**

Before move request:

```js
let oldName = nameOfPath(renameTarget);
if(extOf(oldName)!==extOf(nn) && !confirm('This changes the file extension. The file may stop working. Continue?')) return;
```

**Step 5: Run test to verify pass**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 6: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: warn before changing file extensions"
```

---

## Task 8: Add copy and move API behavior

**Objective:** Support moving and copying files/folders, not just renaming within the same folder.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing e2e API tests**

Add inside authenticated Files block:

```bash
mkdir -p "data/users/$USER_ID/files/source" "data/users/$USER_ID/files/target"
printf 'copy me' > "data/users/$USER_ID/files/source/copy.txt"
COPY_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/source/copy.txt","target_path":"/target","mode":"copy"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/transfer")
if echo "$COPY_BODY" | grep -q '"status":"ok"' && [ "$(cat "data/users/$USER_ID/files/target/copy.txt" 2>/dev/null || true)" = "copy me" ]; then
    pass_msg "Files copy copies file to target folder"
else
    fail_msg "Files copy copies file to target folder"
fi

MOVE_BODY=$(curl -s -b "dar_session=$SESSION_COOKIE" -H 'Content-Type: application/json' --data '{"path":"/source/copy.txt","target_path":"/target","mode":"move"}' --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/transfer")
if echo "$MOVE_BODY" | grep -q '"status":"ok"' && [ ! -e "data/users/$USER_ID/files/source/copy.txt" ] && [ -e "data/users/$USER_ID/files/target/copy.txt" ]; then
    pass_msg "Files move moves file to target folder"
else
    fail_msg "Files move moves file to target folder"
fi
```

**Step 2: Run test to verify failure**

```bash
bash test/e2e_test.sh
```

Expected: FAIL because `/api/files/transfer` does not exist.

**Step 3: Add route**

In Files `app.routes`:

```zig
.{ .method = "POST", .path = "/api/files/transfer", .handler = &handleTransfer },
```

**Step 4: Add handler**

```zig
fn handleTransfer(req: *std.http.Server.Request, ctx: *request_mod.RequestContext) !void {
    const user = ctx.user orelse {
        _ = req.readerExpectNone(&[_]u8{});
        try response.json(req, "{\"error\":\"unauthorized\"}");
        return;
    };
    const body = try readBody(req, ctx.allocator);
    defer ctx.allocator.free(body);
    const scope = parseScope(extractJsonString(body, "scope"));
    const src = extractJsonString(body, "path") orelse return response.json(req, "{\"error\":\"missing path\"}");
    const target = extractJsonString(body, "target_path") orelse return response.json(req, "{\"error\":\"missing target_path\"}");
    const mode = extractJsonString(body, "mode") orelse "move";
    if (std.mem.eql(u8, mode, "copy")) {
        try copyUserEntry(ctx.allocator, user.id, scope, src, target);
    } else {
        try moveUserEntryToDir(ctx.allocator, user.id, scope, src, target);
    }
    try response.json(req, "{\"status\":\"ok\"}");
}
```

Note: In Zig, avoid `return response.json(...)` if the type does not line up; expand into `try response.json(...); return;` if needed.

**Step 5: Implement file copy only first**

Keep scope tight. Implement file copy for regular files; directory recursive copy can be a follow-up if needed.

```zig
fn copyUserEntry(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, target_dir_rel: []const u8) !void {
    const src = try joinUserDiskPath(allocator, user_id, scope, rel_path);
    defer allocator.free(src);
    const target_dir = try joinUserDiskPath(allocator, user_id, scope, target_dir_rel);
    defer allocator.free(target_dir);
    const dst = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ target_dir, baseName(rel_path) });
    defer allocator.free(dst);
    const content = try std.Io.Dir.cwd().readFileAlloc(compat.getIo(), src, allocator, .unlimited);
    defer allocator.free(content);
    var f = try std.Io.Dir.cwd().createFile(compat.getIo(), dst, .{});
    defer f.close(compat.getIo());
    _ = try f.writeStreamingAll(compat.getIo(), content);
}
```

**Step 6: Implement move-to-folder**

```zig
fn moveUserEntryToDir(allocator: std.mem.Allocator, user_id: []const u8, scope: FileScope, rel_path: []const u8, target_dir_rel: []const u8) !void {
    const src = try joinUserDiskPath(allocator, user_id, scope, rel_path);
    defer allocator.free(src);
    const target_dir = try joinUserDiskPath(allocator, user_id, scope, target_dir_rel);
    defer allocator.free(target_dir);
    const dst = try std.fmt.allocPrint(allocator, "{s}/{s}", .{ target_dir, baseName(rel_path) });
    defer allocator.free(dst);
    try std.Io.Dir.rename(std.Io.Dir.cwd(), src, std.Io.Dir.cwd(), dst, compat.getIo());
}
```

**Step 7: Add UI modal/prompt from 3-dot menu**

For now, use `prompt` to avoid complex modal state:

```js
async function startMoveCopy(path){
  let target=prompt('Target folder path:', '/');
  if(!target)return;
  let mode=confirm('Copy instead of move? OK = copy, Cancel = move')?'copy':'move';
  await fetch('/api/files/transfer',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({scope:currentScope,path:path,target_path:target,mode:mode})});
  navigate(currentPath);
}
```

Later this can become a proper picker modal.

**Step 8: Run test to verify pass**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 9: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "feat: move and copy files"
```

---

## Task 9: Polish download behavior

**Objective:** Ensure file downloads are available from the 3-dot menu and preserve filename behavior.

**Files:**
- Modify: `src/apps/files/files.zig`
- Test: `test/e2e_test.sh`

**Step 1: Add failing e2e check for content-disposition**

Current download already returns content. Add a header assertion:

```bash
DOWNLOAD_HEADERS=$(curl -s -D- -o /tmp/dar_download_test.out -b "dar_session=$SESSION_COOKIE" --connect-timeout 3 "http://localhost:$DAR_PORT/api/files/download?path=/note.txt")
if echo "$DOWNLOAD_HEADERS" | grep -qi 'content-disposition: attachment; filename="note.txt"'; then
    pass_msg "Files download sets attachment filename"
else
    fail_msg "Files download sets attachment filename"
fi
rm -f /tmp/dar_download_test.out
```

**Step 2: Run test to verify current state**

```bash
bash test/e2e_test.sh
```

Expected: likely PASS already. If it fails, fix the header formatting.

**Step 3: Ensure 3-dot menu has download link**

The Task 6 action menu should include:

```js
'<a href="/api/files/download?path='+encodeURIComponent(path)+scopeParam()+'" ...>Download</a>'
```

**Step 4: Run build/e2e**

```bash
nix develop /home/anas/soosa --command zig build
bash test/e2e_test.sh
```

**Step 5: Commit**

```bash
git add src/apps/files/files.zig test/e2e_test.sh
git commit -m "test: verify files download headers"
```

---

## Task 10: Final visual QA pass

**Objective:** Verify the UI behaves acceptably in a browser/TUI before claiming completion.

**Files:**
- No source changes unless visual issues are found.
- Optional temporary verification script under `/tmp/hermes-verify-*.sh`.

**Step 1: Build**

```bash
nix develop /home/anas/soosa --command zig build
```

Expected: success.

**Step 2: Run e2e**

```bash
bash test/e2e_test.sh
```

Expected: `Failed: 0`.

**Step 3: Launch local server**

```bash
./launch.sh --port 9090
```

Expected: Dar starts with dummy OAuth provider.

**Step 4: Browser checklist**

Manually verify:

- Top bar:
  - `Dar` is all the way on the left.
  - App icons start immediately next to `Dar` and go right.
  - App names are hidden; tooltips/labels remain accessible.
  - Avatar is round and blank/initialed.
  - Avatar dropdown order is: username text, Settings, Logout.
- Files page:
  - Left sidebar shows Files and All files at top.
  - Settings is pinned to the bottom of sidebar.
  - Clicking Files/All files switches scope.
  - Folder rows are clickable across the whole row.
  - Upload works.
  - Hidden toggle works.
  - Columns menu works.
  - Extension toggle works.
  - Sort headers work.
  - 3-dot menu opens and contains Details, Rename, Move/Copy, Download, Delete.
  - Details opens in a right-side pane.
  - Rename warns on extension changes.
  - Delete warns before deleting.
  - Move/copy works.
  - Download works.

**Step 5: Stop server**

If launched in foreground, stop with Ctrl-C. If launched in background, kill the tracked process.

**Step 6: Commit final fixes if any**

```bash
git add src/webui/layout.zig src/apps/registry.zig src/apps/files/files.zig test/e2e_test.sh
git commit -m "polish: refine nextcloud-style files UI"
```

---

## Risks and tradeoffs

1. **Inline JavaScript complexity:** `files.zig` already has a large inline JS string. Adding all controls there increases maintenance cost. If implementation gets unwieldy, extract only the JS string into helper functions inside the same Zig file first; do not introduce a frontend build step.
2. **Visual acceptance:** Build/e2e can verify markup and API behavior, but not whether the design “feels” like Nextcloud/Google Photos. User visual approval is required before considering the UI final.
3. **Settings route:** The dropdown/sidebar can link to `/settings`, but the route may not exist yet. Either add a placeholder settings page in a separate plan/task or accept 404 temporarily. Prefer adding the link now and implementing Settings later only if user asks.
4. **Copying directories:** This plan implements file copy first. Recursive directory copy needs careful traversal and should be a follow-up unless the user explicitly needs it now.
5. **Accessibility:** Icon-only app nav must keep `aria-label` and `title`. Avatar menu needs keyboard handling eventually; initial plan covers click behavior only.
6. **App-wide sidebar API:** Putting sidebar builder in `registry.zig` is pragmatic. If Calendar/Contacts need richer app-specific nav later, move it to `src/webui/sidebar.zig`.

## Open questions

1. Should `/settings` be implemented now as a real page, or is a placeholder/404 acceptable until a dedicated Settings task?
2. Should the avatar show the user initial, or truly blank until photo upload exists?
3. For “show file extensions,” should the actual file name column hide extensions only visually, or should rename default to the name without extension like desktop file managers?
4. For moving/copying, is a prompt-based target path acceptable for the first version, or do you want a proper folder picker modal immediately?
5. Should copy support directories recursively now, or only files for the first implementation?
6. Should view preferences persist per user in SQLite, localStorage, or reset on every page load?
7. Should hidden files be hidden by default only in the Files scope, or also in All files?
