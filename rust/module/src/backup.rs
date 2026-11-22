//! The rest of `index.ts`: Git and history entry points, maintenance (expired transfers, blob
//! collection, checksums, the blob migration), backups, exports and restores, sessions, the
//! personal profile, identity administration and connected applications.

use std::collections::{HashMap, HashSet};
use std::ops::Bound;

use serde_json::{Map, Value, json};
use spacetimedb::{ProcedureContext, ReducerContext, Table};

use crate::blobs::{check_key_at, is_blob_id, release_blob};
use crate::files::{Unlock, access};
use crate::history::HistoryFiles;
use crate::identity::{Actor, directory_operation, identity_actor, profile, provider_operation, snapshot_rows};
use crate::tables::*;
use crate::{Res, fail, from_session, now, run_json, service};

type Doc = Map<String, Value>;

const BLOB_MIGRATION: u32 = 1;
const FORMATS: [&str; 12] = [
    "dar-backup-v1", "dar-backup-v2", "dar-backup-v3", "dar-backup-v4", "dar-backup-v5", "dar-backup-v6", "dar-backup-v7", "dar-backup-v8", "dar-backup-v9", "dar-backup-v10", "dar-backup-v11", "dar-backup-v12",
];

fn object(text: &str, max: usize) -> Res<Doc> {
    if text.encode_utf16().count() > max {
        return fail("operationTooLarge");
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => fail("invalidInput"),
    }
}

fn parse(text: &str) -> Res<Value> {
    serde_json::from_str(text).map_err(|_| "invalidInput".to_owned())
}

pub fn is_legacy_blob_id(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { matches!(c, b'a'..=b'f' | b'0'..=b'9') })
}

fn hex(value: Option<&Value>, len: usize) -> Option<String> {
    value.and_then(Value::as_str).filter(|s| s.len() == len && s.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9'))).map(str::to_owned)
}

fn browser_only(tx: &ReducerContext, session_id: &str) -> Res<Account> {
    let user = from_session(tx, session_id)?;
    if tx.db.app_passwords().session_id().find(session_id.to_owned()).is_some() {
        return fail("forbidden");
    }
    Ok(user)
}

/// Folder history (docs/spacetimedb/folder-history.md) for browser sessions; app passwords have no history scope.
#[spacetimedb::procedure]
pub fn service_history(ctx: &mut ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = browser_only(tx, &sessionId)?;
        let p = object(&document, 1024 * 1024)?;
        let files = HistoryFiles { user_id: &user.id, unlock: Unlock::Session(&sessionId) };
        crate::history::history_operation(tx, &files, &user.id, &format!("history:{sessionId}"), &operation, &p)
    })
}

/// Git over HTTPS for tracked folders: app passwords with Files access (read to clone and fetch,
/// write to push) and browser sessions (bundle downloads).
#[spacetimedb::procedure]
pub fn service_git(ctx: &mut ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &sessionId)?;
        let password = tx.db.app_passwords().session_id().find(sessionId.clone());
        let write = operation == "pushBegin" || operation == "push";
        if password.is_some_and(|p| p.revoked || p.expires_at <= now(tx) || !crate::scopes::password_allows(&p.scope, "files", if write { "write" } else { "read" })) {
            return fail("forbidden");
        }
        let p = object(&document, 64 * 1024 * 1024)?;
        let files = HistoryFiles { user_id: &user.id, unlock: Unlock::Session(&sessionId) };
        crate::git::git_operation(tx, &files, &user.id, &format!("git:{sessionId}"), &operation, &p)
    })
}

/// Issues and pull requests for browser sessions; app passwords have no access.
#[spacetimedb::procedure]
pub fn service_git_tracker(ctx: &mut ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = browser_only(tx, &sessionId)?;
        let p = object(&document, 256 * 1024)?;
        let files = HistoryFiles { user_id: &user.id, unlock: Unlock::Session(&sessionId) };
        crate::tracker::tracker_operation(tx, &files, &user.id, &operation, &p)
    })
}

/// Tracked folders, so the runtime can drop Git caches of folders that stopped keeping history.
#[spacetimedb::procedure]
pub fn service_git_repos(ctx: &mut ProcedureContext) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        Ok(Value::Array(tx.db.file_repos().iter().map(|r| json!({ "id": r.id, "generation": r.created_at })).collect()))
    })
}

/// Folders the caller may move or copy into, by name (the folder chooser).
#[spacetimedb::procedure]
pub fn folder_choices(ctx: &mut ProcedureContext, parentId: String, query: Option<String>, cursor: Option<String>) -> String {
    crate::read_result(ctx, |tx| {
        use unicode_normalization::UnicodeNormalization;
        let user = crate::active_user(tx)?;
        let owner_id = crate::files::destination(tx, &parentId, &user.id, Unlock::Caller)?;
        if owner_id != user.id {
            return fail("forbidden");
        }
        if query.as_ref().map_or(0, |q| q.encode_utf16().count()) > 100 || cursor.as_ref().map_or(0, |c| c.encode_utf16().count()) > 1024 {
            return fail("invalidInput");
        }
        let prefix = format!("{owner_id}/{parentId}/{}", query.as_deref().map(|q| q.nfc().collect::<String>().to_lowercase()).unwrap_or_default());
        let cursor = cursor.as_deref().filter(|c| c.starts_with(&prefix));
        let range = crate::window_of(&prefix, cursor);
        let mut children: Vec<Value> = Vec::new();
        for entry in tx.db.file_entries().list_key().filter(crate::bounds(&range)) {
            if entry.kind == "folder" && !entry.trashed {
                children.push(json!({ "id": entry.id, "name": entry.name, "listKey": entry.list_key }));
            }
            if children.len() >= 201 {
                break;
            }
        }
        let next = if children.len() > 200 { children[199]["listKey"].clone() } else { json!("") };
        children.truncate(200);
        Ok(crate::stringify(&json!({ "items": children, "nextCursor": next })))
    })
}

#[spacetimedb::procedure]
pub fn service_maintenance(ctx: &mut ProcedureContext) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let at = now(tx);
        // Transfers end an hour after they begin: unfinished ones give back their reserved quota.
        let mut expired = 0;
        let upper = format!("{:016}", at as i64);
        for upload in tx.db.uploads().expires_key().filter((Bound::Unbounded, Bound::Excluded(upper.as_str()))).collect::<Vec<_>>() {
            if upload.expires_at > at {
                continue;
            }
            expired += 1;
            if expired > 500 {
                break;
            }
            if upload.state == "uploading"
                && let Some(meter) = tx.db.usage().user_id().find(upload.owner_id.clone())
            {
                let reserved = (meter.reserved - upload.size).max(0.0);
                tx.db.usage().user_id().update(Usage { reserved, ..meter });
            }
            tx.db.uploads().id().delete(upload.id);
        }
        // Blobs due a check; claimed ones stay due until acknowledged.
        let mut candidates: Vec<String> = Vec::new();
        let (low, high) = (check_key_at(0.0), check_key_at(at));
        for blob in tx.db.file_blobs().check_key().filter((Bound::Included(low.as_str()), Bound::Included(high.as_str()))).collect::<Vec<_>>() {
            if candidates.len() >= 100 {
                break;
            }
            let used = tx.db.versions().blob_id().filter(blob.id.as_str()).next().is_some()
                || tx.db.backup_pins().blob_id().filter(blob.id.as_str()).next().is_some()
                || tx.db.chat_media().blob_id().filter(blob.id.as_str()).next().is_some();
            if used {
                tx.db.file_blobs().id().update(FileBlob { state: "stored".into(), check_key: String::new(), ..blob });
                continue;
            }
            let id = blob.id.clone();
            if blob.state != "deleting" {
                tx.db.file_blobs().id().update(FileBlob { state: "deleting".into(), ..blob });
            }
            candidates.push(id);
        }
        Ok(json!(candidates))
    })
}

/// The runtime removed a claimed blob; one an upload used again meanwhile is "stored" and stays.
#[spacetimedb::reducer]
pub fn service_acknowledge_gc(ctx: &ReducerContext, id: String) -> Res<()> {
    service(ctx)?;
    if ctx.db.file_blobs().id().find(id.clone()).is_some_and(|b| b.state == "deleting") {
        ctx.db.file_blobs().id().delete(id.clone());
        ctx.db.blob_checksums().id().delete(id.clone());
        ctx.db.blob_texts().id().delete(id);
    }
    Ok(())
}

/// Stored blobs without checksums, in ID order after `after`, at most `limit`.
#[spacetimedb::procedure]
pub fn service_blobs_without_checksum(ctx: &mut ProcedureContext, after: String, limit: u32) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if !(1..=500).contains(&limit) {
            return fail("invalidInput");
        }
        let mut ids: Vec<String> = tx.db.file_blobs().iter().filter(|b| b.state == "stored" && b.id > after && tx.db.blob_checksums().id().find(b.id.clone()).is_none()).map(|b| b.id).collect();
        ids.sort();
        ids.truncate(limit as usize);
        Ok(json!(ids))
    })
}

#[spacetimedb::reducer]
pub fn service_record_checksums(ctx: &ReducerContext, items: String) -> Res<()> {
    service(ctx)?;
    let items = parse(&items)?;
    let Some(items) = items.as_array().filter(|i| i.len() <= 500) else { return fail("invalidInput") };
    for item in items {
        let id = item.get("id").and_then(Value::as_str).filter(|id| is_blob_id(id));
        let (Some(id), Some(sha1), Some(md5)) = (id, hex(item.get("sha1"), 40), hex(item.get("md5"), 32)) else { return fail("invalidInput") };
        // A blob collected while it was hashed gets no row.
        if ctx.db.file_blobs().id().find(id.to_owned()).is_some() && ctx.db.blob_checksums().id().find(id.to_owned()).is_none() {
            ctx.db.blob_checksums().insert(BlobChecksum { id: id.into(), sha1, md5 });
        }
    }
    Ok(())
}

/// The subset of these blob IDs the database has no record of.
#[spacetimedb::procedure]
pub fn service_unknown_blobs(ctx: &mut ProcedureContext, ids: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let ids = parse(&ids)?;
        let Some(ids) = ids.as_array().filter(|i| i.len() <= 1000) else { return fail("invalidInput") };
        let mut out = Vec::new();
        for id in ids {
            let Some(id) = id.as_str().filter(|id| is_blob_id(id)) else { return fail("invalidInput") };
            if tx.db.file_blobs().id().find(id.to_owned()).is_none() {
                out.push(id.to_owned());
            }
        }
        Ok(json!(out))
    })
}

/// Content stored under upload IDs (before content addressing) that anything still references.
#[spacetimedb::procedure]
pub fn service_blob_migration_plan(ctx: &mut ProcedureContext) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if tx.db.file_migrations().id().find(BLOB_MIGRATION).is_some() {
            return Ok(json!({ "done": true, "blobs": [] }));
        }
        let mut order: Vec<String> = Vec::new();
        let mut blobs: HashMap<String, Value> = HashMap::new();
        let mut add = |id: &str, size: f64, required: bool| {
            if !is_legacy_blob_id(id) {
                return;
            }
            if let Some(known) = blobs.get_mut(id) {
                if required {
                    known["required"] = json!(true);
                }
                return;
            }
            let upload = tx.db.uploads().id().find(id.to_owned());
            order.push(id.to_owned());
            blobs.insert(id.to_owned(), json!({ "id": id, "size": upload.as_ref().map_or(size, |u| u.size), "digest": upload.map(|u| u.digest).unwrap_or_default(), "required": required }));
        };
        for row in tx.db.versions().iter() {
            add(&row.blob_id, row.size, true);
        }
        for row in tx.db.file_entries().iter() {
            if row.kind == "file" {
                add(&row.blob_id, row.size, true);
            }
        }
        for row in tx.db.file_git_blobs().iter() {
            add(&row.blob_id, row.size, false);
        }
        for row in tx.db.photo_assets().iter() {
            add(&row.blob_id, -1.0, false);
        }
        for row in tx.db.backup_pins().iter() {
            add(&row.blob_id, -1.0, false);
        }
        Ok(json!({ "done": false, "blobs": order.iter().map(|id| blobs[id].clone()).collect::<Vec<_>>() }))
    })
}

/// Switches every reference from upload IDs to digests in one transaction.
#[spacetimedb::reducer]
pub fn service_migrate_blobs(ctx: &ReducerContext, hashed: String) -> Res<()> {
    service(ctx)?;
    if ctx.db.file_migrations().id().find(BLOB_MIGRATION).is_some() {
        return Ok(());
    }
    let hashed = parse(&hashed)?;
    let sizes = std::cell::RefCell::new(HashMap::<String, f64>::new());
    let mapped = |id: &str| -> Option<String> {
        if !is_legacy_blob_id(id) {
            return Some(id.to_owned());
        }
        let upload = ctx.db.uploads().id().find(id.to_owned()).filter(|u| !u.digest.is_empty());
        let digest = upload.as_ref().map(|u| u.digest.clone()).or_else(|| hashed.get(id).and_then(|h| h.get("digest")).and_then(Value::as_str).map(str::to_owned));
        let size = match &upload {
            Some(u) => Some(u.size),
            None => hashed.get(id).and_then(|h| h.get("size")).and_then(Value::as_f64),
        };
        let digest = digest.filter(|d| is_blob_id(d))?;
        sizes.borrow_mut().insert(digest.clone(), size?);
        Some(digest)
    };
    let digest_of = |id: &str| mapped(id).ok_or_else(|| "contentUnavailable".to_owned());
    let mut used: Vec<String> = Vec::new();
    let mut use_id = |id: String| {
        if !used.contains(&id) {
            used.push(id);
        }
    };
    for row in ctx.db.versions().iter().collect::<Vec<_>>() {
        let blob_id = digest_of(&row.blob_id)?;
        use_id(blob_id.clone());
        sizes.borrow_mut().entry(blob_id.clone()).or_insert(row.size);
        if blob_id != row.blob_id {
            ctx.db.versions().id().update(Version { blob_id, ..row });
        }
    }
    for row in ctx.db.file_entries().iter().collect::<Vec<_>>() {
        if row.kind == "file" && is_legacy_blob_id(&row.blob_id) {
            let blob_id = digest_of(&row.blob_id)?;
            ctx.db.file_entries().id().update(FileEntry { blob_id, ..row });
        }
    }
    for row in ctx.db.backup_pins().iter().collect::<Vec<_>>() {
        if !is_legacy_blob_id(&row.blob_id) {
            use_id(row.blob_id.clone());
            continue;
        }
        match mapped(&row.blob_id) {
            Some(blob_id) => {
                use_id(blob_id.clone());
                ctx.db.backup_pins().id().update(BackupPin { blob_id, ..row });
            }
            None => {
                ctx.db.backup_pins().id().delete(row.id);
            }
        }
    }
    for row in ctx.db.file_git_blobs().iter().collect::<Vec<_>>() {
        if is_legacy_blob_id(&row.blob_id) {
            match mapped(&row.blob_id) {
                Some(blob_id) => {
                    ctx.db.file_git_blobs().id().update(FileGitBlob { blob_id, ..row });
                }
                None => {
                    ctx.db.file_git_blobs().id().delete(row.id);
                }
            }
        }
    }
    for row in ctx.db.photo_assets().iter().collect::<Vec<_>>() {
        if is_legacy_blob_id(&row.blob_id) {
            let blob_id = mapped(&row.blob_id).or_else(|| ctx.db.file_entries().id().find(row.file_id.clone()).map(|e| e.blob_id)).unwrap_or_default();
            ctx.db.photo_assets().file_id().update(PhotoAsset { blob_id, ..row });
        }
    }
    // Everything recorded here is in use; content nothing used is left behind under its upload ID.
    for id in used {
        let Some(size) = sizes.borrow().get(&id).copied() else { return fail("contentUnavailable") };
        if ctx.db.file_blobs().id().find(id.clone()).is_none() {
            ctx.db.file_blobs().insert(FileBlob { id, size, state: "stored".into(), check_key: String::new() });
        }
    }
    for upload in ctx.db.uploads().iter().collect::<Vec<_>>() {
        if upload.state != "uploading" {
            ctx.db.uploads().id().delete(upload.id);
        }
    }
    ctx.db.file_migrations().insert(FileMigration { id: BLOB_MIGRATION, applied_at: now(ctx) });
    Ok(())
}

fn rows<T: serde::Serialize>(items: impl Iterator<Item = T>) -> Value {
    Value::Array(items.map(|r| crate::row(&r)).collect())
}

#[spacetimedb::procedure]
pub fn service_backup(ctx: &mut ProcedureContext, id: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let history: Vec<Version> = tx.db.versions().iter().collect();
        let mut blob_ids: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for b in history.iter().map(|v| v.blob_id.clone()).chain(crate::chat::chat_blob_ids(tx)) {
            if seen.insert(b.clone()) {
                blob_ids.push(b);
            }
        }
        let mut blobs = Vec::new();
        for blob_id in blob_ids {
            let Some(blob) = tx.db.file_blobs().id().find(blob_id.clone()).filter(|b| b.state != "deleting") else { return fail("contentUnavailable") };
            tx.db.backup_pins().insert(BackupPin { id: format!("{id}/{blob_id}"), backup_id: id.clone(), blob_id: blob_id.clone() });
            let mut v = json!({ "id": blob_id, "size": blob.size, "digest": blob_id });
            if let Some(sums) = tx.db.blob_checksums().id().find(blob_id.clone()) {
                v["sha1"] = json!(sums.sha1);
                v["md5"] = json!(sums.md5);
            }
            blobs.push(v);
        }
        Ok(json!({
            "format": "dar-backup-v12",
            "chat": crate::chat::chat_snapshot(tx),
            "books": crate::books::books_snapshot(tx),
            "history": crate::history::history_snapshot(tx),
            "photos": crate::photos::photos_snapshot(tx),
            "news": crate::news::news_snapshot(tx),
            "agent": crate::agent::agent_snapshot(tx),
            "apps": crate::app_settings::app_snapshot(tx),
            "workspace": crate::workspace::workspace_snapshot(tx),
            "identity": crate::identity::identity_snapshot(tx),
            "pim": crate::pim::pim_snapshot(tx),
            "id": id,
            "createdAt": now(tx),
            "blobs": blobs,
            "accounts": rows(tx.db.accounts().iter()),
            "quotas": tx.db.usage().iter().filter(|r| r.quota != 0.0).map(|r| json!({ "userId": r.user_id, "quota": r.quota })).collect::<Vec<_>>(),
            "files": rows(tx.db.file_entries().iter()),
            "versions": rows(history.into_iter()),
            "grants": rows(tx.db.grants().iter()),
            "publicShares": rows(tx.db.public_shares().iter()),
            "favorites": rows(tx.db.favorites().iter()),
            "activity": rows(tx.db.activity().iter()),
        }))
    })
}

#[spacetimedb::reducer]
pub fn service_release_backup(ctx: &ReducerContext, id: String) -> Res<()> {
    service(ctx)?;
    for pin in ctx.db.backup_pins().backup_id().filter(id.as_str()).collect::<Vec<_>>() {
        ctx.db.backup_pins().id().delete(pin.id);
        release_blob(ctx, &pin.blob_id);
    }
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_export(ctx: &mut ProcedureContext, id: String, username: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let Some(user) = tx.db.accounts().username().find(username.to_lowercase()) else { return fail("notFound") };
        let files: Vec<FileEntry> = tx.db.file_entries().owner_id().filter(user.id.as_str()).filter(|e| access(tx, e, &user.id, Unlock::Trusted).is_some()).collect();
        let mut seen = HashSet::new();
        let mut blobs = Vec::new();
        for entry in files.iter().filter(|e| e.kind == "file") {
            if !seen.insert(entry.blob_id.clone()) {
                continue;
            }
            let Some(blob) = tx.db.file_blobs().id().find(entry.blob_id.clone()).filter(|b| b.state != "deleting") else { return fail("contentUnavailable") };
            tx.db.backup_pins().insert(BackupPin { id: format!("{id}/{}", entry.blob_id), backup_id: id.clone(), blob_id: entry.blob_id.clone() });
            blobs.push(json!({ "id": entry.blob_id, "size": blob.size, "digest": entry.blob_id }));
        }
        Ok(json!({ "username": user.username, "files": rows(files.into_iter()), "blobs": blobs }))
    })
}

fn list_of<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Res<Vec<T>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(_) => snapshot_rows(value, key),
    }
}

#[spacetimedb::reducer]
pub fn service_restore(ctx: &ReducerContext, snapshot: String) -> Res<()> {
    service(ctx)?;
    if ctx.db.accounts().count() != 0 || ctx.db.file_entries().count() != 0 {
        return fail("conflict");
    }
    let value = parse(&snapshot)?;
    let format = value.get("format").and_then(Value::as_str).unwrap_or_default();
    let version = FORMATS.iter().position(|f| *f == format).map_or(0, |i| i + 1);
    let has = |key: &str| value.get(key).is_some_and(|v| !v.is_null());
    let required = [(2, "pim"), (3, "identity"), (4, "workspace"), (5, "apps"), (6, "agent"), (7, "news"), (8, "photos"), (9, "history"), (10, "books"), (12, "chat")];
    if version == 0 || required.iter().any(|(v, key)| version >= *v && !has(key)) {
        return fail("invalidInput");
    }
    for row in snapshot_rows::<Account>(&value, "accounts")? {
        let user_id = row.id.clone();
        ctx.db.accounts().insert(row);
        ctx.db.usage().insert(Usage { user_id, used: 0.0, reserved: 0.0, quota: 0.0 });
    }
    for row in list_of::<Value>(&value, "quotas")? {
        let meter = ctx.db.usage().user_id().find(crate::text_of(row.get("userId")));
        let quota = row.get("quota").and_then(Value::as_f64).filter(|q| q.fract() == 0.0 && *q >= 0.0 && *q <= 9_007_199_254_740_991.0);
        let (Some(meter), Some(quota)) = (meter, quota) else { return fail("invalidInput") };
        ctx.db.usage().user_id().update(Usage { quota, ..meter });
    }
    let files: Vec<FileEntry> = snapshot_rows(&value, "files")?;
    for row in &files {
        if ctx.db.accounts().id().find(row.owner_id.clone()).is_none() || crate::name_of(&row.name).ok().as_ref() != Some(&row.name) || crate::files::find_key(ctx, &row.list_key).is_some() {
            return fail("invalidInput");
        }
        ctx.db.file_entries().insert(row.clone());
    }
    for row in &files {
        let mut parent_id = row.parent_id.clone();
        let mut depth = 0;
        while !parent_id.is_empty() {
            let parent = ctx.db.file_entries().id().find(parent_id.clone());
            let Some(parent) = parent.filter(|p| p.owner_id == row.owner_id && p.kind == "folder" && p.id != row.id && depth < 128) else { return fail("invalidInput") };
            parent_id = parent.parent_id;
            depth += 1;
        }
    }
    // Versions of files deleted for good (or staged for a branch) remain while history uses them.
    let kept = crate::history::history_version_owners(value.get("history"));
    for row in snapshot_rows::<Version>(&value, "versions")? {
        let entry = ctx.db.file_entries().id().find(row.file_id.clone());
        let owner_id = match &entry {
            Some(e) if e.kind == "file" => e.owner_id.clone(),
            Some(_) => return fail("invalidInput"),
            None => match kept.get(&row.id) {
                Some(o) => o.clone(),
                None => return fail("invalidInput"),
            },
        };
        let size = row.size;
        ctx.db.versions().insert(row);
        let Some(meter) = ctx.db.usage().user_id().find(owner_id) else { return fail("invalidInput") };
        let used = meter.used + size;
        ctx.db.usage().user_id().update(Usage { used, ..meter });
    }
    for row in snapshot_rows::<GrantRow>(&value, "grants")? {
        if ctx.db.file_entries().id().find(row.file_id.clone()).is_none() || ctx.db.accounts().id().find(row.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.grants().insert(row);
    }
    for row in snapshot_rows::<PublicShare>(&value, "publicShares")? {
        ctx.db.public_shares().insert(row);
    }
    for row in snapshot_rows::<Favorite>(&value, "favorites")? {
        ctx.db.favorites().insert(row);
    }
    // v11 names content by digest. Older backups name it by upload ID; their uploads carry each
    // digest, and the runtime's blob migration renames them on its next start.
    let blobs: Vec<Value> = list_of(&value, "blobs")?;
    let safe_size = |v: &Value| v.get("size").and_then(Value::as_f64).filter(|s| s.fract() == 0.0 && *s >= 0.0 && *s <= 9_007_199_254_740_991.0);
    if version >= 11 {
        for row in &blobs {
            let id = crate::text_of(row.get("id"));
            let size = safe_size(row);
            let Some(size) = size.filter(|_| Some(id.as_str()) == row.get("digest").and_then(Value::as_str) && is_blob_id(&id) && ctx.db.file_blobs().id().find(id.clone()).is_none()) else { return fail("invalidInput") };
            ctx.db.file_blobs().insert(FileBlob { id: id.clone(), size, state: "stored".into(), check_key: String::new() });
            if let (Some(sha1), Some(md5)) = (hex(row.get("sha1"), 40), hex(row.get("md5"), 32)) {
                ctx.db.blob_checksums().insert(BlobChecksum { id, sha1, md5 });
            }
        }
        if ctx.db.file_migrations().id().find(BLOB_MIGRATION).is_none() {
            ctx.db.file_migrations().insert(FileMigration { id: BLOB_MIGRATION, applied_at: now(ctx) });
        }
    } else {
        for row in list_of::<Value>(&value, "uploads")? {
            let mut row = row;
            row["state"] = json!("complete");
            if row.get("expiresKey").is_none() {
                row["expiresKey"] = json!("");
            }
            let upload: Upload = serde_json::from_value(row).map_err(|_| "invalidInput")?;
            ctx.db.uploads().insert(upload);
        }
        // Content already named by its digest needs no migration.
        for row in &blobs {
            let id = crate::text_of(row.get("id"));
            if let Some(size) = safe_size(row)
                && Some(id.as_str()) == row.get("digest").and_then(Value::as_str)
                && is_blob_id(&id)
                && ctx.db.file_blobs().id().find(id.clone()).is_none()
            {
                ctx.db.file_blobs().insert(FileBlob { id, size, state: "stored".into(), check_key: String::new() });
            }
        }
        ctx.db.file_migrations().id().delete(BLOB_MIGRATION);
    }
    for row in snapshot_rows::<Activity>(&value, "activity")? {
        ctx.db.activity().insert(row);
    }
    crate::history::restore_history(ctx, value.get("history"))?;
    crate::pim::restore_pim(ctx, value.get("pim"))?;
    crate::identity::restore_identity(ctx, value.get("identity"))?;
    crate::workspace::restore_workspace(ctx, value.get("workspace"))?;
    crate::app_settings::restore_apps(ctx, value.get("apps"))?;
    crate::agent::restore_agent(ctx, value.get("agent"))?;
    crate::news::restore_news(ctx, value.get("news"))?;
    crate::photos::restore_photos(ctx, value.get("photos"))?;
    crate::books::restore_books(ctx, value.get("books"))?;
    crate::chat::restore_chat(ctx, value.get("chat"))?;
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_sessions(ctx: &mut ProcedureContext, sessionId: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &sessionId)?;
        let at = now(tx);
        let items: Vec<Value> = tx
            .db
            .sessions()
            .user_id()
            .filter(user.id.as_str())
            .filter(|s| s.active && !s.digest.starts_with("protocol:") && s.expires_at > at)
            .map(|s| json!({ "id": s.id, "createdAt": s.created_at, "expiresAt": s.expires_at, "current": s.id == sessionId }))
            .collect();
        Ok(Value::Array(items))
    })
}

fn email_like(value: &str) -> bool {
    let bad = |c: char| c.is_whitespace() || c == '@';
    let Some((local, domain)) = value.split_once('@') else { return false };
    if local.is_empty() || local.chars().any(bad) || domain.chars().any(bad) {
        return false;
    }
    // [^\s@]+\.[^\s@]+ : some dot with text on both sides.
    domain.char_indices().any(|(i, c)| c == '.' && i > 0 && i + 1 < domain.len())
}

/// Personal profile writes cannot rename an account, grant roles or alter security.
#[spacetimedb::procedure]
pub fn service_profile(ctx: &mut ProcedureContext, sessionId: String, operation: String, payload: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = browser_only(tx, &sessionId)?;
        let old = profile(tx, &user.id).ok_or_else(|| "notFound".to_owned())?;
        if operation == "read" {
            return Ok(old);
        }
        if operation != "save" {
            return fail("invalidInput");
        }
        let input = parse(&payload)?;
        if input.get("revision") != old.get("revision") || input.get("revision").is_none() {
            return fail("conflict");
        }
        for key in ["displayName", "givenName", "familyName"] {
            match input.get(key).and_then(Value::as_str) {
                Some(s) if s.encode_utf16().count() <= 100 && !s.chars().any(|c| (c as u32) < 0x20) => {}
                _ => return fail("invalidInput"),
            }
        }
        let display_name = input["displayName"].as_str().unwrap_or_default().trim().to_owned();
        let email = input.get("email").and_then(Value::as_str);
        let Some(email) = email.filter(|e| !display_name.is_empty() && e.encode_utf16().count() <= 254 && (e.is_empty() || email_like(e))) else { return fail("invalidInput") };
        let mut attributes: Doc = tx.db.directory_profiles().user_id().find(user.id.clone()).and_then(|p| serde_json::from_str(&p.document).ok()).unwrap_or_default();
        let mut emails: Vec<Value> = attributes.get("emails").and_then(Value::as_array).cloned().unwrap_or_default();
        let position = emails.iter().position(|e| e.get("primary").and_then(Value::as_bool) == Some(true)).unwrap_or(0);
        if !email.is_empty() {
            let mut next = emails.get(position).cloned().filter(Value::is_object).unwrap_or(json!({}));
            next["value"] = json!(email);
            next["primary"] = json!(true);
            if emails.is_empty() {
                emails.push(next);
            } else {
                emails[position] = next;
            }
        } else if !emails.is_empty() {
            emails.remove(position);
        }
        if !emails.is_empty() && !emails.iter().any(|e| e.get("primary").and_then(Value::as_bool).unwrap_or(false)) {
            if !emails[0].is_object() {
                emails[0] = json!({});
            }
            emails[0]["primary"] = json!(true);
        }
        let mut name = attributes.get("name").cloned().filter(Value::is_object).unwrap_or(json!({}));
        name["givenName"] = input["givenName"].clone();
        name["familyName"] = input["familyName"].clone();
        attributes.insert("userName".into(), json!(user.username));
        attributes.insert("displayName".into(), json!(display_name));
        attributes.insert("active".into(), json!(user.enabled));
        attributes.insert("name".into(), name);
        attributes.insert("emails".into(), Value::Array(emails));
        let mut p = Doc::new();
        p.insert("id".into(), json!(user.id));
        p.insert("revision".into(), old["revision"].clone());
        p.insert("document".into(), json!(crate::stringify(&Value::Object(attributes))));
        directory_operation(tx, "saveUser", &p, &Actor { id: user.id.clone(), scim: false })
    })
}

const READ_IDENTITY: [&str; 8] = ["authorize", "users", "user", "groups", "group", "clients", "tokens", "storage"];

#[spacetimedb::procedure]
pub fn service_identity(ctx: &mut ProcedureContext, sessionId: String, bearer: String, proof: String, operation: String, payload: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let actor = identity_actor(tx, &sessionId, &bearer, &proof, !READ_IDENTITY.contains(&operation.as_str()))?;
        let Value::Object(p) = parse(&payload)? else { return fail("invalidInput") };
        directory_operation(tx, &operation, &p, &actor)
    })
}

#[spacetimedb::reducer]
pub fn service_identity_proof(ctx: &ReducerContext, sessionId: String, digest: String, expectedVerifier: String) -> Res<()> {
    service(ctx)?;
    let user = from_session(ctx, &sessionId)?;
    if !user.administrator || user.verifier != expectedVerifier {
        return fail("forbidden");
    }
    let at = now(ctx);
    for proof in ctx.db.identity_proofs().iter().collect::<Vec<_>>() {
        if proof.expires_at <= at {
            ctx.db.identity_proofs().digest().delete(proof.digest);
        }
    }
    if ctx.db.identity_proofs().count() >= 2000 {
        return fail("operationTooLarge");
    }
    ctx.db.identity_proofs().insert(IdentityProof { digest, session_id: sessionId, verifier: user.verifier, expires_at: at + 300_000.0 });
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_provider(ctx: &mut ProcedureContext, operation: String, payload: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let Value::Object(p) = parse(&payload)? else { return fail("invalidInput") };
        provider_operation(tx, &operation, &p)
    })
}

#[spacetimedb::procedure]
pub fn service_connected_applications(ctx: &mut ProcedureContext, sessionId: String, revoke: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &sessionId)?;
        let at = now(tx);
        let grants: Vec<ProviderModel> = tx.db.provider_models().account_id().filter(user.id.as_str()).filter(|m| m.model == "Grant" && m.expires_at > at).collect();
        if !revoke.is_empty() {
            let matching: Vec<&ProviderModel> = grants.iter().filter(|g| g.client_id == revoke).collect();
            if matching.is_empty() {
                return fail("notFound");
            }
            for grant in matching {
                let mut p = Doc::new();
                p.insert("id".into(), json!(grant.id));
                provider_operation(tx, "revokeGrant", &p)?;
            }
        }
        let mut ids: Vec<String> = Vec::new();
        for g in grants.iter().filter(|g| g.client_id != revoke) {
            if !ids.contains(&g.client_id) {
                ids.push(g.client_id.clone());
            }
        }
        Ok(Value::Array(
            ids.into_iter()
                .map(|id| {
                    let metadata: Value = tx.db.identity_clients().id().find(id.clone()).and_then(|c| serde_json::from_str(&c.metadata).ok()).unwrap_or(json!({}));
                    let name = metadata.get("client_name").cloned().filter(|n| !n.is_null()).or_else(|| (id == crate::photos::IMMICH_CLIENT_ID).then(|| json!("Immich")));
                    let mut v = json!({ "id": id, "clientId": id });
                    if let Some(name) = name {
                        v["name"] = name;
                    }
                    v
                })
                .collect(),
        ))
    })
}
