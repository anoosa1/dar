//! What browsers use directly in Files (`next/module/src/index.ts`): the `my_files` and
//! `my_usage` views and `browse`, favorites, sharing, public links, version restore and
//! `file_details`, plus the runtime's picker, Locked folder, search, preferences and
//! appearance procedures.

use serde_json::json;
use spacetimedb::{ProcedureContext, ReducerContext, ScheduleAt, Table, TimeDuration, ViewContext};
use unicode_normalization::UnicodeNormalization;

use crate::auth::caller;
use crate::files::{Unlock, access, check_revision, entry_json, event, find_key, fits_quota, is_locked_root, locked_root, meter, touch_folder, writable};
use crate::tables::*;
use crate::{Res, fail, from_session, opaque_id, run, service, now, unused_uuid, user_for_view, window_of};

/// How long one password confirmation keeps a browser session's Locked folder open.
pub const LOCKED_UNLOCK_MINUTES: f64 = 10.0;
const LOCKED_FOLDER_NAME: &str = "Locked folder";

fn range<'a>(lower: &'a std::ops::Bound<String>, upper: &'a std::ops::Bound<String>) -> (std::ops::Bound<&'a str>, std::ops::Bound<&'a str>) {
    (lower.as_ref().map(String::as_str), upper.as_ref().map(String::as_str))
}

fn preferences_of(value: Option<PimPreference>) -> serde_json::Value {
    value.and_then(|p| serde_json::from_str(&p.value).ok()).unwrap_or_else(|| json!({}))
}

// ---------- Views ----------

/// `access` for views, which have no clock: a Locked folder unlock counts while its row
/// exists (scheduled expiry removes it).
fn view_access(ctx: &ViewContext, entry: &FileEntry, user_id: &str) -> bool {
    let mut owner = entry.owner_id == user_id;
    let mut node = Some(entry.clone());
    for _ in 0..128 {
        let Some(n) = node else { return false };
        if n.trashed {
            return false;
        }
        if is_locked_root(&n) {
            let session = ctx.db.principals().identity().find(ctx.sender()).map(|p| p.session_id);
            let open = session.and_then(|s| ctx.db.file_unlocks().session_id().find(s)).is_some_and(|u| u.user_id == user_id);
            if n.owner_id != user_id || !open {
                return false;
            }
        }
        if !owner && ctx.db.grants().file_id().filter(n.id.as_str()).any(|g| g.user_id == user_id) {
            owner = true;
        }
        if n.parent_id.is_empty() {
            return owner;
        }
        node = ctx.db.file_entries().id().find(n.parent_id);
    }
    false
}

/// The folder, section page or prefix search the tab last browsed (at most 201 rows, so the
/// client can tell a next page exists).
#[spacetimedb::view(accessor = my_files, public)]
pub fn my_files(ctx: &ViewContext) -> Vec<FileEntry> {
    let Some(user) = user_for_view(ctx) else { return Vec::new() };
    let preferences = preferences_of(ctx.db.pim_preferences().user_id().find(user.id.clone()));
    let show_hidden = preferences["filesShowHidden"] == json!(true);
    let visible = |e: &FileEntry| show_hidden || !e.name.starts_with('.');
    let nav = ctx.db.navigation().session_id().find(ctx.sender().to_hex().to_string());
    let (parent_id, section, query, cursor) = match &nav {
        Some(n) => (n.parent_id.clone(), n.section.clone(), n.query.clone(), n.cursor.clone()),
        None => (String::new(), "files".to_owned(), String::new(), String::new()),
    };
    let mut result = Vec::new();
    if section == "favorites" || section == "shared" {
        let prefix = format!("{}/{query}", user.id);
        let (lower, upper) = window_of(&prefix, cursor.starts_with(&prefix).then_some(cursor.as_str()).filter(|c| !c.is_empty()));
        let rows: Vec<(String, String)> = if section == "favorites" {
            ctx.db.favorites().list_key().filter(range(&lower, &upper)).map(|f| (f.file_id, f.list_key)).collect()
        } else {
            ctx.db.grants().list_key().filter(range(&lower, &upper)).map(|g| (g.file_id, g.list_key)).collect()
        };
        for (file_id, list_key) in rows {
            if let Some(entry) = ctx.db.file_entries().id().find(file_id)
                && visible(&entry) && view_access(ctx, &entry, &user.id) {
                    let favorite = section == "favorites" && entry.favorite;
                    result.push(FileEntry { list_key, favorite, ..entry });
                }
            if result.len() >= 201 {
                break;
            }
        }
        return result;
    }
    let owner_id = if parent_id.is_empty() { Some(user.id.clone()) } else { ctx.db.file_entries().id().find(parent_id.clone()).map(|p| p.owner_id) };
    let Some(owner_id) = owner_id else { return result };
    let prefix = if section == "trash" { format!("{}/trash/{query}", user.id) } else { format!("{owner_id}/{parent_id}/{query}") };
    let (lower, upper) = window_of(&prefix, (!cursor.is_empty() && cursor.starts_with(&prefix)).then_some(cursor.as_str()));
    for entry in ctx.db.file_entries().list_key().filter(range(&lower, &upper)) {
        if !visible(&entry) || section != "trash" && !view_access(ctx, &entry, &user.id) {
            continue;
        }
        if !query.is_empty() && !entry.name.to_lowercase().contains(&query) {
            continue;
        }
        result.push(entry);
        if result.len() >= 201 {
            break;
        }
    }
    result
}

#[spacetimedb::view(accessor = my_usage, public)]
pub fn my_usage(ctx: &ViewContext) -> Option<Usage> {
    let user = user_for_view(ctx)?;
    ctx.db.usage().user_id().find(user.id)
}

// ---------- Reducers browsers call ----------

#[spacetimedb::reducer]
pub fn browse(ctx: &ReducerContext, parentId: String, section: String, query: String, cursor: String) -> Res<()> {
    let parent_id = parentId;
    let user = caller(ctx)?;
    if !["files", "favorites", "shared", "trash"].contains(&section.as_str()) || query.chars().count() > 100 || cursor.len() > 1024 {
        return fail("invalidInput");
    }
    if !parent_id.is_empty() {
        let parent = ctx.db.file_entries().id().find(parent_id.clone());
        if !parent.is_some_and(|p| p.kind == "folder" && access(ctx, &p, &user.id, Unlock::Caller).is_some()) {
            return fail("notFound");
        }
    }
    let session_id = ctx.sender().to_hex().to_string();
    let row = Navigation { session_id: session_id.clone(), parent_id, section, query: query.nfc().collect::<String>().to_lowercase(), cursor };
    if ctx.db.navigation().session_id().find(session_id).is_some() {
        ctx.db.navigation().session_id().update(row);
    } else {
        ctx.db.navigation().insert(row);
    }
    Ok(())
}

fn favorite_key(entry: &FileEntry) -> String {
    format!("{}/{}/{}", entry.owner_id, entry.name.to_lowercase(), entry.id)
}

#[spacetimedb::reducer]
pub fn favorite_entry(ctx: &ReducerContext, id: String, favorite: bool, expectedRevision: u32) -> Res<()> {
    let expected_revision = expectedRevision;
    let user = caller(ctx)?;
    let entry = writable(ctx, &id, &user.id, Unlock::Caller)?;
    if entry.owner_id != user.id || is_locked_root(&entry) {
        return fail("forbidden");
    }
    check_revision(&entry, expected_revision)?;
    ctx.db.file_entries().id().update(FileEntry { favorite, revision: entry.revision + 1, ..entry.clone() });
    if favorite && ctx.db.favorites().id().find(entry.id.clone()).is_none() {
        ctx.db.favorites().insert(Favorite { id: entry.id.clone(), file_id: entry.id.clone(), list_key: favorite_key(&entry) });
    }
    if !favorite {
        ctx.db.favorites().id().delete(entry.id.clone());
    }
    touch_folder(ctx, &entry.owner_id, &entry.parent_id);
    Ok(())
}

#[spacetimedb::reducer]
pub fn grant_access(ctx: &ReducerContext, id: String, fileId: String, username: String, role: String, expectedRevision: u32) -> Res<()> {
    let (file_id, expected_revision) = (fileId, expectedRevision);
    opaque_id(&id)?;
    let user = caller(ctx)?;
    let entry = writable(ctx, &file_id, &user.id, Unlock::Caller)?;
    if entry.owner_id != user.id {
        return fail("forbidden");
    }
    if locked_root(ctx, &entry).is_some() {
        return fail("lockedFolder");
    }
    check_revision(&entry, expected_revision)?;
    let recipient = ctx.db.accounts().username().find(username.to_lowercase());
    let Some(recipient) = recipient.filter(|r| r.enabled && r.id != user.id && (role == "read" || role == "write")) else {
        return fail("invalidInput");
    };
    for grant in ctx.db.grants().file_id().filter(entry.id.as_str()).filter(|g| g.user_id == recipient.id).collect::<Vec<_>>() {
        ctx.db.grants().id().delete(grant.id);
    }
    let list_key = format!("{}/{}/{}", recipient.id, entry.name.to_lowercase(), id);
    ctx.db.grants().insert(GrantRow { id, file_id: entry.id.clone(), user_id: recipient.id, role, list_key });
    ctx.db.file_entries().id().update(FileEntry { revision: entry.revision + 1, ..entry });
    Ok(())
}

#[spacetimedb::reducer]
pub fn revoke_grant(ctx: &ReducerContext, id: String) -> Res<()> {
    let user = caller(ctx)?;
    let grant = ctx.db.grants().id().find(id.clone()).ok_or("notFound")?;
    if ctx.db.file_entries().id().find(grant.file_id).map(|e| e.owner_id) != Some(user.id) {
        return fail("forbidden");
    }
    ctx.db.grants().id().delete(id);
    Ok(())
}

#[spacetimedb::reducer]
pub fn restore_version(ctx: &ReducerContext, id: String, versionId: String, expectedRevision: u32) -> Res<()> {
    let (version_id, expected_revision) = (versionId, expectedRevision);
    let user = caller(ctx)?;
    let entry = writable(ctx, &id, &user.id, Unlock::Caller)?;
    check_revision(&entry, expected_revision)?;
    let version = ctx.db.versions().id().find(version_id).filter(|v| v.file_id == entry.id).ok_or("notFound")?;
    let usage = meter(ctx, &entry.owner_id);
    if !fits_quota(&usage, version.size) {
        return fail("quotaExceeded");
    }
    ctx.db.usage().user_id().update(Usage { used: usage.used + version.size, ..usage });
    let copy = Version { id: unused_uuid(ctx), file_id: version.file_id.clone(), blob_id: version.blob_id.clone(), size: version.size, media_type: version.media_type.clone(), created_at: now(ctx) };
    ctx.db.versions().insert(copy);
    ctx.db.file_entries().id().update(FileEntry { blob_id: version.blob_id, size: version.size, media_type: version.media_type, revision: entry.revision + 1, modified_at: now(ctx), ..entry.clone() });
    event(ctx, &entry.owner_id, &entry.id, "versionRestored", None);
    Ok(())
}

#[spacetimedb::reducer]
pub fn create_public_share(ctx: &ReducerContext, id: String, fileId: String, digest: String, expiresAt: f64, expectedRevision: u32) -> Res<()> {
    let (file_id, expires_at, expected_revision) = (fileId, expiresAt, expectedRevision);
    opaque_id(&id)?;
    let user = caller(ctx)?;
    let entry = writable(ctx, &file_id, &user.id, Unlock::Caller)?;
    if entry.owner_id != user.id || entry.kind != "file" {
        return fail("forbidden");
    }
    if locked_root(ctx, &entry).is_some() {
        return fail("lockedFolder");
    }
    check_revision(&entry, expected_revision)?;
    let at = now(ctx);
    if !crate::blobs::is_blob_id(&digest) || !expires_at.is_finite() || expires_at <= at || expires_at > at + 30.0 * 86_400_000.0 {
        return fail("invalidInput");
    }
    ctx.db.public_shares().insert(PublicShare { id, file_id, digest, expires_at });
    Ok(())
}

#[spacetimedb::reducer]
pub fn revoke_public_share(ctx: &ReducerContext, id: String) -> Res<()> {
    let user = caller(ctx)?;
    let share = ctx.db.public_shares().id().find(id.clone()).ok_or("forbidden")?;
    if ctx.db.file_entries().id().find(share.file_id).map(|e| e.owner_id) != Some(user.id) {
        return fail("forbidden");
    }
    ctx.db.public_shares().id().delete(id);
    Ok(())
}

// ---------- Procedures ----------

/// An entry with its role, ancestors, versions, grants and public links.
#[spacetimedb::procedure]
pub fn file_details(ctx: &mut ProcedureContext, id: String) -> String {
    crate::read_result(ctx, |tx| {
        let user = caller(tx)?;
        let entry = tx.db.file_entries().id().find(id.clone()).ok_or("notFound")?;
        let role = if entry.owner_id == user.id { Some(crate::files::Role::Owner) } else { access(tx, &entry, &user.id, Unlock::Caller) };
        if !(entry.owner_id == user.id && entry.trashed) && access(tx, &entry, &user.id, Unlock::Caller).is_none() {
            return fail("notFound");
        }
        let role = match role {
            Some(crate::files::Role::Owner) => "owner",
            Some(crate::files::Role::Write) => "write",
            _ => "read",
        };
        let mut versions: Vec<Version> = tx.db.versions().file_id().filter(id.as_str()).collect();
        versions.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        let versions: Vec<_> = versions.iter().map(|v| json!({ "id": v.id, "fileId": v.file_id, "blobId": v.blob_id, "size": v.size, "mediaType": v.media_type, "createdAt": v.created_at })).collect();
        let owner = role == "owner";
        let grants: Vec<_> = if owner {
            tx.db.grants().file_id().filter(id.as_str()).map(|g| json!({ "id": g.id, "username": tx.db.accounts().id().find(g.user_id.clone()).map(|a| a.username), "role": g.role })).collect()
        } else {
            Vec::new()
        };
        let mut ancestors = Vec::new();
        let mut parent_id = entry.parent_id.clone();
        for _ in 0..128 {
            if parent_id.is_empty() {
                break;
            }
            let Some(parent) = tx.db.file_entries().id().find(parent_id.clone()).filter(|p| access(tx, p, &user.id, Unlock::Caller).is_some()) else { break };
            ancestors.insert(0, json!({ "id": parent.id, "name": parent.name }));
            parent_id = parent.parent_id;
        }
        let shares: Vec<_> = if owner { tx.db.public_shares().file_id().filter(id.as_str()).map(|s| json!({ "id": s.id, "expiresAt": s.expires_at })).collect() } else { Vec::new() };
        let mut entry_value = entry_json(&entry);
        entry_value["listKey"] = json!(entry.list_key);
        Ok(json!({ "entry": entry_value, "role": role, "ancestors": ancestors, "versions": versions, "grants": grants, "publicShares": shares }).to_string())
    })
}

/// Rejects app passwords: these operations are for browser sessions only.
fn browser_user(tx: &ReducerContext, session_id: &str) -> Res<Account> {
    service(tx)?;
    let user = from_session(tx, session_id)?;
    if tx.db.app_passwords().session_id().find(session_id.to_owned()).is_some() {
        return fail("forbidden");
    }
    Ok(user)
}

/// Bounded browsing for the shared file and folder picker (`file-picker.ts`).
#[spacetimedb::procedure]
pub fn service_file_picker(ctx: &mut ProcedureContext, sessionId: String, parentId: String, cursor: String, kind: String) -> String {
    let (session_id, parent_id) = (sessionId, parentId);
    run(ctx, |tx| {
        let user = browser_user(tx, &session_id)?;
        let can_read = |row: &FileEntry| access(tx, row, &user.id, Unlock::Session(&session_id)).is_some();
        if !["file", "folder"].contains(&kind.as_str()) || cursor.len() > 2048 || !parent_id.is_empty() && opaque_id(&parent_id).is_err() {
            return fail("invalidInput");
        }
        let parent = if parent_id.is_empty() { None } else { tx.db.file_entries().id().find(parent_id.clone()) };
        if !parent_id.is_empty() && !parent.as_ref().is_some_and(|p| p.kind == "folder" && can_read(p)) {
            return fail("notFound");
        }
        let prefix = format!("{}/{parent_id}/", parent.as_ref().map(|p| p.owner_id.clone()).unwrap_or_else(|| user.id.clone()));
        if !cursor.is_empty() && !cursor.starts_with(&prefix) {
            return fail("invalidInput");
        }
        let (lower, upper) = window_of(&prefix, (!cursor.is_empty()).then_some(cursor.as_str()));
        let (mut items, mut scanned, mut next_cursor) = (Vec::new(), 0, None);
        for row in tx.db.file_entries().list_key().filter(range(&lower, &upper)) {
            if (row.kind == "folder" || kind == "file") && can_read(&row) {
                items.push(json!({ "id": row.id, "name": row.name, "kind": row.kind }));
            }
            scanned += 1;
            if scanned >= 200 {
                next_cursor = Some(row.list_key);
                break;
            }
        }
        let mut ancestors = Vec::new();
        let mut id = parent.as_ref().map(|p| p.parent_id.clone()).unwrap_or_default();
        for _ in 0..128 {
            if id.is_empty() {
                break;
            }
            let Some(row) = tx.db.file_entries().id().find(id.clone()).filter(|r| can_read(r)) else { break };
            ancestors.insert(0, json!({ "id": row.id, "name": row.name }));
            id = row.parent_id;
        }
        let folder = parent.map(|p| json!({ "id": p.id, "name": p.name, "kind": "folder", "parentId": p.parent_id }));
        Ok(json!({ "folder": folder, "ancestors": ancestors, "items": items, "nextCursor": next_cursor }).to_string())
    })
}

/// The account's Locked folder, created on first use outside the folder tree.
fn locked_folder_for(tx: &ReducerContext, user_id: &str) -> String {
    if let Some(existing) = find_key(tx, &format!("{user_id}/locked")) {
        return existing.id;
    }
    let id = unused_uuid(tx);
    tx.db.file_entries().insert(FileEntry {
        id: id.clone(), owner_id: user_id.into(), parent_id: String::new(), name: LOCKED_FOLDER_NAME.into(), kind: "folder".into(), blob_id: String::new(),
        size: 0.0, media_type: crate::files::LOCKED_FOLDER_TYPE.into(), revision: 1, modified_at: now(tx), trashed: false, favorite: false, list_key: format!("{user_id}/locked"),
    });
    id
}

#[spacetimedb::reducer]
pub fn expire_file_unlock(ctx: &ReducerContext, expiry: FileUnlockExpiry) -> Res<()> {
    if !ctx.sender_auth().is_internal() {
        return fail("forbidden");
    }
    if let Some(row) = ctx.db.file_unlocks().session_id().find(expiry.session_id)
        && row.expires_at <= now(ctx) {
            ctx.db.file_unlocks().session_id().delete(row.session_id);
        }
    Ok(())
}

/// Locked folder state for a browser session. The runtime verifies the password and passes
/// the verifier it checked, so a password changed meanwhile is rejected.
#[spacetimedb::procedure]
pub fn service_locked_folder(ctx: &mut ProcedureContext, sessionId: String, operation: String, verifier: String) -> String {
    let session_id = sessionId;
    run(ctx, |tx| {
        let user = browser_user(tx, &session_id)?;
        let current = tx.db.file_unlocks().session_id().find(session_id.clone());
        match operation.as_str() {
            "unlock" => {
                if verifier.is_empty() || user.verifier != verifier {
                    return fail("conflict");
                }
                locked_folder_for(tx, &user.id);
                let row = FileUnlock { session_id: session_id.clone(), user_id: user.id.clone(), expires_at: now(tx) + LOCKED_UNLOCK_MINUTES * 60_000.0 };
                if current.is_some() {
                    tx.db.file_unlocks().session_id().update(row);
                } else {
                    tx.db.file_unlocks().insert(row);
                }
                let at = tx.timestamp + TimeDuration::from_micros((LOCKED_UNLOCK_MINUTES * 60_000_000.0) as i64);
                tx.db.file_unlock_expiry().insert(FileUnlockExpiry { scheduled_id: 0, scheduled_at: ScheduleAt::Time(at), session_id: session_id.clone() });
            }
            "lock" => {
                if current.is_some() {
                    tx.db.file_unlocks().session_id().delete(session_id.clone());
                }
            }
            "state" => {}
            _ => return fail("invalidInput"),
        }
        let row = tx.db.file_unlocks().session_id().find(session_id.clone());
        let folder_id = find_key(tx, &format!("{}/locked", user.id)).map(|e| e.id).unwrap_or_default();
        let until = row.filter(|r| r.user_id == user.id && r.expires_at > now(tx)).map(|r| r.expires_at).unwrap_or(0.0);
        Ok(json!({ "folderId": folder_id, "unlockedUntil": until }).to_string())
    })
}

/// The instance logo; administrators change it with `value` at `revision`.
#[spacetimedb::procedure]
pub fn service_appearance(ctx: &mut ProcedureContext, sessionId: String, revision: u32, value: String) -> String {
    let session_id = sessionId;
    run(ctx, |tx| {
        service(tx)?;
        let key = "instance:appearance".to_owned();
        let old = tx.db.pim_preferences().user_id().find(key.clone());
        if !value.is_empty() {
            let user = from_session(tx, &session_id)?;
            if !user.administrator || tx.db.app_passwords().session_id().find(session_id.clone()).is_some() {
                return fail("forbidden");
            }
            if old.as_ref().map(|o| o.revision).unwrap_or(0) != revision {
                return fail("conflict");
            }
            let parsed: serde_json::Value = serde_json::from_str(&value).map_err(|_| "invalidInput")?;
            let (logo, media) = (parsed["logo"].as_str().unwrap_or_default(), parsed["mediaType"].as_str().unwrap_or("?"));
            if logo.len() > 350_000 || !["", "image/png", "image/webp"].contains(&media) {
                return fail("invalidInput");
            }
            let row = PimPreference { user_id: key.clone(), revision: revision + 1, value: json!({ "logo": logo, "mediaType": media }).to_string() };
            if old.is_some() {
                tx.db.pim_preferences().user_id().update(row);
            } else {
                tx.db.pim_preferences().insert(row);
            }
        }
        let row = tx.db.pim_preferences().user_id().find(key.clone());
        Ok(match row {
            Some(r) => json!({ "userId": r.user_id, "revision": r.revision, "value": r.value }),
            None => json!({ "userId": key, "revision": 0, "value": "{\"logo\":\"\",\"mediaType\":\"\"}" }),
        }
        .to_string())
    })
}

/// The file behind a public link's digest, while the link and its owner are valid.
#[spacetimedb::procedure]
pub fn service_public_download(ctx: &mut ProcedureContext, digest: String) -> String {
    run(ctx, |tx| {
        service(tx)?;
        let share = tx.db.public_shares().digest().find(digest.clone()).filter(|s| s.expires_at > now(tx)).ok_or("notFound")?;
        let entry = tx.db.file_entries().id().find(share.file_id).ok_or("notFound")?;
        let owner_enabled = tx.db.accounts().id().find(entry.owner_id.clone()).is_some_and(|a| a.enabled);
        if access(tx, &entry, &entry.owner_id, Unlock::Nobody).is_none() || !owner_enabled {
            return fail("notFound");
        }
        let mut value = entry_json(&entry);
        value["locked"] = json!(false);
        Ok(value.to_string())
    })
}
