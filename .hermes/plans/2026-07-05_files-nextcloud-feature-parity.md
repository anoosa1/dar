# Dar Files App — Nextcloud Feature Parity Plan

> **For Hermes:** User will confirm which features to implement before execution begins.

**Goal:** Make the Dar Files app as feature-rich as Nextcloud Files, with proper URL routing and full-width layout.

**Architecture:** Files daemon serves disk-based storage. Main server proxies `/api/files/*` and routes web UI at `/files`, `/files/files`, `/files/trash`, `/files/<folder>`. Sidebar tabs drive URL-based navigation. Main content fills available width (no max-w constraint).

---
---

## URL Structure

| URL | Content |
|-----|---------|
| `/files` | User home — shows all folders including app folders |
| `/files/files` | Personal files (the user's own documents) |
| `/files/trash` | Deleted files with restore/delete forever |
| `/files?filter=favorites` | All favorited files across folders |
| `/files?filter=recent` | Recently modified files |
| `/files/<folder>` | Any folder created by user or apps |

Folders like `Documents`, `Photos`, `Music` are just subdirectories of `/files/files/`. Apps that place files in the user's home (e.g., a backup app) create folders at `/files/<app-folder>`.

---

## Nextcloud Files — Complete Feature List

Below is every feature Nextcloud Files offers. Mark which ones to implement.

### Core File Operations
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 1 | **Upload** | Single and multi-file upload via button or drag-and-drop | ☐ |
| 2 | **Download** | Download individual files or folders (as zip) | ☐ |
| 3 | **New folder** | Create directories | ☐ |
| 4 | **New text file** | Create a blank .txt or .md file inline | ☐ |
| 5 | **Rename** | Rename files and folders (inline or dialog) | ☐ |
| 6 | **Move/Copy** | Move or copy files between folders (with picker dialog) | ☐ |
| 7 | **Delete** | Move to trash (soft delete) | ☐ |
| 8 | **Delete permanently** | Delete forever from trash | ☐ |
| 9 | **Restore from trash** | Restore deleted file back to original location | ☐ |
| 10 | **Bulk operations** | Select multiple files, apply action to all | ☐ |

### File Previews & Thumbnails
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 11 | **Image thumbnails** | Grid view with image previews | ☐ |
| 12 | **Image preview** | Click image → full preview with prev/next navigation | ☐ |
| 13 | **Video preview** | Inline video player | ☐ |
| 14 | **Audio preview** | Inline audio player | ☐ |
| 15 | **Text preview** | View text files inline with syntax highlighting | ☐ |
| 16 | **PDF preview** | Embedded PDF viewer | ☐ |
| 17 | **Office document preview** | Preview .docx, .xlsx, .pptx (requires collabora/onlyoffice integration) | ☐ |
| 18 | **Markdown preview** | Rendered markdown view | ☐ |

### Sharing
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 19 | **Share link** | Create public share link with optional password and expiration | ☐ |
| 20 | **Share with users** | Share file/folder with other Dar users (read/write permissions) | ☐ |
| 21 | **File drop** | Share folder where others can upload but not see contents | ☐ |
| 22 | **Share management** | List all shares, edit permissions, revoke | ☐ |
| 23 | **Internal link** | Copy direct link to file for other Dar users | ☐ |
| 24 | **Email share** | Send share link via email | ☐ |

### Favorites & Tags
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 25 | **Favorites** | Star files/folders, filter view to show only favorites | ☐ |
| 26 | **Tags** | Assign colored tags to files, filter by tag | ☐ |
| 27 | **Tag management** | Create, rename, delete, recolor tags | ☐ |

### Comments
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 28 | **Comments sidebar** | Add comments to any file, threaded discussion | ☐ |
| 29 | **Comment notifications** | Notify mentioned users | ☐ |

### Versioning
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 30 | **Version history** | Track changes, list previous versions with timestamps | ☐ |
| 31 | **Restore version** | Roll back to any previous version | ☐ |
| 32 | **Download version** | Download a specific version without restoring | ☐ |
| 33 | **Version diff** | Show diff between versions (for text files) | ☐ |

### Activity & Audit
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 34 | **Activity feed** | Show recent changes (who did what, when) | ☐ |
| 35 | **File locking** | Lock a file while editing to prevent conflicts | ☐ |

### Search
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 36 | **Full-text search** | Search file names and content | ☐ |
| 37 | **Advanced search** | Filter by type, date, size, owner, tag | ☐ |
| 38 | **Search within folder** | Scope search to current folder | ☐ |

### UI/UX
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 39 | **Grid/List toggle** | Switch between grid (thumbnails) and list (table) views | ☐ |
| 40 | **Sort by** | Sort by name, size, date, type (ascending/descending) | ☐ |
| 41 | **Breadcrumbs** | Path navigation bar at top | ☐ |
| 42 | **File details sidebar** | Click file → show details panel (size, type, date, versions, shares, comments, tags) | ☐ |
| 43 | **Keyboard shortcuts** | `Ctrl+Click` multi-select, `Del` delete, `F2` rename, `Ctrl+A` select all | ☐ |
| 44 | **Drag-and-drop upload** | Drag files from desktop into browser | ☐ |
| 45 | **Drag-and-drop move** | Drag files between folders within the UI | ☐ |
| 46 | **Right-click context menu** | Context menu with file actions | ☐ |
| 47 | **Loading states** | Skeleton loaders, progress bars for uploads | ☐ |
| 48 | **Empty states** | Helpful messages when folders are empty | ☐ |
| 49 | **Responsive design** | Mobile-friendly layout | ☐ |
| 50 | **Dark mode** | Respects system/browser dark mode preference | ☐ |

### Quota & Storage
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 51 | **Storage usage bar** | Show used/total space | ☐ |
| 52 | **Per-folder size** | Show folder sizes | ☐ |

### External Storage & Federation
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 53 | **External storage mounts** | Mount S3, FTP, SMB, WebDAV as folders | ☐ |
| 54 | **Federated sharing** | Share with users on other Nextcloud instances | ☐ |

### WebDAV
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 55 | **WebDAV access** | Mount files via WebDAV (Finder, Nautilus, Windows Explorer) | ☐ |

### Automation
| # | Feature | Description | Implement? |
|---|---------|-------------|------------|
| 56 | **Flow/automation** | Trigger actions on file events (file created → notify, tag, move) | ☐ |

---
---

## Layout Design

The Files app must fill the full content area — no `max-w` constraint squeezing it into a column. The layout:

```
┌─────────────────────────────────────────────────────┐
│ App Bar (Dar logo + app icons + user menu)          │
├──────────┬──────────────────────────────────────────┤
│ Sidebar  │ Main Content (full remaining width)       │
│          │                                           │
│ All files│ ┌─────────────────────────────────────┐  │
│ Files    │ │ Toolbar: upload, new folder, search │  │
│ Trash    │ ├─────────────────────────────────────┤  │
│ Photos/  │ │                                     │  │
│ Music/   │ │ File/folder grid or list            │  │
│ ...      │ │ (fills full width)                  │  │
│          │ │                                     │  │
│ ──────── │ └─────────────────────────────────────┘  │
│ Settings │                                           │
└──────────┴──────────────────────────────────────────┘
```

Current layout uses `max-w-4xl mx-auto` which constrains to ~896px. Change to fill 100% width with padding.

---

## Sidebar Tabs (URL-driven)

| Label | URL | Icon |
|-------|-----|------|
| All files | `/files` | `fa-folder` |
| Files | `/files/files` | `fa-file` |
| Trash | `/files/trash` | `fa-trash` |
| Photos | `/files/Photos` | `fa-image` |
| Music | `/files/Music` | `fa-music` |
| Settings | `/settings` | `fa-gear` |

The "All files" view at `/files` is the root — it shows everything including personal files, shared folders, and app folders. "Files" at `/files/files` is the user's personal files directory.

Favorites and Recent are filters applied via query param: `/files?filter=favorites`, `/files?filter=recent`. When a filter is active, the sidebar highlights a "Favorites" or "Recent" tab and the view shows matching files from all folders.

---
---

## Open questions

1. **Which features from the 56-item list should be implemented?** Mark the ☐ boxes above.

2. **File details sidebar** — Nextcloud shows a right-side panel with file metadata (size, type, date, versions, shares, comments, tags). Implement now or defer?

3. **Versioning** — Store file versions on disk (like current Zig implementation), or defer to later phase?

4. **Sharing** — Full Nextcloud-style sharing (user shares, public links, passwords, expiration), or start with public links only?

5. **Comments** — Nextcloud has threaded comments on files. Worth implementing now?

6. **Right-click context menu** — Worth the JS complexity, or stick with click actions?

7. **Grid view with thumbnails** — The current view is a list/table. Add grid view toggle using photos thumbnails?

8. **App folders in /files/ root** — Which apps should place folders in the user's root? Currently Photos has its own daemon. Should it appear under `/files/Photos` or stay at `/photos`?
