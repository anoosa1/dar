//! Git over HTTPS for tracked folders (`git.ts`, docs/spacetimedb/folder-history.md#git). The
//! runtime keeps a Git repository as a cache and does the Git work; these operations keep the
//! history itself: commit and tag objects, refs, and pushes applied to main and the live folder.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::blobs::{release_blob, store_blob};
use crate::files::Role;
use crate::history::{
    self, HistoryFiles, TreeItem, charge, folder_of, fresh, head_tree, keys, release, scan, status,
};
use crate::tables::*;
use crate::{Res, fail, now};

type Doc = Map<String, Value>;

const ZERO: &str = "0000000000000000000000000000000000000000";
const MAIN_REF: &str = "refs/heads/main";
const OBJECT_BYTES: usize = 256 * 1024;
const COMMITS: usize = 5000;
const CHANGES: usize = 50_000;
pub const READ_OPERATIONS: [&str; 5] =
    ["state", "export", "repositories", "resolve", "changesetOf"];

fn pseudo_file(repo_id: &str) -> String {
    format!("git:{repo_id}")
}

fn sha(value: Option<&Value>) -> Res<String> {
    match value.and_then(Value::as_str) {
        Some(s) if s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => {
            Ok(s.to_owned())
        }
        _ => fail("invalidInput"),
    }
}

fn list(value: Option<&Value>, max: usize) -> Res<Vec<Value>> {
    match value.and_then(Value::as_array) {
        Some(a) if a.len() <= max => Ok(a.clone()),
        _ => fail("invalidInput"),
    }
}

fn line(value: Option<&Value>, max: usize) -> Res<String> {
    match value.and_then(Value::as_str) {
        Some(s) if s.encode_utf16().count() <= max && !s.contains(['\0', '\n', '\r']) => {
            Ok(s.to_owned())
        }
        _ => fail("invalidInput"),
    }
}

fn base64(value: Option<&Value>, max: usize) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else {
        return fail("invalidInput");
    };
    let body = s.trim_end_matches('=');
    if s.is_empty()
        || s.len() > max
        || body.is_empty()
        || !body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn object_of(ctx: &ReducerContext, repo: &FileRepo, value: &str) -> Option<FileGitObject> {
    ctx.db
        .file_git_objects()
        .id()
        .find(format!("{}/{value}", repo.id))
}

fn pin_of(ctx: &ReducerContext, changeset_id: &str) -> Option<FileGitObject> {
    if changeset_id.is_empty() {
        return None;
    }
    ctx.db
        .file_git_objects()
        .changeset_id()
        .filter(changeset_id)
        .next()
}

fn referenced(ctx: &ReducerContext, version_id: &str) -> bool {
    ctx.db
        .file_changes()
        .version_id()
        .filter(version_id)
        .next()
        .is_some()
}

/// Stored content with this Git blob ID, if a version still holds it.
fn known_blob(ctx: &ReducerContext, repo: &FileRepo, value: &str) -> Option<FileGitBlob> {
    ctx.db
        .file_git_blobs()
        .id()
        .find(format!("{}/{value}", repo.id))
        .filter(|r| {
            ctx.db
                .versions()
                .blob_id()
                .filter(r.blob_id.as_str())
                .next()
                .is_some()
        })
}

fn git_sha_of(ctx: &ReducerContext, repo: &FileRepo, blob_id: &str) -> String {
    ctx.db
        .file_git_blobs()
        .blob_id()
        .filter(blob_id)
        .find(|r| r.repo_id == repo.id)
        .map(|r| r.sha)
        .unwrap_or_default()
}

/// A ref name Git accepts (git check-ref-format) under the namespaces Dar keeps.
pub fn valid_ref(name: &str) -> bool {
    let namespace = ["refs/heads/", "refs/tags/", "refs/notes/"]
        .iter()
        .any(|p| name.len() > p.len() && name.starts_with(p));
    if name.len() > 255 || !namespace {
        return false;
    }
    let bad_char = name
        .chars()
        .any(|c| (c as u32) <= 0x20 || c as u32 == 0x7f || "~^:?*[\\".contains(c));
    let bad = bad_char
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("/.")
        || name.split('/').any(|part| part.ends_with(".lock"));
    !bad && name != "refs/heads/HEAD"
}

fn repo_of(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    p: &Doc,
    write: bool,
) -> Res<(FileEntry, FileRepo)> {
    let (root, _) = folder_of(ctx, files, p.get("folderId"), write)?;
    let Some(repo) = ctx.db.file_repos().id().find(root.id.clone()) else {
        return fail("notFound");
    };
    Ok((root, repo))
}

fn person(ctx: &ReducerContext, user_id: &str) -> Value {
    let account = ctx.db.accounts().id().find(user_id.to_owned());
    let name = account
        .as_ref()
        .map(|a| {
            if a.display_name.is_empty() {
                a.username.clone()
            } else {
                a.display_name.clone()
            }
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Removed account".into());
    json!({ "name": name, "username": account.map(|a| a.username).unwrap_or_else(|| "removed".into()) })
}

/// A changeset as the runtime needs it to write Git objects.
fn commit_view(ctx: &ReducerContext, repo: &FileRepo, set: &FileChangeset) -> Res<Value> {
    let pin = pin_of(ctx, &set.id);
    let mut changes = Vec::new();
    for c in ctx.db.file_changes().changeset_id().filter(set.id.as_str()) {
        let version = if c.version_id.is_empty() {
            None
        } else {
            ctx.db.versions().id().find(c.version_id.clone())
        };
        if !c.version_id.is_empty() && version.is_none() {
            return fail("contentUnavailable");
        }
        let mut v = json!({ "path": c.path, "previousPath": c.previous_path, "kind": c.kind, "mode": c.mode });
        if let Some(version) = version {
            v["blobId"] = json!(version.blob_id);
            v["size"] = json!(version.size);
            v["sha"] = json!(git_sha_of(ctx, repo, &version.blob_id));
        }
        changes.push(v);
    }
    let mut v = json!({ "id": set.id, "parentId": set.parent_id, "main": set.branch.is_empty(), "message": set.message, "createdAt": set.created_at, "author": person(ctx, &set.author_id) });
    if let Some(pin) = pin {
        v["sha"] = json!(pin.sha);
        v["raw"] = json!(pin.raw);
    }
    v["changes"] = Value::Array(changes);
    Ok(v)
}

/// A folder's path from its owner's root, by name.
fn path_of(ctx: &ReducerContext, entry: &FileEntry) -> String {
    let mut names = vec![entry.name.clone()];
    let mut parent = entry.parent_id.clone();
    for _ in 0..128 {
        if parent.is_empty() {
            break;
        }
        let Some(p) = ctx.db.file_entries().id().find(parent.clone()) else {
            break;
        };
        names.insert(0, p.name.clone());
        parent = p.parent_id;
    }
    names.join("/")
}

/// The caller's tracked folders for the Git app, and those at or under folders shared with them.
fn repositories(ctx: &ReducerContext, files: &HistoryFiles, user_id: &str) -> Value {
    let mut found: Vec<FileRepo> = ctx.db.file_repos().owner_id().filter(user_id).collect();
    for grant in ctx.db.grants().user_id().filter(user_id) {
        let Some(shared) = ctx
            .db
            .file_entries()
            .id()
            .find(grant.file_id)
            .filter(|s| s.kind == "folder")
        else {
            continue;
        };
        for repo in ctx
            .db
            .file_repos()
            .owner_id()
            .filter(shared.owner_id.as_str())
        {
            let mut id = repo.id.clone();
            for _ in 0..128 {
                if id.is_empty() || found.iter().any(|r| r.id == repo.id) {
                    break;
                }
                if id == shared.id {
                    found.push(repo.clone());
                }
                id = ctx
                    .db
                    .file_entries()
                    .id()
                    .find(id)
                    .map(|e| e.parent_id)
                    .unwrap_or_default();
            }
        }
    }
    let mut out = Vec::new();
    for repo in found {
        let Some(root) = ctx
            .db
            .file_entries()
            .id()
            .find(repo.id.clone())
            .filter(|r| !r.trashed)
        else {
            continue;
        };
        let Some(role) = files.role(ctx, &root) else {
            continue;
        };
        let dirty = status(ctx, &repo, &root).map_or(-1i64, |s| s.changes.len() as i64);
        let mut seen = HashSet::new();
        let mut size = 0.0;
        for set in ctx.db.file_changesets().repo_id().filter(repo.id.as_str()) {
            for c in ctx.db.file_changes().changeset_id().filter(set.id.as_str()) {
                if c.version_id.is_empty() || !seen.insert(c.version_id.clone()) {
                    continue;
                }
                size += ctx
                    .db
                    .versions()
                    .id()
                    .find(c.version_id)
                    .map_or(0.0, |v| v.size);
            }
        }
        let owner = role == Role::Owner;
        let mut v = json!({ "id": repo.id, "name": root.name, "path": if owner { path_of(ctx, &root) } else { root.name.clone() }, "parentId": if owner { root.parent_id.clone() } else { String::new() },
            "revision": repo.revision, "createdAt": repo.created_at, "modifiedAt": repo.modified_at, "dirty": dirty,
            "agentBranches": ctx.db.file_branches().repo_id().filter(repo.id.as_str()).count(), "size": size,
            "role": match role { Role::Owner => "owner", Role::Write => "write", Role::Read => "read" } });
        if !owner {
            v["owner"] = person(ctx, &root.owner_id)["name"].clone();
        }
        if let Value::Object(counts) = crate::tracker::open_counts(ctx, &repo.id) {
            for (k, c) in counts {
                v[k] = c;
            }
        }
        out.push(v);
    }
    Value::Array(out)
}

fn refs_of(ctx: &ReducerContext, repo: &FileRepo) -> Vec<(String, String)> {
    ctx.db
        .file_git_refs()
        .repo_id()
        .filter(repo.id.as_str())
        .map(|r| (r.name, r.target))
        .collect()
}

fn refs_json(refs: &[(String, String)]) -> Value {
    Value::Array(
        refs.iter()
            .map(|(n, s)| json!({ "name": n, "sha": s }))
            .collect(),
    )
}

/// Operations for `service_git`; `upload_key` ties a push's blobs to the pushing session.
pub fn git_operation(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    user_id: &str,
    upload_key: &str,
    operation: &str,
    p: &Doc,
) -> Res<Value> {
    match operation {
        "repositories" => {
            keys(p, &[])?;
            Ok(repositories(ctx, files, user_id))
        }
        "changesetOf" => {
            // The main commit a Git ID is, so the Git app can restore the folder to it.
            keys(p, &["folderId", "sha"])?;
            let (_, repo) = repo_of(ctx, files, p, false)?;
            let set = object_of(ctx, &repo, &sha(p.get("sha"))?)
                .filter(|o| !o.changeset_id.is_empty())
                .and_then(|o| ctx.db.file_changesets().id().find(o.changeset_id));
            Ok(
                json!({ "revision": repo.revision, "changesetId": set.filter(|s| s.branch.is_empty()).map(|s| s.id) }),
            )
        }
        "resolve" => {
            // A clone alias: the caller's own tracked folder at a path from their root.
            keys(p, &["path"])?;
            let path = p
                .get("path")
                .and_then(Value::as_str)
                .filter(|s| s.encode_utf16().count() <= 4096)
                .unwrap_or_default();
            let parts: Vec<&str> = if path.is_empty() {
                Vec::new()
            } else {
                path.split('/').collect()
            };
            if parts.is_empty()
                || parts.len() > history::DEPTH
                || parts.iter().any(|x| x.is_empty())
            {
                return fail("notFound");
            }
            let mut parent_id = String::new();
            let mut entry: Option<FileEntry> = None;
            for part in parts {
                let found = ctx
                    .db
                    .file_entries()
                    .list_key()
                    .filter(format!("{user_id}/{parent_id}/{}", part.to_lowercase()).as_str())
                    .find(|r| !r.trashed && r.kind == "folder");
                let Some(found) = found else {
                    return fail("notFound");
                };
                parent_id = found.id.clone();
                entry = Some(found);
            }
            let Some(entry) =
                entry.filter(|e| ctx.db.file_repos().id().find(e.id.clone()).is_some())
            else {
                return fail("notFound");
            };
            Ok(json!({ "folderId": entry.id }))
        }
        "state" => {
            // What a fetch needs: the refs, and main's commits Git has not seen yet, oldest first.
            keys(p, &["folderId"])?;
            let (_, repo) = repo_of(ctx, files, p, false)?;
            let mut pending: Vec<FileChangeset> = Vec::new();
            let mut cursor = repo.head_id.clone();
            while !cursor.is_empty() && pin_of(ctx, &cursor).is_none() {
                let Some(set) = ctx.db.file_changesets().id().find(cursor.clone()) else {
                    return fail("contentUnavailable");
                };
                cursor = set.parent_id.clone();
                pending.push(set);
            }
            pending.reverse();
            let base = if cursor.is_empty() {
                Value::Null
            } else {
                json!(pin_of(ctx, &cursor).map(|p| p.sha))
            };
            Ok(
                json!({ "revision": repo.revision, "generation": repo.created_at, "main": pin_of(ctx, &repo.head_id).map(|p| p.sha), "base": base,
                "pending": pending.iter().map(|s| commit_view(ctx, &repo, s)).collect::<Res<Vec<_>>>()?, "refs": refs_json(&refs_of(ctx, &repo)) }),
            )
        }
        "export" => {
            // Everything, to rebuild the runtime's Git repository from scratch.
            keys(p, &["folderId"])?;
            let (_, repo) = repo_of(ctx, files, p, false)?;
            let sets: Vec<FileChangeset> = ctx
                .db
                .file_changesets()
                .repo_id()
                .filter(repo.id.as_str())
                .filter(|s| s.branch.is_empty() || s.branch == "git")
                .collect();
            Ok(
                json!({ "revision": repo.revision, "generation": repo.created_at, "main": pin_of(ctx, &repo.head_id).map(|p| p.sha), "headId": repo.head_id,
                "commits": sets.iter().map(|s| commit_view(ctx, &repo, s)).collect::<Res<Vec<_>>>()?,
                "tags": ctx.db.file_git_objects().repo_id().filter(repo.id.as_str()).filter(|r| r.r#type == "tag").map(|r| json!({ "sha": r.sha, "raw": r.raw })).collect::<Vec<_>>(),
                "refs": refs_json(&refs_of(ctx, &repo)) }),
            )
        }
        "pin" => {
            // Writes down the Git commits the runtime made for Dar's own commits, and the blob IDs it hashed.
            keys(p, &["folderId", "commits", "blobs"])?;
            let (_, repo) = repo_of(ctx, files, p, false)?;
            let mut conflict = false;
            for commit in list(p.get("commits"), history::CHANGESETS as usize)? {
                let Some(commit) = commit.as_object() else {
                    return fail("invalidInput");
                };
                let set = ctx
                    .db
                    .file_changesets()
                    .id()
                    .find(line(commit.get("id"), 64)?);
                let object_id = sha(commit.get("sha"))?;
                let parents = list(commit.get("parents"), 1)?
                    .iter()
                    .map(|v| sha(Some(v)))
                    .collect::<Res<Vec<_>>>()?;
                let raw = base64(commit.get("raw"), OBJECT_BYTES * 2)?;
                let author = line(commit.get("author"), 256)?;
                let Some(set) = set.filter(|s| s.repo_id == repo.id && s.branch.is_empty()) else {
                    return fail("invalidInput");
                };
                if let Some(existing) = pin_of(ctx, &set.id) {
                    if existing.sha != object_id {
                        conflict = true;
                    }
                    continue;
                }
                if object_of(ctx, &repo, &object_id).is_some() {
                    conflict = true;
                    continue;
                }
                ctx.db.file_git_objects().insert(FileGitObject {
                    id: format!("{}/{object_id}", repo.id),
                    repo_id: repo.id.clone(),
                    sha: object_id,
                    r#type: "commit".into(),
                    changeset_id: set.id,
                    parents: parents.join(" "),
                    author,
                    raw,
                    created_at: now(ctx),
                });
            }
            for blob in list(p.get("blobs"), history::CHANGES * 10)? {
                let Some(blob) = blob.as_object() else {
                    return fail("invalidInput");
                };
                let object_id = sha(blob.get("sha"))?;
                let blob_id = line(blob.get("blobId"), 64)?;
                let Some(size) = blob
                    .get("size")
                    .and_then(Value::as_f64)
                    .filter(|s| s.fract() == 0.0 && *s >= 0.0)
                else {
                    return fail("invalidInput");
                };
                let id = format!("{}/{object_id}", repo.id);
                if ctx.db.file_git_blobs().id().find(id.clone()).is_none()
                    && ctx
                        .db
                        .versions()
                        .blob_id()
                        .filter(blob_id.as_str())
                        .next()
                        .map(|v| v.size)
                        == Some(size)
                {
                    ctx.db.file_git_blobs().insert(FileGitBlob {
                        id,
                        repo_id: repo.id.clone(),
                        sha: object_id,
                        blob_id,
                        size,
                    });
                }
            }
            Ok(json!({ "conflict": conflict }))
        }
        "pushBegin" => {
            // Content the folder already has is reused; the rest is reserved for upload.
            keys(p, &["folderId", "blobs"])?;
            let (root, repo) = repo_of(ctx, files, p, true)?;
            let expires_at = now(ctx) + 3_600_000.0;
            let mut total = 0.0;
            let mut rows: Vec<(String, Result<String, f64>)> = Vec::new();
            for value in list(p.get("blobs"), CHANGES)? {
                let object_id = sha(value.get("sha"))?;
                let Some(size) = value
                    .get("size")
                    .and_then(Value::as_f64)
                    .filter(|s| s.fract() == 0.0 && *s >= 0.0 && *s <= 16.0 * 1024f64.powi(3))
                else {
                    return fail("invalidInput");
                };
                match known_blob(ctx, &repo, &object_id).filter(|k| k.size == size) {
                    Some(k) => rows.push((object_id, Ok(k.blob_id))),
                    None => {
                        total += size;
                        rows.push((object_id, Err(size)));
                    }
                }
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
            if !files.fits_quota(&meter, total) {
                return fail("quotaExceeded");
            }
            let reserved = meter.reserved + total;
            ctx.db.usage().user_id().update(Usage { reserved, ..meter });
            let blobs: Vec<Value> = rows
                .into_iter()
                .map(|(object_id, r)| match r {
                    Ok(blob_id) => json!({ "sha": object_id, "blobId": blob_id }),
                    Err(size) => {
                        let upload_id = files.uuid(ctx);
                        ctx.db.uploads().insert(Upload {
                            id: upload_id.clone(),
                            session_id: upload_key.into(),
                            owner_id: root.owner_id.clone(),
                            parent_id: String::new(),
                            file_id: String::new(),
                            name: String::new(),
                            size,
                            media_type: "application/octet-stream".into(),
                            expected_revision: 0,
                            expires_at,
                            state: "uploading".into(),
                            digest: String::new(),
                            expires_key: format!("{:016}", expires_at as i64),
                        });
                        json!({ "sha": object_id, "uploadId": upload_id })
                    }
                })
                .collect();
            Ok(json!({ "blobs": blobs }))
        }
        "push" => push(ctx, files, user_id, upload_key, p),
        _ => fail("invalidInput"),
    }
}

#[derive(Clone)]
struct PushChange {
    path: String,
    previous_path: String,
    kind: String,
    mode: String,
    previous_mode: String,
    blob: String,
    previous_blob: String,
    media_type: String,
}

struct PushCommit {
    sha: String,
    raw: String,
    parents: Vec<String>,
    author: String,
    message: String,
    time: f64,
    changes: Vec<PushChange>,
}

struct PushTag {
    sha: String,
    raw: String,
    target: String,
    author: String,
}

fn valid_mode(m: &str) -> bool {
    ["", "100755", "120000"].contains(&m)
}

fn changes_of(value: Option<&Value>) -> Res<Vec<PushChange>> {
    list(value, history::CHANGES)?
        .iter()
        .map(|r| {
            let Some(r) = r.as_object() else {
                return fail("invalidInput");
            };
            let kind = r
                .get("kind")
                .and_then(Value::as_str)
                .filter(|k| ["added", "modified", "deleted", "renamed"].contains(k))
                .ok_or_else(|| "invalidInput".to_owned())?
                .to_owned();
            let path = line(r.get("path"), 4096)?;
            let previous_path = if kind == "renamed" {
                line(r.get("previousPath"), 4096)?
            } else {
                String::new()
            };
            let mode = if kind == "deleted" {
                String::new()
            } else {
                r.get("mode")
                    .map(|m| crate::text_of(Some(m)))
                    .filter(|m| m != "null")
                    .unwrap_or_default()
            };
            let previous_mode = if kind == "added" {
                String::new()
            } else {
                r.get("previousMode")
                    .map(|m| crate::text_of(Some(m)))
                    .filter(|m| m != "null")
                    .unwrap_or_default()
            };
            if path.is_empty() || !valid_mode(&mode) || !valid_mode(&previous_mode) {
                return fail("invalidInput");
            }
            let blob = if kind == "deleted" {
                String::new()
            } else {
                sha(r.get("blob"))?
            };
            let previous_blob = if kind == "added" {
                String::new()
            } else {
                sha(r.get("previousBlob"))?
            };
            let token = |s: &str| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
            };
            let media_type =
                if kind == "deleted" || r.get("mediaType").and_then(Value::as_str) == Some("") {
                    String::new()
                } else {
                    match r.get("mediaType").and_then(Value::as_str) {
                        Some(m)
                            if m.len() <= 150
                                && m.split_once('/').is_some_and(|(a, b)| token(a) && token(b)) =>
                        {
                            m.to_lowercase()
                        }
                        _ => return fail("invalidInput"),
                    }
                };
            Ok(PushChange {
                path,
                previous_path,
                kind,
                mode,
                previous_mode,
                blob,
                previous_blob,
                media_type,
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
struct Slot {
    file_id: String,
    version_id: String,
    blob: String,
    mode: String,
}

#[derive(Clone, Debug)]
struct Step {
    file_id: String,
    previous: Option<Slot>,
}

struct ChainItem {
    sha: String,
    changes: Vec<PushChange>,
}

struct Plan {
    head: BTreeMap<String, TreeItem>,
    steps: Vec<Vec<Step>>,
}

/// Applies a push. Every ref is checked like Git's receive-pack: the old value must match, main
/// only fast-forwards and is never deleted. main's new commits join the folder's history and
/// change the live folder, unless a file they touch has uncommitted changes in Dar.
fn push(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    user_id: &str,
    upload_key: &str,
    p: &Doc,
) -> Res<Value> {
    keys(
        p,
        &[
            "folderId", "updates", "atomic", "commits", "tags", "main", "blobs",
        ],
    )?;
    let (root, repo) = repo_of(ctx, files, p, true)?;
    let at = now(ctx);
    let updates: Vec<(String, String, String)> = list(p.get("updates"), 1000)?
        .iter()
        .map(|r| {
            Ok((
                line(r.get("name"), 255)?,
                sha(r.get("old"))?,
                sha(r.get("new"))?,
            ))
        })
        .collect::<Res<Vec<_>>>()?;
    let names: HashSet<&String> = updates.iter().map(|u| &u.0).collect();
    if names.len() != updates.len() || updates.is_empty() {
        return fail("invalidInput");
    }
    let mut commit_order: Vec<String> = Vec::new();
    let mut commits: HashMap<String, PushCommit> = HashMap::new();
    let mut tags: Vec<PushTag> = Vec::new();
    for r in list(p.get("commits"), COMMITS)? {
        let value = PushCommit {
            sha: sha(r.get("sha"))?,
            raw: base64(r.get("raw"), OBJECT_BYTES * 2)?,
            parents: list(r.get("parents"), 64)?
                .iter()
                .map(|v| sha(Some(v)))
                .collect::<Res<Vec<_>>>()?,
            author: line(r.get("author"), 256)?,
            message: match r.get("message").and_then(Value::as_str) {
                Some(m) => m
                    .encode_utf16()
                    .take(history::MESSAGE_BYTES / 2)
                    .collect::<Vec<u16>>()
                    .pipe(),
                None => return fail("invalidInput"),
            },
            time: r
                .get("time")
                .and_then(Value::as_f64)
                .filter(|t| t.is_finite())
                .ok_or_else(|| "invalidInput".to_owned())?,
            changes: changes_of(r.get("changes"))?,
        };
        if object_of(ctx, &repo, &value.sha).is_none() && !commits.contains_key(&value.sha) {
            commit_order.push(value.sha.clone());
            commits.insert(value.sha.clone(), value);
        }
    }
    for r in list(p.get("tags"), 1000)? {
        let value = PushTag {
            sha: sha(r.get("sha"))?,
            raw: base64(r.get("raw"), OBJECT_BYTES * 2)?,
            target: sha(r.get("target"))?,
            author: line(r.get("author"), 256)?,
        };
        if object_of(ctx, &repo, &value.sha).is_none() && !tags.iter().any(|t| t.sha == value.sha) {
            tags.push(value);
        }
    }
    if commits.values().map(|c| c.changes.len()).sum::<usize>() > CHANGES {
        return fail("operationTooLarge");
    }
    let type_of = |value: &str| -> Option<String> {
        object_of(ctx, &repo, value).map(|o| o.r#type).or_else(|| {
            if commits.contains_key(value) {
                Some("commit".into())
            } else if tags.iter().any(|t| t.sha == value) {
                Some("tag".into())
            } else {
                None
            }
        })
    };
    let parents_of = |value: &str| -> Vec<String> {
        if let Some(r) = object_of(ctx, &repo, value) {
            return r
                .parents
                .split(' ')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect();
        }
        if let Some(c) = commits.get(value) {
            return c.parents.clone();
        }
        tags.iter()
            .find(|t| t.sha == value)
            .map(|t| vec![t.target.clone()])
            .unwrap_or_default()
    };
    let peel = |value: &str| -> String {
        let mut current = value.to_owned();
        for _ in 0..16 {
            if type_of(&current).as_deref() != Some("tag") {
                break;
            }
            current = parents_of(&current).first().cloned().unwrap_or_default();
        }
        current
    };
    let is_ancestor = |ancestor: &str, descendant: &str| -> bool {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = vec![descendant.to_owned()];
        let mut i = 0;
        while i < queue.len() && seen.len() < 200_000 {
            let value = queue[i].clone();
            i += 1;
            if value == ancestor {
                return true;
            }
            for parent in parents_of(&value) {
                if seen.insert(parent.clone()) {
                    queue.push(parent);
                }
            }
        }
        false
    };
    for c in commits.values() {
        for parent in &c.parents {
            if type_of(parent).is_none() {
                return fail("invalidInput");
            }
        }
    }
    for t in &tags {
        if type_of(&t.target).is_none() {
            return fail("invalidInput");
        }
    }

    // Ref checks, as receive-pack makes them.
    let main_pin = pin_of(ctx, &repo.head_id);
    let current_main = if repo.head_id.is_empty() {
        ZERO.to_owned()
    } else {
        main_pin.map(|p| p.sha).unwrap_or_default()
    };
    let current: HashMap<String, String> = refs_of(ctx, &repo).into_iter().collect();
    let mut errors: HashMap<String, String> = HashMap::new();
    for (name, old, new) in &updates {
        let reason: Option<&str> = (|| {
            if !valid_ref(name) {
                return Some("Dar keeps branches, tags and notes with valid names");
            }
            if name == MAIN_REF {
                if new == ZERO {
                    return Some("main cannot be deleted");
                }
                if *old != current_main {
                    return Some("fetch first");
                }
                if type_of(new).as_deref() != Some("commit") {
                    return Some("main must point at a commit");
                }
                if current_main != ZERO && !is_ancestor(&current_main, new) {
                    return Some("non-fast-forward");
                }
                return None;
            }
            if name.starts_with("refs/heads/agent/") {
                return Some("agent/ branches belong to Agent");
            }
            if old != current.get(name).map_or(ZERO, |s| s.as_str()) {
                return Some("fetch first");
            }
            if new == ZERO {
                return None;
            }
            let kind = type_of(new);
            if kind.as_deref() != Some("commit") && kind.as_deref() != Some("tag") {
                return Some("missing object");
            }
            if name.starts_with("refs/heads/") && kind.as_deref() != Some("commit") {
                return Some("branches must point at commits");
            }
            if type_of(&peel(new)).as_deref() != Some("commit") {
                return Some("tags must point at commits");
            }
            None
        })();
        if let Some(reason) = reason {
            errors.insert(name.clone(), reason.into());
        }
    }
    let atomic = p.get("atomic") == Some(&json!(true));
    let finish = |errors: &HashMap<String, String>| -> Value {
        json!({ "results": updates.iter().map(|(name, _, _)| { let mut v = json!({ "name": name }); if let Some(e) = errors.get(name) { v["error"] = json!(e); } v }).collect::<Vec<_>>() })
    };
    let fail_atomic = |errors: &mut HashMap<String, String>| {
        for (name, _, _) in &updates {
            errors
                .entry(name.clone())
                .or_insert_with(|| "atomic push failed".into());
        }
    };
    if atomic && !errors.is_empty() {
        fail_atomic(&mut errors);
        return Ok(finish(&errors));
    }

    // Content: uploads finished by this push, or content the folder already holds.
    let mut blobs: HashMap<String, (String, f64)> = HashMap::new();
    let mut reserved_bytes = 0.0;
    for r in list(p.get("blobs"), CHANGES)? {
        let object_id = sha(r.get("sha"))?;
        let Some(size) = r
            .get("size")
            .and_then(Value::as_f64)
            .filter(|s| s.fract() == 0.0 && *s >= 0.0)
        else {
            return fail("invalidInput");
        };
        if let Some(upload_id) = r.get("uploadId").and_then(Value::as_str) {
            let upload = ctx.db.uploads().id().find(upload_id.to_owned());
            let digest = r
                .get("digest")
                .and_then(Value::as_str)
                .filter(|d| crate::blobs::is_blob_id(d));
            let (Some(upload), Some(digest)) = (
                upload.filter(|u| {
                    u.session_id == upload_key
                        && u.state == "uploading"
                        && u.owner_id == root.owner_id
                        && u.size == size
                }),
                digest,
            ) else {
                return fail("notFound");
            };
            ctx.db.uploads().id().update(Upload {
                state: "complete".into(),
                digest: digest.into(),
                ..upload
            });
            reserved_bytes += size;
            store_blob(ctx, digest, size)?;
            blobs.insert(object_id.clone(), (digest.into(), size));
            let id = format!("{}/{object_id}", repo.id);
            let next = FileGitBlob {
                id: id.clone(),
                repo_id: repo.id.clone(),
                sha: object_id,
                blob_id: digest.into(),
                size,
            };
            if ctx.db.file_git_blobs().id().find(id).is_some() {
                ctx.db.file_git_blobs().id().update(next);
            } else {
                ctx.db.file_git_blobs().insert(next);
            }
        } else {
            let known = known_blob(ctx, &repo, &object_id);
            let Some(known) = known.filter(|k| {
                Some(k.blob_id.as_str()) == r.get("blobId").and_then(Value::as_str)
                    && k.size == size
            }) else {
                return fail("conflict");
            };
            blobs.insert(object_id, (known.blob_id, size));
        }
    }
    if let Some(meter) = ctx.db.usage().user_id().find(root.owner_id.clone())
        && reserved_bytes != 0.0
    {
        let reserved = (meter.reserved - reserved_bytes).max(0.0);
        ctx.db.usage().user_id().update(Usage { reserved, ..meter });
    }
    let new_version = |file_id: &str, blob: &str, media_type: &str| -> Res<String> {
        let content = match blobs.get(blob) {
            Some(c) => c.clone(),
            None => match known_blob(ctx, &repo, blob) {
                Some(k) => (k.blob_id, k.size),
                None => return fail("contentUnavailable"),
            },
        };
        charge(ctx, files, &root.owner_id, content.1)?;
        // An unknown type is the one the same content already has here.
        let kind = if !media_type.is_empty() {
            media_type.to_owned()
        } else {
            ctx.db
                .versions()
                .blob_id()
                .filter(content.0.as_str())
                .next()
                .map(|v| v.media_type)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "application/octet-stream".into())
        };
        Ok(ctx
            .db
            .versions()
            .insert(Version {
                id: files.uuid(ctx),
                file_id: file_id.into(),
                blob_id: content.0,
                size: content.1,
                media_type: kind,
                created_at: at,
            })
            .id)
    };

    // main: replay its new commits over the head tree, then check the live folder before changing anything.
    let main_update = updates
        .iter()
        .find(|(name, _, _)| name == MAIN_REF && !errors.contains_key(name))
        .cloned();
    let mut chain: Vec<ChainItem> = Vec::new();
    if main_update.is_some() {
        let main = p.get("main").and_then(|m| m.get("chain"));
        for r in list(main, COMMITS)? {
            let changes = if r.get("changes").is_none() {
                None
            } else {
                Some(changes_of(r.get("changes"))?)
            };
            let s = sha(r.get("sha"))?;
            let changes = match changes {
                Some(c) => c,
                None => match commits.get(&s) {
                    Some(c) => c.changes.clone(),
                    None => return fail("invalidInput"),
                },
            };
            chain.push(ChainItem { sha: s, changes });
        }
    }
    let mut plan: Option<Plan> = None;
    if let Some((_, _, new)) = &main_update {
        if chain.last().is_none_or(|c| &c.sha != new) {
            return fail("invalidInput");
        }
        let rewrites = chain.iter().any(|item| {
            object_of(ctx, &repo, &item.sha).is_some_and(|known| {
                ctx.db
                    .file_changesets()
                    .id()
                    .find(known.changeset_id)
                    .is_some_and(|s| s.branch.is_empty())
            })
        });
        let planned = if rewrites {
            Err("main's earlier commits cannot be rewritten".to_owned())
        } else {
            plan_main(ctx, files, &repo, &root, &chain)?
        };
        match planned {
            Ok(planned) => plan = Some(planned),
            Err(reason) => {
                errors.insert(MAIN_REF.into(), reason);
                if atomic {
                    fail_atomic(&mut errors);
                    return Ok(finish(&errors));
                }
            }
        }
    }
    let on_main: HashSet<String> = if plan.is_some() {
        chain.iter().map(|c| c.sha.clone()).collect()
    } else {
        HashSet::new()
    };
    let mut changeset_of: HashMap<String, String> = HashMap::new();
    for s in &commit_order {
        let id = fresh(ctx, files);
        changeset_of.insert(s.clone(), id);
    }
    let set_for = |value: &str| -> Res<String> {
        changeset_of
            .get(value)
            .cloned()
            .or_else(|| object_of(ctx, &repo, value).map(|o| o.changeset_id))
            .ok_or_else(|| "invalidInput".to_owned())
    };
    if ctx
        .db
        .file_changesets()
        .repo_id()
        .filter(repo.id.as_str())
        .count()
        + commits.len()
        > history::CHANGESETS as usize
    {
        return fail("operationTooLarge");
    }

    // Commits off main: their changes are against their first parent; content belongs to no file.
    let mut shared: HashMap<String, String> = HashMap::new();
    for s in &commit_order {
        if on_main.contains(s) {
            continue;
        }
        let commit = &commits[s];
        let changeset_id = changeset_of[s].clone();
        if commit.changes.len() > history::CHANGES {
            return fail("operationTooLarge");
        }
        let parent_id = match commit.parents.first() {
            Some(first) => set_for(first)?,
            None => String::new(),
        };
        ctx.db.file_changesets().insert(FileChangeset {
            id: changeset_id.clone(),
            repo_id: repo.id.clone(),
            branch: "git".into(),
            parent_id,
            sequence: 0,
            kind: if commit.parents.len() > 1 {
                "merge".into()
            } else {
                "commit".into()
            },
            message: commit.message.clone(),
            author_id: user_id.into(),
            file_count: commit.changes.len() as u32,
            created_at: commit.time,
        });
        for (index, change) in commit.changes.iter().enumerate() {
            let version_id = if change.kind == "deleted" {
                String::new()
            } else if let Some(v) = shared.get(&change.blob) {
                v.clone()
            } else {
                let content_blob = blobs
                    .get(&change.blob)
                    .map(|b| b.0.clone())
                    .or_else(|| known_blob(ctx, &repo, &change.blob).map(|k| k.blob_id));
                let existing = content_blob.and_then(|b| {
                    ctx.db
                        .versions()
                        .blob_id()
                        .filter(b.as_str())
                        .find(|v| v.file_id == pseudo_file(&repo.id))
                        .map(|v| v.id)
                });
                let v = match existing {
                    Some(v) => v,
                    None => new_version(&pseudo_file(&repo.id), &change.blob, &change.media_type)?,
                };
                shared.insert(change.blob.clone(), v.clone());
                v
            };
            ctx.db.file_changes().insert(FileChange {
                id: format!("{changeset_id}/{index}"),
                changeset_id: changeset_id.clone(),
                path: change.path.clone(),
                previous_path: change.previous_path.clone(),
                kind: change.kind.clone(),
                file_id: String::new(),
                version_id,
                previous_version_id: String::new(),
                expect_revision: 0,
                mode: change.mode.clone(),
                previous_mode: change.previous_mode.clone(),
            });
        }
    }
    if let Some(plan) = &plan {
        apply_main(
            ctx,
            files,
            &repo,
            &root,
            user_id,
            plan,
            &chain,
            &commits,
            &changeset_of,
            &new_version,
        )?;
    }
    for s in &commit_order {
        let c = &commits[s];
        ctx.db.file_git_objects().insert(FileGitObject {
            id: format!("{}/{}", repo.id, c.sha),
            repo_id: repo.id.clone(),
            sha: c.sha.clone(),
            r#type: "commit".into(),
            changeset_id: changeset_of[s].clone(),
            parents: c.parents.join(" "),
            author: c.author.clone(),
            raw: c.raw.clone(),
            created_at: at,
        });
    }
    for t in &tags {
        ctx.db.file_git_objects().insert(FileGitObject {
            id: format!("{}/{}", repo.id, t.sha),
            repo_id: repo.id.clone(),
            sha: t.sha.clone(),
            r#type: "tag".into(),
            changeset_id: String::new(),
            parents: t.target.clone(),
            author: t.author.clone(),
            raw: t.raw.clone(),
            created_at: at,
        });
    }
    for (name, _, new) in &updates {
        if errors.contains_key(name) || name == MAIN_REF {
            continue;
        }
        let row_id = format!("{}/{name}", repo.id);
        if new == ZERO {
            ctx.db.file_git_refs().id().delete(row_id);
        } else {
            let next = FileGitRef {
                id: row_id.clone(),
                repo_id: repo.id.clone(),
                name: name.clone(),
                target: new.clone(),
                updated_at: at,
            };
            if ctx.db.file_git_refs().id().find(row_id).is_some() {
                ctx.db.file_git_refs().id().update(next);
            } else {
                ctx.db.file_git_refs().insert(next);
            }
        }
    }
    // Pull requests follow their branches: merged once their base contains them, closed when their branch goes.
    let applied: HashMap<String, String> = updates
        .iter()
        .filter(|(name, _, _)| !errors.contains_key(name) && (name != MAIN_REF || plan.is_some()))
        .map(|(n, _, v)| (n.clone(), v.clone()))
        .collect();
    let tip = |name: &str, after: bool| -> Option<String> {
        let reference = format!("refs/heads/{name}");
        let before = if reference == MAIN_REF {
            current_main.clone()
        } else {
            current
                .get(&reference)
                .cloned()
                .unwrap_or_else(|| ZERO.into())
        };
        let value = if after {
            applied.get(&reference).cloned().unwrap_or(before)
        } else {
            before
        };
        if value == ZERO || value.is_empty() {
            None
        } else {
            Some(value)
        }
    };
    crate::tracker::pulls_after_push(ctx, files, &repo, user_id, &tip, &is_ancestor)?;
    collect(ctx, &repo);
    let latest = ctx
        .db
        .file_repos()
        .id()
        .find(repo.id.clone())
        .expect("repo");
    ctx.db.file_repos().id().update(FileRepo {
        revision: latest.revision + 1,
        modified_at: at,
        ..latest
    });
    Ok(finish(&errors))
}

trait Utf16 {
    fn pipe(self) -> String;
}
impl Utf16 for Vec<u16> {
    fn pipe(self) -> String {
        String::from_utf16_lossy(&self)
    }
}

/// Replays main's new commits over the head tree without writing: assigns file IDs (a rename
/// keeps its file), checks names and limits, and checks every path the push changes is
/// committed in Dar. `Err` is why main is refused.
fn plan_main(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    repo: &FileRepo,
    root: &FileEntry,
    chain: &[ChainItem],
) -> Res<Result<Plan, String>> {
    let head = head_tree(ctx, &repo.id);
    let mut tree: BTreeMap<String, Slot> = head
        .iter()
        .map(|(p, i)| {
            (
                p.clone(),
                Slot {
                    file_id: i.file_id.clone(),
                    version_id: i.version_id.clone(),
                    blob: String::new(),
                    mode: i.mode.clone(),
                },
            )
        })
        .collect();
    let mut steps: Vec<Vec<Step>> = Vec::new();
    if repo.sequence as usize + chain.len() > history::CHANGESETS as usize {
        return Ok(Err("the folder's history is full".into()));
    }
    for commit in chain {
        if commit.changes.len() > history::CHANGES {
            return Ok(Err("a commit changes too many files for Dar".into()));
        }
        let mut result: Vec<Step> = Vec::new();
        // Removals first: a path can be renamed away and another file added there in one commit.
        for change in &commit.changes {
            if change.kind != "deleted" && change.kind != "renamed" {
                result.push(Step {
                    file_id: String::new(),
                    previous: None,
                });
                continue;
            }
            let from = if change.kind == "renamed" {
                &change.previous_path
            } else {
                &change.path
            };
            let Some(slot) = tree.remove(from) else {
                return fail("invalidInput");
            };
            result.push(Step {
                file_id: slot.file_id.clone(),
                previous: Some(slot),
            });
        }
        for (index, change) in commit.changes.iter().enumerate() {
            if change.kind == "deleted" {
                continue;
            }
            let parts: Vec<&str> = change.path.split('/').collect();
            if parts
                .iter()
                .any(|part| crate::name_of(part).ok().as_deref() != Some(*part))
            {
                return Ok(Err(format!(
                    "“{}” is not a file name Dar can keep",
                    change.path
                )));
            }
            if parts.len() > history::DEPTH {
                return Ok(Err("folders are nested too deeply for Dar".into()));
            }
            let same = change.blob == change.previous_blob;
            if change.kind == "renamed" {
                let step = result[index].clone();
                let previous = step.previous.clone().expect("previous");
                tree.insert(
                    change.path.clone(),
                    Slot {
                        file_id: step.file_id,
                        version_id: if same {
                            previous.version_id
                        } else {
                            String::new()
                        },
                        blob: if same {
                            previous.blob
                        } else {
                            change.blob.clone()
                        },
                        mode: change.mode.clone(),
                    },
                );
                continue;
            }
            if change.kind == "modified" {
                let Some(slot) = tree.get(&change.path).cloned() else {
                    return fail("invalidInput");
                };
                result[index] = Step {
                    file_id: slot.file_id.clone(),
                    previous: Some(slot.clone()),
                };
                tree.insert(
                    change.path.clone(),
                    Slot {
                        file_id: slot.file_id,
                        version_id: if same { slot.version_id } else { String::new() },
                        blob: if same { slot.blob } else { change.blob.clone() },
                        mode: change.mode.clone(),
                    },
                );
                continue;
            }
            if tree.contains_key(&change.path) {
                return fail("invalidInput");
            }
            let file_id = files.uuid(ctx);
            result[index] = Step {
                file_id: file_id.clone(),
                previous: None,
            };
            tree.insert(
                change.path.clone(),
                Slot {
                    file_id,
                    version_id: String::new(),
                    blob: change.blob.clone(),
                    mode: change.mode.clone(),
                },
            );
        }
        steps.push(result);
    }
    if tree.len() > history::FILES {
        return Ok(Err(
            "the folder would have more files than Dar tracks".into()
        ));
    }
    // Dar names are unique without regard to case, among files and folders alike.
    let mut names: HashSet<String> = HashSet::new();
    for path in tree.keys() {
        let parts: Vec<String> = path.to_lowercase().split('/').map(str::to_owned).collect();
        for i in 1..=parts.len() {
            let prefix = parts[..i].join("/");
            let folder = i < parts.len();
            let clash = if folder {
                names.contains(&format!("{prefix}\0file"))
            } else {
                names.contains(&format!("{prefix}\0folder"))
            };
            if clash || !folder && names.contains(&format!("{prefix}\0file")) {
                return Ok(Err(format!(
                    "“{path}” differs from another name only by case"
                )));
            }
            names.insert(format!(
                "{prefix}{}",
                if folder { "\0folder" } else { "\0file" }
            ));
        }
    }
    // Every path the push changes must be as main last committed it.
    let live = scan(ctx, root)?;
    let path_of: HashMap<String, String> = tree
        .iter()
        .map(|(p, s)| (s.file_id.clone(), p.clone()))
        .collect();
    for (path, old) in &head {
        if let Some(next) = tree.get(path)
            && next.file_id == old.file_id
            && next.version_id == old.version_id
            && path_of.get(&old.file_id) == Some(path)
        {
            continue;
        }
        let entry = live.get(path);
        let version = ctx.db.versions().id().find(old.version_id.clone());
        if entry.is_none_or(|e| {
            e.id != old.file_id || Some(e.blob_id.clone()) != version.map(|v| v.blob_id)
        }) {
            return Ok(Err(format!(
                "“{path}” has uncommitted changes in Dar; commit them there, then pull"
            )));
        }
    }
    for (path, slot) in &tree {
        if head.get(path).is_some_and(|h| h.file_id == slot.file_id) {
            continue;
        }
        if live.contains_key(path) && !head.contains_key(path) {
            return Ok(Err(format!(
                "“{path}” is in the Dar folder but not committed; commit it there, then pull"
            )));
        }
        // New paths must not reach into a folder that keeps its own history.
        let mut parent_id = root.id.clone();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            let Some(entry) =
                crate::files::find_key(ctx, &crate::files::key(&root.owner_id, &parent_id, part))
            else {
                break;
            };
            if entry.kind != "folder" || ctx.db.file_repos().id().find(entry.id.clone()).is_some() {
                return Ok(Err(format!(
                    "“{path}” would go inside a file or a folder with its own history"
                )));
            }
            parent_id = entry.id;
        }
    }
    Ok(Ok(Plan { head, steps }))
}

/// Records main's new commits (or ones Git branches already had) and brings the live folder to the last one.
#[allow(clippy::too_many_arguments)]
fn apply_main(
    ctx: &ReducerContext,
    files: &HistoryFiles,
    repo: &FileRepo,
    root: &FileEntry,
    user_id: &str,
    plan: &Plan,
    chain: &[ChainItem],
    commits: &HashMap<String, PushCommit>,
    changeset_of: &HashMap<String, String>,
    new_version: &dyn Fn(&str, &str, &str) -> Res<String>,
) -> Res<()> {
    let at = now(ctx);
    let mut tree: BTreeMap<String, Slot> = plan
        .head
        .iter()
        .map(|(p, i)| {
            (
                p.clone(),
                Slot {
                    file_id: i.file_id.clone(),
                    version_id: i.version_id.clone(),
                    blob: String::new(),
                    mode: i.mode.clone(),
                },
            )
        })
        .collect();
    let mut head_id = repo.head_id.clone();
    let mut sequence = repo.sequence;
    for (commit_index, item) in chain.iter().enumerate() {
        let commit = commits.get(&item.sha);
        let changeset_id = match commit {
            Some(_) => changeset_of[&item.sha].clone(),
            None => {
                object_of(ctx, repo, &item.sha)
                    .expect("object")
                    .changeset_id
            }
        };
        let existing = if commit.is_some() {
            None
        } else {
            ctx.db.file_changesets().id().find(changeset_id.clone())
        };
        // A commit Git branches already had joins main: its changes are now against main's previous commit.
        if existing.is_some() {
            for r in ctx
                .db
                .file_changes()
                .changeset_id()
                .filter(changeset_id.as_str())
                .collect::<Vec<_>>()
            {
                ctx.db.file_changes().id().delete(r.id);
            }
        }
        let before: Vec<Option<Slot>> = item
            .changes
            .iter()
            .map(|c| {
                if c.kind == "added" {
                    None
                } else {
                    tree.get(if c.kind == "renamed" {
                        &c.previous_path
                    } else {
                        &c.path
                    })
                    .cloned()
                }
            })
            .collect();
        for c in &item.changes {
            if c.kind == "deleted" || c.kind == "renamed" {
                tree.remove(if c.kind == "renamed" {
                    &c.previous_path
                } else {
                    &c.path
                });
            }
        }
        for (index, change) in item.changes.iter().enumerate() {
            let step = &plan.steps[commit_index][index];
            let previous = &before[index];
            let mut version_id = String::new();
            if change.kind != "deleted" {
                version_id = match previous {
                    Some(prev) if change.blob == change.previous_blob => prev.version_id.clone(),
                    _ => new_version(&step.file_id, &change.blob, &change.media_type)?,
                };
                tree.insert(
                    change.path.clone(),
                    Slot {
                        file_id: step.file_id.clone(),
                        version_id: version_id.clone(),
                        blob: change.blob.clone(),
                        mode: change.mode.clone(),
                    },
                );
            }
            ctx.db.file_changes().insert(FileChange {
                id: format!("{changeset_id}/{index}"),
                changeset_id: changeset_id.clone(),
                path: change.path.clone(),
                previous_path: change.previous_path.clone(),
                kind: change.kind.clone(),
                file_id: step.file_id.clone(),
                version_id,
                previous_version_id: previous
                    .as_ref()
                    .map(|p| p.version_id.clone())
                    .unwrap_or_default(),
                expect_revision: 0,
                mode: change.mode.clone(),
                previous_mode: change.previous_mode.clone(),
            });
        }
        sequence += 1;
        let parents = match commit {
            Some(c) => c.parents.len(),
            None => object_of(ctx, repo, &item.sha).map_or(0, |o| {
                o.parents.split(' ').filter(|x| !x.is_empty()).count()
            }),
        };
        let next = FileChangeset {
            id: changeset_id.clone(),
            repo_id: repo.id.clone(),
            branch: String::new(),
            parent_id: head_id.clone(),
            sequence,
            kind: if parents > 1 {
                "merge".into()
            } else {
                "commit".into()
            },
            message: commit
                .map(|c| c.message.clone())
                .or_else(|| existing.as_ref().map(|e| e.message.clone()))
                .unwrap_or_default(),
            author_id: existing
                .as_ref()
                .map_or(user_id.to_owned(), |e| e.author_id.clone()),
            file_count: item.changes.len() as u32,
            created_at: commit
                .map(|c| c.time)
                .or_else(|| existing.as_ref().map(|e| e.created_at))
                .unwrap_or(at),
        };
        if existing.is_some() {
            ctx.db.file_changesets().id().update(next);
        } else {
            ctx.db.file_changesets().insert(next);
        }
        head_id = changeset_id;
    }
    // The live folder: trash what main no longer has, move renamed files, then set content and add files.
    let live = scan(ctx, root)?;
    let path_of: HashMap<String, String> = tree
        .iter()
        .map(|(p, s)| (s.file_id.clone(), p.clone()))
        .collect();
    for (path, old) in &plan.head {
        if !path_of.contains_key(&old.file_id)
            && let Some(entry) = live.get(path)
        {
            history::trash(ctx, files, entry);
        }
    }
    let moved: Vec<(String, TreeItem)> = plan
        .head
        .iter()
        .filter(|(path, old)| path_of.get(&old.file_id).is_some_and(|p| p != *path))
        .map(|(p, o)| (p.clone(), o.clone()))
        .collect();
    // Moves free their old names first, so files can swap places.
    for (_, old) in &moved {
        if let Some(entry) = ctx.db.file_entries().id().find(old.file_id.clone()) {
            let list_key = format!("{}/moving/{}", entry.owner_id, entry.id);
            ctx.db
                .file_entries()
                .id()
                .update(FileEntry { list_key, ..entry });
        }
    }
    for (_, old) in &moved {
        let entry = ctx
            .db
            .file_entries()
            .id()
            .find(old.file_id.clone())
            .expect("entry");
        history::move_file(ctx, files, root, &entry, &path_of[&old.file_id])?;
    }
    // Only content the push changed: other files keep any uncommitted edits made in Dar.
    let head_version: HashMap<String, String> = plan
        .head
        .values()
        .map(|i| (i.file_id.clone(), i.version_id.clone()))
        .collect();
    for (path, slot) in &tree {
        if head_version.get(&slot.file_id) == Some(&slot.version_id) {
            continue;
        }
        let version = ctx
            .db
            .versions()
            .id()
            .find(slot.version_id.clone())
            .expect("version");
        match ctx.db.file_entries().id().find(slot.file_id.clone()) {
            Some(entry) => {
                if entry.blob_id != version.blob_id {
                    history::set_content(ctx, files, &entry, &version, true)?;
                }
            }
            None => {
                history::create_file(ctx, files, root, path, &version, false)?;
            }
        }
    }
    for r in ctx
        .db
        .file_trees()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        if !tree.contains_key(&r.path) {
            ctx.db.file_trees().id().delete(r.id);
        }
    }
    for (path, slot) in &tree {
        let next = FileTree {
            id: format!("{}/{path}", repo.id),
            repo_id: repo.id.clone(),
            path: path.clone(),
            file_id: slot.file_id.clone(),
            version_id: slot.version_id.clone(),
            mode: slot.mode.clone(),
        };
        if ctx.db.file_trees().id().find(next.id.clone()).is_some() {
            ctx.db.file_trees().id().update(next);
        } else {
            ctx.db.file_trees().insert(next);
        }
    }
    let latest = ctx
        .db
        .file_repos()
        .id()
        .find(repo.id.clone())
        .expect("repo");
    ctx.db.file_repos().id().update(FileRepo {
        head_id,
        sequence,
        modified_at: at,
        ..latest
    });
    Ok(())
}

/// Drops commits and tags no ref reaches any more, and the content only they kept. main's commits always stay.
fn collect(ctx: &ReducerContext, repo: &FileRepo) {
    let objects: HashMap<String, FileGitObject> = ctx
        .db
        .file_git_objects()
        .repo_id()
        .filter(repo.id.as_str())
        .map(|r| (r.sha.clone(), r))
        .collect();
    let mut queue: Vec<String> = ctx
        .db
        .file_git_refs()
        .repo_id()
        .filter(repo.id.as_str())
        .map(|r| r.target)
        .collect();
    for r in objects.values() {
        if r.r#type == "commit"
            && ctx
                .db
                .file_changesets()
                .id()
                .find(r.changeset_id.clone())
                .is_none_or(|s| s.branch.is_empty())
        {
            queue.push(r.sha.clone());
        }
    }
    let mut seen: HashSet<String> = queue.iter().cloned().collect();
    let mut i = 0;
    while i < queue.len() {
        let value = queue[i].clone();
        i += 1;
        if let Some(o) = objects.get(&value) {
            for parent in o.parents.split(' ').filter(|x| !x.is_empty()) {
                if seen.insert(parent.to_owned()) {
                    queue.push(parent.to_owned());
                }
            }
        }
    }
    for r in objects.values() {
        if seen.contains(&r.sha) {
            continue;
        }
        if r.r#type == "commit" {
            let set = ctx.db.file_changesets().id().find(r.changeset_id.clone());
            let Some(set) = set.filter(|s| s.branch == "git") else {
                continue;
            };
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
        ctx.db.file_git_objects().id().delete(r.id.clone());
    }
    let mut bytes = 0.0;
    for v in ctx
        .db
        .versions()
        .file_id()
        .filter(pseudo_file(&repo.id).as_str())
        .collect::<Vec<_>>()
    {
        if !referenced(ctx, &v.id) {
            bytes += v.size;
            ctx.db.versions().id().delete(v.id.clone());
            release_blob(ctx, &v.blob_id);
        }
    }
    release(ctx, &repo.owner_id, bytes);
    for r in ctx
        .db
        .file_git_blobs()
        .repo_id()
        .filter(repo.id.as_str())
        .collect::<Vec<_>>()
    {
        if ctx
            .db
            .versions()
            .blob_id()
            .filter(r.blob_id.as_str())
            .next()
            .is_none()
        {
            ctx.db.file_git_blobs().id().delete(r.id);
        }
    }
}
