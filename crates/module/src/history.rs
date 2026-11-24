//! Folder history (`history.ts`, docs/spacetimedb/folder-history.md): tracked folders, their
//! changesets on main, Agent branches staged against the live folder, restore and backups.
//! Changes point at file versions, which carry the bytes and quota.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::blobs::{release_blob, store_blob};
use crate::files::{Role, Unlock};
use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

pub const FILES: usize = 2000;
pub const DEPTH: usize = 32;
pub const CHANGES: usize = 2000;
pub const CHANGESETS: u32 = 10000;
pub const REPOS: usize = 50;
pub const MESSAGE_BYTES: usize = 2048;
const PAGE: usize = 50;
pub const READ_OPERATIONS: [&str; 4] = ["overview", "changeset", "content", "branch"];

/// File operations from the core, bound to the acting account and browser session.
#[derive(Clone, Copy)]
pub struct HistoryFiles<'a> {
    pub user_id: &'a str,
    pub unlock: Unlock<'a>,
}

impl HistoryFiles<'_> {
    pub fn role(&self, ctx: &ReducerContext, entry: &FileEntry) -> Option<Role> {
        crate::files::access(ctx, entry, self.user_id, self.unlock)
    }
    pub fn in_locked(&self, ctx: &ReducerContext, entry: &FileEntry) -> bool {
        crate::files::locked_root(ctx, entry).is_some()
    }
    pub fn uuid(&self, ctx: &ReducerContext) -> String {
        crate::unused_uuid(ctx)
    }
    pub fn event(
        &self,
        ctx: &ReducerContext,
        owner_id: &str,
        file_id: &str,
        action: &str,
        parent_id: Option<&str>,
    ) {
        crate::files::event(ctx, owner_id, file_id, action, parent_id);
    }
    pub fn fits_quota(&self, meter: &Usage, bytes: f64) -> bool {
        meter.quota == 0.0 || meter.used + meter.reserved + bytes <= meter.quota
    }
    /// Moves or renames a file the caller may write, keeping its favorite and share listings in step.
    pub fn move_entry(&self, ctx: &ReducerContext, entry: &FileEntry, parent_id: &str, name: &str) {
        crate::files::move_listed(ctx, entry, parent_id, name);
    }
}

pub fn id(value: Option<&Value>) -> Res<String> {
    match value.and_then(Value::as_str) {
        Some(s) if crate::opaque_id(s).is_ok() => Ok(s.to_owned()),
        _ => fail("invalidInput"),
    }
}

pub fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) {
        fail("invalidInput")
    } else {
        Ok(())
    }
}

fn text(value: Option<&Value>, required: bool) -> Res<String> {
    if value.is_none() && !required {
        return Ok(String::new());
    }
    let Some(s) = value.and_then(Value::as_str) else {
        return fail("invalidInput");
    };
    if s.contains('\0') {
        return fail("invalidInput");
    }
    let result = s.trim();
    if required && result.is_empty() || result.len() > MESSAGE_BYTES {
        return fail("invalidInput");
    }
    Ok(result.to_owned())
}

fn revision(repo: &FileRepo, value: Option<&Value>) -> Res<()> {
    if value.and_then(Value::as_f64) == Some(repo.revision as f64) {
        Ok(())
    } else {
        fail("conflict")
    }
}

fn referenced(ctx: &ReducerContext, version_id: &str) -> bool {
    ctx.db
        .file_changes()
        .version_id()
        .filter(version_id)
        .next()
        .is_some()
}

pub fn charge(ctx: &ReducerContext, files: &HistoryFiles, owner_id: &str, bytes: f64) -> Res<()> {
    let meter = match ctx.db.usage().user_id().find(owner_id.to_owned()) {
        Some(m) => m,
        None => ctx.db.usage().insert(Usage {
            user_id: owner_id.into(),
            used: 0.0,
            reserved: 0.0,
            quota: 0.0,
        }),
    };
    if !files.fits_quota(&meter, bytes) {
        return fail("quotaExceeded");
    }
    let used = meter.used + bytes;
    ctx.db.usage().user_id().update(Usage { used, ..meter });
    Ok(())
}

pub fn release(ctx: &ReducerContext, owner_id: &str, bytes: f64) {
    if let Some(meter) = ctx.db.usage().user_id().find(owner_id.to_owned())
        && bytes != 0.0
    {
        let used = (meter.used - bytes).max(0.0);
        ctx.db.usage().user_id().update(Usage { used, ..meter });
    }
}

pub fn fresh(ctx: &ReducerContext, files: &HistoryFiles) -> String {
    loop {
        let value = files.uuid(ctx);
        if ctx.db.file_changesets().id().find(value.clone()).is_none()
            && ctx.db.file_branches().id().find(value.clone()).is_none()
        {
            return value;
        }
    }
}

/// A folder the caller can read (or write); never a trashed one.
pub fn folder_of(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    value: Option<&Value>,
    write: bool,
) -> Res<(FileEntry, Role)> {
    let entry = ctx.db.file_entries().id().find(id(value)?);
    let Some(entry) = entry.filter(|e| e.kind == "folder" && !e.trashed) else {
        return fail("notFound");
    };
    let Some(role) = files.role(ctx, &entry) else {
        return fail("notFound");
    };
    if write && role == Role::Read {
        return fail("forbidden");
    }
    Ok((entry, role))
}

fn tracked_ancestor(ctx: &ReducerContext, entry: &FileEntry) -> bool {
    let mut parent = entry.parent_id.clone();
    for _ in 0..128 {
        if parent.is_empty() {
            return false;
        }
        if ctx.db.file_repos().id().find(parent.clone()).is_some() {
            return true;
        }
        parent = ctx
            .db
            .file_entries()
            .id()
            .find(parent)
            .map(|e| e.parent_id)
            .unwrap_or_default();
    }
    false
}

/// Live files under a tracked folder by path. Tracked folders inside it keep their own history.
pub fn scan(ctx: &ReducerContext, root: &FileEntry) -> Res<BTreeMap<String, FileEntry>> {
    let mut result = BTreeMap::new();
    let mut queue: Vec<(String, String, usize)> = vec![(root.id.clone(), String::new(), 0)];
    let mut i = 0;
    while i < queue.len() {
        let (folder_id, path, depth) = queue[i].clone();
        i += 1;
        let window = crate::folder_window(&format!("{}/{folder_id}/", root.owner_id));
        for entry in ctx
            .db
            .file_entries()
            .list_key()
            .filter(crate::bounds(&window))
        {
            if entry.trashed {
                continue;
            }
            let child = format!("{path}{}", entry.name);
            if entry.kind == "folder" {
                if ctx.db.file_repos().id().find(entry.id.clone()).is_some() {
                    continue;
                }
                if depth + 1 >= DEPTH {
                    return fail("operationTooLarge");
                }
                queue.push((entry.id.clone(), format!("{child}/"), depth + 1));
            } else {
                if result.len() >= FILES {
                    return fail("operationTooLarge");
                }
                result.insert(child, entry);
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct TreeItem {
    pub file_id: String,
    pub version_id: String,
    pub mode: String,
}

pub fn head_tree(ctx: &ReducerContext, repo_id: &str) -> BTreeMap<String, TreeItem> {
    ctx.db
        .file_trees()
        .repo_id()
        .filter(repo_id)
        .map(|r| {
            (
                r.path,
                TreeItem {
                    file_id: r.file_id,
                    version_id: r.version_id,
                    mode: r.mode,
                },
            )
        })
        .collect()
}

/// A path that differs between the live folder and main's head tree.
#[derive(Clone)]
pub struct Pending {
    pub path: String,
    pub previous_path: String,
    pub kind: String,
    pub entry: Option<FileEntry>,
    pub previous: Option<TreeItem>,
}

pub struct Status {
    pub live: BTreeMap<String, FileEntry>,
    pub changes: Vec<Pending>,
    pub token: String,
}

/// Uncommitted changes: the live folder against main's head, by content. A moved file is a rename.
pub fn status(ctx: &ReducerContext, repo: &FileRepo, root: &FileEntry) -> Res<Status> {
    let live = scan(ctx, root)?;
    let head = head_tree(ctx, &repo.id);
    let mut changes: Vec<Pending> = Vec::new();
    let mut added: Vec<(String, Pending)> = Vec::new();
    let mut deleted: Vec<Pending> = Vec::new();
    for (path, entry) in &live {
        match head.get(path) {
            None => added.push((
                entry.id.clone(),
                Pending {
                    path: path.clone(),
                    previous_path: String::new(),
                    kind: "added".into(),
                    entry: Some(entry.clone()),
                    previous: None,
                },
            )),
            Some(old) => {
                if ctx
                    .db
                    .versions()
                    .id()
                    .find(old.version_id.clone())
                    .map(|v| v.blob_id)
                    != Some(entry.blob_id.clone())
                {
                    changes.push(Pending {
                        path: path.clone(),
                        previous_path: String::new(),
                        kind: "modified".into(),
                        entry: Some(entry.clone()),
                        previous: Some(old.clone()),
                    });
                }
            }
        }
    }
    for (path, old) in &head {
        if !live.contains_key(path) {
            deleted.push(Pending {
                path: path.clone(),
                previous_path: String::new(),
                kind: "deleted".into(),
                entry: None,
                previous: Some(old.clone()),
            });
        }
    }
    for gone in deleted {
        let file_id = gone
            .previous
            .as_ref()
            .map(|p| p.file_id.clone())
            .unwrap_or_default();
        match added.iter().position(|(id, _)| *id == file_id) {
            Some(index) => {
                let (_, moved) = added.remove(index);
                changes.push(Pending {
                    path: moved.path,
                    previous_path: gone.path,
                    kind: "renamed".into(),
                    entry: moved.entry,
                    previous: gone.previous,
                });
            }
            None => changes.push(gone),
        }
    }
    changes.extend(added.into_iter().map(|(_, p)| p));
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let token = token(repo, &changes);
    Ok(Status {
        live,
        changes,
        token,
    })
}

/// Identifies a status, so a commit fails if the folder changed after it was reviewed.
fn token(repo: &FileRepo, changes: &[Pending]) -> String {
    let lines: Vec<String> = changes
        .iter()
        .map(|c| {
            format!(
                "{}\0{}\0{}\0{}",
                c.kind,
                c.path,
                c.previous_path,
                c.entry.as_ref().map(|e| e.blob_id.as_str()).unwrap_or("")
            )
        })
        .collect();
    let value = format!("{}\n{}", repo.head_id, lines.join("\n"));
    let (mut a, mut b): (u32, u32) = (0x811c_9dc5, 0x9747_b28c);
    for c in value.encode_utf16() {
        a = (a ^ c as u32).wrapping_mul(0x0100_0193);
        b = (b ^ c as u32).wrapping_mul(0x5bd1_e995);
        b ^= b >> 15;
    }
    format!("{a:08x}{b:08x}")
}

fn pending_view(ctx: &ReducerContext, item: &Pending) -> Value {
    let mut v = json!({ "path": item.path, "kind": item.kind });
    let o = v.as_object_mut().expect("change");
    if !item.previous_path.is_empty() {
        o.insert("previousPath".into(), json!(item.previous_path));
    }
    if let Some(e) = &item.entry {
        o.insert("fileId".into(), json!(e.id));
        o.insert("size".into(), json!(e.size));
        o.insert("mediaType".into(), json!(e.media_type));
    }
    if let Some(old) = item
        .previous
        .as_ref()
        .and_then(|p| ctx.db.versions().id().find(p.version_id.clone()))
    {
        o.insert("previousVersionId".into(), json!(old.id));
        o.insert("previousSize".into(), json!(old.size));
        o.insert("previousMediaType".into(), json!(old.media_type));
    }
    v
}

fn change_view(ctx: &ReducerContext, change: &FileChange) -> Value {
    let mut v = json!({ "path": change.path, "kind": change.kind });
    let o = v.as_object_mut().expect("change");
    if !change.previous_path.is_empty() {
        o.insert("previousPath".into(), json!(change.previous_path));
    }
    if !change.file_id.is_empty() {
        o.insert("fileId".into(), json!(change.file_id));
    }
    if !change.version_id.is_empty()
        && let Some(next) = ctx.db.versions().id().find(change.version_id.clone())
    {
        o.insert("versionId".into(), json!(next.id));
        o.insert("size".into(), json!(next.size));
        o.insert("mediaType".into(), json!(next.media_type));
    }
    if !change.previous_version_id.is_empty()
        && let Some(old) = ctx
            .db
            .versions()
            .id()
            .find(change.previous_version_id.clone())
    {
        o.insert("previousVersionId".into(), json!(old.id));
        o.insert("previousSize".into(), json!(old.size));
        o.insert("previousMediaType".into(), json!(old.media_type));
    }
    v
}

fn by_path(a: &Value, b: &Value) -> std::cmp::Ordering {
    a["path"]
        .as_str()
        .unwrap_or_default()
        .cmp(b["path"].as_str().unwrap_or_default())
}

pub fn author(ctx: &ReducerContext, user_id: &str) -> String {
    match ctx.db.accounts().id().find(user_id.to_owned()) {
        Some(a) if !a.display_name.is_empty() => a.display_name,
        Some(a) => a.username,
        None => "Removed account".into(),
    }
}

/// Commits Git has seen show their Git ID and author.
pub fn changeset_view(ctx: &ReducerContext, r: &FileChangeset) -> Value {
    let git = ctx
        .db
        .file_git_objects()
        .changeset_id()
        .filter(r.id.as_str())
        .next();
    let author = git
        .as_ref()
        .map(|g| g.author.clone())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| author(ctx, &r.author_id));
    let mut v = json!({ "id": r.id, "sequence": r.sequence, "kind": r.kind, "message": r.message, "author": author, "createdAt": r.created_at, "fileCount": r.file_count });
    if let Some(g) = git {
        v["sha"] = json!(g.sha);
    }
    v
}

fn changes_of(ctx: &ReducerContext, changeset_id: &str) -> Vec<Value> {
    let mut list: Vec<Value> = ctx
        .db
        .file_changes()
        .changeset_id()
        .filter(changeset_id)
        .map(|c| change_view(ctx, &c))
        .collect();
    list.sort_by(by_path);
    list
}

fn branch_view(ctx: &ReducerContext, branch: &FileBranch) -> Value {
    let set = ctx
        .db
        .file_changesets()
        .id()
        .find(branch.changeset_id.clone());
    json!({ "name": branch.name, "threadId": branch.thread_id, "message": set.as_ref().map(|s| s.message.clone()).unwrap_or_default(), "author": author(ctx, &branch.author_id),
        "createdAt": branch.created_at, "modifiedAt": branch.modified_at, "changes": set.map(|s| changes_of(ctx, &s.id)).unwrap_or_default() })
}

/// The version holding a file's current content; files without one get one.
fn current_version(ctx: &ReducerContext, files: &HistoryFiles, entry: &FileEntry) -> Res<Version> {
    let mut best: Option<Version> = None;
    for v in ctx.db.versions().file_id().filter(entry.id.as_str()) {
        if v.blob_id == entry.blob_id && best.as_ref().is_none_or(|b| v.created_at > b.created_at) {
            best = Some(v);
        }
    }
    if let Some(b) = best {
        return Ok(b);
    }
    charge(ctx, files, &entry.owner_id, entry.size)?;
    Ok(ctx.db.versions().insert(Version {
        id: files.uuid(ctx),
        file_id: entry.id.clone(),
        blob_id: entry.blob_id.clone(),
        size: entry.size,
        media_type: entry.media_type.clone(),
        created_at: now(ctx),
    }))
}

/// Appends a changeset to main and moves the head tree. A file keeps its Git mode through edits
/// and renames; `modes` sets them explicitly, as restore does.
#[allow(clippy::too_many_arguments)]
pub fn record(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    repo: &FileRepo,
    user_id: &str,
    items: &[Pending],
    kind: &str,
    message: &str,
    modes: Option<&HashMap<String, String>>,
) -> Res<Value> {
    if items.is_empty() {
        return fail("nothingToCommit");
    }
    if items.len() > CHANGES || repo.sequence >= CHANGESETS {
        return fail("operationTooLarge");
    }
    let changeset_id = fresh(ctx, files);
    let sequence = repo.sequence + 1;
    ctx.db.file_changesets().insert(FileChangeset {
        id: changeset_id.clone(),
        repo_id: repo.id.clone(),
        branch: String::new(),
        parent_id: repo.head_id.clone(),
        sequence,
        kind: kind.into(),
        message: message.into(),
        author_id: user_id.into(),
        file_count: items.len() as u32,
        created_at: now(ctx),
    });
    for (index, item) in items.iter().enumerate() {
        let version = match &item.entry {
            Some(e) => Some(current_version(ctx, files, e)?),
            None => None,
        };
        let previous_mode = item
            .previous
            .as_ref()
            .map(|p| p.mode.clone())
            .unwrap_or_default();
        let mode = if item.kind == "deleted" {
            String::new()
        } else {
            modes
                .and_then(|m| m.get(&item.path).cloned())
                .unwrap_or_else(|| previous_mode.clone())
        };
        ctx.db.file_changes().insert(FileChange {
            id: format!("{changeset_id}/{index}"),
            changeset_id: changeset_id.clone(),
            path: item.path.clone(),
            previous_path: item.previous_path.clone(),
            kind: item.kind.clone(),
            file_id: item
                .entry
                .as_ref()
                .map(|e| e.id.clone())
                .or_else(|| item.previous.as_ref().map(|p| p.file_id.clone()))
                .unwrap_or_default(),
            version_id: version.as_ref().map(|v| v.id.clone()).unwrap_or_default(),
            previous_version_id: item
                .previous
                .as_ref()
                .map(|p| p.version_id.clone())
                .unwrap_or_default(),
            expect_revision: 0,
            mode: mode.clone(),
            previous_mode,
        });
        if item.kind == "deleted" {
            ctx.db
                .file_trees()
                .id()
                .delete(format!("{}/{}", repo.id, item.path));
        }
        if item.kind == "renamed" {
            ctx.db
                .file_trees()
                .id()
                .delete(format!("{}/{}", repo.id, item.previous_path));
        }
        if let Some(version) = version {
            let next = FileTree {
                id: format!("{}/{}", repo.id, item.path),
                repo_id: repo.id.clone(),
                path: item.path.clone(),
                file_id: item.entry.as_ref().expect("entry").id.clone(),
                version_id: version.id,
                mode,
            };
            if ctx.db.file_trees().id().find(next.id.clone()).is_some() {
                ctx.db.file_trees().id().update(next);
            } else {
                ctx.db.file_trees().insert(next);
            }
        }
    }
    let current = ctx
        .db
        .file_repos()
        .id()
        .find(repo.id.clone())
        .unwrap_or_else(|| repo.clone());
    ctx.db.file_repos().id().update(FileRepo {
        head_id: changeset_id.clone(),
        sequence,
        revision: current.revision + 1,
        modified_at: now(ctx),
        ..current
    });
    Ok(json!({ "id": changeset_id, "sequence": sequence }))
}

/// Main's tree at a changeset: the head tree with every later changeset undone, newest first.
pub fn tree_at(
    ctx: &ReducerContext,
    repo: &FileRepo,
    target: &FileChangeset,
) -> BTreeMap<String, TreeItem> {
    let mut result = head_tree(ctx, &repo.id);
    let mut later: Vec<FileChangeset> = ctx
        .db
        .file_changesets()
        .repo_id()
        .filter(repo.id.as_str())
        .filter(|r| r.branch.is_empty() && r.sequence > target.sequence)
        .collect();
    later.sort_by(|a, b| b.sequence.cmp(&a.sequence));
    for set in later {
        let changes: Vec<FileChange> = ctx
            .db
            .file_changes()
            .changeset_id()
            .filter(set.id.as_str())
            .collect();
        // Removals first: a path can be renamed away and another file added there in one changeset.
        for c in &changes {
            if c.kind == "added" || c.kind == "renamed" {
                result.remove(&c.path);
            }
        }
        for c in &changes {
            if c.kind != "added" {
                let path = if c.kind == "renamed" {
                    c.previous_path.clone()
                } else {
                    c.path.clone()
                };
                result.insert(
                    path,
                    TreeItem {
                        file_id: c.file_id.clone(),
                        version_id: c.previous_version_id.clone(),
                        mode: c.previous_mode.clone(),
                    },
                );
            }
        }
    }
    result
}

pub fn trash(ctx: &ReducerContext, files: &HistoryFiles, entry: &FileEntry) {
    let list_key = format!(
        "{}/trash/{}/{}",
        entry.owner_id,
        entry.name.to_lowercase(),
        entry.id
    );
    ctx.db.file_entries().id().update(FileEntry {
        trashed: true,
        list_key,
        revision: entry.revision + 1,
        modified_at: now(ctx),
        ..entry.clone()
    });
    files.event(ctx, &entry.owner_id, &entry.id, "trashed", None);
}

/// Gives a file a version's content. A version from another file is copied and counted;
/// `adopt` hands over a staged branch version that is already counted.
pub fn set_content(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    entry: &FileEntry,
    version: &Version,
    adopt: bool,
) -> Res<()> {
    if adopt {
        ctx.db.versions().id().update(Version {
            file_id: entry.id.clone(),
            ..version.clone()
        });
    } else if version.file_id != entry.id {
        charge(ctx, files, &entry.owner_id, version.size)?;
        ctx.db.versions().insert(Version {
            id: files.uuid(ctx),
            file_id: entry.id.clone(),
            created_at: now(ctx),
            ..version.clone()
        });
    }
    let current = ctx
        .db
        .file_entries()
        .id()
        .find(entry.id.clone())
        .unwrap_or_else(|| entry.clone());
    ctx.db.file_entries().id().update(FileEntry {
        blob_id: version.blob_id.clone(),
        size: version.size,
        media_type: version.media_type.clone(),
        revision: current.revision + 1,
        modified_at: now(ctx),
        ..current
    });
    files.event(ctx, &entry.owner_id, &entry.id, "updated", None);
    Ok(())
}

/// The folder at a path under the tracked folder, created with any missing parents.
fn folder_for(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    root: &FileEntry,
    parts: &[&str],
) -> Res<String> {
    let mut parent_id = root.id.clone();
    for part in parts {
        if let Some(existing) =
            crate::files::find_key(ctx, &crate::files::key(&root.owner_id, &parent_id, part))
        {
            if existing.kind != "folder" {
                return fail("conflict");
            }
            parent_id = existing.id;
            continue;
        }
        let folder_id = files.uuid(ctx);
        ctx.db.file_entries().insert(FileEntry {
            id: folder_id.clone(),
            owner_id: root.owner_id.clone(),
            parent_id: parent_id.clone(),
            name: (*part).into(),
            kind: "folder".into(),
            blob_id: String::new(),
            size: 0.0,
            media_type: "inode/directory".into(),
            revision: 1,
            modified_at: now(ctx),
            trashed: false,
            favorite: false,
            list_key: crate::files::key(&root.owner_id, &parent_id, part),
        });
        files.event(ctx, &root.owner_id, &folder_id, "created", None);
        parent_id = folder_id;
    }
    Ok(parent_id)
}

/// Moves a file to a path under the tracked folder.
pub fn move_file(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    root: &FileEntry,
    entry: &FileEntry,
    path: &str,
) -> Res<()> {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    let parent_id = folder_for(ctx, files, root, &parts)?;
    if crate::files::find_key(ctx, &crate::files::key(&root.owner_id, &parent_id, name)).is_some() {
        return fail("conflict");
    }
    files.move_entry(ctx, entry, &parent_id, name);
    Ok(())
}

/// Creates a file at a path under the tracked folder, with any missing folders.
pub fn create_file(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    root: &FileEntry,
    path: &str,
    version: &Version,
    adopt: bool,
) -> Res<String> {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    let parent_id = folder_for(ctx, files, root, &parts)?;
    let list_key = crate::files::key(&root.owner_id, &parent_id, name);
    if crate::files::find_key(ctx, &list_key).is_some() {
        return fail("conflict");
    }
    // A file deleted for good comes back with its own ID, and so with its kept versions.
    let own = !adopt
        && crate::opaque_id(&version.file_id).is_ok()
        && ctx
            .db
            .file_entries()
            .id()
            .find(version.file_id.clone())
            .is_none();
    let file_id = if own {
        version.file_id.clone()
    } else {
        files.uuid(ctx)
    };
    ctx.db.file_entries().insert(FileEntry {
        id: file_id.clone(),
        owner_id: root.owner_id.clone(),
        parent_id,
        name: name.into(),
        kind: "file".into(),
        blob_id: version.blob_id.clone(),
        size: version.size,
        media_type: version.media_type.clone(),
        revision: 1,
        modified_at: now(ctx),
        trashed: false,
        favorite: false,
        list_key,
    });
    if adopt {
        ctx.db.versions().id().update(Version {
            file_id: file_id.clone(),
            ..version.clone()
        });
    } else if !own {
        charge(ctx, files, &root.owner_id, version.size)?;
        ctx.db.versions().insert(Version {
            id: files.uuid(ctx),
            file_id: file_id.clone(),
            created_at: now(ctx),
            ..version.clone()
        });
    }
    files.event(ctx, &root.owner_id, &file_id, "created", None);
    Ok(file_id)
}

/// Drops a branch and the staged versions nothing else uses.
fn drop_branch(ctx: &ReducerContext, repo: &FileRepo, branch: &FileBranch) {
    if let Some(set) = ctx
        .db
        .file_changesets()
        .id()
        .find(branch.changeset_id.clone())
    {
        for c in ctx
            .db
            .file_changes()
            .changeset_id()
            .filter(set.id.as_str())
            .collect::<Vec<_>>()
        {
            ctx.db.file_changes().id().delete(c.id);
        }
        ctx.db.file_changesets().id().delete(set.id);
    }
    let mut bytes = 0.0;
    for v in ctx
        .db
        .versions()
        .file_id()
        .filter(format!("staged:{}", branch.id).as_str())
        .collect::<Vec<_>>()
    {
        if !referenced(ctx, &v.id) {
            bytes += v.size;
            ctx.db.versions().id().delete(v.id.clone());
            release_blob(ctx, &v.blob_id);
        }
    }
    release(ctx, &repo.owner_id, bytes);
    ctx.db.file_branches().id().delete(branch.id.clone());
}

/// Deletes a folder's history. Versions of live files stay; versions only history kept are released.
pub fn drop_repo(ctx: &ReducerContext, repo: &FileRepo) -> Res<()> {
    crate::tracker::drop_tracker(ctx, &repo.id);
    for branch in ctx
        .db
        .file_branches()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        drop_branch(ctx, repo, &branch);
    }
    let mut version_ids: HashSet<String> = HashSet::new();
    let mut rows = 0;
    for set in ctx
        .db
        .file_changesets()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        for c in ctx
            .db
            .file_changes()
            .changeset_id()
            .filter(set.id.as_str())
            .collect::<Vec<_>>()
        {
            rows += 1;
            if rows > 250_000 {
                return fail("operationTooLarge");
            }
            if !c.version_id.is_empty() {
                version_ids.insert(c.version_id.clone());
            }
            ctx.db.file_changes().id().delete(c.id);
        }
        ctx.db.file_changesets().id().delete(set.id);
    }
    for r in ctx
        .db
        .file_trees()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        ctx.db.file_trees().id().delete(r.id);
    }
    for r in ctx
        .db
        .file_git_objects()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        ctx.db.file_git_objects().id().delete(r.id);
    }
    for r in ctx
        .db
        .file_git_refs()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        ctx.db.file_git_refs().id().delete(r.id);
    }
    for r in ctx
        .db
        .file_git_blobs()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        ctx.db.file_git_blobs().id().delete(r.id);
    }
    let mut bytes = 0.0;
    for version_id in version_ids {
        let Some(v) = ctx.db.versions().id().find(version_id.clone()) else {
            continue;
        };
        if referenced(ctx, &version_id)
            || ctx.db.file_entries().id().find(v.file_id.clone()).is_some()
        {
            continue;
        }
        bytes += v.size;
        ctx.db.versions().id().delete(version_id);
        release_blob(ctx, &v.blob_id);
    }
    release(ctx, &repo.owner_id, bytes);
    ctx.db.file_repos().id().delete(repo.id.clone());
    Ok(())
}

/// Versions history keeps when their file is deleted for good.
pub fn history_keeps(ctx: &ReducerContext, version_id: &str) -> bool {
    referenced(ctx, version_id)
}

fn agent_thread(
    ctx: &ReducerContext,
    user_id: &str,
    value: Option<&Value>,
    folder_id: &str,
) -> Res<AgentThread> {
    let thread = ctx.db.agent_threads().id().find(id(value)?);
    let project = thread
        .as_ref()
        .and_then(|t| ctx.db.agent_projects().id().find(t.project_id.clone()));
    match (thread, project) {
        (Some(t), Some(p))
            if t.owner_id == user_id && p.source_type == "files" && p.source_ref == folder_id =>
        {
            Ok(t)
        }
        _ => fail("notFound"),
    }
}

fn safe_path(value: Option<&Value>) -> Res<String> {
    let Some(v) = value
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.encode_utf16().count() <= 2048)
    else {
        return fail("invalidInput");
    };
    let parts: Vec<&str> = v.split('/').collect();
    if parts.len() > DEPTH
        || parts
            .iter()
            .any(|p| crate::name_of(p).ok().as_deref() != Some(*p))
    {
        return fail("invalidInput");
    }
    Ok(v.to_owned())
}

fn media_type(value: Option<&Value>) -> Res<String> {
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match value.and_then(Value::as_str) {
        Some(v)
            if v.len() <= 150 && v.split_once('/').is_some_and(|(a, b)| token(a) && token(b)) =>
        {
            Ok(v.to_lowercase())
        }
        _ => fail("invalidInput"),
    }
}

/// Every check runs in the caller's transaction; `upload_key` ties staged blobs to the browser session.
pub fn history_operation(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    user_id: &str,
    upload_key: &str,
    operation: &str,
    p: &Doc,
) -> Res<Value> {
    let write = !READ_OPERATIONS.contains(&operation);
    let (root, role) = folder_of(ctx, files, p.get("folderId"), write)?;
    let repo = ctx.db.file_repos().id().find(root.id.clone());
    if operation == "track" || operation == "overview" {
        let blocked = if files.in_locked(ctx, &root) {
            Some("locked")
        } else if tracked_ancestor(ctx, &root) {
            Some("nested")
        } else if ctx
            .db
            .file_repos()
            .owner_id()
            .filter(root.owner_id.as_str())
            .count()
            >= REPOS
        {
            Some("limit")
        } else {
            None
        };
        if operation == "track" {
            keys(p, &["folderId"])?;
            if role != Role::Owner {
                return fail("forbidden");
            }
            if let Some(repo) = repo {
                return Ok(json!({ "revision": repo.revision }));
            }
            match blocked {
                Some("limit") => return fail("quotaExceeded"),
                Some(_) => return fail("forbidden"),
                None => {}
            }
            ctx.db.file_repos().insert(FileRepo {
                id: root.id.clone(),
                owner_id: root.owner_id.clone(),
                head_id: String::new(),
                sequence: 0,
                revision: 1,
                created_at: now(ctx),
                modified_at: now(ctx),
            });
            return Ok(json!({ "revision": 1 }));
        }
        keys(p, &["folderId", "before"])?;
        let mut base = json!({ "folder": { "id": root.id, "name": root.name }, "tracked": repo.is_some(), "canWrite": role != Role::Read, "owner": role == Role::Owner });
        let Some(repo) = repo else {
            if let Some(b) = blocked {
                base["blocked"] = json!(b);
            }
            for (k, v) in [
                ("branches", json!([])),
                ("gitRefs", json!([])),
                ("changesets", json!([])),
                ("nextBefore", Value::Null),
            ] {
                base[k] = v;
            }
            return Ok(base);
        };
        let before = match p.get("before") {
            None => f64::INFINITY,
            Some(v) => match v.as_f64() {
                Some(f) if f.fract() == 0.0 && f > 0.0 => f,
                _ => return fail("invalidInput"),
            },
        };
        let current = status(ctx, &repo, &root)?;
        let mut rows: Vec<FileChangeset> = ctx
            .db
            .file_changesets()
            .repo_id()
            .filter(repo.id.as_str())
            .filter(|r| r.branch.is_empty() && (r.sequence as f64) < before)
            .collect();
        rows.sort_by(|a, b| b.sequence.cmp(&a.sequence));
        let page: Vec<&FileChangeset> = rows.iter().take(PAGE).collect();
        let mut branches: Vec<Value> = ctx
            .db
            .file_branches()
            .repo_id()
            .filter(repo.id.as_str())
            .map(|b| branch_view(ctx, &b))
            .collect();
        branches.sort_by(|a, b| {
            b["modifiedAt"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&a["modifiedAt"].as_f64().unwrap_or(0.0))
        });
        let mut refs: Vec<(String, String)> = ctx
            .db
            .file_git_refs()
            .repo_id()
            .filter(repo.id.as_str())
            .map(|r| (r.name, r.target))
            .collect();
        refs.sort();
        base["repo"] = json!({ "revision": repo.revision, "headId": repo.head_id, "sequence": repo.sequence, "createdAt": repo.created_at });
        base["status"] = json!({ "changes": current.changes.iter().map(|c| pending_view(ctx, c)).collect::<Vec<_>>(), "token": current.token });
        base["branches"] = json!(branches);
        base["gitRefs"] = json!(
            refs.iter()
                .map(|(name, sha)| json!({ "name": name, "sha": sha }))
                .collect::<Vec<_>>()
        );
        base["changesets"] = json!(
            page.iter()
                .map(|r| changeset_view(ctx, r))
                .collect::<Vec<_>>()
        );
        base["nextBefore"] = if rows.len() > page.len() {
            json!(page.last().map(|r| r.sequence))
        } else {
            Value::Null
        };
        return Ok(base);
    }
    let Some(repo) = repo else {
        return fail("notFound");
    };
    match operation {
        "untrack" => {
            keys(p, &["folderId", "revision"])?;
            if role != Role::Owner {
                return fail("forbidden");
            }
            revision(&repo, p.get("revision"))?;
            drop_repo(ctx, &repo)?;
            Ok(json!({ "tracked": false }))
        }
        "commit" => {
            keys(p, &["folderId", "message", "token", "paths", "snapshot"])?;
            let current = status(ctx, &repo, &root)?;
            if p.get("token").and_then(Value::as_str) != Some(current.token.as_str()) {
                return fail("conflict");
            }
            let snapshot = p.get("snapshot") == Some(&json!(true));
            let message = text(p.get("message"), !snapshot)?;
            let message = if message.is_empty() {
                "Snapshot".to_owned()
            } else {
                message
            };
            let mut items = current.changes;
            if let Some(paths) = p.get("paths") {
                let Some(list) = paths
                    .as_array()
                    .filter(|l| l.len() <= CHANGES && l.iter().all(Value::is_string))
                else {
                    return fail("invalidInput");
                };
                let chosen: HashSet<&str> = list.iter().filter_map(Value::as_str).collect();
                items.retain(|i| chosen.contains(i.path.as_str()));
                if items.len() != chosen.len() {
                    return fail("conflict");
                }
            }
            record(
                ctx,
                files,
                &repo,
                user_id,
                &items,
                if snapshot { "snapshot" } else { "commit" },
                &message,
                None,
            )
        }
        "changeset" => {
            keys(p, &["folderId", "id"])?;
            let Some(r) = ctx
                .db
                .file_changesets()
                .id()
                .find(id(p.get("id"))?)
                .filter(|r| r.repo_id == repo.id && r.branch.is_empty())
            else {
                return fail("notFound");
            };
            Ok(json!({ "changeset": changeset_view(ctx, &r), "changes": changes_of(ctx, &r.id) }))
        }
        "content" => {
            // Any version this folder's history or branches refer to; live content uses the file's own route.
            keys(p, &["folderId", "versionId"])?;
            let version_id = id(p.get("versionId"))?;
            let Some(version) = ctx.db.versions().id().find(version_id.clone()) else {
                return fail("notFound");
            };
            let mut path = String::new();
            for c in ctx
                .db
                .file_changes()
                .version_id()
                .filter(version_id.as_str())
            {
                if ctx
                    .db
                    .file_changesets()
                    .id()
                    .find(c.changeset_id.clone())
                    .is_some_and(|s| s.repo_id == repo.id)
                {
                    path = c.path;
                    break;
                }
            }
            // A branch change's old side is a version of a file in this folder.
            if path.is_empty() {
                for (live_path, entry) in scan(ctx, &root)? {
                    if entry.id == version.file_id {
                        path = live_path;
                        break;
                    }
                }
            }
            if path.is_empty() {
                return fail("notFound");
            }
            Ok(
                json!({ "id": version.id, "name": path.rsplit('/').next().unwrap_or_default(), "blobId": version.blob_id, "size": version.size, "mediaType": version.media_type, "revision": 0 }),
            )
        }
        "restore" => {
            keys(p, &["folderId", "id", "revision"])?;
            revision(&repo, p.get("revision"))?;
            let Some(target) = ctx
                .db
                .file_changesets()
                .id()
                .find(id(p.get("id"))?)
                .filter(|r| r.repo_id == repo.id && r.branch.is_empty())
            else {
                return fail("notFound");
            };
            let current = status(ctx, &repo, &root)?;
            if !current.changes.is_empty() {
                return fail("uncommittedChanges");
            }
            let want = tree_at(ctx, &repo, &target);
            for (path, entry) in &current.live {
                if !want.contains_key(path) {
                    trash(ctx, files, entry);
                }
            }
            for (path, item) in &want {
                let Some(version) = ctx.db.versions().id().find(item.version_id.clone()) else {
                    return fail("contentUnavailable");
                };
                match current.live.get(path) {
                    None => {
                        create_file(ctx, files, &root, path, &version, false)?;
                    }
                    Some(entry) if entry.blob_id != version.blob_id => {
                        set_content(ctx, files, entry, &version, false)?
                    }
                    Some(_) => {}
                }
            }
            let message: String = if target.message.chars().count() > 120 {
                format!("{}…", target.message.chars().take(119).collect::<String>())
            } else {
                target.message.clone()
            };
            let after = status(ctx, &repo, &root)?;
            let mut items = after.changes;
            let head = head_tree(ctx, &repo.id);
            // Same content with another Git mode (an executable bit) is a change too.
            for (path, item) in &want {
                if let (Some(old), Some(entry)) = (head.get(path), after.live.get(path))
                    && old.mode != item.mode
                    && !items.iter().any(|c| &c.path == path)
                {
                    items.push(Pending {
                        path: path.clone(),
                        previous_path: String::new(),
                        kind: "modified".into(),
                        entry: Some(entry.clone()),
                        previous: Some(old.clone()),
                    });
                }
            }
            items.sort_by(|a, b| a.path.cmp(&b.path));
            let modes: HashMap<String, String> = want
                .iter()
                .map(|(p, i)| (p.clone(), i.mode.clone()))
                .collect();
            let current_repo = ctx
                .db
                .file_repos()
                .id()
                .find(repo.id.clone())
                .expect("repo");
            record(
                ctx,
                files,
                &current_repo,
                user_id,
                &items,
                "restore",
                &format!("Restore to “{message}”"),
                Some(&modes),
            )
        }
        "branch" => {
            keys(p, &["folderId", "threadId"])?;
            let thread = agent_thread(ctx, user_id, p.get("threadId"), &root.id)?;
            let branch = ctx
                .db
                .file_branches()
                .repo_id()
                .filter(repo.id.as_str())
                .find(|b| b.thread_id == thread.id);
            Ok(json!({ "revision": repo.revision, "branch": branch.map(|b| branch_view(ctx, &b)) }))
        }
        "merge" | "discard" => {
            keys(p, &["folderId", "name", "revision"])?;
            revision(&repo, p.get("revision"))?;
            let name = crate::text_of(p.get("name"));
            let Some(branch) = ctx
                .db
                .file_branches()
                .repo_id()
                .filter(repo.id.as_str())
                .find(|b| b.name == name)
            else {
                return fail("notFound");
            };
            if operation == "discard" {
                crate::tracker::agent_branch_done(
                    ctx,
                    files,
                    &repo.id,
                    &branch.name,
                    user_id,
                    false,
                )?;
                drop_branch(ctx, &repo, &branch);
                let current = ctx
                    .db
                    .file_repos()
                    .id()
                    .find(repo.id.clone())
                    .expect("repo");
                ctx.db.file_repos().id().update(FileRepo {
                    revision: current.revision + 1,
                    modified_at: now(ctx),
                    ..current
                });
                return Ok(json!({ "merged": false }));
            }
            let set = ctx
                .db
                .file_changesets()
                .id()
                .find(branch.changeset_id.clone())
                .expect("changeset");
            let changes: Vec<FileChange> = ctx
                .db
                .file_changes()
                .changeset_id()
                .filter(set.id.as_str())
                .collect();
            let live = scan(ctx, &root)?;
            // Each file must still be as the agent found it; nothing is applied otherwise.
            let conflicts: Vec<String> = changes
                .iter()
                .filter(|c| {
                    let entry = live.get(&c.path);
                    if c.kind == "added" {
                        entry.is_some()
                    } else {
                        entry.is_none_or(|e| e.id != c.file_id || e.revision != c.expect_revision)
                    }
                })
                .map(|c| c.path.clone())
                .collect();
            if !conflicts.is_empty() {
                return Ok(json!({ "merged": false, "conflicts": conflicts }));
            }
            for c in &changes {
                let entry = live.get(&c.path);
                let version = if c.version_id.is_empty() {
                    None
                } else {
                    ctx.db.versions().id().find(c.version_id.clone())
                };
                if c.kind == "deleted" {
                    trash(ctx, files, entry.expect("entry"));
                } else if let Some(version) = version {
                    match entry {
                        Some(e) => set_content(ctx, files, e, &version, true)?,
                        None => {
                            create_file(ctx, files, &root, &c.path, &version, true)?;
                        }
                    }
                } else {
                    return fail("contentUnavailable");
                }
            }
            let paths: HashSet<&String> = changes.iter().map(|c| &c.path).collect();
            let items: Vec<Pending> = status(ctx, &repo, &root)?
                .changes
                .into_iter()
                .filter(|i| paths.contains(&i.path) || paths.contains(&i.previous_path))
                .collect();
            // The changes now belong to main; the branch's staged versions became the files' own.
            for c in &changes {
                ctx.db.file_changes().id().delete(c.id.clone());
            }
            ctx.db.file_changesets().id().delete(set.id.clone());
            ctx.db.file_branches().id().delete(branch.id.clone());
            crate::tracker::agent_branch_done(ctx, files, &repo.id, &branch.name, user_id, true)?;
            let current = ctx
                .db
                .file_repos()
                .id()
                .find(repo.id.clone())
                .expect("repo");
            if items.is_empty() {
                ctx.db.file_repos().id().update(FileRepo {
                    revision: current.revision + 1,
                    modified_at: now(ctx),
                    ..current
                });
                return Ok(json!({ "merged": true }));
            }
            let message: String = format!("Merge {}: {}", branch.name, set.message)
                .encode_utf16()
                .take(MESSAGE_BYTES / 2)
                .collect::<Vec<u16>>()
                .into_iter()
                .map(|u| char::from_u32(u as u32).unwrap_or('?'))
                .collect();
            let mut result = record(
                ctx, files, &current, user_id, &items, "merge", &message, None,
            )?;
            result["merged"] = json!(true);
            Ok(result)
        }
        "stageBegin" => {
            // Reserves blobs for an Agent branch; maintenance releases what is never staged.
            keys(p, &["folderId", "threadId", "files"])?;
            agent_thread(ctx, user_id, p.get("threadId"), &root.id)?;
            let Some(list) = p
                .get("files")
                .and_then(Value::as_array)
                .filter(|l| l.len() <= CHANGES)
            else {
                return fail("invalidInput");
            };
            let mut rows = Vec::new();
            for v in list {
                let size = v
                    .get("size")
                    .and_then(Value::as_f64)
                    .filter(|s| s.fract() == 0.0 && *s >= 0.0 && *s <= 16.0 * 1024.0 * 1024.0)
                    .ok_or_else(|| "invalidInput".to_owned())?;
                rows.push((size, media_type(v.get("mediaType"))?));
            }
            let meter = match ctx.db.usage().user_id().find(root.owner_id.clone()) {
                Some(m) => m,
                None => ctx.db.usage().insert(Usage {
                    user_id: root.owner_id.clone(),
                    used: 0.0,
                    reserved: 0.0,
                    quota: 0.0,
                }),
            };
            let total: f64 = rows.iter().map(|r| r.0).sum();
            if !files.fits_quota(&meter, total) {
                return fail("quotaExceeded");
            }
            let reserved = meter.reserved + total;
            ctx.db.usage().user_id().update(Usage { reserved, ..meter });
            let expires_at = now(ctx) + 3_600_000.0;
            let uploads: Vec<String> = rows
                .into_iter()
                .map(|(size, media_type)| {
                    let upload_id = files.uuid(ctx);
                    ctx.db.uploads().insert(Upload {
                        id: upload_id.clone(),
                        session_id: upload_key.into(),
                        owner_id: root.owner_id.clone(),
                        parent_id: String::new(),
                        file_id: String::new(),
                        name: String::new(),
                        size,
                        media_type,
                        expected_revision: 0,
                        expires_at,
                        state: "uploading".into(),
                        digest: String::new(),
                        expires_key: format!("{:016}", expires_at as i64),
                    });
                    upload_id
                })
                .collect();
            Ok(json!({ "uploads": uploads }))
        }
        "stage" => {
            keys(p, &["folderId", "threadId", "title", "files"])?;
            let thread = agent_thread(ctx, user_id, p.get("threadId"), &root.id)?;
            let name = format!("agent/{}", thread.id);
            let title = text(p.get("title"), true)?;
            let Some(list) = p
                .get("files")
                .and_then(Value::as_array)
                .filter(|l| l.len() <= CHANGES)
            else {
                return fail("invalidInput");
            };
            let mut branch = ctx
                .db
                .file_branches()
                .repo_id()
                .filter(repo.id.as_str())
                .find(|b| b.thread_id == thread.id);
            if let Some(b) = &branch {
                drop_branch(ctx, &repo, b);
            }
            let branch_id = branch
                .as_ref()
                .map(|b| b.id.clone())
                .unwrap_or_else(|| fresh(ctx, files));
            let changeset_id = fresh(ctx, files);
            let mut seen: HashSet<String> = HashSet::new();
            let mut changes: Vec<FileChange> = Vec::new();
            for (index, r) in list.iter().enumerate() {
                let Some(r) = r.as_object() else {
                    return fail("invalidInput");
                };
                let path = safe_path(r.get("path"))?;
                if !seen.insert(path.to_lowercase()) {
                    return fail("invalidInput");
                }
                let kind = r
                    .get("kind")
                    .and_then(Value::as_str)
                    .filter(|k| ["added", "modified", "deleted"].contains(k))
                    .ok_or_else(|| "invalidInput".to_owned())?;
                let (mut file_id, mut expect_revision, mut previous_version_id) =
                    (String::new(), 0u32, String::new());
                if kind != "added" {
                    let entry = ctx.db.file_entries().id().find(id(r.get("fileId"))?);
                    let expect = r
                        .get("expectRevision")
                        .and_then(Value::as_f64)
                        .filter(|e| e.fract() == 0.0 && *e >= 1.0);
                    let (Some(entry), Some(expect)) = (
                        entry.filter(|e| e.kind == "file" && e.owner_id == root.owner_id),
                        expect,
                    ) else {
                        return fail("invalidInput");
                    };
                    file_id = entry.id.clone();
                    expect_revision = expect as u32;
                    for v in ctx.db.versions().file_id().filter(entry.id.as_str()) {
                        if v.blob_id == entry.blob_id {
                            previous_version_id = v.id;
                        }
                    }
                }
                let mut version_id = String::new();
                if kind != "deleted" {
                    let upload = ctx.db.uploads().id().find(id(r.get("uploadId"))?);
                    let digest = r
                        .get("digest")
                        .and_then(Value::as_str)
                        .filter(|d| crate::blobs::is_blob_id(d));
                    let (Some(upload), Some(digest)) = (
                        upload.filter(|u| {
                            u.session_id == upload_key
                                && u.state == "uploading"
                                && u.owner_id == root.owner_id
                        }),
                        digest,
                    ) else {
                        return fail("notFound");
                    };
                    ctx.db.uploads().id().update(Upload {
                        state: "complete".into(),
                        digest: digest.into(),
                        ..upload.clone()
                    });
                    let meter = ctx
                        .db
                        .usage()
                        .user_id()
                        .find(upload.owner_id.clone())
                        .expect("meter");
                    let (reserved, used) = (
                        (meter.reserved - upload.size).max(0.0),
                        meter.used + upload.size,
                    );
                    ctx.db.usage().user_id().update(Usage {
                        reserved,
                        used,
                        ..meter
                    });
                    store_blob(ctx, digest, upload.size)?;
                    ctx.db.versions().insert(Version {
                        id: upload.id.clone(),
                        file_id: format!("staged:{branch_id}"),
                        blob_id: digest.into(),
                        size: upload.size,
                        media_type: upload.media_type.clone(),
                        created_at: now(ctx),
                    });
                    version_id = upload.id;
                }
                changes.push(FileChange {
                    id: format!("{changeset_id}/{index}"),
                    changeset_id: changeset_id.clone(),
                    path,
                    previous_path: String::new(),
                    kind: kind.into(),
                    file_id,
                    version_id,
                    previous_version_id,
                    expect_revision,
                    mode: String::new(),
                    previous_mode: String::new(),
                });
            }
            if changes.is_empty() {
                return Ok(json!({ "revision": repo.revision, "branch": null }));
            }
            ctx.db.file_changesets().insert(FileChangeset {
                id: changeset_id.clone(),
                repo_id: repo.id.clone(),
                branch: name.clone(),
                parent_id: repo.head_id.clone(),
                sequence: 0,
                kind: "branch".into(),
                message: title,
                author_id: user_id.into(),
                file_count: changes.len() as u32,
                created_at: now(ctx),
            });
            for c in changes {
                ctx.db.file_changes().insert(c);
            }
            let created = FileBranch {
                id: branch_id,
                repo_id: repo.id.clone(),
                name,
                thread_id: thread.id.clone(),
                changeset_id,
                author_id: user_id.into(),
                created_at: branch.take().map_or(now(ctx), |b| b.created_at),
                modified_at: now(ctx),
            };
            ctx.db.file_branches().insert(created.clone());
            let current = ctx
                .db
                .file_repos()
                .id()
                .find(repo.id.clone())
                .expect("repo");
            ctx.db.file_repos().id().update(FileRepo {
                revision: current.revision + 1,
                modified_at: now(ctx),
                ..current.clone()
            });
            Ok(json!({ "revision": current.revision + 1, "branch": branch_view(ctx, &created) }))
        }
        _ => fail("invalidInput"),
    }
}

/// Backup rows; restore validates references after Files and versions are back.
pub fn history_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "repos": ctx.db.file_repos().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "changesets": ctx.db.file_changesets().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "changes": ctx.db.file_changes().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "trees": ctx.db.file_trees().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "branches": ctx.db.file_branches().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "gitObjects": ctx.db.file_git_objects().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "gitRefs": ctx.db.file_git_refs().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "gitBlobs": ctx.db.file_git_blobs().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

/// The owner of each version only history keeps, so restore can count it against the right quota.
pub fn history_version_owners(value: Option<&Value>) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    let Some(value) = value else { return owners };
    let repo_owner: HashMap<&str, &str> = value["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((r["id"].as_str()?, r["ownerId"].as_str()?)))
        .collect();
    let set_owner: HashMap<&str, &str> = value["changesets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["id"].as_str()?, *repo_owner.get(s["repoId"].as_str()?)?)))
        .collect();
    for c in value["changes"].as_array().into_iter().flatten() {
        if let (Some(owner), Some(version)) = (
            c["changesetId"].as_str().and_then(|s| set_owner.get(s)),
            c["versionId"].as_str().filter(|v| !v.is_empty()),
        ) {
            owners.insert(version.to_owned(), owner.to_string());
        }
    }
    owners
}

fn with_modes(value: &Value, keys: &[&str]) -> Value {
    let mut v = value.clone();
    if let Some(o) = v.as_object_mut() {
        for k in keys {
            if o.get(*k).is_none_or(Value::is_null) {
                o.insert((*k).into(), json!(""));
            }
        }
    }
    v
}

pub fn restore_history(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    let list = |k: &str| value[k].as_array().cloned().unwrap_or_default();
    let parse = |v: Value| -> Res<Value> { Ok(v) };
    for v in list("repos") {
        let r: FileRepo =
            serde_json::from_value(parse(v)?).map_err(|_| "invalidInput".to_owned())?;
        if ctx
            .db
            .file_entries()
            .id()
            .find(r.id.clone())
            .is_none_or(|f| f.kind != "folder" || f.owner_id != r.owner_id)
        {
            return fail("invalidInput");
        }
        ctx.db.file_repos().insert(r);
    }
    for v in list("changesets") {
        let r: FileChangeset = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.file_changesets().insert(r);
    }
    for v in list("changes") {
        let r: FileChange = serde_json::from_value(with_modes(&v, &["mode", "previousMode"]))
            .map_err(|_| "invalidInput".to_owned())?;
        if ctx
            .db
            .file_changesets()
            .id()
            .find(r.changeset_id.clone())
            .is_none()
            || !r.version_id.is_empty()
                && ctx.db.versions().id().find(r.version_id.clone()).is_none()
        {
            return fail("invalidInput");
        }
        ctx.db.file_changes().insert(r);
    }
    for v in list("trees") {
        let r: FileTree = serde_json::from_value(with_modes(&v, &["mode"]))
            .map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none()
            || ctx.db.versions().id().find(r.version_id.clone()).is_none()
        {
            return fail("invalidInput");
        }
        ctx.db.file_trees().insert(r);
    }
    for v in list("branches") {
        let r: FileBranch = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none()
            || ctx
                .db
                .file_changesets()
                .id()
                .find(r.changeset_id.clone())
                .is_none()
        {
            return fail("invalidInput");
        }
        ctx.db.file_branches().insert(r);
    }
    // Backups from before Git support have no Git rows.
    for v in list("gitObjects") {
        let r: FileGitObject = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none()
            || !r.changeset_id.is_empty()
                && ctx
                    .db
                    .file_changesets()
                    .id()
                    .find(r.changeset_id.clone())
                    .is_none_or(|s| s.repo_id != r.repo_id)
        {
            return fail("invalidInput");
        }
        ctx.db.file_git_objects().insert(r);
    }
    for v in list("gitRefs") {
        let r: FileGitRef = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none()
            || ctx
                .db
                .file_git_objects()
                .id()
                .find(format!("{}/{}", r.repo_id, r.target))
                .is_none()
        {
            return fail("invalidInput");
        }
        ctx.db.file_git_refs().insert(r);
    }
    for v in list("gitBlobs") {
        let r: FileGitBlob = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.file_repos().id().find(r.repo_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.file_git_blobs().insert(r);
    }
    Ok(())
}
