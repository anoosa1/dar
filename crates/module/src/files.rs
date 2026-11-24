//! Files records, uploads and WebDAV operations (`next/module/src/index.ts`, Files parts).
//! Not yet ported: navigation and the `my_files` view, favorites and public-share reducers,
//! history, Photos and Git hooks, maintenance, backups and restore.

use serde_json::json;
use spacetimedb::{ProcedureContext, ReducerContext, Table};

use crate::auth::caller;
use crate::blobs::{release_blob, store_blob};
use crate::scopes::password_allows;
use crate::tables::*;
use crate::{
    Res, fail, from_session, name_of, now, opaque_id, run, service, unused_uuid, window_of,
};

pub const LOCKED_FOLDER_TYPE: &str = "inode/x-dar-locked";
pub const MAX_FILE_BYTES: f64 = 16.0 * 1024.0 * 1024.0 * 1024.0;

/// Which session may open the Locked folder.
#[derive(Clone, Copy)]
pub enum Unlock<'a> {
    /// The caller's own principal.
    Caller,
    /// A browser session (Photos and uploads act for one).
    Session(&'a str),
    /// No session: protocol access never reaches the Locked folder.
    Nobody,
    /// Trusted service work (exports) sees inside the Locked folder.
    Trusted,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Role {
    Read,
    Write,
    Owner,
}

pub(crate) fn is_locked_root(e: &FileEntry) -> bool {
    e.kind == "folder" && e.parent_id.is_empty() && e.media_type == LOCKED_FOLDER_TYPE
}

/// The Locked folder containing an entry (or the folder itself).
pub(crate) fn locked_root(ctx: &ReducerContext, entry: &FileEntry) -> Option<FileEntry> {
    let mut node = Some(entry.clone());
    for _ in 0..128 {
        let n = node?;
        if is_locked_root(&n) {
            return Some(n);
        }
        if n.parent_id.is_empty() {
            return None;
        }
        node = ctx.db.file_entries().id().find(n.parent_id);
    }
    None
}

pub(crate) fn unlocked(ctx: &ReducerContext, user_id: &str, unlock: Unlock) -> bool {
    let session = match unlock {
        Unlock::Nobody => return false,
        Unlock::Trusted => return true,
        Unlock::Session(s) => s.to_owned(),
        Unlock::Caller => match ctx.db.principals().identity().find(ctx.sender()) {
            Some(p) => p.session_id,
            None => return false,
        },
    };
    ctx.db
        .file_unlocks()
        .session_id()
        .find(session)
        .is_some_and(|row| row.user_id == user_id && row.expires_at > now(ctx))
}

/// The caller's role on an entry: owner, a grant on it or an ancestor, or none.
pub fn access(
    ctx: &ReducerContext,
    entry: &FileEntry,
    user_id: &str,
    unlock: Unlock,
) -> Option<Role> {
    let mut role = (entry.owner_id == user_id).then_some(Role::Owner);
    let mut node = Some(entry.clone());
    for _ in 0..128 {
        let n = node?;
        if n.trashed {
            return None;
        }
        // Only the owner's unlocked session reaches the Locked folder; grants inside it pause.
        if is_locked_root(&n) && (n.owner_id != user_id || !unlocked(ctx, user_id, unlock)) {
            return None;
        }
        if role.is_none() || role == Some(Role::Read) {
            for grant in ctx
                .db
                .grants()
                .file_id()
                .filter(n.id.as_str())
                .filter(|g| g.user_id == user_id)
            {
                if grant.role == "write" {
                    role = Some(Role::Write);
                } else if role.is_none() {
                    role = Some(Role::Read);
                }
            }
        }
        if n.parent_id.is_empty() {
            return role;
        }
        node = ctx.db.file_entries().id().find(n.parent_id);
    }
    None
}

pub fn writable(ctx: &ReducerContext, id: &str, user_id: &str, unlock: Unlock) -> Res<FileEntry> {
    let entry = ctx
        .db
        .file_entries()
        .id()
        .find(id.to_owned())
        .ok_or("notFound")?;
    let role = access(ctx, &entry, user_id, unlock);
    // The owner learns that a password is needed; anyone else sees nothing.
    if role.is_none()
        && entry.owner_id == user_id
        && !entry.trashed
        && locked_root(ctx, &entry).is_some()
    {
        return fail("lockedFolder");
    }
    if !matches!(role, Some(Role::Write | Role::Owner)) {
        return fail("forbidden");
    }
    Ok(entry)
}

/// The owner of a new child of `parent_id`.
pub(crate) fn destination(
    ctx: &ReducerContext,
    parent_id: &str,
    user_id: &str,
    unlock: Unlock,
) -> Res<String> {
    if parent_id.is_empty() {
        return Ok(user_id.to_owned());
    }
    let parent = writable(ctx, parent_id, user_id, unlock)?;
    if parent.kind != "folder" {
        return fail("invalidInput");
    }
    Ok(parent.owner_id)
}

pub fn key(owner_id: &str, parent_id: &str, name: &str) -> String {
    format!("{owner_id}/{parent_id}/{}", name.to_lowercase())
}

pub(crate) fn find_key(ctx: &ReducerContext, value: &str) -> Option<FileEntry> {
    ctx.db.file_entries().list_key().filter(value).next()
}

pub(crate) fn children(ctx: &ReducerContext, owner_id: &str, folder_id: &str) -> Vec<FileEntry> {
    let (lower, upper) = window_of(&format!("{owner_id}/{folder_id}/"), None);
    ctx.db
        .file_entries()
        .list_key()
        .filter((
            lower.as_ref().map(String::as_str),
            upper.as_ref().map(String::as_str),
        ))
        .collect()
}

pub(crate) fn touch_folder(ctx: &ReducerContext, owner_id: &str, parent_id: &str) {
    if !parent_id.is_empty() {
        if let Some(parent) = ctx.db.file_entries().id().find(parent_id.to_owned()) {
            ctx.db.file_entries().id().update(FileEntry {
                revision: parent.revision + 1,
                modified_at: now(ctx),
                ..parent
            });
        }
        return;
    }
    let root = ctx.db.root_revisions().user_id().find(owner_id.to_owned());
    let row = RootRevision {
        user_id: owner_id.into(),
        revision: root.as_ref().map_or(1, |r| r.revision) + 1,
        modified_at: now(ctx),
    };
    if root.is_some() {
        ctx.db.root_revisions().user_id().update(row);
    } else {
        ctx.db.root_revisions().insert(row);
    }
}

pub(crate) fn event(
    ctx: &ReducerContext,
    owner_id: &str,
    file_id: &str,
    action: &str,
    parent_id: Option<&str>,
) {
    let id = unused_uuid(ctx);
    ctx.db.activity().insert(Activity {
        id,
        owner_id: owner_id.into(),
        file_id: file_id.into(),
        action: action.into(),
        at: now(ctx),
    });
    let parent = parent_id
        .map(str::to_owned)
        .or_else(|| {
            ctx.db
                .file_entries()
                .id()
                .find(file_id.to_owned())
                .map(|e| e.parent_id)
        })
        .unwrap_or_default();
    touch_folder(ctx, owner_id, &parent);
    crate::photo_sync::photo_file_event(ctx, owner_id, file_id, action);
}

/// Moves or renames an entry, keeping its favorite and share listings in step (`historyFiles.move`).
pub fn move_listed(ctx: &ReducerContext, entry: &FileEntry, parent_id: &str, name: &str) {
    ctx.db.file_entries().id().update(FileEntry {
        name: name.into(),
        parent_id: parent_id.into(),
        list_key: key(&entry.owner_id, parent_id, name),
        revision: entry.revision + 1,
        modified_at: now(ctx),
        ..entry.clone()
    });
    if entry.favorite {
        ctx.db.favorites().id().update(Favorite {
            id: entry.id.clone(),
            file_id: entry.id.clone(),
            list_key: format!("{}/{}/{}", entry.owner_id, name.to_lowercase(), entry.id),
        });
    }
    rename_grants(ctx, &entry.id, name);
    event(ctx, &entry.owner_id, &entry.id, "moved", None);
    if entry.parent_id != parent_id {
        touch_folder(ctx, &entry.owner_id, &entry.parent_id);
    }
}

pub(crate) fn check_revision(entry: &FileEntry, expected: u32) -> Res<()> {
    if entry.revision == expected {
        Ok(())
    } else {
        fail("conflict")
    }
}

fn rename_grants(ctx: &ReducerContext, file_id: &str, name: &str) {
    for grant in ctx
        .db
        .grants()
        .file_id()
        .filter(file_id)
        .collect::<Vec<_>>()
    {
        let list_key = format!("{}/{}/{}", grant.user_id, name.to_lowercase(), grant.id);
        ctx.db.grants().id().update(GrantRow { list_key, ..grant });
    }
}

fn update_favorite(ctx: &ReducerContext, entry: &FileEntry, name: &str) {
    if entry.favorite {
        ctx.db.favorites().id().update(Favorite {
            id: entry.id.clone(),
            file_id: entry.id.clone(),
            list_key: format!("{}/{}/{}", entry.owner_id, name.to_lowercase(), entry.id),
        });
    }
}

pub(crate) fn fits_quota(meter: &Usage, bytes: f64) -> bool {
    meter.quota == 0.0 || meter.used + meter.reserved + bytes <= meter.quota
}

pub(crate) fn meter(ctx: &ReducerContext, user_id: &str) -> Usage {
    ctx.db
        .usage()
        .user_id()
        .find(user_id.to_owned())
        .unwrap_or_else(|| {
            ctx.db.usage().insert(Usage {
                user_id: user_id.into(),
                used: 0.0,
                reserved: 0.0,
                quota: 0.0,
            })
        })
}

// ---------- Entry operations ----------

pub fn create_folder_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    parent_id: &str,
    name: &str,
    unlock: Unlock,
) -> Res<()> {
    opaque_id(id)?;
    let name = name_of(name)?;
    let owner_id = destination(ctx, parent_id, user_id, unlock)?;
    let list_key = key(&owner_id, parent_id, &name);
    if find_key(ctx, &list_key).is_some() {
        return fail("conflict");
    }
    ctx.db.file_entries().insert(FileEntry {
        id: id.into(),
        owner_id: owner_id.clone(),
        parent_id: parent_id.into(),
        name,
        kind: "folder".into(),
        blob_id: String::new(),
        size: 0.0,
        media_type: "inode/directory".into(),
        revision: 1,
        modified_at: now(ctx),
        trashed: false,
        favorite: false,
        list_key,
    });
    event(ctx, &owner_id, id, "created", None);
    Ok(())
}

#[spacetimedb::reducer]
pub fn create_folder(ctx: &ReducerContext, id: String, parentId: String, name: String) -> Res<()> {
    let parent_id = parentId;
    let user = caller(ctx)?;
    create_folder_for(ctx, &user.id, &id, &parent_id, &name, Unlock::Caller)
}

#[spacetimedb::reducer]
pub fn rename_entry(
    ctx: &ReducerContext,
    id: String,
    name: String,
    expectedRevision: u32,
) -> Res<()> {
    let expected_revision = expectedRevision;
    let user = caller(ctx)?;
    let entry = writable(ctx, &id, &user.id, Unlock::Caller)?;
    if is_locked_root(&entry) {
        return fail("forbidden");
    }
    check_revision(&entry, expected_revision)?;
    let name = name_of(&name)?;
    let list_key = key(&entry.owner_id, &entry.parent_id, &name);
    if find_key(ctx, &list_key).is_some_and(|c| c.id != entry.id) {
        return fail("conflict");
    }
    // Renaming and moving keep the modified time: it dates the content.
    ctx.db.file_entries().id().update(FileEntry {
        name: name.clone(),
        list_key,
        revision: entry.revision + 1,
        ..entry.clone()
    });
    update_favorite(ctx, &entry, &name);
    rename_grants(ctx, &entry.id, &name);
    event(ctx, &entry.owner_id, &entry.id, "renamed", None);
    Ok(())
}

pub fn move_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    parent_id: &str,
    expected_revision: u32,
    name: Option<&str>,
    unlock: Unlock,
) -> Res<()> {
    let entry = writable(ctx, id, user_id, unlock)?;
    if entry.owner_id != user_id
        || is_locked_root(&entry)
        || destination(ctx, parent_id, user_id, unlock)? != entry.owner_id
    {
        return fail("forbidden");
    }
    check_revision(&entry, expected_revision)?;
    let mut ancestor = parent_id.to_owned();
    for depth in 0.. {
        if ancestor.is_empty() {
            break;
        }
        if ancestor == entry.id || depth >= 128 {
            return fail("invalidInput");
        }
        ancestor = ctx
            .db
            .file_entries()
            .id()
            .find(ancestor)
            .map(|e| e.parent_id)
            .unwrap_or_default();
    }
    let name = match name {
        Some(n) => name_of(n)?,
        None => entry.name.clone(),
    };
    let list_key = key(&entry.owner_id, parent_id, &name);
    if find_key(ctx, &list_key).is_some_and(|e| e.id != entry.id) {
        return fail("conflict");
    }
    ctx.db.file_entries().id().update(FileEntry {
        name: name.clone(),
        parent_id: parent_id.into(),
        list_key,
        revision: entry.revision + 1,
        ..entry.clone()
    });
    update_favorite(ctx, &entry, &name);
    rename_grants(ctx, &entry.id, &name);
    event(ctx, &entry.owner_id, &entry.id, "moved", None);
    if entry.parent_id != parent_id {
        touch_folder(ctx, &entry.owner_id, &entry.parent_id);
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn move_entry(
    ctx: &ReducerContext,
    id: String,
    parentId: String,
    expectedRevision: u32,
    name: Option<String>,
) -> Res<()> {
    let (parent_id, expected_revision) = (parentId, expectedRevision);
    let user = caller(ctx)?;
    move_for(
        ctx,
        &user.id,
        &id,
        &parent_id,
        expected_revision,
        name.as_deref(),
        Unlock::Caller,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn copy_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    new_id: &str,
    parent_id: &str,
    name: &str,
    expected_revision: u32,
    unlock: Unlock,
) -> Res<()> {
    opaque_id(new_id)?;
    let source = ctx
        .db
        .file_entries()
        .id()
        .find(id.to_owned())
        .filter(|s| access(ctx, s, user_id, unlock).is_some())
        .ok_or("notFound")?;
    if is_locked_root(&source) {
        return fail("forbidden");
    }
    check_revision(&source, expected_revision)?;
    let owner_id = destination(ctx, parent_id, user_id, unlock)?;
    let name = name_of(name)?;
    if find_key(ctx, &key(&owner_id, parent_id, &name)).is_some() {
        return fail("conflict");
    }
    let mut ancestor = parent_id.to_owned();
    for depth in 0.. {
        if ancestor.is_empty() {
            break;
        }
        if ancestor == source.id || depth >= 128 {
            return fail("invalidInput");
        }
        ancestor = ctx
            .db
            .file_entries()
            .id()
            .find(ancestor)
            .map(|e| e.parent_id)
            .unwrap_or_default();
    }
    let mut pending = vec![(
        source.clone(),
        new_id.to_owned(),
        parent_id.to_owned(),
        name,
    )];
    let (mut count, mut bytes) = (0, 0.0);
    while let Some((item, item_id, item_parent, item_name)) = pending.pop() {
        count += 1;
        if count > 500 {
            return fail("operationTooLarge");
        }
        if access(ctx, &item, user_id, unlock).is_none() {
            return fail("forbidden");
        }
        let entry = FileEntry {
            id: item_id.clone(),
            name: item_name.clone(),
            parent_id: item_parent.clone(),
            owner_id: owner_id.clone(),
            favorite: false,
            revision: 1,
            modified_at: now(ctx),
            list_key: key(&owner_id, &item_parent, &item_name),
            ..item.clone()
        };
        ctx.db.file_entries().insert(entry.clone());
        if entry.kind == "file" {
            bytes += entry.size;
            ctx.db.versions().insert(Version {
                id: unused_uuid(ctx),
                file_id: entry.id.clone(),
                blob_id: entry.blob_id.clone(),
                size: entry.size,
                media_type: entry.media_type.clone(),
                created_at: now(ctx),
            });
        } else {
            for child in children(ctx, &source.owner_id, &item.id) {
                let child_name = child.name.clone();
                pending.push((child, unused_uuid(ctx), entry.id.clone(), child_name));
            }
        }
    }
    let usage = meter(ctx, &owner_id);
    if !fits_quota(&usage, bytes) {
        return fail("quotaExceeded");
    }
    ctx.db.usage().user_id().update(Usage {
        used: usage.used + bytes,
        ..usage
    });
    event(ctx, &owner_id, new_id, "copied", None);
    Ok(())
}

#[spacetimedb::reducer]
pub fn copy_entry(
    ctx: &ReducerContext,
    id: String,
    newId: String,
    parentId: String,
    name: String,
    expectedRevision: u32,
) -> Res<()> {
    let (new_id, parent_id, expected_revision) = (newId, parentId, expectedRevision);
    let user = caller(ctx)?;
    copy_for(
        ctx,
        &user.id,
        &id,
        &new_id,
        &parent_id,
        &name,
        expected_revision,
        Unlock::Caller,
    )
}

/// Locked items are deleted permanently instead: the trash is not locked.
pub fn trash_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    expected_revision: u32,
    unlock: Unlock,
) -> Res<()> {
    let entry = writable(ctx, id, user_id, unlock)?;
    if entry.owner_id != user_id || is_locked_root(&entry) {
        return fail("forbidden");
    }
    if locked_root(ctx, &entry).is_some() {
        return fail("lockedFolder");
    }
    check_revision(&entry, expected_revision)?;
    let list_key = format!("{user_id}/trash/{}/{}", entry.name.to_lowercase(), entry.id);
    ctx.db.file_entries().id().update(FileEntry {
        trashed: true,
        list_key,
        revision: entry.revision + 1,
        modified_at: now(ctx),
        ..entry.clone()
    });
    event(ctx, &entry.owner_id, &entry.id, "trashed", None);
    Ok(())
}

#[spacetimedb::reducer]
pub fn trash_entry(ctx: &ReducerContext, id: String, expectedRevision: u32) -> Res<()> {
    let expected_revision = expectedRevision;
    let user = caller(ctx)?;
    trash_for(ctx, &user.id, &id, expected_revision, Unlock::Caller)
}

#[spacetimedb::reducer]
pub fn restore_entry(ctx: &ReducerContext, id: String, expectedRevision: u32) -> Res<()> {
    let user = caller(ctx)?;
    restore_for(ctx, &user.id, &id, expectedRevision, Unlock::Caller)
}

pub fn restore_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    expected_revision: u32,
    unlock: Unlock,
) -> Res<()> {
    let entry = ctx
        .db
        .file_entries()
        .id()
        .find(id.to_owned())
        .filter(|e| e.owner_id == user_id && e.trashed)
        .ok_or("notFound")?;
    check_revision(&entry, expected_revision)?;
    // A parent now inside a locked Locked folder is not used: the item returns to All files.
    let parent = (!entry.parent_id.is_empty())
        .then(|| ctx.db.file_entries().id().find(entry.parent_id.clone()))
        .flatten();
    let parent_id = parent
        .filter(|p| access(ctx, p, user_id, unlock).is_some())
        .map(|p| p.id)
        .unwrap_or_default();
    let list_key = key(user_id, &parent_id, &entry.name);
    if find_key(ctx, &list_key).is_some() {
        return fail("conflict");
    }
    ctx.db.file_entries().id().update(FileEntry {
        parent_id,
        trashed: false,
        list_key,
        revision: entry.revision + 1,
        modified_at: now(ctx),
        ..entry.clone()
    });
    event(ctx, user_id, &entry.id, "restored", None);
    Ok(())
}

/// Deletes a trashed item, or one inside the unlocked Locked folder, with its versions.
pub fn delete_for(
    ctx: &ReducerContext,
    user_id: &str,
    id: &str,
    expected_revision: u32,
    unlock: Unlock,
) -> Res<()> {
    let entry = ctx
        .db
        .file_entries()
        .id()
        .find(id.to_owned())
        .filter(|e| e.owner_id == user_id)
        .ok_or("notFound")?;
    if !entry.trashed && (is_locked_root(&entry) || locked_root(ctx, &entry).is_none()) {
        return fail("notFound");
    }
    if !entry.trashed && access(ctx, &entry, user_id, unlock).is_none() {
        return fail("lockedFolder");
    }
    check_revision(&entry, expected_revision)?;
    let mut pending = vec![entry.clone()];
    let (mut count, mut bytes) = (0, 0.0);
    while let Some(item) = pending.pop() {
        count += 1;
        if count > 2000 {
            return fail("operationTooLarge");
        }
        if item.kind == "folder" {
            pending.extend(children(ctx, user_id, &item.id));
        }
        // A tracked folder's history goes with it; versions another folder's history uses stay (and stay counted).
        if item.kind == "folder"
            && let Some(repo) = ctx.db.file_repos().id().find(item.id.clone())
        {
            crate::history::drop_repo(ctx, &repo)?;
        }
        for version in ctx
            .db
            .versions()
            .file_id()
            .filter(item.id.as_str())
            .collect::<Vec<_>>()
        {
            if crate::history::history_keeps(ctx, &version.id) {
                continue;
            }
            bytes += version.size;
            ctx.db.versions().id().delete(version.id.clone());
            release_blob(ctx, &version.blob_id);
        }
        for grant in ctx
            .db
            .grants()
            .file_id()
            .filter(item.id.as_str())
            .collect::<Vec<_>>()
        {
            ctx.db.grants().id().delete(grant.id);
        }
        for share in ctx
            .db
            .public_shares()
            .file_id()
            .filter(item.id.as_str())
            .collect::<Vec<_>>()
        {
            ctx.db.public_shares().id().delete(share.id);
        }
        if item.kind == "file" {
            crate::photo_sync::touch_asset(ctx, &item.id);
        }
        ctx.db.file_entries().id().delete(item.id.clone());
        ctx.db.favorites().id().delete(item.id.clone());
    }
    if let Some(usage) = ctx.db.usage().user_id().find(user_id.to_owned()) {
        ctx.db.usage().user_id().update(Usage {
            used: (usage.used - bytes).max(0.0),
            ..usage
        });
    }
    event(ctx, user_id, &entry.id, "deleted", Some(&entry.parent_id));
    Ok(())
}

#[spacetimedb::reducer]
pub fn delete_entry(ctx: &ReducerContext, id: String, expectedRevision: u32) -> Res<()> {
    let expected_revision = expectedRevision;
    let user = caller(ctx)?;
    delete_for(ctx, &user.id, &id, expected_revision, Unlock::Caller)
}

// ---------- Uploads ----------

fn valid_media_type(value: &str) -> bool {
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    value.len() <= 150
        && value
            .split_once('/')
            .is_some_and(|(a, b)| token(a) && token(b))
}

/// Reserves an upload. `upload_key` ties the transfer to its sender (a session ID).
#[allow(clippy::too_many_arguments)]
pub fn begin_upload_for(
    tx: &ReducerContext,
    user_id: &str,
    upload_key: &str,
    id: &str,
    file_id: &str,
    parent_id: &str,
    name: &str,
    size: f64,
    media_type: &str,
    expected_revision: u32,
    unlock: Unlock,
) -> Res<Upload> {
    opaque_id(id)?;
    opaque_id(file_id)?;
    if size.fract() != 0.0
        || !(0.0..=MAX_FILE_BYTES).contains(&size)
        || !valid_media_type(media_type)
    {
        return fail("invalidInput");
    }
    let existing = tx.db.file_entries().id().find(file_id.to_owned());
    let (owner_id, name, parent_id) = if expected_revision > 0 {
        let existing = existing.filter(|e| e.kind == "file").ok_or("notFound")?;
        writable(tx, &existing.id, user_id, unlock)?;
        check_revision(&existing, expected_revision)?;
        (existing.owner_id, existing.name, existing.parent_id)
    } else {
        let owner_id = destination(tx, parent_id, user_id, unlock)?;
        let name = name_of(name)?;
        if existing.is_some() || find_key(tx, &key(&owner_id, parent_id, &name)).is_some() {
            return fail("conflict");
        }
        (owner_id, name, parent_id.to_owned())
    };
    let usage = meter(tx, &owner_id);
    if !fits_quota(&usage, size) {
        return fail("quotaExceeded");
    }
    tx.db.usage().user_id().update(Usage {
        reserved: usage.reserved + size,
        ..usage
    });
    let expires_at = now(tx) + 3_600_000.0;
    let upload = Upload {
        id: id.into(),
        session_id: upload_key.into(),
        owner_id,
        parent_id,
        file_id: file_id.into(),
        name,
        size,
        media_type: media_type.into(),
        expected_revision,
        expires_at,
        state: "uploading".into(),
        digest: String::new(),
        expires_key: format!("{:016}", expires_at as u64),
    };
    tx.db.uploads().insert(upload.clone());
    Ok(upload)
}

#[allow(clippy::too_many_arguments)]
#[spacetimedb::procedure]
pub fn service_begin_upload(
    ctx: &mut ProcedureContext,
    sessionId: String,
    id: String,
    fileId: String,
    parentId: String,
    name: String,
    size: f64,
    mediaType: String,
    expectedRevision: u32,
) -> String {
    let (session_id, file_id, parent_id, media_type, expected_revision) =
        (sessionId, fileId, parentId, mediaType, expectedRevision);
    run(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &session_id)?;
        let upload = begin_upload_for(
            tx,
            &user.id,
            &session_id,
            &id,
            &file_id,
            &parent_id,
            &name,
            size,
            &media_type,
            expected_revision,
            Unlock::Session(&session_id),
        )?;
        serde_json::to_string(&upload).map_err(|_| "internal".into())
    })
}

#[spacetimedb::procedure]
pub fn service_upload(ctx: &mut ProcedureContext, sessionId: String, id: String) -> String {
    let session_id = sessionId;
    run(ctx, |tx| {
        service(tx)?;
        from_session(tx, &session_id)?;
        let upload = tx
            .db
            .uploads()
            .id()
            .find(id.clone())
            .filter(|u| u.session_id == session_id && u.expires_at > now(tx))
            .ok_or("notFound")?;
        serde_json::to_string(&upload).map_err(|_| "internal".into())
    })
}

/// Completes an upload into a file entry and returns its ID (also when already complete).
/// `file_time` (from WebDAV and Immich clients) becomes the modified time.
pub fn finish_upload_for(
    ctx: &ReducerContext,
    user_id: &str,
    upload_key: &str,
    id: &str,
    digest: &str,
    unlock: Unlock,
    file_time: f64,
) -> Res<String> {
    let upload = ctx.db.uploads().id().find(id.to_owned());
    let Some(upload) = upload.filter(|u| {
        u.session_id == upload_key && u.expires_at > now(ctx) && crate::blobs::is_blob_id(digest)
    }) else {
        return fail("notFound");
    };
    if upload.state == "complete" {
        return if upload.digest == digest {
            Ok(upload.file_id)
        } else {
            fail("conflict")
        };
    }
    if upload.state != "uploading" {
        return fail("conflict");
    }
    let existing = ctx.db.file_entries().id().find(upload.file_id.clone());
    if upload.expected_revision > 0 {
        let existing = existing.as_ref().ok_or("notFound")?;
        writable(ctx, &existing.id, user_id, unlock)?;
        check_revision(existing, upload.expected_revision)?;
    } else if existing.is_some()
        || find_key(ctx, &key(&upload.owner_id, &upload.parent_id, &upload.name)).is_some()
    {
        return fail("conflict");
    }
    // Creating a child requires access to its destination; a direct file grant need not grant the parent.
    if upload.expected_revision == 0
        && destination(ctx, &upload.parent_id, user_id, unlock)? != upload.owner_id
    {
        return fail("forbidden");
    }
    // The runtime stored the bytes under their digest before this call.
    store_blob(ctx, digest, upload.size)?;
    let at = now(ctx);
    let entry = FileEntry {
        id: upload.file_id.clone(),
        owner_id: upload.owner_id.clone(),
        parent_id: upload.parent_id.clone(),
        name: upload.name.clone(),
        kind: "file".into(),
        blob_id: digest.into(),
        size: upload.size,
        media_type: upload.media_type.clone(),
        revision: existing.as_ref().map_or(0, |e| e.revision) + 1,
        modified_at: if file_time > 0.0 && file_time < at + 86_400_000.0 {
            file_time
        } else {
            at
        },
        trashed: false,
        favorite: existing.as_ref().is_some_and(|e| e.favorite),
        list_key: key(&upload.owner_id, &upload.parent_id, &upload.name),
    };
    if existing.is_some() {
        ctx.db.file_entries().id().update(entry.clone());
    } else {
        ctx.db.file_entries().insert(entry.clone());
    }
    ctx.db.versions().insert(Version {
        id: upload.id.clone(),
        file_id: entry.id.clone(),
        blob_id: digest.into(),
        size: upload.size,
        media_type: upload.media_type.clone(),
        created_at: at,
    });
    ctx.db.uploads().id().update(Upload {
        state: "complete".into(),
        digest: digest.into(),
        ..upload.clone()
    });
    let usage = meter(ctx, &upload.owner_id);
    ctx.db.usage().user_id().update(Usage {
        reserved: usage.reserved - upload.size,
        used: usage.used + upload.size,
        ..usage
    });
    event(
        ctx,
        &upload.owner_id,
        &entry.id,
        if existing.is_some() {
            "updated"
        } else {
            "uploaded"
        },
        None,
    );
    Ok(entry.id)
}

#[spacetimedb::reducer]
pub fn service_finish_upload(
    ctx: &ReducerContext,
    sessionId: String,
    id: String,
    digest: String,
    fileTime: f64,
) -> Res<()> {
    let (session_id, file_time) = (sessionId, fileTime);
    service(ctx)?;
    let user = from_session(ctx, &session_id)?;
    finish_upload_for(
        ctx,
        &user.id,
        &session_id,
        &id,
        &digest,
        Unlock::Session(&session_id),
        file_time,
    )
    .map(|_| ())
}

#[spacetimedb::reducer]
pub fn service_cancel_upload(ctx: &ReducerContext, sessionId: String, id: String) -> Res<()> {
    let session_id = sessionId;
    service(ctx)?;
    from_session(ctx, &session_id)?;
    let upload = ctx
        .db
        .uploads()
        .id()
        .find(id)
        .filter(|u| u.session_id == session_id)
        .ok_or("notFound")?;
    if upload.state != "uploading" {
        return Ok(());
    }
    let usage = meter(ctx, &upload.owner_id);
    ctx.db.usage().user_id().update(Usage {
        reserved: (usage.reserved - upload.size).max(0.0),
        ..usage
    });
    ctx.db.uploads().id().update(Upload {
        state: "cancelled".into(),
        ..upload
    });
    Ok(())
}

pub(crate) fn entry_json(e: &FileEntry) -> serde_json::Value {
    json!({
        "id": e.id, "ownerId": e.owner_id, "parentId": e.parent_id, "name": e.name, "kind": e.kind, "blobId": e.blob_id, "size": e.size,
        "mediaType": e.media_type, "revision": e.revision, "modifiedAt": e.modified_at, "trashed": e.trashed, "favorite": e.favorite,
    })
}

#[spacetimedb::procedure]
pub fn service_download(
    ctx: &mut ProcedureContext,
    sessionId: String,
    id: String,
    versionId: String,
) -> String {
    let (session_id, version_id) = (sessionId, versionId);
    run(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &session_id)?;
        if let Some(password) = tx.db.app_passwords().session_id().find(session_id.clone())
            && !password_allows(&password.scope, "files", "read")
        {
            return fail("forbidden");
        }
        let entry = tx.db.file_entries().id().find(id.clone());
        let entry = entry
            .filter(|e| {
                e.kind == "file" && access(tx, e, &user.id, Unlock::Session(&session_id)).is_some()
            })
            .ok_or("notFound")?;
        let version = (!version_id.is_empty())
            .then(|| tx.db.versions().id().find(version_id.clone()))
            .flatten();
        if !version_id.is_empty() && version.as_ref().is_none_or(|v| v.file_id != entry.id) {
            return fail("notFound");
        }
        let mut value = entry_json(&entry);
        if let Some(v) = version {
            value["blobId"] = json!(v.blob_id);
            value["size"] = json!(v.size);
            value["mediaType"] = json!(v.media_type);
        }
        // `locked` keeps Locked folder files out of browser caches.
        value["locked"] = json!(locked_root(tx, &entry).is_some());
        Ok(value.to_string())
    })
}

// ---------- WebDAV ----------

/// Dar's WebDAV verbs on logical paths, for an app password's protocol session.
#[allow(clippy::too_many_arguments)]
#[spacetimedb::procedure]
pub fn service_dav(
    ctx: &mut ProcedureContext,
    sessionId: String,
    method: String,
    path: String,
    destination: String,
    expectedRevision: u32,
    overwrite: bool,
    depth: u32,
    modifiedAt: f64,
) -> String {
    let (session_id, expected_revision, modified_at) = (sessionId, expectedRevision, modifiedAt);
    run(ctx, |tx| {
        dav(
            tx,
            &session_id,
            &method,
            &path,
            &destination,
            expected_revision,
            overwrite,
            depth,
            modified_at,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn dav(
    tx: &ReducerContext,
    session_id: &str,
    method: &str,
    path: &str,
    dest: &str,
    expected_revision: u32,
    overwrite: bool,
    depth: u32,
    modified_at: f64,
) -> Res<String> {
    service(tx)?;
    let user = from_session(tx, session_id)?;
    let password = tx
        .db
        .app_passwords()
        .session_id()
        .find(session_id.to_owned());
    let Some(password) = password.filter(|p| !p.revoked && p.expires_at > now(tx)) else {
        return fail("unauthenticated");
    };
    let permission = if matches!(method, "STAT" | "PROPFIND") {
        "read"
    } else {
        "write"
    };
    // Writes need metadata for revision checks, without granting content reads.
    if !password_allows(&password.scope, "files", permission)
        && !(method == "STAT" && password_allows(&password.scope, "files", "write"))
    {
        return fail("forbidden");
    }
    let unlock = Unlock::Nobody;
    let resolve = |value: &str| -> Res<Option<FileEntry>> {
        let parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() > 128 || value.len() > 4096 {
            return fail("invalidInput");
        }
        let (mut parent_id, mut result) = (String::new(), None);
        for (i, part) in parts.iter().enumerate() {
            let found = find_key(tx, &key(&user.id, &parent_id, &name_of(part)?));
            let Some(found) = found.filter(|f| access(tx, f, &user.id, unlock).is_some()) else {
                return Ok(None);
            };
            if i != parts.len() - 1 && found.kind != "folder" {
                return Ok(None);
            }
            parent_id = found.id.clone();
            result = Some(found);
        }
        Ok(result)
    };
    // WebDAV answers a missing parent with 409, which clients take as "create the parents first".
    let parent = |value: &str| -> Res<(String, String)> {
        let mut parts: Vec<&str> = value.split('/').filter(|p| !p.is_empty()).collect();
        let name = name_of(parts.pop().unwrap_or(""))?;
        if parts.is_empty() {
            return Ok((String::new(), name));
        }
        match resolve(&parts.join("/"))? {
            Some(d) if d.kind == "folder" => Ok((d.id, name)),
            _ => fail("conflict"),
        }
    };
    let entry = resolve(path)?;
    if method == "PREPARE" {
        let (parent_id, name) = parent(path)?;
        return Ok(
            json!({ "entry": entry.as_ref().map(entry_json), "parentId": parent_id, "name": name })
                .to_string(),
        );
    }
    let has_path = path.split('/').any(|p| !p.is_empty());
    if method != "MKCOL" && has_path && entry.is_none() {
        return fail("notFound");
    }
    let root_state = tx.db.root_revisions().user_id().find(user.id.clone());
    let root_entry = json!({
        "id": "", "ownerId": user.id, "name": user.username, "kind": "folder", "size": 0, "mediaType": "inode/directory",
        "revision": root_state.as_ref().map_or(1, |r| r.revision), "blobId": "", "modifiedAt": root_state.as_ref().map_or(user.created_at, |r| r.modified_at),
    });
    match method {
        "STAT" => return Ok(entry.as_ref().map_or(root_entry, entry_json).to_string()),
        "PROPFIND" => {
            let sums = |blob_id: &str| {
                let row = (!blob_id.is_empty())
                    .then(|| tx.db.blob_checksums().id().find(blob_id.to_owned()))
                    .flatten();
                (
                    row.as_ref().map_or(String::new(), |r| r.sha1.clone()),
                    row.map_or(String::new(), |r| r.md5),
                )
            };
            let (root_id, root_kind, mut root_json) = match &entry {
                Some(e) => (e.id.clone(), e.kind.clone(), entry_json(e)),
                None => (String::new(), "folder".to_owned(), root_entry),
            };
            let (sha1, md5) = sums(root_json["blobId"].as_str().unwrap_or(""));
            root_json["sha1"] = json!(sha1);
            root_json["md5"] = json!(md5);
            // Depth 1 lists a folder; depth 2 stands for infinity, breadth first.
            let mut found = Vec::new();
            let mut pending: Vec<(String, String)> = if root_kind == "folder" && depth > 0 {
                vec![(root_id, String::new())]
            } else {
                Vec::new()
            };
            let mut index = 0;
            while index < pending.len() {
                let (folder_id, folder_path) = pending[index].clone();
                index += 1;
                for child in children(tx, &user.id, &folder_id) {
                    if found.len() >= 10_000 {
                        return fail("operationTooLarge");
                    }
                    let child_path = if folder_path.is_empty() {
                        child.name.clone()
                    } else {
                        format!("{folder_path}/{}", child.name)
                    };
                    if depth > 1
                        && child.kind == "folder"
                        && access(tx, &child, &user.id, unlock).is_some()
                    {
                        pending.push((child.id.clone(), child_path.clone()));
                    }
                    let (sha1, md5) = sums(&child.blob_id);
                    let mut value = entry_json(&child);
                    value["path"] = json!(child_path);
                    value["sha1"] = json!(sha1);
                    value["md5"] = json!(md5);
                    found.push(value);
                }
            }
            return Ok(json!({ "root": root_json, "children": found }).to_string());
        }
        "MKCOL" => {
            if entry.is_some() {
                return Ok(json!({ "created": false }).to_string());
            }
            let (parent_id, name) = parent(path)?;
            create_folder_for(tx, &user.id, &unused_uuid(tx), &parent_id, &name, unlock)?;
            return Ok(json!({ "created": true }).to_string());
        }
        _ => {}
    }
    let Some(entry) = entry else {
        return fail("forbidden");
    };
    if method == "TOUCH" {
        // The modified time a client keeps for its copy; the content does not change.
        let target = writable(tx, &entry.id, &user.id, unlock)?;
        if is_locked_root(&target) {
            return fail("forbidden");
        }
        if !modified_at.is_finite() || modified_at <= 0.0 || modified_at >= now(tx) + 86_400_000.0 {
            return fail("invalidInput");
        }
        tx.db.file_entries().id().update(FileEntry {
            modified_at: modified_at.floor(),
            revision: target.revision + 1,
            ..target.clone()
        });
        touch_folder(tx, &target.owner_id, &target.parent_id);
        return Ok(json!({ "revision": target.revision + 1 }).to_string());
    }
    check_revision(&entry, expected_revision)?;
    match method {
        "DELETE" => trash_for(tx, &user.id, &entry.id, entry.revision, unlock)?,
        "MOVE" | "COPY" => {
            let (parent_id, name) = parent(dest)?;
            let existing = resolve(dest)?;
            if existing.as_ref().is_some_and(|e| e.id == entry.id) {
                return fail("invalidInput");
            }
            if let Some(existing) = &existing {
                if !overwrite {
                    return fail("conflict");
                }
                trash_for(tx, &user.id, &existing.id, existing.revision, unlock)?;
            }
            if method == "MOVE" {
                move_for(
                    tx,
                    &user.id,
                    &entry.id,
                    &parent_id,
                    entry.revision,
                    Some(&name),
                    unlock,
                )?;
            } else {
                copy_for(
                    tx,
                    &user.id,
                    &entry.id,
                    &unused_uuid(tx),
                    &parent_id,
                    &name,
                    entry.revision,
                    unlock,
                )?;
            }
            return Ok(json!({ "replaced": existing.is_some() }).to_string());
        }
        _ => return fail("invalidInput"),
    }
    Ok(json!({ "ok": true }).to_string())
}
