//! Immich mobile-app compatibility (`immich.ts`, server v3.4.1 contract). Sessions bind an
//! Immich access token to a Photos app password; the sync stream is served in channels, each a
//! sequence of JSON lines ordered by Dar's change sequence. Update IDs are UUID-shaped: the time
//! in milliseconds, then the sequence in the last group, so acks round-trip to a sequence.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::folders::photo_kind;
use crate::photo_sync::*;
use crate::tables::*;
use crate::{Res, fail, now};

fn is_uuid(s: &str) -> bool {
    crate::opaque_id(s).is_ok()
}

/// A UUIDv7-shaped update ID: 48 bits of time, then the change sequence in the last group.
pub fn update_id(ms: f64, seq: f64) -> String {
    let t = format!("{:012x}", ms.max(0.0).floor() as u64);
    let t = &t[t.len() - 12..];
    format!(
        "{}-{}-7000-8000-{:012x}",
        &t[..8],
        &t[8..12],
        seq.max(0.0) as u64
    )
}

pub fn ack_seq(ack: Option<&str>) -> f64 {
    let id = ack.and_then(|a| a.split('|').nth(1)).unwrap_or_default();
    if is_uuid(id) {
        u64::from_str_radix(&id[24..], 16).map_or(-1.0, |v| v as f64)
    } else {
        -1.0
    }
}

pub fn ack_time(ack: Option<&str>) -> f64 {
    let id = ack.and_then(|a| a.split('|').nth(1)).unwrap_or_default();
    if is_uuid(id) {
        u64::from_str_radix(&id.replace('-', "")[..12], 16).map_or(0.0, |v| v as f64)
    } else {
        0.0
    }
}

fn iso(ms: f64) -> String {
    crate::iso_string(ms.max(0.0))
}

fn channel_ack(channel: &str) -> Option<&'static str> {
    Some(match channel {
        "authUserV1" => "AuthUserV1",
        "authUser" => "AuthUserV2",
        "users" => "UserV1",
        "assetDeletes" => "AssetDeleteV1",
        "assets" => "AssetV2",
        "stackDeletes" => "StackDeleteV1",
        "stacks" => "StackV1",
        "albumAssets" => "AlbumAssetCreateV2",
        "albumDeletes" => "AlbumDeleteV1",
        "albums" => "AlbumV2",
        "albumUserDeletes" => "AlbumUserDeleteV1",
        "albumUsers" => "AlbumUserV1",
        "albumToAssetDeletes" => "AlbumToAssetDeleteV1",
        "albumToAssets" => "AlbumToAssetV1",
        "exifs" => "AssetExifV1",
        "albumAssetExifs" => "AlbumAssetExifCreateV1",
        _ => return None,
    })
}

const OTHER_ENTITY_TYPES: [&str; 50] = [
    "SyncAckV1",
    "SyncResetV1",
    "SyncCompleteV1",
    "AssetV1",
    "AlbumV1",
    "PartnerV1",
    "PartnerDeleteV1",
    "PartnerAssetV1",
    "PartnerAssetV2",
    "PartnerAssetBackfillV1",
    "PartnerAssetBackfillV2",
    "PartnerAssetDeleteV1",
    "PartnerAssetExifV1",
    "PartnerAssetExifBackfillV1",
    "PartnerStackV1",
    "PartnerStackBackfillV1",
    "PartnerStackDeleteV1",
    "UserDeleteV1",
    "AssetEditV1",
    "AssetEditDeleteV1",
    "AssetMetadataV1",
    "AssetMetadataDeleteV1",
    "AssetOcrV1",
    "AssetOcrDeleteV1",
    "AlbumUserBackfillV1",
    "AlbumAssetCreateV1",
    "AlbumAssetUpdateV1",
    "AlbumAssetUpdateV2",
    "AlbumAssetBackfillV1",
    "AlbumAssetBackfillV2",
    "AlbumAssetExifUpdateV1",
    "AlbumAssetExifBackfillV1",
    "AlbumToAssetBackfillV1",
    "MemoryV1",
    "MemoryV2",
    "MemoryDeleteV1",
    "MemoryToAssetV1",
    "MemoryToAssetV2",
    "MemoryToAssetDeleteV1",
    "PersonV1",
    "PersonDeleteV1",
    "AssetFaceV1",
    "AssetFaceV2",
    "AssetFaceV3",
    "AssetFaceDeleteV1",
    "UserMetadataV1",
    "UserMetadataDeleteV1",
    "AuthUserV1",
    "AuthUserV2",
    "UserV1",
];

fn entity_type(t: &str) -> bool {
    OTHER_ENTITY_TYPES.contains(&t)
        || [
            "AssetDeleteV1",
            "AssetV2",
            "StackDeleteV1",
            "StackV1",
            "AlbumAssetCreateV2",
            "AlbumDeleteV1",
            "AlbumV2",
            "AlbumUserDeleteV1",
            "AlbumUserV1",
            "AlbumToAssetDeleteV1",
            "AlbumToAssetV1",
            "AssetExifV1",
            "AlbumAssetExifCreateV1",
        ]
        .contains(&t)
}

/// The account an Immich access token speaks for, while its app password is valid and allows Photos.
pub fn immich_session(
    ctx: &ReducerContext,
    digest: &str,
    write: bool,
) -> Res<(ImmichSession, Account)> {
    let at = now(ctx);
    let hex64 = digest.len() == 64
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let session = if hex64 {
        ctx.db.immich_sessions().digest().find(digest.to_owned())
    } else {
        None
    };
    let Some(session) = session else {
        return fail("unauthenticated");
    };
    let protocol = ctx.db.sessions().id().find(session.session_id.clone());
    let password = ctx
        .db
        .app_passwords()
        .session_id()
        .find(session.session_id.clone());
    let user = ctx.db.accounts().id().find(session.user_id.clone());
    let (Some(protocol), Some(password), Some(user)) = (protocol, password, user) else {
        return fail("unauthenticated");
    };
    if !protocol.active
        || protocol.expires_at <= at
        || password.revoked
        || password.expires_at <= at
        || !user.enabled
    {
        return fail("unauthenticated");
    }
    if !crate::scopes::password_allows(
        &password.scope,
        "photos",
        if write { "write" } else { "read" },
    ) {
        return fail("forbidden");
    }
    let session = if at - session.used_at > 60_000.0 {
        ctx.db.immich_sessions().digest().update(ImmichSession {
            used_at: at,
            ..session
        })
    } else {
        session
    };
    Ok((session, user))
}

/// The account's own email for the app: its directory email (primary, else the first), else username@dar.
pub fn immich_email(ctx: &ReducerContext, user: &Account) -> String {
    let profile = ctx.db.directory_profiles().user_id().find(user.id.clone());
    let emails: Value = profile
        .and_then(|p| serde_json::from_str::<Value>(&p.document).ok())
        .map(|d| d["emails"].clone())
        .unwrap_or(Value::Null);
    let valid = |v: &str| {
        v.split_once('@').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && !b.contains('@')
                && !v.chars().any(char::is_whitespace)
        })
    };
    let list: Vec<&Value> = emails
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["value"].as_str().is_some_and(valid))
                .collect()
        })
        .unwrap_or_default();
    let chosen = list
        .iter()
        .find(|e| e["primary"] == json!(true))
        .or(list.first())
        .and_then(|e| e["value"].as_str().map(str::to_owned));
    chosen
        .unwrap_or_else(|| format!("{}@dar", user.username))
        .to_lowercase()
}

/// Sign-in names the app may send: the directory email, or the username followed by "@" and anything.
pub fn immich_login_matches(ctx: &ReducerContext, user: &Account, login: &str) -> bool {
    let value = login.trim().to_lowercase();
    value == immich_email(ctx, user) || value.split('@').next() == Some(user.username.as_str())
}

pub fn immich_user(ctx: &ReducerContext, user: &Account) -> Value {
    let meter = ctx.db.usage().user_id().find(user.id.clone());
    let quota = meter.as_ref().map_or(0.0, |m| m.quota);
    json!({ "id": user.id, "email": immich_email(ctx, user), "name": user.display_name, "isAdmin": user.administrator, "avatarColor": "primary", "profileImagePath": "",
        "profileChangedAt": iso(user.created_at), "createdAt": iso(user.created_at), "updatedAt": iso(user.created_at), "deletedAt": null, "oauthId": "",
        "shouldChangePassword": false, "storageLabel": null, "quotaSizeInBytes": if quota == 0.0 { Value::Null } else { json!(quota) },
        "quotaUsageInBytes": meter.map_or(0.0, |m| m.used), "status": "active", "license": null, "clusterGroupId": user.id })
}

/// Where a file is (as in photos.rs): library, locked, trash or gone.
struct Place<'a> {
    ctx: &'a ReducerContext,
    cache: std::cell::RefCell<HashMap<String, &'static str>>,
}

impl<'a> Place<'a> {
    fn new(ctx: &'a ReducerContext) -> Self {
        Self {
            ctx,
            cache: std::cell::RefCell::new(HashMap::new()),
        }
    }
    fn folder(&self, id: &str) -> &'static str {
        if let Some(hit) = self.cache.borrow().get(id) {
            return hit;
        }
        let f = self.ctx.db.file_entries().id().find(id.to_owned());
        let state = match f {
            None => "gone",
            Some(f) if f.trashed => "gone",
            Some(f) if f.parent_id.is_empty() => {
                if f.media_type == crate::files::LOCKED_FOLDER_TYPE {
                    "locked"
                } else {
                    "library"
                }
            }
            Some(f) => self.folder(&f.parent_id),
        };
        self.cache.borrow_mut().insert(id.to_owned(), state);
        state
    }
    fn of(&self, entry: &FileEntry) -> &'static str {
        if entry.trashed {
            if entry
                .list_key
                .starts_with(&format!("{}/trash/", entry.owner_id))
            {
                "trash"
            } else {
                "gone"
            }
        } else if entry.parent_id.is_empty() {
            "library"
        } else {
            self.folder(&entry.parent_id)
        }
    }
}

fn file_of(ctx: &ReducerContext, asset: &PhotoAsset) -> Option<FileEntry> {
    ctx.db
        .file_entries()
        .id()
        .find(asset.file_id.clone())
        .filter(|e| {
            e.kind == "file"
                && e.owner_id == asset.owner_id
                && photo_kind(&e.media_type, &e.name).is_some()
        })
}

/// Motion halves of Live Photos are hidden, and their stills point at them.
fn live_links(
    ctx: &ReducerContext,
    asset: &PhotoAsset,
    entry: &FileEntry,
) -> (Option<String>, bool) {
    let sibling = |name: &str| {
        ctx.db
            .file_entries()
            .list_key()
            .filter(
                format!(
                    "{}/{}/{}",
                    entry.owner_id,
                    entry.parent_id,
                    name.to_lowercase()
                )
                .as_str(),
            )
            .next()
    };
    let short = |a: &PhotoAsset| a.kind == "video" && a.duration > 0.0 && a.duration <= 4.0;
    let (stem, ext) = crate::folders::name_parts(&entry.name);
    if asset.kind == "image" {
        if !asset.live_key.is_empty() {
            for other in ctx
                .db
                .photo_assets()
                .live_key()
                .filter(asset.live_key.as_str())
            {
                if other.file_id != asset.file_id && short(&other) {
                    return (Some(other.file_id), false);
                }
            }
        }
        if ["heic", "heif", "jpg", "jpeg"].contains(&ext.as_str()) {
            for e in ["mov", "mp4"] {
                if let Some(a) = sibling(&format!("{stem}.{e}"))
                    .and_then(|s| ctx.db.photo_assets().file_id().find(s.id))
                    && short(&a)
                {
                    return (Some(a.file_id), false);
                }
            }
        }
        return (None, false);
    }
    if !short(asset) {
        return (None, false);
    }
    if !asset.live_key.is_empty()
        && ctx
            .db
            .photo_assets()
            .live_key()
            .filter(asset.live_key.as_str())
            .any(|o| o.kind == "image" && o.file_id != asset.file_id)
    {
        return (None, true);
    }
    if ["mov", "mp4"].contains(&ext.as_str()) {
        for e in ["heic", "heif", "jpg", "jpeg"] {
            if sibling(&format!("{stem}.{e}"))
                .and_then(|s| ctx.db.photo_assets().file_id().find(s.id))
                .is_some_and(|a| a.kind == "image")
            {
                return (None, true);
            }
        }
    }
    (None, false)
}

/// The checksum the app sees: the copy with the lowest file ID in the library or trash keeps the SHA-1.
fn device_checksum(ctx: &ReducerContext, asset: &PhotoAsset, place: &Place) -> String {
    if asset.sha1.is_empty() {
        return format!("dar:{}", asset.file_id);
    }
    for copy in ctx.db.photo_assets().sha1().filter(asset.sha1.as_str()) {
        if copy.owner_id != asset.owner_id || copy.file_id >= asset.file_id {
            continue;
        }
        let state = file_of(ctx, &copy).map_or("gone", |e| place.of(&e));
        if state == "library" || state == "trash" {
            return format!("{}:{}", asset.sha1, asset.file_id);
        }
    }
    asset.sha1.clone()
}

fn null_if_zero(v: u32) -> Value {
    if v == 0 { Value::Null } else { json!(v) }
}

fn asset_data(
    ctx: &ReducerContext,
    asset: &PhotoAsset,
    entry: &FileEntry,
    state: &str,
    viewer: &str,
    place: &Place,
) -> Value {
    let (motion, hidden) = live_links(ctx, asset, entry);
    let mine = asset.owner_id == viewer;
    json!({ "id": asset.file_id, "ownerId": asset.owner_id, "originalFileName": entry.name, "checksum": device_checksum(ctx, asset, place), "thumbhash": null,
        "fileCreatedAt": iso(asset.taken_at), "fileModifiedAt": iso(entry.modified_at), "localDateTime": iso(asset.taken_at), "createdAt": iso(asset.indexed_at),
        "deletedAt": if state == "trash" { json!(iso(entry.modified_at)) } else { Value::Null }, "type": if asset.kind == "video" { "VIDEO" } else { "IMAGE" },
        "isFavorite": mine && asset.favorite, "isEdited": false, "libraryId": null,
        "visibility": if hidden { "hidden" } else if mine && asset.archived { "archive" } else { "timeline" },
        "duration": if asset.kind == "video" { json!(crate::js_round(asset.duration * 1000.0)) } else { Value::Null },
        "width": null_if_zero(asset.width), "height": null_if_zero(asset.height), "livePhotoVideoId": motion,
        "stackId": if mine && !asset.stack_id.is_empty() { json!(asset.stack_id) } else { Value::Null } })
}

fn exif_data(asset: &PhotoAsset, entry: &FileEntry) -> Value {
    let e: Value = serde_json::from_str(if asset.exif.is_empty() {
        "{}"
    } else {
        &asset.exif
    })
    .unwrap_or(json!({}));
    let exposure = e["exposure"].as_f64().map(|x| {
        if x < 1.0 {
            format!("1/{}", crate::js_round(1.0 / x) as i64)
        } else {
            crate::text_of(Some(&json!(x)))
        }
    });
    let or_null = |k: &str| e.get(k).cloned().unwrap_or(Value::Null);
    json!({ "assetId": asset.file_id, "description": asset.description, "exifImageWidth": null_if_zero(asset.width), "exifImageHeight": null_if_zero(asset.height),
        "fileSizeInByte": entry.size, "orientation": null, "dateTimeOriginal": iso(asset.taken_at), "modifyDate": iso(entry.modified_at), "timeZone": null,
        "latitude": if asset.has_location { json!(asset.latitude) } else { Value::Null }, "longitude": if asset.has_location { json!(asset.longitude) } else { Value::Null },
        "projectionType": null, "city": null, "state": null, "country": null, "make": or_null("make"), "model": or_null("model"), "lensModel": or_null("lens"),
        "fNumber": or_null("fNumber"), "focalLength": or_null("focalLength"), "iso": or_null("iso"), "exposureTime": exposure, "profileDescription": null, "rating": null, "fps": null })
}

/// Albums the user owns or is a member of.
fn visible_albums(ctx: &ReducerContext, user_id: &str) -> Vec<PhotoAlbum> {
    let mut albums: Vec<PhotoAlbum> = ctx.db.photo_albums().owner_id().filter(user_id).collect();
    for m in ctx.db.photo_album_members().user_id().filter(user_id) {
        if let Some(a) = ctx.db.photo_albums().id().find(m.album_id) {
            albums.push(a);
        }
    }
    albums
}

/// The user and everyone in their albums: the people the device knows.
fn people_of(ctx: &ReducerContext, user_id: &str) -> Vec<String> {
    let mut ids = vec![user_id.to_owned()];
    let mut add = |id: String| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };
    for a in visible_albums(ctx, user_id) {
        add(a.owner_id.clone());
        for m in ctx
            .db
            .photo_album_members()
            .album_id()
            .filter(a.id.as_str())
        {
            add(m.user_id);
        }
    }
    ids
}

/// The start of a sync stream: whether the device must start over, and its checkpoints.
pub fn immich_sync_start(ctx: &ReducerContext, session: &ImmichSession, reset: bool) -> Value {
    let at = now(ctx);
    let mut checkpoints: Vec<ImmichCheckpoint> = ctx
        .db
        .immich_checkpoints()
        .immich_id()
        .filter(session.id.as_str())
        .collect();
    if reset {
        for r in &checkpoints {
            ctx.db.immich_checkpoints().id().delete(r.id.clone());
        }
        checkpoints.clear();
    }
    let complete = checkpoints
        .iter()
        .find(|c| c.r#type == "SyncCompleteV1")
        .cloned();
    // Devices synced before copies of a picture were told apart (format 2) start over once.
    let format = checkpoints
        .iter()
        .find(|c| c.r#type == "DarFormat")
        .map(|c| c.ack.clone());
    if !checkpoints.is_empty()
        && (format.as_deref() != Some("2")
            || epoch_of(ctx, &session.user_id) > session.epoch_seen
            || complete.is_some_and(|c| at - ack_time(Some(&c.ack)) > 30.0 * 86_400_000.0))
    {
        return json!({ "reset": true, "checkpoints": {}, "nowId": "", "backfilling": false, "upTo": 0 });
    }
    // Rows from before the upgrade have no sequence yet: number them, in bounded batches.
    let budget = 2000;
    let mut numbered = 0;
    for r in ctx
        .db
        .photo_assets()
        .owner_id()
        .filter(session.user_id.as_str())
        .collect::<Vec<_>>()
    {
        if numbered >= budget {
            break;
        }
        if r.sync_key.is_empty() {
            update_asset(ctx, r, false);
            numbered += 1;
        }
    }
    for album in visible_albums(ctx, &session.user_id) {
        if numbered >= budget {
            break;
        }
        if album.sync_seq == 0.0 {
            update_album(ctx, album.clone());
            numbered += 1;
        }
        for member in ctx
            .db
            .photo_album_members()
            .album_id()
            .filter(album.id.as_str())
            .collect::<Vec<_>>()
        {
            if member.sync_seq == 0.0 && numbered < budget {
                update_album_member(ctx, member);
                numbered += 1;
            }
        }
        for item in ctx
            .db
            .photo_album_items()
            .album_id()
            .filter(album.id.as_str())
            .collect::<Vec<_>>()
        {
            if numbered >= budget {
                break;
            }
            if let Some(asset) = ctx.db.photo_assets().file_id().find(item.file_id.clone())
                && asset.sync_key.is_empty()
            {
                update_asset(ctx, asset, false);
                numbered += 1;
            }
            if item.sync_seq == 0.0 {
                let seq = next_seq(ctx);
                ctx.db.photo_album_items().id().update(PhotoAlbumItem {
                    sync_seq: seq,
                    ..item
                });
                numbered += 1;
            }
        }
    }
    for stack in ctx
        .db
        .photo_stacks()
        .owner_id()
        .filter(session.user_id.as_str())
        .collect::<Vec<_>>()
    {
        if stack.sync_seq == 0.0 && numbered < budget {
            update_stack(ctx, stack);
            numbered += 1;
        }
    }
    if numbered >= budget {
        return json!({ "reset": false, "checkpoints": {}, "nowId": "", "backfilling": true, "upTo": 0 });
    }
    if checkpoints.is_empty() {
        if let Some(current) = ctx
            .db
            .immich_sessions()
            .digest()
            .find(session.digest.clone())
        {
            ctx.db.immich_sessions().digest().update(ImmichSession {
                epoch_seen: current_seq(ctx),
                ..current
            });
        }
        ctx.db.immich_checkpoints().insert(ImmichCheckpoint {
            id: format!("{}/DarFormat", session.id),
            immich_id: session.id.clone(),
            r#type: "DarFormat".into(),
            ack: "2".into(),
        });
    }
    let map: Map<String, Value> = checkpoints
        .iter()
        .map(|c| (c.r#type.clone(), json!(c.ack)))
        .collect();
    json!({ "reset": false, "checkpoints": map, "nowId": update_id(at, current_seq(ctx)), "backfilling": false, "upTo": current_seq(ctx) })
}

/// One page of a sync channel: lines after the given sequence, at most `limit`, stopping at `up_to`.
pub fn immich_sync_page(
    ctx: &ReducerContext,
    session: &ImmichSession,
    user: &Account,
    channel: &str,
    after: f64,
    limit: usize,
    up_to: f64,
) -> Res<Value> {
    let _ = session;
    let at = now(ctx);
    let user_id = user.id.as_str();
    let Some(ack_type) = channel_ack(channel) else {
        return fail("invalidInput");
    };
    let mut lines: Vec<Value> = Vec::new();
    let mut last = after;
    let place = Place::new(ctx);
    let emit = |lines: &mut Vec<Value>, last: &mut f64, kind: &str, data: Value, seq: f64| {
        lines.push(json!({ "type": kind, "data": data, "ack": format!("{ack_type}|{}", update_id(at, seq)) }));
        *last = seq;
    };
    let result = |lines: Vec<Value>, last: f64, more: bool| {
        Ok(json!({ "lines": lines, "last": last, "more": more }))
    };
    if channel == "authUser" || channel == "authUserV1" {
        let u = immich_user(ctx, user);
        let data = json!({ "id": u["id"], "name": u["name"], "email": u["email"], "isAdmin": u["isAdmin"], "avatarColor": "primary", "deletedAt": null, "hasProfileImage": false,
            "profileChangedAt": u["profileChangedAt"], "oauthId": if channel == "authUserV1" { json!("") } else { Value::Null }, "pinCode": null, "storageLabel": null,
            "quotaSizeInBytes": u["quotaSizeInBytes"], "quotaUsageInBytes": u["quotaUsageInBytes"] });
        emit(
            &mut lines,
            &mut last,
            ack_type,
            data,
            current_seq(ctx).max(1.0),
        );
        return result(lines, last, false);
    }
    if channel == "users" {
        for id in people_of(ctx, user_id) {
            if let Some(a) = ctx.db.accounts().id().find(id) {
                let email = if a.id == user_id {
                    immich_email(ctx, &a)
                } else {
                    format!("{}@dar", a.username)
                };
                let data = json!({ "id": a.id, "name": a.display_name, "email": email, "avatarColor": "primary", "deletedAt": null, "hasProfileImage": false, "profileChangedAt": iso(a.created_at) });
                emit(
                    &mut lines,
                    &mut last,
                    "UserV1",
                    data,
                    current_seq(ctx).max(1.0),
                );
            }
        }
        return result(lines, last, false);
    }
    let deletes: Option<(&str, fn(&PhotoSyncDelete) -> Value)> = match channel {
        "assetDeletes" => Some(("asset", |r| json!({ "assetId": r.entity_id }))),
        "stackDeletes" => Some(("stack", |r| json!({ "stackId": r.entity_id }))),
        "albumDeletes" => Some(("album", |r| json!({ "albumId": r.entity_id }))),
        "albumUserDeletes" => Some((
            "albumUser",
            |r| json!({ "albumId": r.entity_id, "userId": r.extra_id }),
        )),
        "albumToAssetDeletes" => Some((
            "albumToAsset",
            |r| json!({ "albumId": r.entity_id, "assetId": r.extra_id }),
        )),
        _ => None,
    };
    if let Some((kind, shape)) = deletes {
        let range = (
            std::ops::Bound::Excluded(format!("{:016}", after.max(0.0) as i64)),
            std::ops::Bound::Unbounded,
        );
        for r in ctx
            .db
            .photo_sync_deletes()
            .key()
            .filter(crate::bounds(&range))
        {
            if r.seq > up_to {
                break;
            }
            if lines.len() >= limit {
                return result(lines, last, true);
            }
            if r.kind != kind || !r.audience.split(',').any(|a| a == user_id) {
                continue;
            }
            emit(&mut lines, &mut last, ack_type, shape(&r), r.seq);
        }
        return result(lines, last, false);
    }
    if channel == "assets" || channel == "exifs" {
        let upper = if up_to.is_finite() {
            std::ops::Bound::Included(sync_key(user_id, up_to))
        } else {
            std::ops::Bound::Excluded(format!("{user_id}0"))
        };
        let range = (
            std::ops::Bound::Excluded(sync_key(user_id, after.max(0.0))),
            upper,
        );
        for asset in ctx
            .db
            .photo_assets()
            .sync_key()
            .filter(crate::bounds(&range))
        {
            if lines.len() >= limit {
                return result(lines, last, true);
            }
            let entry = file_of(ctx, &asset);
            let state = entry.as_ref().map_or("gone", |e| place.of(e));
            let Some(entry) = entry.filter(|_| state != "gone" && state != "locked") else {
                if channel == "assets" {
                    emit(
                        &mut lines,
                        &mut last,
                        "AssetDeleteV1",
                        json!({ "assetId": asset.file_id }),
                        asset.sync_seq,
                    );
                } else {
                    last = asset.sync_seq;
                }
                continue;
            };
            if channel == "assets" {
                emit(
                    &mut lines,
                    &mut last,
                    "AssetV2",
                    asset_data(ctx, &asset, &entry, state, user_id, &place),
                    asset.sync_seq,
                );
            } else {
                emit(
                    &mut lines,
                    &mut last,
                    "AssetExifV1",
                    exif_data(&asset, &entry),
                    asset.sync_seq,
                );
            }
        }
        return result(lines, last, false);
    }
    // Rows without an index by sequence are gathered, sorted and paged.
    let mut pending: Vec<(f64, &str, Value)> = Vec::new();
    let albums = visible_albums(ctx, user_id);
    let people: HashSet<String> = people_of(ctx, user_id).into_iter().collect();
    // Whether the device holds this asset: its own photos outside the Locked folder (the trash
    // too), and others' library photos in a shared album.
    let held = |asset: Option<&PhotoAsset>| -> bool {
        let Some(asset) = asset else { return false };
        let state = file_of(ctx, asset).map_or("gone", |e| place.of(&e));
        if asset.sync_seq > up_to {
            return false;
        }
        if asset.owner_id == user_id {
            state == "library" || state == "trash"
        } else {
            state == "library" && people.contains(&asset.owner_id)
        }
    };
    match channel {
        "albums" => {
            for a in &albums {
                if a.sync_seq <= after {
                    continue;
                }
                let mut cover = if !a.cover_id.is_empty()
                    && held(
                        ctx.db
                            .photo_assets()
                            .file_id()
                            .find(a.cover_id.clone())
                            .as_ref(),
                    ) {
                    Some(a.cover_id.clone())
                } else {
                    None
                };
                if cover.is_none() {
                    for item in ctx.db.photo_album_items().album_id().filter(a.id.as_str()) {
                        if held(
                            ctx.db
                                .photo_assets()
                                .file_id()
                                .find(item.file_id.clone())
                                .as_ref(),
                        ) {
                            cover = Some(item.file_id);
                            break;
                        }
                    }
                }
                pending.push((a.sync_seq, "AlbumV2", json!({ "id": a.id, "name": a.name, "description": a.description, "createdAt": iso(a.created_at), "updatedAt": iso(a.updated_at), "thumbnailAssetId": cover, "isActivityEnabled": false, "order": "desc" })));
            }
        }
        "albumUsers" => {
            for a in &albums {
                if a.sync_seq > after {
                    pending.push((
                        a.sync_seq,
                        "AlbumUserV1",
                        json!({ "albumId": a.id, "userId": a.owner_id, "role": "owner" }),
                    ));
                }
                for m in ctx
                    .db
                    .photo_album_members()
                    .album_id()
                    .filter(a.id.as_str())
                {
                    if m.sync_seq > after {
                        pending.push((m.sync_seq, "AlbumUserV1", json!({ "albumId": a.id, "userId": m.user_id, "role": if m.role == "editor" { "editor" } else { "viewer" } })));
                    }
                }
            }
        }
        "albumToAssets" => {
            // Album rows follow their asset too: one held again gets its albums back.
            for a in &albums {
                for item in ctx.db.photo_album_items().album_id().filter(a.id.as_str()) {
                    let asset = ctx.db.photo_assets().file_id().find(item.file_id.clone());
                    let seq = item
                        .sync_seq
                        .max(asset.as_ref().map_or(0.0, |x| x.sync_seq));
                    if seq > after && held(asset.as_ref()) {
                        pending.push((
                            seq,
                            "AlbumToAssetV1",
                            json!({ "albumId": a.id, "assetId": item.file_id }),
                        ));
                    }
                }
            }
        }
        "albumAssets" | "albumAssetExifs" => {
            // Others' photos come with the album rows that name them.
            let mut latest: Vec<(PhotoAsset, f64)> = Vec::new();
            for a in &albums {
                for item in ctx.db.photo_album_items().album_id().filter(a.id.as_str()) {
                    let Some(asset) = ctx
                        .db
                        .photo_assets()
                        .file_id()
                        .find(item.file_id.clone())
                        .filter(|x| x.owner_id != user_id)
                    else {
                        continue;
                    };
                    let seq = item.sync_seq.max(asset.sync_seq);
                    match latest.iter_mut().find(|(x, _)| x.file_id == asset.file_id) {
                        Some(known) if seq > known.1 => *known = (asset, seq),
                        Some(_) => {}
                        None => latest.push((asset, seq)),
                    }
                }
            }
            for (asset, seq) in latest {
                if seq <= after || !held(Some(&asset)) {
                    continue;
                }
                let entry = file_of(ctx, &asset).expect("held");
                if channel == "albumAssets" {
                    pending.push((
                        seq,
                        "AlbumAssetCreateV2",
                        asset_data(ctx, &asset, &entry, "library", user_id, &place),
                    ));
                } else {
                    pending.push((seq, "AlbumAssetExifCreateV1", exif_data(&asset, &entry)));
                }
            }
        }
        "stacks" => {
            for s in ctx.db.photo_stacks().owner_id().filter(user_id) {
                if s.sync_seq > after {
                    pending.push((s.sync_seq, "StackV1", json!({ "id": s.id, "ownerId": s.owner_id, "primaryAssetId": s.primary_id, "createdAt": iso(s.created_at), "updatedAt": iso(s.updated_at) })));
                }
            }
        }
        _ => {}
    }
    let mut due: Vec<(f64, &str, Value)> = pending.into_iter().filter(|r| r.0 <= up_to).collect();
    due.sort_by(|x, y| x.0.total_cmp(&y.0));
    let more = due.len() > limit;
    for (seq, kind, data) in due.into_iter().take(limit) {
        emit(&mut lines, &mut last, kind, data, seq);
    }
    result(lines, last, more)
}

pub fn immich_set_acks(
    ctx: &ReducerContext,
    session: &ImmichSession,
    acks: Option<&Value>,
) -> Res<()> {
    let Some(acks) = acks.and_then(Value::as_array).filter(|a| a.len() <= 1000) else {
        return fail("invalidInput");
    };
    let mut latest: Vec<(String, String)> = Vec::new();
    for ack in acks {
        let Some(ack) = ack.as_str().filter(|a| a.encode_utf16().count() <= 200) else {
            return fail("invalidInput");
        };
        let kind = ack.split('|').next().unwrap_or_default();
        // Acknowledging a reset starts the device over.
        if kind == "SyncResetV1" {
            for r in ctx
                .db
                .immich_checkpoints()
                .immich_id()
                .filter(session.id.as_str())
                .collect::<Vec<_>>()
            {
                ctx.db.immich_checkpoints().id().delete(r.id);
            }
            if let Some(current) = ctx
                .db
                .immich_sessions()
                .digest()
                .find(session.digest.clone())
            {
                ctx.db.immich_sessions().digest().update(ImmichSession {
                    epoch_seen: current_seq(ctx),
                    ..current
                });
            }
            return Ok(());
        }
        if !entity_type(kind) {
            return fail("invalidInput");
        }
        match latest.iter_mut().find(|(t, _)| t == kind) {
            Some(previous) => {
                if ack_seq(Some(ack)) >= ack_seq(Some(&previous.1)) {
                    previous.1 = ack.into();
                }
            }
            None => latest.push((kind.into(), ack.into())),
        }
    }
    for (kind, ack) in latest {
        let id = format!("{}/{kind}", session.id);
        let next = ImmichCheckpoint {
            id: id.clone(),
            immich_id: session.id.clone(),
            r#type: kind,
            ack,
        };
        if ctx.db.immich_checkpoints().id().find(id).is_some() {
            ctx.db.immich_checkpoints().id().update(next);
        } else {
            ctx.db.immich_checkpoints().insert(next);
        }
    }
    Ok(())
}

pub fn immich_acks(ctx: &ReducerContext, session: &ImmichSession) -> Value {
    Value::Array(
        ctx.db
            .immich_checkpoints()
            .immich_id()
            .filter(session.id.as_str())
            .filter(|c| entity_type(&c.r#type))
            .map(|c| json!({ "type": c.r#type, "ack": c.ack }))
            .collect(),
    )
}

pub fn immich_delete_acks(
    ctx: &ReducerContext,
    session: &ImmichSession,
    types: Option<&Value>,
) -> Res<()> {
    let types: Option<Vec<String>> = match types {
        None => None,
        Some(Value::Array(list)) if list.iter().all(Value::is_string) => Some(
            list.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => return fail("invalidInput"),
    };
    for r in ctx
        .db
        .immich_checkpoints()
        .immich_id()
        .filter(session.id.as_str())
        .collect::<Vec<_>>()
    {
        if types.as_ref().is_none_or(|t| t.contains(&r.r#type)) {
            ctx.db.immich_checkpoints().id().delete(r.id);
        }
    }
    Ok(())
}

/// Bulk upload check: an own photo with the same SHA-1 (also in the trash) is a duplicate.
pub fn immich_bulk_check(
    ctx: &ReducerContext,
    user_id: &str,
    assets: Option<&Value>,
) -> Res<Value> {
    let Some(assets) = assets.and_then(Value::as_array).filter(|a| a.len() <= 5000) else {
        return fail("invalidInput");
    };
    let place = Place::new(ctx);
    let mut results = Vec::new();
    for item in assets {
        let (Some(id), Some(checksum)) = (item["id"].as_str(), item["checksum"].as_str()) else {
            return fail("invalidInput");
        };
        if id.encode_utf16().count() > 200 || checksum.encode_utf16().count() > 100 {
            return fail("invalidInput");
        }
        let sha1 = checksum_to_base64(checksum);
        let mut found = None;
        if !sha1.is_empty() {
            for asset in ctx.db.photo_assets().sha1().filter(sha1.as_str()) {
                if asset.owner_id != user_id {
                    continue;
                }
                let state = file_of(ctx, &asset).map_or("gone", |e| place.of(&e));
                if state != "gone" {
                    found = Some(
                        json!({ "id": id, "action": "reject", "reason": "duplicate", "assetId": asset.file_id, "isTrashed": state == "trash" }),
                    );
                    break;
                }
            }
        }
        results.push(found.unwrap_or_else(|| json!({ "id": id, "action": "accept" })));
    }
    Ok(json!({ "results": results }))
}

/// Immich clients send SHA-1 checksums as base64 (and older ones as hex).
pub fn checksum_to_base64(value: &str) -> String {
    if value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes: Vec<u8> = (0..20)
            .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect();
        return base64(&bytes);
    }
    if value.len() == 28
        && value.ends_with('=')
        && value[..27]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        value.to_owned()
    } else {
        String::new()
    }
}

fn base64(bytes: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(CHARS[(n >> 18 & 63) as usize] as char);
        out.push(CHARS[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// An own asset with this SHA-1, if any.
pub fn immich_duplicate(ctx: &ReducerContext, user_id: &str, sha1: &str) -> String {
    if sha1.is_empty() {
        return String::new();
    }
    let place = Place::new(ctx);
    for asset in ctx.db.photo_assets().sha1().filter(sha1) {
        if asset.owner_id != user_id {
            continue;
        }
        if file_of(ctx, &asset).is_some_and(|e| place.of(&e) != "gone") {
            return asset.file_id;
        }
    }
    String::new()
}

pub fn immich_sessions(ctx: &ReducerContext, user_id: &str) -> Value {
    let mut list: Vec<ImmichSession> = ctx.db.immich_sessions().user_id().filter(user_id).collect();
    list.sort_by(|a, b| b.used_at.total_cmp(&a.used_at));
    Value::Array(list.iter().map(|s| json!({ "id": s.id, "deviceType": s.device_type, "deviceOS": s.device_os, "createdAt": s.created_at, "usedAt": s.used_at })).collect())
}

pub fn delete_immich_session(ctx: &ReducerContext, session: &ImmichSession) {
    for r in ctx
        .db
        .immich_checkpoints()
        .immich_id()
        .filter(session.id.as_str())
        .collect::<Vec<_>>()
    {
        ctx.db.immich_checkpoints().id().delete(r.id);
    }
    ctx.db
        .immich_sessions()
        .digest()
        .delete(session.digest.clone());
    // A device signed in with Dar has its own app password, with the device's ID; it ends with the device.
    if let Some(password) = ctx.db.app_passwords().id().find(session.id.clone())
        && password.session_id == session.session_id
        && password.user_id == session.user_id
        && !password.revoked
    {
        let protocol = password.session_id.clone();
        ctx.db.app_passwords().id().update(AppPassword {
            revoked: true,
            ..password
        });
        if let Some(s) = ctx.db.sessions().id().find(protocol) {
            ctx.db
                .sessions()
                .id()
                .update(Session { active: false, ..s });
        }
    }
}

/// Dar's connection to an Immich server, as the owner sees it (the API key never leaves the runtime).
pub fn immich_connection_view(ctx: &ReducerContext, user_id: &str) -> Value {
    let Some(r) = ctx
        .db
        .immich_connections()
        .user_id()
        .find(user_id.to_owned())
    else {
        return json!({ "connected": false });
    };
    let summary: Value = serde_json::from_str(if r.summary.is_empty() {
        "{}"
    } else {
        &r.summary
    })
    .unwrap_or(json!({}));
    json!({ "connected": true, "url": r.url, "intervalMinutes": r.interval_minutes, "enabled": r.enabled, "lastRunAt": r.last_run_at, "lastError": r.last_error, "summary": summary, "revision": r.revision })
}

pub fn save_immich_connection(
    ctx: &ReducerContext,
    user_id: &str,
    p: &Map<String, Value>,
) -> Res<Value> {
    if p.keys().any(|k| {
        ![
            "url",
            "sealedKey",
            "direction",
            "intervalMinutes",
            "enabled",
            "revision",
        ]
        .contains(&k.as_str())
    }) {
        return fail("invalidInput");
    }
    let old = ctx
        .db
        .immich_connections()
        .user_id()
        .find(user_id.to_owned());
    if p.get("revision").and_then(Value::as_f64)
        != Some(old.as_ref().map_or(0, |o| o.revision) as f64)
    {
        return fail("conflict");
    }
    let url = match p.get("url").and_then(Value::as_str) {
        Some(u) => u.trim().trim_end_matches('/').to_owned(),
        None => old.as_ref().map(|o| o.url.clone()).unwrap_or_default(),
    };
    let lower = url.to_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let valid = rest.is_some_and(|r| {
        let host = r.split('/').next().unwrap_or_default();
        !host.is_empty() && !r.chars().any(|c| c.is_whitespace() || c == '?' || c == '#')
    });
    if !valid || url.encode_utf16().count() > 500 {
        return fail("invalidInput");
    }
    // Import only (two-way was removed); older connections are kept and treated the same.
    if p.get("direction")
        .is_some_and(|d| d.as_str() != Some("import"))
    {
        return fail("invalidInput");
    }
    let interval = match p.get("intervalMinutes") {
        None => old.as_ref().map_or(60.0, |o| o.interval_minutes as f64),
        Some(v) => v.as_f64().unwrap_or(f64::NAN),
    };
    if ![15.0, 60.0, 360.0, 1440.0].contains(&interval) {
        return fail("invalidInput");
    }
    let sealed_key = match p.get("sealedKey") {
        None => old
            .as_ref()
            .map(|o| o.sealed_key.clone())
            .unwrap_or_default(),
        Some(v) => v.as_str().map(str::to_owned).unwrap_or_default(),
    };
    if sealed_key.is_empty() || sealed_key.encode_utf16().count() > 2000 {
        return fail("invalidInput");
    }
    let enabled = match p.get("enabled") {
        None => old.as_ref().is_none_or(|o| o.enabled),
        Some(Value::Bool(b)) => *b,
        _ => return fail("invalidInput"),
    };
    let same = old.as_ref().is_some_and(|o| o.url == url);
    let next = ImmichConnection {
        user_id: user_id.into(),
        url,
        sealed_key,
        direction: "import".into(),
        interval_minutes: interval as u32,
        enabled,
        last_run_at: if same {
            old.as_ref().map_or(0.0, |o| o.last_run_at)
        } else {
            0.0
        },
        last_error: if same {
            old.as_ref()
                .map(|o| o.last_error.clone())
                .unwrap_or_default()
        } else {
            String::new()
        },
        summary: if same {
            old.as_ref()
                .map(|o| o.summary.clone())
                .unwrap_or_else(|| "{}".into())
        } else {
            "{}".into()
        },
        revision: old.as_ref().map_or(0, |o| o.revision) + 1,
    };
    if old.is_some() {
        ctx.db.immich_connections().user_id().update(next);
    } else {
        ctx.db.immich_connections().insert(next);
    }
    Ok(immich_connection_view(ctx, user_id))
}

pub fn delete_immich_connection(ctx: &ReducerContext, user_id: &str) -> Value {
    ctx.db
        .immich_connections()
        .user_id()
        .delete(user_id.to_owned());
    for r in ctx
        .db
        .immich_remote()
        .user_id()
        .filter(user_id)
        .collect::<Vec<_>>()
    {
        ctx.db.immich_remote().id().delete(r.id);
    }
    json!({ "deleted": true })
}

/// Connections whose interval has passed (or one account's, when asked to sync now).
pub fn immich_due(ctx: &ReducerContext, user_id: &str) -> Value {
    let at = now(ctx);
    let rows: Vec<ImmichConnection> = if user_id.is_empty() {
        ctx.db
            .immich_connections()
            .iter()
            .filter(|r| r.enabled && r.last_run_at + r.interval_minutes as f64 * 60_000.0 <= at)
            .collect()
    } else {
        ctx.db
            .immich_connections()
            .user_id()
            .find(user_id.to_owned())
            .into_iter()
            .collect()
    };
    json!({ "connections": rows.iter().filter(|r| ctx.db.accounts().id().find(r.user_id.clone()).is_some_and(|a| a.enabled)).map(|r| json!({ "userId": r.user_id, "url": r.url, "sealedKey": r.sealed_key })).collect::<Vec<_>>() })
}

fn utf16_prefix(value: &str, max: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().take(max).collect();
    String::from_utf16_lossy(&units)
}

pub fn immich_report(
    ctx: &ReducerContext,
    user_id: &str,
    error: &str,
    summary: Option<&Value>,
) -> Value {
    let Some(r) = ctx
        .db
        .immich_connections()
        .user_id()
        .find(user_id.to_owned())
    else {
        return json!({ "ok": false });
    };
    let summary = match summary {
        None => "undefined".to_owned(),
        Some(v) => crate::stringify(v),
    };
    ctx.db
        .immich_connections()
        .user_id()
        .update(ImmichConnection {
            last_run_at: now(ctx),
            last_error: utf16_prefix(error, 500),
            summary: utf16_prefix(&summary, 2000),
            ..r
        });
    json!({ "ok": true })
}

/// What is already known locally: mapped remote IDs and the SHA-1 of the owner's photos (paged by file ID).
pub fn immich_known(ctx: &ReducerContext, user_id: &str, after: &str, limit: usize) -> Value {
    let remote: Vec<Value> = if after.is_empty() {
        ctx.db
            .immich_remote()
            .user_id()
            .filter(user_id)
            .map(
                |r| json!({ "remoteId": r.remote_id, "fileId": r.file_id, "checksum": r.checksum }),
            )
            .collect()
    } else {
        Vec::new()
    };
    let place = Place::new(ctx);
    let mut rows: Vec<PhotoAsset> = ctx.db.photo_assets().owner_id().filter(user_id).collect();
    rows.sort_by(|a, b| crate::locale_compare(&a.file_id, &b.file_id));
    let mut local: Vec<Value> = Vec::new();
    for asset in rows {
        if !after.is_empty() && asset.file_id.as_str() <= after {
            continue;
        }
        if local.len() >= limit {
            break;
        }
        let Some(entry) = file_of(ctx, &asset) else {
            continue;
        };
        if place.of(&entry) != "library" || asset.sha1.is_empty() {
            continue;
        }
        local.push(json!({ "fileId": asset.file_id, "sha1": asset.sha1, "name": entry.name, "mediaType": entry.media_type, "takenAt": asset.taken_at, "modifiedAt": entry.modified_at,
            "blobId": entry.blob_id, "size": entry.size, "favorite": asset.favorite, "archived": asset.archived }));
    }
    let next = if local.len() >= limit {
        local
            .last()
            .map(|l| l["fileId"].clone())
            .unwrap_or(json!(""))
    } else {
        json!("")
    };
    json!({ "remote": remote, "local": local, "next": next })
}

pub fn immich_map(
    ctx: &ReducerContext,
    user_id: &str,
    remote_id: &str,
    file_id: &str,
    checksum: &str,
) -> Res<Value> {
    if remote_id.encode_utf16().count() > 100
        || file_id.encode_utf16().count() > 100
        || checksum.encode_utf16().count() > 100
    {
        return fail("invalidInput");
    }
    let id = format!("{user_id}/{remote_id}");
    let next = ImmichRemote {
        id: id.clone(),
        user_id: user_id.into(),
        remote_id: remote_id.into(),
        file_id: file_id.into(),
        checksum: checksum.into(),
    };
    if ctx.db.immich_remote().id().find(id).is_some() {
        ctx.db.immich_remote().id().update(next);
    } else {
        ctx.db.immich_remote().insert(next);
    }
    Ok(json!({ "ok": true }))
}

/// After an imported file is indexed: Immich's favorite, archive and description.
pub fn immich_apply(
    ctx: &ReducerContext,
    user_id: &str,
    file_id: &str,
    favorite: bool,
    archived: bool,
    description: &str,
) -> Value {
    let Some(asset) = ctx
        .db
        .photo_assets()
        .file_id()
        .find(file_id.to_owned())
        .filter(|a| a.owner_id == user_id)
    else {
        return json!({ "applied": false });
    };
    let revision = asset.revision + 1;
    update_asset(
        ctx,
        PhotoAsset {
            favorite,
            archived,
            description: utf16_prefix(description, 2000),
            revision,
            ..asset
        },
        false,
    );
    json!({ "applied": true })
}

/// An Immich album, imported once by remote ID and kept in step with the imported photos.
pub fn immich_album(
    ctx: &ReducerContext,
    user_id: &str,
    remote_id: &str,
    name: &str,
    description: &str,
    file_ids: &[String],
) -> Res<Value> {
    let at = now(ctx);
    let map_id = format!("{user_id}/album:{remote_id}");
    let mut album_id = ctx
        .db
        .immich_remote()
        .id()
        .find(map_id)
        .map(|r| r.file_id)
        .unwrap_or_default();
    if album_id.is_empty() || ctx.db.photo_albums().id().find(album_id.clone()).is_none() {
        if ctx.db.photo_albums().owner_id().filter(user_id).count() >= 500 {
            return Ok(json!({ "albumId": "", "added": 0 }));
        }
        album_id = ctx.new_uuid_v4().expect("randomness").to_string();
        while ctx.db.photo_albums().id().find(album_id.clone()).is_some() {
            album_id = ctx.new_uuid_v4().expect("randomness").to_string();
        }
        let title = utf16_prefix(name, 200);
        insert_album(
            ctx,
            PhotoAlbum {
                id: album_id.clone(),
                owner_id: user_id.into(),
                name: if title.is_empty() {
                    "Immich album".into()
                } else {
                    title
                },
                description: utf16_prefix(description, 2000),
                cover_id: String::new(),
                created_at: at,
                updated_at: at,
                revision: 1,
                sync_seq: 0.0,
            },
        );
        immich_map(ctx, user_id, &format!("album:{remote_id}"), &album_id, "")?;
    }
    let mut added = 0;
    for file_id in file_ids.iter().take(50000) {
        let id = format!("{album_id}/{file_id}");
        if ctx
            .db
            .photo_assets()
            .file_id()
            .find(file_id.clone())
            .is_none_or(|a| a.owner_id != user_id)
            || ctx.db.photo_album_items().id().find(id.clone()).is_some()
        {
            continue;
        }
        insert_album_item(
            ctx,
            PhotoAlbumItem {
                id,
                album_id: album_id.clone(),
                file_id: file_id.clone(),
                added_at: at,
                sync_seq: 0.0,
            },
        );
        added += 1;
    }
    Ok(json!({ "albumId": album_id, "added": added }))
}
