//! Change tracking for Immich-compatible sync (`photo-sync.ts`). Every Photos write takes the
//! next value of one sequence; deletions leave tombstones naming who may hear of them. A change
//! that cannot be expressed row by row raises a user's epoch, and their mobile sessions start
//! over with a full sync.

use spacetimedb::{ReducerContext, Table};

use crate::now;
use crate::tables::*;

fn put(ctx: &ReducerContext, name: &str, value: String) {
    let next = PhotoState {
        name: name.into(),
        value,
    };
    if ctx.db.photo_state().name().find(name.to_owned()).is_some() {
        ctx.db.photo_state().name().update(next);
    } else {
        ctx.db.photo_state().insert(next);
    }
}

pub fn current_seq(ctx: &ReducerContext) -> f64 {
    ctx.db
        .photo_state()
        .name()
        .find("seq".to_owned())
        .and_then(|r| r.value.parse().ok())
        .unwrap_or(0.0)
}

pub fn next_seq(ctx: &ReducerContext) -> f64 {
    let seq = current_seq(ctx) + 1.0;
    put(ctx, "seq", format!("{}", seq as i64));
    seq
}

pub fn sync_key(owner_id: &str, seq: f64) -> String {
    format!("{owner_id}/{:016}", seq as i64)
}

pub fn epoch_of(ctx: &ReducerContext, user_id: &str) -> f64 {
    ctx.db
        .photo_state()
        .name()
        .find(format!("epoch:{user_id}"))
        .and_then(|r| r.value.parse().ok())
        .unwrap_or(0.0)
}

pub fn raise_epoch(ctx: &ReducerContext, user_ids: &[String]) {
    let seq = next_seq(ctx);
    let mut seen: Vec<&String> = Vec::new();
    for id in user_ids {
        if !seen.contains(&id) {
            seen.push(id);
            put(ctx, &format!("epoch:{id}"), format!("{}", seq as i64));
        }
    }
}

/// The owner and members of an album.
pub fn album_audience(ctx: &ReducerContext, album_id: &str) -> Vec<String> {
    match ctx.db.photo_albums().id().find(album_id.to_owned()) {
        Some(album) => std::iter::once(album.owner_id)
            .chain(
                ctx.db
                    .photo_album_members()
                    .album_id()
                    .filter(album_id)
                    .map(|m| m.user_id),
            )
            .collect(),
        None => Vec::new(),
    }
}

fn tombstone(
    ctx: &ReducerContext,
    kind: &str,
    entity_id: &str,
    extra_id: &str,
    audience: &[String],
) {
    let seq = next_seq(ctx);
    let key = format!("{:016}", seq as i64);
    let mut unique: Vec<&str> = Vec::new();
    for a in audience {
        if !unique.contains(&a.as_str()) {
            unique.push(a);
        }
    }
    ctx.db.photo_sync_deletes().insert(PhotoSyncDelete {
        id: key.clone(),
        key,
        kind: kind.into(),
        entity_id: entity_id.into(),
        extra_id: extra_id.into(),
        audience: unique.join(","),
        seq,
        at: now(ctx),
    });
}

/// The Immich app keeps one asset per owner and checksum; when a copy appears, leaves or
/// changes place, the other copies are sent again.
fn touch_copies(ctx: &ReducerContext, owner_id: &str, sha1: &str, file_id: &str) {
    if sha1.is_empty() {
        return;
    }
    for copy in ctx
        .db
        .photo_assets()
        .sha1()
        .filter(sha1)
        .collect::<Vec<_>>()
    {
        if copy.owner_id == owner_id && copy.file_id != file_id {
            let seq = next_seq(ctx);
            ctx.db.photo_assets().file_id().update(PhotoAsset {
                sync_seq: seq,
                sync_key: sync_key(owner_id, seq),
                ..copy
            });
        }
    }
}

pub fn insert_asset(ctx: &ReducerContext, asset: PhotoAsset) -> PhotoAsset {
    let seq = next_seq(ctx);
    let key = sync_key(&asset.owner_id, seq);
    let result = ctx.db.photo_assets().insert(PhotoAsset {
        sync_seq: seq,
        sync_key: key,
        ..asset
    });
    touch_copies(ctx, &result.owner_id, &result.sha1, &result.file_id);
    result
}

/// `keep_sha1`: the caller's row carries no checksum (TypeScript leaves `sha1` out), so the stored one stays.
pub fn update_asset(ctx: &ReducerContext, asset: PhotoAsset, keep_sha1: bool) -> PhotoAsset {
    let old = ctx.db.photo_assets().file_id().find(asset.file_id.clone());
    let seq = next_seq(ctx);
    let sha1 = if keep_sha1 {
        old.as_ref().map(|o| o.sha1.clone()).unwrap_or_default()
    } else {
        asset.sha1.clone()
    };
    let key = sync_key(&asset.owner_id, seq);
    let result = ctx.db.photo_assets().file_id().update(PhotoAsset {
        sha1,
        sync_seq: seq,
        sync_key: key,
        ..asset
    });
    if let Some(old) = old
        && old.sha1 != result.sha1
    {
        touch_copies(ctx, &old.owner_id, &old.sha1, &result.file_id);
        touch_copies(ctx, &result.owner_id, &result.sha1, &result.file_id);
    }
    result
}

pub fn delete_asset(ctx: &ReducerContext, file_id: &str) -> bool {
    let old = ctx.db.photo_assets().file_id().find(file_id.to_owned());
    if let Some(r) = &old {
        tombstone(ctx, "asset", file_id, "", std::slice::from_ref(&r.owner_id));
    }
    let deleted = ctx.db.photo_assets().file_id().delete(file_id.to_owned());
    if let Some(r) = old {
        touch_copies(ctx, &r.owner_id, &r.sha1, file_id);
    }
    deleted
}

/// A file event in Files: the photo's visibility may have changed, and with it which copy the app shows.
pub fn touch_asset(ctx: &ReducerContext, file_id: &str) {
    if let Some(r) = ctx.db.photo_assets().file_id().find(file_id.to_owned()) {
        let (owner, sha1) = (r.owner_id.clone(), r.sha1.clone());
        update_asset(ctx, r, false);
        touch_copies(ctx, &owner, &sha1, file_id);
    }
}

pub fn insert_album(ctx: &ReducerContext, album: PhotoAlbum) -> PhotoAlbum {
    let seq = next_seq(ctx);
    ctx.db.photo_albums().insert(PhotoAlbum {
        sync_seq: seq,
        ..album
    })
}

pub fn update_album(ctx: &ReducerContext, album: PhotoAlbum) -> PhotoAlbum {
    let seq = next_seq(ctx);
    ctx.db.photo_albums().id().update(PhotoAlbum {
        sync_seq: seq,
        ..album
    })
}

pub fn delete_album(ctx: &ReducerContext, id: &str) -> bool {
    let audience = album_audience(ctx, id);
    tombstone(ctx, "album", id, "", &audience);
    ctx.db.photo_albums().id().delete(id.to_owned())
}

pub fn insert_album_item(ctx: &ReducerContext, item: PhotoAlbumItem) -> PhotoAlbumItem {
    let seq = next_seq(ctx);
    ctx.db.photo_album_items().insert(PhotoAlbumItem {
        sync_seq: seq,
        ..item
    })
}

pub fn delete_album_item(ctx: &ReducerContext, id: &str) -> bool {
    if let Some(r) = ctx.db.photo_album_items().id().find(id.to_owned()) {
        let audience = album_audience(ctx, &r.album_id);
        tombstone(ctx, "albumToAsset", &r.album_id, &r.file_id, &audience);
    }
    ctx.db.photo_album_items().id().delete(id.to_owned())
}

/// A new member, or one who leaves, gets a full sync; the others hear of the membership row.
pub fn insert_album_member(ctx: &ReducerContext, member: PhotoAlbumMember) -> PhotoAlbumMember {
    let seq = next_seq(ctx);
    let user = member.user_id.clone();
    let result = ctx.db.photo_album_members().insert(PhotoAlbumMember {
        sync_seq: seq,
        ..member
    });
    raise_epoch(ctx, &[user]);
    result
}

pub fn update_album_member(ctx: &ReducerContext, member: PhotoAlbumMember) -> PhotoAlbumMember {
    let seq = next_seq(ctx);
    ctx.db.photo_album_members().id().update(PhotoAlbumMember {
        sync_seq: seq,
        ..member
    })
}

pub fn delete_album_member(ctx: &ReducerContext, id: &str) -> bool {
    if let Some(r) = ctx.db.photo_album_members().id().find(id.to_owned()) {
        let audience = album_audience(ctx, &r.album_id);
        tombstone(ctx, "albumUser", &r.album_id, &r.user_id, &audience);
        raise_epoch(ctx, &[r.user_id]);
    }
    ctx.db.photo_album_members().id().delete(id.to_owned())
}

pub fn insert_stack(ctx: &ReducerContext, stack: PhotoStack) -> PhotoStack {
    let seq = next_seq(ctx);
    ctx.db.photo_stacks().insert(PhotoStack {
        sync_seq: seq,
        ..stack
    })
}

pub fn update_stack(ctx: &ReducerContext, stack: PhotoStack) -> PhotoStack {
    let seq = next_seq(ctx);
    ctx.db.photo_stacks().id().update(PhotoStack {
        sync_seq: seq,
        ..stack
    })
}

pub fn delete_stack(ctx: &ReducerContext, id: &str) -> bool {
    if let Some(r) = ctx.db.photo_stacks().id().find(id.to_owned()) {
        tombstone(ctx, "stack", id, "", std::slice::from_ref(&r.owner_id));
    }
    ctx.db.photo_stacks().id().delete(id.to_owned())
}

/// Files calls this for every file event. A folder trashed, restored, moved or deleted may
/// hide or show many photos at once, so its owner and the members of their albums resync.
pub fn photo_file_event(ctx: &ReducerContext, owner_id: &str, file_id: &str, action: &str) {
    let entry = ctx.db.file_entries().id().find(file_id.to_owned());
    if entry.is_some_and(|e| e.kind == "folder") {
        if !["trashed", "restored", "moved", "deleted"].contains(&action) {
            return;
        }
        let mut audience = vec![owner_id.to_owned()];
        for album in ctx.db.photo_albums().owner_id().filter(owner_id) {
            for member in ctx.db.photo_album_members().album_id().filter(&album.id) {
                audience.push(member.user_id);
            }
        }
        raise_epoch(ctx, &audience);
        return;
    }
    touch_asset(ctx, file_id);
}

/// Tombstones older than 31 days are dropped; sessions that old start over.
pub fn prune_tombstones(ctx: &ReducerContext, before: f64) {
    let mut removed = 0;
    for r in ctx.db.photo_sync_deletes().iter().collect::<Vec<_>>() {
        if r.at < before {
            ctx.db.photo_sync_deletes().id().delete(r.id);
            removed += 1;
            if removed >= 5000 {
                break;
            }
        }
    }
}
