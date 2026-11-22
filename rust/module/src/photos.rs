//! Photos (`photos.ts`): an index of image and video files in Files, albums, stacks, Live
//! Photos, duplicates, sharing with people and shared links. Rows never own bytes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::folders::{name_parts, photo_kind};
use crate::photo_sync::*;
use crate::tables::*;
use crate::{Res, fail, now, row};

type Doc = Map<String, Value>;

pub const IMMICH_CLIENT_ID: &str = "immich-app";
pub const INDEX_VERSION: u32 = 3;
const ALBUMS: usize = 500;
const ALBUM_ITEMS: usize = 50000;
const DESCRIPTION: usize = 2000;
const ALBUM_NAME: usize = 200;
const BATCH: usize = 500;
const STACK_ITEMS: usize = 200;
const LINKS: usize = 200;
const MEMBERS: usize = 100;
const PASSWORD_DAYS: f64 = 0.5;
const EXPIRY_DAYS: f64 = 3650.0;
const LIVE_MAX_SECONDS: f64 = 4.0;
const RAW: [&str; 8] = ["dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf"];
const STILL: [&str; 4] = ["heic", "heif", "jpg", "jpeg"];
const MOTION: [&str; 2] = ["mov", "mp4"];
pub const READ_OPERATIONS: [&str; 7] = ["overview", "page", "detail", "asset", "duplicates", "stackDetail", "albumSharing"];

/// File changes Photos makes through Files' own checks, acting for the browser session.
pub struct PhotoFiles<'a> {
    pub unlocked: bool,
    pub trash: &'a dyn Fn(&FileEntry) -> Res<()>,
    pub restore: &'a dyn Fn(&FileEntry) -> Res<()>,
    pub remove: &'a dyn Fn(&FileEntry) -> Res<()>,
    /// Moves into a folder, numbering the name if it is taken.
    pub move_to: &'a dyn Fn(&FileEntry, &str) -> Res<()>,
    pub locked_folder: &'a dyn Fn() -> String,
    pub month_folder: &'a dyn Fn(i64, u32) -> Res<String>,
    pub upload_folder: &'a dyn Fn() -> String,
    /// The Photos folder's ID, or "" while it does not exist.
    pub upload_folder_id: &'a dyn Fn() -> String,
}

fn text(value: Option<&Value>, max: usize, allow_empty: bool) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else { return fail("invalidInput") };
    if !allow_empty && s.trim().is_empty() || s.contains('\0') || s.encode_utf16().count() > max {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

fn num(value: Option<&Value>, min: f64, max: f64) -> Res<f64> {
    match value.and_then(Value::as_f64) {
        Some(f) if f.is_finite() && f >= min && f <= max => Ok(f),
        _ => fail("invalidInput"),
    }
}

fn int(value: Option<&Value>, min: f64, max: f64) -> Res<f64> {
    match value.and_then(Value::as_f64) {
        Some(f) if f.fract() == 0.0 && f >= min && f <= max => Ok(f),
        _ => fail("invalidInput"),
    }
}

fn keys(p: &Doc, allowed: &[&str]) -> Res<()> {
    if p.keys().any(|k| !allowed.contains(&k.as_str())) { fail("invalidInput") } else { Ok(()) }
}

fn opaque(value: Option<&Value>) -> Res<String> {
    match value.and_then(Value::as_str) {
        Some(s) if crate::opaque_id(s).is_ok() => Ok(s.to_owned()),
        _ => fail("invalidInput"),
    }
}

fn ids(value: Option<&Value>, max: usize) -> Res<Vec<String>> {
    let Some(list) = value.and_then(Value::as_array).filter(|l| !l.is_empty() && l.len() <= max) else { return fail("invalidInput") };
    let mut out: Vec<String> = Vec::new();
    for v in list {
        let id = opaque(Some(v))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// An owner's keys: `owner/…`, or after `after` within them.
fn owner_range(owner_id: &str, after: Option<&str>) -> (std::ops::Bound<String>, std::ops::Bound<String>) {
    let lower = match after {
        Some(a) => std::ops::Bound::Excluded(a.to_owned()),
        None => std::ops::Bound::Included(format!("{owner_id}/")),
    };
    (lower, std::ops::Bound::Excluded(format!("{owner_id}0")))
}

/// Newest first within an owner; ties keep file ID order.
pub fn photo_time_key(owner_id: &str, taken_at: f64, file_id: &str) -> String {
    let clamped = crate::js_round(taken_at).clamp(0.0, 9e15 - 1.0);
    format!("{owner_id}/{:016}/{file_id}", (9e15 - clamped) as i64)
}

fn newest(a: &PhotoAsset, b: &PhotoAsset) -> std::cmp::Ordering {
    b.taken_at.total_cmp(&a.taken_at).then_with(|| crate::locale_compare(&a.file_id, &b.file_id))
}

fn new_id(ctx: &ReducerContext) -> String {
    loop {
        let id = ctx.new_uuid_v4().expect("randomness").to_string();
        if ctx.db.photo_stacks().id().find(id.clone()).is_none() && ctx.db.photo_albums().id().find(id.clone()).is_none() {
            return id;
        }
    }
}

fn exif_of(asset: &PhotoAsset) -> Value {
    let text = if asset.exif.is_empty() { "{}" } else { asset.exif.as_str() };
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

/// Where a file is: in the folder tree, inside the Locked folder, directly in the trash, or gone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Library,
    Locked,
    Trash,
    Gone,
}

/// Per-call reader over the index; folder placement and stacks are cached.
struct Reader<'a> {
    ctx: &'a ReducerContext,
    viewer: String,
    folders: RefCell<HashMap<String, State>>,
    stacks: RefCell<HashMap<String, Option<(String, usize)>>>,
}

impl<'a> Reader<'a> {
    fn new(ctx: &'a ReducerContext, viewer: &str) -> Self {
        Self { ctx, viewer: viewer.into(), folders: RefCell::new(HashMap::new()), stacks: RefCell::new(HashMap::new()) }
    }

    fn folder_state(&self, id: &str) -> State {
        let mut chain: Vec<String> = Vec::new();
        let mut state = State::Gone;
        let mut current = id.to_owned();
        for _ in 0..128 {
            if let Some(cached) = self.folders.borrow().get(&current) {
                state = *cached;
                break;
            }
            let folder = self.ctx.db.file_entries().id().find(current.clone());
            chain.push(current.clone());
            let Some(folder) = folder.filter(|f| !f.trashed) else { break };
            if folder.parent_id.is_empty() {
                state = if folder.media_type == crate::files::LOCKED_FOLDER_TYPE { State::Locked } else { State::Library };
                break;
            }
            current = folder.parent_id;
        }
        let mut cache = self.folders.borrow_mut();
        for id in chain {
            cache.insert(id, state);
        }
        state
    }

    fn state_of(&self, entry: &FileEntry) -> State {
        if entry.trashed {
            if entry.list_key.starts_with(&format!("{}/trash/", entry.owner_id)) { State::Trash } else { State::Gone }
        } else if entry.parent_id.is_empty() {
            State::Library
        } else {
            self.folder_state(&entry.parent_id)
        }
    }

    /// The owner's live media file for an index row.
    fn file_of(&self, asset: &PhotoAsset) -> Option<FileEntry> {
        self.ctx.db.file_entries().id().find(asset.file_id.clone()).filter(|e| e.kind == "file" && e.owner_id == asset.owner_id && photo_kind(&e.media_type, &e.name).is_some())
    }

    fn sibling(&self, entry: &FileEntry, name: &str) -> Option<FileEntry> {
        let lower = name.to_lowercase();
        if !entry.trashed {
            return self.ctx.db.file_entries().list_key().filter(format!("{}/{}/{lower}", entry.owner_id, entry.parent_id).as_str()).next();
        }
        let window = crate::folder_window(&format!("{}/trash/{lower}/", entry.owner_id));
        self.ctx.db.file_entries().list_key().filter(crate::bounds(&window)).find(|o| o.parent_id == entry.parent_id)
    }

    fn short(video: &PhotoAsset) -> bool {
        video.duration > 0.0 && video.duration <= LIVE_MAX_SECONDS
    }

    /// The other half of a Live Photo in the same place.
    fn live_partner(&self, asset: &PhotoAsset, entry: &FileEntry) -> Option<PhotoAsset> {
        if asset.kind == "video" && !Self::short(asset) {
            return None;
        }
        let want = if asset.kind == "image" { "video" } else { "image" };
        let state = self.state_of(entry);
        let fits = |other: &PhotoAsset| -> bool {
            if other.owner_id != asset.owner_id || other.kind != want || want == "video" && !Self::short(other) {
                return false;
            }
            self.file_of(other).is_some_and(|e| self.state_of(&e) == state)
        };
        if !asset.live_key.is_empty() {
            for other in self.ctx.db.photo_assets().live_key().filter(asset.live_key.as_str()) {
                if other.file_id != asset.file_id && fits(&other) {
                    return Some(other);
                }
            }
        }
        let (stem, ext) = name_parts(&entry.name);
        let own: &[&str] = if asset.kind == "image" { &STILL } else { &MOTION };
        if !own.contains(&ext.as_str()) {
            return None;
        }
        let others: &[&str] = if asset.kind == "image" { &MOTION } else { &STILL };
        for other in others {
            if let Some(e) = self.sibling(entry, &format!("{stem}.{other}"))
                && let Some(r) = self.ctx.db.photo_assets().file_id().find(e.id)
                && fits(&r)
            {
                return Some(r);
            }
        }
        None
    }

    /// A stack's cover and size among members in the given state; stacks of one act like none.
    fn stack_of(&self, asset: &PhotoAsset, state: State) -> Option<(String, String, usize)> {
        if asset.stack_id.is_empty() {
            return None;
        }
        let cache_key = format!("{state:?}/{}", asset.stack_id);
        if !self.stacks.borrow().contains_key(&cache_key) {
            let stack = self.ctx.db.photo_stacks().id().find(asset.stack_id.clone());
            let value = stack.and_then(|stack| {
                let mut members: Vec<PhotoAsset> = self.ctx.db.photo_assets().stack_id().filter(stack.id.as_str()).filter(|a| self.file_of(a).is_some_and(|e| self.state_of(&e) == state)).collect();
                members.sort_by(newest);
                if members.len() < 2 {
                    return None;
                }
                let primary = if members.iter().any(|m| m.file_id == stack.primary_id) { stack.primary_id.clone() } else { members[0].file_id.clone() };
                Some((primary, members.len()))
            });
            self.stacks.borrow_mut().insert(cache_key.clone(), value);
        }
        self.stacks.borrow()[&cache_key].as_ref().map(|(primary, count)| (asset.stack_id.clone(), primary.clone(), *count))
    }

    /// Timelines show one tile per Live Photo and per stack.
    fn shown(&self, asset: &PhotoAsset, entry: &FileEntry, state: State, stacked: bool) -> bool {
        if asset.kind == "video" && self.live_partner(asset, entry).is_some() {
            return false;
        }
        let stack = if stacked { self.stack_of(asset, state) } else { None };
        stack.is_none_or(|(_, primary, _)| primary == asset.file_id)
    }

    fn expose(&self, asset: &PhotoAsset, entry: &FileEntry, state: Option<State>) -> Value {
        let state = state.unwrap_or_else(|| self.state_of(entry));
        let thumbnail = if asset.blob_id != entry.blob_id { "pending" } else if asset.thumbnail == "failed" { "failed" } else { "ready" };
        let mut v = json!({ "id": asset.file_id, "name": entry.name, "kind": if asset.kind == "video" { "video" } else { "image" }, "mediaType": entry.media_type,
            "size": entry.size, "takenAt": asset.taken_at, "width": asset.width, "height": asset.height, "duration": asset.duration, "favorite": asset.favorite,
            "archived": asset.archived, "thumbnail": thumbnail, "revision": asset.revision });
        let o = v.as_object_mut().expect("asset");
        if asset.kind == "video" {
            if ["original", "remux", "transcode"].contains(&asset.playback.as_str()) {
                o.insert("playback".into(), json!(asset.playback));
            }
            if let Some(codec) = exif_of(asset)["videoCodec"].as_str().filter(|c| !c.is_empty()) {
                o.insert("videoCodec".into(), json!(codec));
            }
        } else if let Some(motion) = self.live_partner(asset, entry) {
            o.insert("live".into(), json!(motion.file_id));
        }
        if state == State::Library || state == State::Locked {
            if let Some((id, primary, count)) = self.stack_of(asset, state)
                && primary == asset.file_id
            {
                o.insert("stack".into(), json!({ "id": id, "count": count }));
            }
        }
        if state == State::Trash {
            o.insert("trashedAt".into(), json!(entry.modified_at));
        }
        // Favorite and archive belong to the owner; others see neither.
        if !self.viewer.is_empty() && asset.owner_id != self.viewer {
            o.insert("owner".into(), json!(self.ctx.db.accounts().id().find(asset.owner_id.clone()).map(|a| a.display_name).unwrap_or_else(|| "Someone".into())));
            o.insert("favorite".into(), json!(false));
            o.insert("archived".into(), json!(false));
            o.remove("stack");
        }
        v
    }

    fn folder_path(&self, entry: &FileEntry) -> String {
        let mut names: Vec<String> = Vec::new();
        let mut id = entry.parent_id.clone();
        for _ in 0..32 {
            if id.is_empty() {
                break;
            }
            let Some(folder) = self.ctx.db.file_entries().id().find(id.clone()) else { break };
            names.insert(0, folder.name);
            id = folder.parent_id;
        }
        names.join("/")
    }
}

/// Whether an entry is somewhere under a folder; folder chains are cached per call.
fn within<'a>(ctx: &'a ReducerContext, folder_id: String) -> impl Fn(&FileEntry) -> bool + 'a {
    let known: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
    move |entry: &FileEntry| {
        if folder_id.is_empty() {
            return false;
        }
        let mut chain: Vec<String> = Vec::new();
        let mut found = false;
        let mut current = entry.parent_id.clone();
        for _ in 0..128 {
            if current.is_empty() {
                break;
            }
            if let Some(cached) = known.borrow().get(&current) {
                found = *cached;
                break;
            }
            chain.push(current.clone());
            if current == folder_id {
                found = true;
                break;
            }
            current = ctx.db.file_entries().id().find(current.clone()).map(|e| e.parent_id).unwrap_or_default();
        }
        let mut cache = known.borrow_mut();
        for id in chain {
            cache.insert(id, found);
        }
        found
    }
}

fn in_view(view: &str, asset: &PhotoAsset) -> bool {
    match view {
        "favorites" => asset.favorite,
        "videos" => asset.kind == "video" && !asset.archived,
        "archive" => asset.archived,
        "locked" => true,
        _ => !asset.archived,
    }
}

fn album_rows(ctx: &ReducerContext, r: &Reader, album: &PhotoAlbum) -> Vec<(PhotoAsset, FileEntry)> {
    let mut rows = Vec::new();
    for item in ctx.db.photo_album_items().album_id().filter(album.id.as_str()) {
        if let Some(asset) = ctx.db.photo_assets().file_id().find(item.file_id)
            && let Some(entry) = r.file_of(&asset)
            && r.state_of(&entry) == State::Library
            && r.shown(&asset, &entry, State::Library, false)
        {
            rows.push((asset, entry));
        }
    }
    rows
}

fn expose_album(ctx: &ReducerContext, r: &Reader, album: &PhotoAlbum, user_id: &str) -> Value {
    let members = album_rows(ctx, r, album);
    let cover = if !album.cover_id.is_empty() && members.iter().any(|m| m.0.file_id == album.cover_id) { album.cover_id.clone() } else { members.first().map(|m| m.0.file_id.clone()).unwrap_or_default() };
    let mine = album.owner_id == user_id;
    let role = if mine { "owner" } else if ctx.db.photo_album_members().id().find(format!("{}/{user_id}", album.id)).is_some_and(|m| m.role == "editor") { "editor" } else { "viewer" };
    let mut v = json!({ "id": album.id, "name": album.name, "description": album.description, "count": members.len(), "coverId": cover, "updatedAt": album.updated_at, "revision": album.revision, "role": role });
    let o = v.as_object_mut().expect("album");
    if !mine {
        o.insert("owner".into(), json!({ "id": album.owner_id, "name": ctx.db.accounts().id().find(album.owner_id.clone()).map(|a| a.display_name).unwrap_or_else(|| "Someone".into()) }));
    }
    let shared = mine && (ctx.db.photo_album_members().album_id().filter(album.id.as_str()).next().is_some() || ctx.db.photo_links().album_id().filter(album.id.as_str()).next().is_some());
    o.insert("shared".into(), json!(shared));
    v
}

/// A queued indexing job: the runtime reads the blob and reports facts back.
pub fn job(entry: &FileEntry) -> Value {
    json!({ "fileId": entry.id, "blobId": entry.blob_id, "name": entry.name, "mediaType": entry.media_type, "modifiedAt": entry.modified_at, "kind": photo_kind(&entry.media_type, &entry.name).unwrap_or("image") })
}

fn needs_index(ctx: &ReducerContext, entry: &FileEntry) -> bool {
    if entry.kind != "file" || entry.trashed || entry.blob_id.is_empty() || photo_kind(&entry.media_type, &entry.name).is_none() {
        return false;
    }
    ctx.db.photo_assets().file_id().find(entry.id.clone()).is_none_or(|a| a.blob_id != entry.blob_id || a.owner_id != entry.owner_id || a.index_version < INDEX_VERSION)
}

/// Removes an index row with its album entries; a stack left empty goes too.
pub fn forget(ctx: &ReducerContext, file_id: &str) {
    let old = ctx.db.photo_assets().file_id().find(file_id.to_owned());
    for item in ctx.db.photo_album_items().file_id().filter(file_id).collect::<Vec<_>>() {
        delete_album_item(ctx, &item.id);
    }
    for item in ctx.db.photo_link_items().file_id().filter(file_id).collect::<Vec<_>>() {
        ctx.db.photo_link_items().id().delete(item.id);
    }
    let Some(old) = old else { return };
    delete_asset(ctx, file_id);
    if !old.stack_id.is_empty() && ctx.db.photo_assets().stack_id().filter(old.stack_id.as_str()).next().is_none() {
        delete_stack(ctx, &old.stack_id);
    }
}

fn leave_stack(ctx: &ReducerContext, asset: PhotoAsset) -> PhotoAsset {
    if asset.stack_id.is_empty() {
        return asset;
    }
    let stack_id = asset.stack_id.clone();
    let next = update_asset(ctx, PhotoAsset { stack_id: String::new(), ..asset }, false);
    if ctx.db.photo_assets().stack_id().filter(stack_id.as_str()).next().is_none() {
        delete_stack(ctx, &stack_id);
    }
    next
}

/// Duplicate groups: same perceptual hash, shape and kind, outside the trash and Locked folder.
fn duplicate_groups(ctx: &ReducerContext, r: &Reader, user_id: &str, cursor: &str, max_groups: usize) -> Value {
    let mut groups: Vec<Value> = Vec::new();
    let mut scanned = 0;
    let mut current = String::new();
    let mut rows: Vec<PhotoAsset> = Vec::new();
    let mut last = String::new();
    let flush = |current: &str, rows: &[PhotoAsset], groups: &mut Vec<Value>| {
        if current.is_empty() || rows.len() < 2 {
            return;
        }
        let mut members: Vec<(PhotoAsset, FileEntry)> = rows
            .iter()
            .filter_map(|a| r.file_of(a).map(|e| (a.clone(), e)))
            .filter(|(a, e)| r.state_of(e) == State::Library && !(a.kind == "video" && r.live_partner(a, e).is_some()))
            .collect();
        // A RAW+JPEG pair or a burst already grouped in a stack is not a duplicate of itself.
        let distinct: HashSet<String> = members.iter().map(|(a, _)| if a.stack_id.is_empty() { a.file_id.clone() } else { a.stack_id.clone() }).collect();
        if distinct.len() < 2 {
            return;
        }
        let digests: HashSet<&String> = members.iter().map(|(_, e)| &e.blob_id).collect();
        let identical = digests.len() == 1;
        members.sort_by(|a, b| b.1.size.total_cmp(&a.1.size).then_with(|| newest(&a.0, &b.0)));
        let items: Vec<Value> = members
            .iter()
            .map(|(a, e)| {
                let mut v = r.expose(a, e, Some(State::Library));
                v["path"] = json!(r.folder_path(e));
                v
            })
            .collect();
        groups.push(json!({ "key": current, "identical": identical, "items": items }));
    };
    let range = owner_range(user_id, if cursor.is_empty() { None } else { Some(cursor) });
    for row in ctx.db.photo_assets().dup_key().filter(crate::bounds(&range)) {
        if row.dup_key != current {
            flush(&current, &rows, &mut groups);
            if groups.len() >= max_groups {
                return json!({ "groups": groups, "cursor": last });
            }
            current = row.dup_key.clone();
            rows.clear();
        }
        last = row.dup_key.clone();
        rows.push(row);
        scanned += 1;
        if scanned > 50000 {
            return json!({ "groups": groups, "cursor": "" });
        }
    }
    flush(&current, &rows, &mut groups);
    json!({ "groups": groups, "cursor": "" })
}

fn utc_year_month(ms: f64) -> (i64, u32) {
    let (y, m, _) = crate::civil_from_days((ms / 86_400_000.0).floor() as i64);
    (y, m)
}

struct Found {
    row: PhotoAsset,
    entry: FileEntry,
    state: State,
    mine: bool,
}

pub fn photos_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Doc, files: &PhotoFiles) -> Res<Value> {
    if !app_enabled(ctx, "photos") {
        return fail("notFound");
    }
    let at = now(ctx);
    let r = Reader::new(ctx, user_id);
    // Someone else's photo is visible in the library through an album you own or that is shared with you.
    let via_album = |file_id: &str| -> bool {
        ctx.db.photo_album_items().file_id().filter(file_id).any(|item| {
            ctx.db.photo_albums().id().find(item.album_id).is_some_and(|a| a.owner_id == user_id || ctx.db.photo_album_members().id().find(format!("{}/{user_id}", a.id)).is_some())
        })
    };
    let find = |value: Option<&Value>, allowed: &[State], shared: bool| -> Res<Found> {
        let id = opaque(value)?;
        let Some(row) = ctx.db.photo_assets().file_id().find(id) else { return fail("notFound") };
        let Some(entry) = r.file_of(&row) else { return fail("notFound") };
        let state = r.state_of(&entry);
        if row.owner_id != user_id {
            if !shared || state != State::Library || !via_album(&row.file_id) {
                return fail("notFound");
            }
            return Ok(Found { row, entry, state, mine: false });
        }
        if state == State::Locked && allowed.contains(&State::Locked) && !files.unlocked {
            return fail("lockedFolder");
        }
        if !allowed.contains(&state) {
            return fail("notFound");
        }
        Ok(Found { row, entry, state, mine: true })
    };
    let library = [State::Library, State::Locked];
    let album = |value: Option<&Value>, need: &str| -> Res<PhotoAlbum> {
        let id = opaque(value)?;
        let Some(row) = ctx.db.photo_albums().id().find(id) else { return fail("notFound") };
        let role = if row.owner_id == user_id { Some("owner".to_owned()) } else { ctx.db.photo_album_members().id().find(format!("{}/{user_id}", row.id)).map(|m| m.role) };
        let Some(role) = role else { return fail("notFound") };
        if need == "own" && role != "owner" || need == "add" && role == "viewer" {
            return fail("forbidden");
        }
        Ok(row)
    };
    let sealed_link = |link: &PhotoLink| -> Value {
        let count = if link.album_id.is_empty() { ctx.db.photo_link_items().link_id().filter(link.id.as_str()).count() } else { ctx.db.photo_albums().id().find(link.album_id.clone()).map_or(0, |a| album_rows(ctx, &r, &a).len()) };
        let album_name = if link.album_id.is_empty() { String::new() } else { ctx.db.photo_albums().id().find(link.album_id.clone()).map(|a| a.name).unwrap_or_default() };
        json!({ "id": link.id, "albumId": link.album_id, "albumName": album_name, "count": count, "sealed": link.sealed, "expiresAt": link.expires_at,
            "hasPassword": !link.password_verifier.is_empty(), "allowDownload": link.allow_download, "allowUpload": link.allow_upload, "showMetadata": link.show_metadata,
            "createdAt": link.created_at, "revision": link.revision })
    };
    let link = |value: Option<&Value>| -> Res<PhotoLink> {
        let id = opaque(value)?;
        ctx.db.photo_links().id().find(id).filter(|l| l.owner_id == user_id).ok_or_else(|| "notFound".to_owned())
    };
    struct Settings {
        expires_at: Option<f64>,
        password_verifier: Option<String>,
        allow_download: Option<bool>,
        allow_upload: Option<bool>,
        show_metadata: Option<bool>,
    }
    let link_settings = |q: &Doc, album_link: bool| -> Res<Settings> {
        let flag = |k: &str| -> Res<Option<bool>> {
            match q.get(k) {
                None => Ok(None),
                Some(Value::Bool(b)) => Ok(Some(*b)),
                _ => fail("invalidInput"),
            }
        };
        let expires_at = match q.get("expiresAt") {
            None => None,
            v => Some(num(v, 0.0, at + EXPIRY_DAYS * 86_400_000.0)?),
        };
        if expires_at.is_some_and(|e| e != 0.0 && e <= at) {
            return fail("invalidInput");
        }
        let password_verifier = match q.get("passwordVerifier") {
            None => None,
            v => Some(text(v, 300, true)?),
        };
        if password_verifier.as_ref().is_some_and(|v| !v.is_empty() && !v.starts_with("scrypt$")) {
            return fail("invalidInput");
        }
        let allow_upload = flag("allowUpload")?;
        if allow_upload == Some(true) && !album_link {
            return fail("invalidInput");
        }
        Ok(Settings { expires_at, password_verifier, allow_download: flag("allowDownload")?, allow_upload, show_metadata: flag("showMetadata")? })
    };
    let drop_link = |id: &str| {
        for item in ctx.db.photo_link_items().link_id().filter(id).collect::<Vec<_>>() {
            ctx.db.photo_link_items().id().delete(item.id);
        }
        for pass in ctx.db.photo_link_passes().link_id().filter(id).collect::<Vec<_>>() {
            ctx.db.photo_link_passes().digest().delete(pass.digest);
        }
        ctx.db.photo_links().id().delete(id.to_owned());
    };
    let touch = |album: &PhotoAlbum| -> PhotoAlbum { update_album(ctx, PhotoAlbum { updated_at: at, revision: album.revision + 1, ..album.clone() }) };
    // The selected photos plus the motion halves of selected Live Photos.
    let with_partners = |rows: Vec<(PhotoAsset, FileEntry)>| -> Vec<(PhotoAsset, FileEntry)> {
        let mut all: Vec<(PhotoAsset, FileEntry)> = rows.clone();
        for (row, entry) in &rows {
            if let Some(partner) = r.live_partner(row, entry)
                && let Some(pe) = r.file_of(&partner)
                && !all.iter().any(|(a, _)| a.file_id == partner.file_id)
            {
                all.push((partner, pe));
            }
        }
        all
    };
    let fresh = |id: &str| -> FileEntry { ctx.db.file_entries().id().find(id.to_owned()).expect("entry") };
    let trash_rows = || -> Vec<(PhotoAsset, FileEntry)> {
        let mut all = Vec::new();
        let window = crate::folder_window(&format!("{user_id}/trash/"));
        for (scanned, entry) in ctx.db.file_entries().list_key().filter(crate::bounds(&window)).enumerate() {
            if scanned >= 20000 {
                break;
            }
            let asset = if entry.kind == "file" { ctx.db.photo_assets().file_id().find(entry.id.clone()) } else { None };
            if let Some(asset) = asset
                && asset.owner_id == user_id
                && r.file_of(&asset).is_some()
                && !(asset.kind == "video" && r.live_partner(&asset, &entry).is_some())
            {
                all.push((asset, entry));
            }
        }
        all
    };
    match operation {
        "overview" => {
            keys(p, &[])?;
            let mut counts: Vec<(&str, usize)> = vec![("library", 0), ("folder", 0), ("favorites", 0), ("videos", 0), ("archive", 0), ("trash", 0), ("duplicates", 0), ("shared", 0)];
            let mut bump = |k: &str| {
                if let Some(c) = counts.iter_mut().find(|c| c.0 == k) {
                    c.1 += 1;
                }
            };
            let in_folder = within(ctx, (files.upload_folder_id)());
            for (scanned, row) in ctx.db.photo_assets().owner_id().filter(user_id).enumerate() {
                if scanned >= 200000 {
                    break;
                }
                let Some(entry) = r.file_of(&row) else { continue };
                if r.state_of(&entry) != State::Library || !r.shown(&row, &entry, State::Library, true) {
                    continue;
                }
                if !row.archived {
                    bump("library");
                    if in_folder(&entry) {
                        bump("folder");
                    }
                }
                if row.favorite {
                    bump("favorites");
                }
                if row.kind == "video" && !row.archived {
                    bump("videos");
                }
                if row.archived {
                    bump("archive");
                }
            }
            for _ in trash_rows() {
                bump("trash");
            }
            let duplicates = duplicate_groups(ctx, &r, user_id, "", 1000)["groups"].as_array().map_or(0, Vec::len);
            let order = |a: &PhotoAlbum, b: &PhotoAlbum| b.updated_at.total_cmp(&a.updated_at).then_with(|| crate::locale_compare(&a.name, &b.name));
            let mut own: Vec<PhotoAlbum> = ctx.db.photo_albums().owner_id().filter(user_id).collect();
            own.sort_by(order);
            let mut shared: Vec<PhotoAlbum> = ctx
                .db
                .photo_album_members()
                .user_id()
                .filter(user_id)
                .filter_map(|m| ctx.db.photo_albums().id().find(m.album_id))
                .filter(|a| ctx.db.accounts().id().find(a.owner_id.clone()).is_some_and(|o| o.enabled))
                .collect();
            shared.sort_by(order);
            let mut c: Map<String, Value> = counts.into_iter().map(|(k, v)| (k.to_owned(), json!(v))).collect();
            c.insert("duplicates".into(), json!(duplicates));
            c.insert("shared".into(), json!(shared.len()));
            let indexing = ctx.db.photo_state().name().find("scan".to_owned()).map(|s| s.value) != Some("idle".into());
            Ok(json!({ "albums": own.iter().map(|a| expose_album(ctx, &r, a, user_id)).collect::<Vec<_>>(), "sharedAlbums": shared.iter().map(|a| expose_album(ctx, &r, a, user_id)).collect::<Vec<_>>(),
                "counts": c, "indexing": indexing, "uploadFolder": (files.upload_folder)() }))
        }
        "page" => {
            keys(p, &["view", "album", "cursor", "limit"])?;
            let view = text(p.get("view"), 20, false)?;
            if !["library", "folder", "favorites", "videos", "archive", "album", "trash", "locked"].contains(&view.as_str()) {
                return fail("invalidInput");
            }
            let limit = int(Some(p.get("limit").unwrap_or(&json!(120))), 1.0, 500.0)? as usize;
            let cursor = match p.get("cursor") {
                None => String::new(),
                Some(Value::String(s)) if s.is_empty() => String::new(),
                v => text(v, 300, false)?,
            };
            let mut items: Vec<Value> = Vec::new();
            if view == "album" || view == "trash" {
                let offset = if cursor.is_empty() { 0 } else { int(Some(&json!(cursor.parse::<f64>().unwrap_or(f64::NAN))), 0.0, 200000.0)? as usize };
                let all: Vec<(PhotoAsset, FileEntry)> = if view == "album" {
                    let mut rows = album_rows(ctx, &r, &album(p.get("album"), "view")?);
                    rows.sort_by(|x, y| newest(&x.0, &y.0));
                    rows
                } else {
                    // Files keeps trashed items until they are deleted; Photos lists the media among them, newest deletion first.
                    let mut rows = trash_rows();
                    rows.sort_by(|x, y| y.1.modified_at.total_cmp(&x.1.modified_at).then_with(|| newest(&x.0, &y.0)));
                    rows
                };
                for (asset, entry) in all.iter().skip(offset).take(limit) {
                    items.push(r.expose(asset, entry, None));
                }
                let next = if offset + limit < all.len() { (offset + limit).to_string() } else { String::new() };
                return Ok(json!({ "items": items, "cursor": next }));
            }
            let state = if view == "locked" { State::Locked } else { State::Library };
            if state == State::Locked && !files.unlocked {
                return fail("lockedFolder");
            }
            if !cursor.is_empty() && !cursor.starts_with(&format!("{user_id}/")) {
                return fail("invalidInput");
            }
            let in_folder: Box<dyn Fn(&FileEntry) -> bool> = if view == "folder" { Box::new(within(ctx, (files.upload_folder_id)())) } else { Box::new(|_: &FileEntry| true) };
            let range = owner_range(user_id, if cursor.is_empty() { None } else { Some(&cursor) });
            let mut last;
            for (scanned, row) in ctx.db.photo_assets().time_key().filter(crate::bounds(&range)).enumerate() {
                last = row.time_key.clone();
                if scanned + 1 > 20000 {
                    return Ok(json!({ "items": items, "cursor": last }));
                }
                let Some(entry) = r.file_of(&row) else { continue };
                if r.state_of(&entry) != state || !in_view(&view, &row) || !r.shown(&row, &entry, state, true) || !in_folder(&entry) {
                    continue;
                }
                items.push(r.expose(&row, &entry, Some(state)));
                if items.len() >= limit {
                    return Ok(json!({ "items": items, "cursor": last }));
                }
            }
            Ok(json!({ "items": items, "cursor": "" }))
        }
        "detail" => {
            keys(p, &["id"])?;
            let f = find(p.get("id"), &[State::Library, State::Locked, State::Trash], true)?;
            let albums: Vec<Value> = ctx
                .db
                .photo_album_items()
                .file_id()
                .filter(f.row.file_id.as_str())
                .filter_map(|item| ctx.db.photo_albums().id().find(item.album_id))
                .filter(|a| a.owner_id == user_id || ctx.db.photo_album_members().id().find(format!("{}/{user_id}", a.id)).is_some())
                .map(|a| json!({ "id": a.id, "name": a.name }))
                .collect();
            let mut v = r.expose(&f.row, &f.entry, Some(f.state));
            let o = v.as_object_mut().expect("asset");
            o.insert("description".into(), json!(f.row.description));
            o.insert("exif".into(), exif_of(&f.row));
            o.insert("latitude".into(), if f.row.has_location { json!(f.row.latitude) } else { Value::Null });
            o.insert("longitude".into(), if f.row.has_location { json!(f.row.longitude) } else { Value::Null });
            o.insert("albums".into(), json!(albums));
            o.insert("parentId".into(), json!(if f.mine { f.entry.parent_id.clone() } else { String::new() }));
            o.insert("modifiedAt".into(), json!(f.entry.modified_at));
            o.insert("locked".into(), json!(f.state == State::Locked));
            o.insert("stackId".into(), json!(if f.mine { f.row.stack_id.clone() } else { String::new() }));
            o.insert("checksum".into(), json!(if f.mine { f.row.sha1.clone() } else { String::new() }));
            Ok(v)
        }
        "asset" => {
            keys(p, &["id"])?;
            let f = find(p.get("id"), &[State::Library, State::Locked, State::Trash], true)?;
            Ok(json!({ "id": f.row.file_id, "revision": f.entry.revision, "blobId": f.entry.blob_id, "indexedBlobId": f.row.blob_id, "kind": f.row.kind, "mediaType": f.entry.media_type,
                "name": f.entry.name, "thumbnail": f.row.thumbnail, "playback": f.row.playback, "audioCodec": exif_of(&f.row)["audioCodec"].as_str().unwrap_or_default(),
                "duration": f.row.duration, "size": f.entry.size }))
        }
        "indexJob" => {
            // The owner's own new upload; anything else waits for the background scan.
            keys(p, &["id"])?;
            let id = opaque(p.get("id"))?;
            let Some(entry) = ctx.db.file_entries().id().find(id).filter(|e| e.owner_id == user_id) else { return fail("notFound") };
            Ok(json!({ "job": if needs_index(ctx, &entry) { job(&entry) } else { Value::Null } }))
        }
        "update" => {
            keys(p, &["ids", "favorite", "archived"])?;
            if !p.contains_key("favorite") && !p.contains_key("archived") {
                return fail("invalidInput");
            }
            let flag = |k: &str| -> Res<Option<bool>> {
                match p.get(k) {
                    None => Ok(None),
                    Some(Value::Bool(b)) => Ok(Some(*b)),
                    _ => fail("invalidInput"),
                }
            };
            let (favorite, archived) = (flag("favorite")?, flag("archived")?);
            let rows = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &library, false).map(|f| f.row)).collect::<Res<Vec<_>>>()?;
            for row in &rows {
                update_asset(ctx, PhotoAsset { favorite: favorite.unwrap_or(row.favorite), archived: archived.unwrap_or(row.archived), revision: row.revision + 1, ..row.clone() }, false);
            }
            Ok(json!({ "updated": rows.len() }))
        }
        "describe" => {
            keys(p, &["id", "description", "revision"])?;
            let f = find(p.get("id"), &library, false)?;
            if int(p.get("revision"), 0.0, 4_294_967_295.0)? != f.row.revision as f64 {
                return fail("conflict");
            }
            let next = PhotoAsset { description: text(p.get("description"), DESCRIPTION, true)?.trim().to_owned(), revision: f.row.revision + 1, ..f.row };
            let next = update_asset(ctx, next, false);
            Ok(r.expose(&next, &f.entry, Some(f.state)))
        }
        "trash" => {
            // Locked items cannot be trashed (Files deletes them permanently instead).
            keys(p, &["ids"])?;
            let picked = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &[State::Library], false).map(|f| (f.row, f.entry))).collect::<Res<Vec<_>>>()?;
            let rows = with_partners(picked);
            for (row, _) in &rows {
                (files.trash)(&fresh(&row.file_id))?;
            }
            Ok(json!({ "trashed": rows.iter().map(|(a, _)| a.file_id.clone()).collect::<Vec<_>>() }))
        }
        "restore" => {
            keys(p, &["ids"])?;
            let picked = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &[State::Trash], false).map(|f| (f.row, f.entry))).collect::<Res<Vec<_>>>()?;
            let rows = with_partners(picked);
            for (row, _) in &rows {
                (files.restore)(&fresh(&row.file_id))?;
            }
            Ok(json!({ "restored": rows.iter().map(|(a, _)| a.file_id.clone()).collect::<Vec<_>>() }))
        }
        "delete" => {
            keys(p, &["ids"])?;
            let picked = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &[State::Trash, State::Locked], false).map(|f| (f.row, f.entry))).collect::<Res<Vec<_>>>()?;
            let rows = with_partners(picked);
            for (row, _) in &rows {
                (files.remove)(&fresh(&row.file_id))?;
                forget(ctx, &row.file_id);
            }
            Ok(json!({ "deleted": rows.iter().map(|(a, _)| a.file_id.clone()).collect::<Vec<_>>() }))
        }
        "emptyTrash" => {
            // Bounded per call; the browser repeats while more remain.
            keys(p, &[])?;
            let mut deleted = 0;
            let window = crate::folder_window(&format!("{user_id}/trash/"));
            let entries: Vec<FileEntry> = ctx.db.file_entries().list_key().filter(crate::bounds(&window)).collect();
            for (scanned, entry) in entries.into_iter().enumerate() {
                if scanned + 1 > 20000 || deleted >= 200 {
                    return Ok(json!({ "deleted": deleted, "remaining": true }));
                }
                let asset = if entry.kind == "file" { ctx.db.photo_assets().file_id().find(entry.id.clone()) } else { None };
                let Some(asset) = asset.filter(|a| a.owner_id == user_id && r.file_of(a).is_some()) else { continue };
                (files.remove)(&fresh(&entry.id))?;
                forget(ctx, &asset.file_id);
                deleted += 1;
            }
            Ok(json!({ "deleted": deleted, "remaining": false }))
        }
        "lock" => {
            if !files.unlocked {
                return fail("lockedFolder");
            }
            keys(p, &["ids"])?;
            let picked = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &[State::Library], false).map(|f| (f.row, f.entry))).collect::<Res<Vec<_>>>()?;
            let rows = with_partners(picked);
            let folder = (files.locked_folder)();
            let mut moved = Vec::new();
            for (row, entry) in &rows {
                moved.push(json!({ "id": row.file_id, "parentId": entry.parent_id }));
                (files.move_to)(&fresh(&row.file_id), &folder)?;
            }
            Ok(json!({ "moved": moved }))
        }
        "unlock" => {
            // Back to where they came from (Undo) or to the Photos folder by capture month.
            if !files.unlocked {
                return fail("lockedFolder");
            }
            keys(p, &["ids", "to"])?;
            let picked = ids(p.get("ids"), BATCH)?.iter().map(|id| find(Some(&json!(id)), &[State::Locked], false).map(|f| (f.row, f.entry))).collect::<Res<Vec<_>>>()?;
            let rows = with_partners(picked);
            let mut to: HashMap<String, String> = HashMap::new();
            if let Some(list) = p.get("to") {
                let Some(list) = list.as_array().filter(|l| l.len() <= BATCH) else { return fail("invalidInput") };
                for item in list {
                    let Some(item) = item.as_object() else { return fail("invalidInput") };
                    keys(item, &["id", "parentId"])?;
                    let parent = if item.get("parentId").and_then(Value::as_str) == Some("") { String::new() } else { opaque(item.get("parentId"))? };
                    to.insert(opaque(item.get("id"))?, parent);
                }
            }
            for (row, _) in &rows {
                let target = match to.get(&row.file_id) {
                    Some(t) => t.clone(),
                    None => {
                        let (y, m) = utc_year_month(row.taken_at);
                        (files.month_folder)(y, m)?
                    }
                };
                (files.move_to)(&fresh(&row.file_id), &target)?;
            }
            Ok(json!({ "moved": rows.iter().map(|(a, _)| a.file_id.clone()).collect::<Vec<_>>() }))
        }
        "stack" => {
            keys(p, &["ids", "primaryId"])?;
            let list = ids(p.get("ids"), STACK_ITEMS)?;
            if list.len() < 2 {
                return fail("invalidInput");
            }
            let rows = list.iter().map(|id| find(Some(&json!(id)), &library, false)).collect::<Res<Vec<_>>>()?;
            let state = rows[0].state;
            if rows.iter().any(|x| x.state != state) {
                return fail("invalidInput");
            }
            let primary = match p.get("primaryId") {
                None => list[0].clone(),
                v => opaque(v)?,
            };
            if !list.contains(&primary) {
                return fail("invalidInput");
            }
            let id = new_id(ctx);
            insert_stack(ctx, PhotoStack { id: id.clone(), owner_id: user_id.into(), key: String::new(), primary_id: primary.clone(), created_at: at, updated_at: at, sync_seq: 0.0 });
            for f in &rows {
                let current = ctx.db.photo_assets().file_id().find(f.row.file_id.clone()).expect("asset");
                let left = leave_stack(ctx, current);
                update_asset(ctx, PhotoAsset { stack_id: id.clone(), ..left }, false);
            }
            Ok(json!({ "id": id, "primaryId": primary, "count": rows.len() }))
        }
        "stackDetail" => {
            keys(p, &["id"])?;
            let Some(stack) = ctx.db.photo_stacks().id().find(opaque(p.get("id"))?).filter(|s| s.owner_id == user_id) else { return fail("notFound") };
            let cover = ctx.db.photo_assets().file_id().find(stack.primary_id.clone()).or_else(|| ctx.db.photo_assets().stack_id().filter(stack.id.as_str()).next());
            let Some(cover) = cover else { return fail("notFound") };
            let state = find(Some(&json!(cover.file_id)), &library, false)?.state;
            let mut members: Vec<(PhotoAsset, FileEntry)> = ctx.db.photo_assets().stack_id().filter(stack.id.as_str()).filter_map(|a| r.file_of(&a).map(|e| (a, e))).filter(|(_, e)| r.state_of(e) == state).collect();
            members.sort_by(|a, b| newest(&a.0, &b.0));
            let info = r.stack_of(members.first().map_or(&cover, |m| &m.0), state);
            Ok(json!({ "id": stack.id, "primaryId": info.map(|i| i.1).unwrap_or(cover.file_id.clone()), "items": members.iter().map(|(a, e)| r.expose(a, e, Some(state))).collect::<Vec<_>>() }))
        }
        "stackCover" => {
            keys(p, &["id", "primaryId"])?;
            let Some(stack) = ctx.db.photo_stacks().id().find(opaque(p.get("id"))?).filter(|s| s.owner_id == user_id) else { return fail("notFound") };
            let f = find(p.get("primaryId"), &library, false)?;
            if f.row.stack_id != stack.id {
                return fail("invalidInput");
            }
            update_stack(ctx, PhotoStack { primary_id: f.row.file_id.clone(), updated_at: at, ..stack.clone() });
            Ok(json!({ "id": stack.id, "primaryId": f.row.file_id }))
        }
        "unstack" => {
            keys(p, &["id", "ids"])?;
            let Some(stack) = ctx.db.photo_stacks().id().find(opaque(p.get("id"))?).filter(|s| s.owner_id == user_id) else { return fail("notFound") };
            let only: Option<HashSet<String>> = match p.get("ids") {
                None => None,
                v => Some(ids(v, BATCH)?.into_iter().collect()),
            };
            let mut removed = 0;
            for row in ctx.db.photo_assets().stack_id().filter(stack.id.as_str()).collect::<Vec<_>>() {
                if only.as_ref().is_none_or(|o| o.contains(&row.file_id)) {
                    leave_stack(ctx, row);
                    removed += 1;
                }
            }
            Ok(json!({ "removed": removed }))
        }
        "duplicates" => {
            keys(p, &["cursor"])?;
            let cursor = match p.get("cursor") {
                None => String::new(),
                Some(Value::String(s)) if s.is_empty() => String::new(),
                v => text(v, 400, false)?,
            };
            if !cursor.is_empty() && !cursor.starts_with(&format!("{user_id}/")) {
                return fail("invalidInput");
            }
            Ok(duplicate_groups(ctx, &r, user_id, &cursor, 20))
        }
        "resolveDuplicates" => {
            // Keeps the chosen photos, gives them the others' albums, favorite and description, and trashes the rest.
            keys(p, &["key", "keep"])?;
            let key = text(p.get("key"), 400, false)?;
            if !key.starts_with(&format!("{user_id}/")) {
                return fail("invalidInput");
            }
            let members: Vec<(PhotoAsset, FileEntry)> = ctx.db.photo_assets().dup_key().filter(key.as_str()).filter_map(|a| r.file_of(&a).map(|e| (a, e))).filter(|(_, e)| r.state_of(e) == State::Library).collect();
            let keep = ids(p.get("keep"), BATCH)?;
            if keep.iter().any(|id| !members.iter().any(|m| &m.0.file_id == id)) {
                return fail("conflict");
            }
            let others: Vec<(PhotoAsset, FileEntry)> = members.iter().filter(|m| !keep.contains(&m.0.file_id)).cloned().collect();
            let target = ctx.db.photo_assets().file_id().find(keep[0].clone()).expect("asset");
            let mut next = target.clone();
            for (row, _) in &others {
                next.favorite = next.favorite || row.favorite;
                if next.description.is_empty() {
                    next.description = row.description.clone();
                }
                for item in ctx.db.photo_album_items().file_id().filter(row.file_id.as_str()).collect::<Vec<_>>() {
                    let id = format!("{}/{}", item.album_id, target.file_id);
                    if ctx.db.photo_album_items().id().find(id.clone()).is_none() {
                        insert_album_item(ctx, PhotoAlbumItem { id, album_id: item.album_id, file_id: target.file_id.clone(), added_at: at, sync_seq: 0.0 });
                    }
                }
            }
            if next.favorite != target.favorite || next.description != target.description {
                update_asset(ctx, PhotoAsset { revision: target.revision + 1, ..next }, false);
            }
            let trashed = with_partners(others);
            for (row, _) in &trashed {
                (files.trash)(&fresh(&row.file_id))?;
            }
            Ok(json!({ "trashed": trashed.iter().map(|(a, _)| a.file_id.clone()).collect::<Vec<_>>(), "kept": keep }))
        }
        "dismissDuplicates" => {
            keys(p, &["key"])?;
            let key = text(p.get("key"), 400, false)?;
            if !key.starts_with(&format!("{user_id}/")) {
                return fail("invalidInput");
            }
            let mut dismissed = 0;
            for row in ctx.db.photo_assets().dup_key().filter(key.as_str()).collect::<Vec<_>>() {
                if row.owner_id == user_id {
                    update_asset(ctx, PhotoAsset { dup_key: String::new(), ..row }, false);
                    dismissed += 1;
                }
            }
            Ok(json!({ "dismissed": dismissed }))
        }
        "createAlbum" => {
            keys(p, &["id", "name", "ids"])?;
            let id = opaque(p.get("id"))?;
            let name = text(p.get("name"), ALBUM_NAME, false)?.trim().to_owned();
            if ctx.db.photo_albums().id().find(id.clone()).is_some() {
                return fail("alreadyExists");
            }
            if ctx.db.photo_albums().owner_id().filter(user_id).count() >= ALBUMS {
                return fail("quotaExceeded");
            }
            let created = insert_album(ctx, PhotoAlbum { id: id.clone(), owner_id: user_id.into(), name, description: String::new(), cover_id: String::new(), created_at: at, updated_at: at, revision: 1, sync_seq: 0.0 });
            if p.contains_key("ids") {
                for file_id in ids(p.get("ids"), BATCH)? {
                    find(Some(&json!(file_id)), &[State::Library], false)?;
                    insert_album_item(ctx, PhotoAlbumItem { id: format!("{id}/{file_id}"), album_id: id.clone(), file_id, added_at: at, sync_seq: 0.0 });
                }
            }
            Ok(expose_album(ctx, &r, &created, user_id))
        }
        "editAlbum" => {
            keys(p, &["id", "name", "description", "coverId", "revision"])?;
            let a = album(p.get("id"), "own")?;
            if int(p.get("revision"), 0.0, 4_294_967_295.0)? != a.revision as f64 {
                return fail("conflict");
            }
            let cover_id = match p.get("coverId") {
                None => a.cover_id.clone(),
                Some(Value::String(s)) if s.is_empty() => String::new(),
                v => find(v, &[State::Library], true)?.row.file_id,
            };
            let name = if p.contains_key("name") { text(p.get("name"), ALBUM_NAME, false)?.trim().to_owned() } else { a.name.clone() };
            let description = if p.contains_key("description") { text(p.get("description"), DESCRIPTION, true)?.trim().to_owned() } else { a.description.clone() };
            let next = update_album(ctx, PhotoAlbum { name, description, cover_id, updated_at: at, revision: a.revision + 1, ..a });
            Ok(expose_album(ctx, &r, &next, user_id))
        }
        "deleteAlbum" => {
            // People it was shared with lose it and its links stop working; the photos stay.
            keys(p, &["id"])?;
            let a = album(p.get("id"), "own")?;
            for item in ctx.db.photo_album_items().album_id().filter(a.id.as_str()).collect::<Vec<_>>() {
                delete_album_item(ctx, &item.id);
            }
            for member in ctx.db.photo_album_members().album_id().filter(a.id.as_str()).collect::<Vec<_>>() {
                delete_album_member(ctx, &member.id);
            }
            for l in ctx.db.photo_links().album_id().filter(a.id.as_str()).collect::<Vec<_>>() {
                drop_link(&l.id);
            }
            delete_album(ctx, &a.id);
            Ok(json!({ "deleted": true }))
        }
        "addToAlbum" => {
            // Editors of a shared album add their own photos.
            keys(p, &["id", "ids"])?;
            let a = album(p.get("id"), "add")?;
            let list = ids(p.get("ids"), BATCH)?;
            let mut added = 0;
            let mut total = ctx.db.photo_album_items().album_id().filter(a.id.as_str()).count();
            for file_id in list {
                find(Some(&json!(file_id)), &[State::Library], false)?;
                let id = format!("{}/{file_id}", a.id);
                if ctx.db.photo_album_items().id().find(id.clone()).is_some() {
                    continue;
                }
                total += 1;
                if total > ALBUM_ITEMS {
                    return fail("quotaExceeded");
                }
                insert_album_item(ctx, PhotoAlbumItem { id, album_id: a.id.clone(), file_id, added_at: at, sync_seq: 0.0 });
                added += 1;
            }
            let mut v = expose_album(ctx, &r, &touch(&a), user_id);
            v["added"] = json!(added);
            Ok(v)
        }
        "removeFromAlbum" => {
            // The owner removes anything; an editor removes only their own photos.
            keys(p, &["id", "ids"])?;
            let a = album(p.get("id"), "add")?;
            let owner = a.owner_id == user_id;
            let mut removed = 0;
            for file_id in ids(p.get("ids"), BATCH)? {
                let id = format!("{}/{file_id}", a.id);
                if ctx.db.photo_album_items().id().find(id.clone()).is_none() {
                    continue;
                }
                if !owner && ctx.db.photo_assets().file_id().find(file_id).is_none_or(|x| x.owner_id != user_id) {
                    return fail("forbidden");
                }
                delete_album_item(ctx, &id);
                removed += 1;
            }
            let base = if !a.cover_id.is_empty() && ctx.db.photo_album_items().id().find(format!("{}/{}", a.id, a.cover_id)).is_none() { PhotoAlbum { cover_id: String::new(), ..a } } else { a };
            let mut v = expose_album(ctx, &r, &touch(&base), user_id);
            v["removed"] = json!(removed);
            Ok(v)
        }
        "albumSharing" => {
            keys(p, &["id"])?;
            let a = album(p.get("id"), "own")?;
            let mut members: Vec<Value> = ctx
                .db
                .photo_album_members()
                .album_id()
                .filter(a.id.as_str())
                .filter_map(|m| ctx.db.accounts().id().find(m.user_id.clone()).map(|acc| (m, acc)))
                .map(|(m, acc)| json!({ "userId": m.user_id, "username": acc.username, "displayName": acc.display_name, "role": if m.role == "editor" { "editor" } else { "viewer" } }))
                .collect();
            members.sort_by(|x, y| crate::locale_compare(x["displayName"].as_str().unwrap_or_default(), y["displayName"].as_str().unwrap_or_default()));
            let mut links: Vec<PhotoLink> = ctx.db.photo_links().album_id().filter(a.id.as_str()).collect();
            links.sort_by(|x, y| x.created_at.total_cmp(&y.created_at));
            Ok(json!({ "members": members, "links": links.iter().map(sealed_link).collect::<Vec<_>>() }))
        }
        "shareAlbum" => {
            keys(p, &["id", "username", "role"])?;
            let a = album(p.get("id"), "own")?;
            let role = text(p.get("role"), 10, false)?;
            if role != "viewer" && role != "editor" {
                return fail("invalidInput");
            }
            let username = text(p.get("username"), 64, false)?;
            let username = username.trim();
            let username = username.strip_prefix('@').unwrap_or(username).to_lowercase();
            let person = ctx.db.accounts().username().find(username);
            let Some(person) = person.filter(|x| x.enabled && x.id != user_id && !ctx.db.directory_profiles().user_id().find(x.id.clone()).is_some_and(|d| d.deleted)) else { return fail("notFound") };
            let id = format!("{}/{}", a.id, person.id);
            match ctx.db.photo_album_members().id().find(id.clone()) {
                Some(existing) => {
                    update_album_member(ctx, PhotoAlbumMember { role: role.clone(), ..existing });
                }
                None => {
                    if ctx.db.photo_album_members().album_id().filter(a.id.as_str()).count() >= MEMBERS {
                        return fail("quotaExceeded");
                    }
                    insert_album_member(ctx, PhotoAlbumMember { id, album_id: a.id.clone(), user_id: person.id.clone(), role: role.clone(), added_at: at, sync_seq: 0.0 });
                }
            }
            touch(&a);
            Ok(json!({ "userId": person.id, "username": person.username, "displayName": person.display_name, "role": role }))
        }
        "unshareAlbum" => {
            // The owner removes anyone; a member may leave.
            keys(p, &["id", "userId"])?;
            let a = album(p.get("id"), "view")?;
            let target = opaque(p.get("userId"))?;
            if a.owner_id != user_id && target != user_id {
                return fail("forbidden");
            }
            let id = format!("{}/{target}", a.id);
            if ctx.db.photo_album_members().id().find(id.clone()).is_none() {
                return fail("notFound");
            }
            delete_album_member(ctx, &id);
            Ok(json!({ "removed": true }))
        }
        "links" => {
            keys(p, &[])?;
            let mut links: Vec<PhotoLink> = ctx.db.photo_links().owner_id().filter(user_id).collect();
            links.sort_by(|x, y| y.created_at.total_cmp(&x.created_at));
            Ok(json!({ "links": links.iter().map(sealed_link).collect::<Vec<_>>() }))
        }
        "createLink" => {
            // To one of your albums, or to photos of yours (a fixed set).
            keys(p, &["id", "digest", "sealed", "albumId", "ids", "expiresAt", "passwordVerifier", "allowDownload", "allowUpload", "showMetadata"])?;
            let id = opaque(p.get("id"))?;
            let digest = text(p.get("digest"), 64, false)?;
            let hex64 = digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if !hex64 || ctx.db.photo_links().id().find(id.clone()).is_some() || ctx.db.photo_links().digest().find(digest.clone()).is_some() {
                return fail("conflict");
            }
            if ctx.db.photo_links().owner_id().filter(user_id).count() >= LINKS {
                return fail("quotaExceeded");
            }
            if p.contains_key("albumId") == p.contains_key("ids") {
                return fail("invalidInput");
            }
            let album_id = if p.contains_key("albumId") { album(p.get("albumId"), "own")?.id } else { String::new() };
            let chosen = if p.contains_key("ids") { ids(p.get("ids"), BATCH)?.iter().map(|f| find(Some(&json!(f)), &[State::Library], false).map(|x| x.row.file_id)).collect::<Res<Vec<_>>>()? } else { Vec::new() };
            let s = link_settings(p, !album_id.is_empty())?;
            let created = PhotoLink {
                id: id.clone(),
                owner_id: user_id.into(),
                digest,
                sealed: text(p.get("sealed"), 2000, false)?,
                album_id,
                expires_at: s.expires_at.unwrap_or(0.0),
                password_verifier: s.password_verifier.unwrap_or_default(),
                allow_download: s.allow_download.unwrap_or(true),
                allow_upload: s.allow_upload.unwrap_or(false),
                show_metadata: s.show_metadata.unwrap_or(true),
                created_at: at,
                revision: 1,
            };
            ctx.db.photo_links().insert(created.clone());
            for file_id in chosen {
                ctx.db.photo_link_items().insert(PhotoLinkItem { id: format!("{id}/{file_id}"), link_id: id.clone(), file_id });
            }
            Ok(sealed_link(&created))
        }
        "editLink" => {
            keys(p, &["id", "revision", "expiresAt", "passwordVerifier", "allowDownload", "allowUpload", "showMetadata"])?;
            let l = link(p.get("id"))?;
            if int(p.get("revision"), 0.0, 4_294_967_295.0)? != l.revision as f64 {
                return fail("conflict");
            }
            let s = link_settings(p, !l.album_id.is_empty())?;
            let password_changed = s.password_verifier.is_some();
            let next = PhotoLink {
                expires_at: s.expires_at.unwrap_or(l.expires_at),
                password_verifier: s.password_verifier.unwrap_or(l.password_verifier.clone()),
                allow_download: s.allow_download.unwrap_or(l.allow_download),
                allow_upload: s.allow_upload.unwrap_or(l.allow_upload),
                show_metadata: s.show_metadata.unwrap_or(l.show_metadata),
                revision: l.revision + 1,
                ..l.clone()
            };
            ctx.db.photo_links().id().update(next.clone());
            // A new or removed password signs visitors out.
            if password_changed {
                for pass in ctx.db.photo_link_passes().link_id().filter(l.id.as_str()).collect::<Vec<_>>() {
                    ctx.db.photo_link_passes().digest().delete(pass.digest);
                }
            }
            Ok(sealed_link(&next))
        }
        "deleteLink" => {
            keys(p, &["id"])?;
            drop_link(&link(p.get("id"))?.id);
            Ok(json!({ "deleted": true }))
        }
        _ => fail("notFound"),
    }
}

fn shape(width: u32, height: u32) -> &'static str {
    let (w, h) = (width as f64, height as f64);
    if w > h * 1.05 { "l" } else if h > w * 1.05 { "p" } else { "s" }
}

/// RAW+JPEG pairs stack with the JPEG/HEIC as cover; burst frames share one stack per BurstUUID.
fn auto_stack(ctx: &ReducerContext, asset: &PhotoAsset, entry: &FileEntry, burst_id: &str, at: f64) -> String {
    if !asset.stack_id.is_empty() || asset.kind != "image" {
        return asset.stack_id.clone();
    }
    if !burst_id.is_empty() {
        let key = format!("{}/burst:{burst_id}", asset.owner_id);
        if let Some(stack) = ctx.db.photo_stacks().key().filter(key.as_str()).next() {
            return stack.id;
        }
        let id = new_id(ctx);
        insert_stack(ctx, PhotoStack { id: id.clone(), owner_id: asset.owner_id.clone(), key, primary_id: asset.file_id.clone(), created_at: at, updated_at: at, sync_seq: 0.0 });
        return id;
    }
    let (stem, ext) = name_parts(&entry.name);
    let raw = RAW.contains(&ext.as_str());
    if !raw && !STILL.contains(&ext.as_str()) {
        return String::new();
    }
    let others: &[&str] = if raw { &STILL } else { &RAW };
    for other in others {
        let sibling = ctx.db.file_entries().list_key().filter(format!("{}/{}/{}", entry.owner_id, entry.parent_id, format!("{stem}.{other}").to_lowercase()).as_str()).next();
        let Some(pair) = sibling.and_then(|s| ctx.db.photo_assets().file_id().find(s.id)) else { continue };
        if pair.owner_id != asset.owner_id || pair.kind != "image" {
            continue;
        }
        if !pair.stack_id.is_empty() {
            return pair.stack_id;
        }
        let id = new_id(ctx);
        insert_stack(ctx, PhotoStack { id: id.clone(), owner_id: asset.owner_id.clone(), key: String::new(), primary_id: if raw { pair.file_id.clone() } else { asset.file_id.clone() }, created_at: at, updated_at: at, sync_seq: 0.0 });
        update_asset(ctx, PhotoAsset { stack_id: id.clone(), ..pair }, false);
        return id;
    }
    String::new()
}

fn put_state(ctx: &ReducerContext, name: &str, value: &str) {
    let next = PhotoState { name: name.into(), value: value.into() };
    if ctx.db.photo_state().name().find(name.to_owned()).is_some() {
        ctx.db.photo_state().name().update(next);
    } else {
        ctx.db.photo_state().insert(next);
    }
}

/// Service-only indexing: bounded scans of Files for media that needs facts, and storing them.
pub fn photos_worker(ctx: &ReducerContext, operation: &str, p: &Doc) -> Res<Value> {
    let at = now(ctx);
    match operation {
        "scan" => {
            keys(p, &["limit", "jobs"])?;
            let limit = int(p.get("limit"), 1.0, 20000.0)? as usize;
            let max = int(p.get("jobs"), 1.0, 100.0)? as usize;
            if !app_enabled(ctx, "photos") {
                return Ok(json!({ "jobs": [], "done": true }));
            }
            let cursor = ctx.db.photo_state().name().find("cursor".to_owned()).map(|s| s.value).unwrap_or_default();
            let range = (if cursor.is_empty() { std::ops::Bound::Unbounded } else { std::ops::Bound::Excluded(cursor) }, std::ops::Bound::Unbounded);
            let mut jobs = Vec::new();
            let mut last = String::new();
            let mut scanned = 0;
            for entry in ctx.db.file_entries().list_key().filter(crate::bounds(&range)) {
                last = entry.list_key.clone();
                if needs_index(ctx, &entry) {
                    jobs.push(job(&entry));
                }
                scanned += 1;
                if scanned >= limit || jobs.len() >= max {
                    break;
                }
            }
            let done = scanned < limit && jobs.len() < max;
            // Remove index rows for files that no longer exist; metadata of trashed files is kept for restore.
            let sweep = ctx.db.photo_state().name().find("sweep".to_owned()).map(|s| s.value).unwrap_or_default();
            let range = (if sweep.is_empty() { std::ops::Bound::Unbounded } else { std::ops::Bound::Excluded(sweep) }, std::ops::Bound::Unbounded);
            let batch: Vec<PhotoAsset> = ctx.db.photo_assets().time_key().filter(crate::bounds(&range)).take(2000).collect();
            let mut swept = String::new();
            let mut checked = 0;
            for asset in batch {
                swept = asset.time_key.clone();
                checked += 1;
                let entry = ctx.db.file_entries().id().find(asset.file_id.clone());
                if entry.is_none_or(|e| e.owner_id != asset.owner_id || e.kind != "file") {
                    forget(ctx, &asset.file_id);
                }
            }
            put_state(ctx, "cursor", if done { "" } else { &last });
            put_state(ctx, "sweep", if checked >= 2000 { &swept } else { "" });
            put_state(ctx, "scan", if done && jobs.is_empty() { "idle" } else { "busy" });
            if done {
                prune_tombstones(ctx, at - 31.0 * 86_400_000.0);
            }
            Ok(json!({ "jobs": jobs, "done": done }))
        }
        "store" => {
            keys(p, &["fileId", "blobId", "facts", "thumbnail"])?;
            let file_id = opaque(p.get("fileId"))?;
            let blob_id = text(p.get("blobId"), 200, false)?;
            let Some(entry) = ctx.db.file_entries().id().find(file_id).filter(|e| e.blob_id == blob_id && photo_kind(&e.media_type, &e.name).is_some()) else { return Ok(json!({ "stored": false })) };
            let Some(f) = p.get("facts").and_then(Value::as_object) else { return fail("invalidInput") };
            let taken_at = num(f.get("takenAt"), 0.0, 8.64e15)?;
            let width = int(f.get("width"), 0.0, 4_294_967_295.0)? as u32;
            let height = int(f.get("height"), 0.0, 4_294_967_295.0)? as u32;
            let duration = num(f.get("duration"), 0.0, 1e9)?;
            let latitude = if f.get("latitude") == Some(&Value::Null) { None } else { Some(num(f.get("latitude"), -90.0, 90.0)?) };
            let longitude = if f.get("longitude") == Some(&Value::Null) { None } else { Some(num(f.get("longitude"), -180.0, 180.0)?) };
            let exif = crate::stringify(f.get("exif").filter(|e| !e.is_null()).unwrap_or(&json!({})));
            if exif.encode_utf16().count() > 4000 {
                return fail("invalidInput");
            }
            let optional = |k: &str, max: usize| -> Res<String> { if f.get(k).and_then(Value::as_str) == Some("") { Ok(String::new()) } else { text(f.get(k), max, false) } };
            let phash = optional("phash", 16)?;
            if !phash.is_empty() && !(phash.len() == 16 && phash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))) {
                return fail("invalidInput");
            }
            let playback = optional("playback", 20)?;
            if !playback.is_empty() && !["original", "remux", "transcode"].contains(&playback.as_str()) {
                return fail("invalidInput");
            }
            let content_id = optional("contentId", 200)?;
            let burst_id = optional("burstId", 200)?;
            let sha1 = if f.get("sha1").is_none() || f.get("sha1").and_then(Value::as_str) == Some("") { String::new() } else { text(f.get("sha1"), 28, false)? };
            if !sha1.is_empty() && !(sha1.len() == 28 && sha1.ends_with('=') && sha1[..27].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')) {
                return fail("invalidInput");
            }
            let thumbnail = if p.get("thumbnail").and_then(Value::as_str) == Some("failed") { "failed" } else { "ready" };
            let old = ctx.db.photo_assets().file_id().find(entry.id.clone());
            if let Some(o) = &old
                && o.owner_id != entry.owner_id
            {
                forget(ctx, &o.file_id);
            }
            let keep = old.filter(|o| o.owner_id == entry.owner_id).and_then(|o| ctx.db.photo_assets().file_id().find(o.file_id));
            let kind = photo_kind(&entry.media_type, &entry.name).unwrap_or("image");
            // "Not duplicates" stays dismissed while the picture is the same.
            let dup_key = if phash.is_empty() || keep.as_ref().is_some_and(|k| k.dup_key.is_empty() && k.phash == phash) {
                String::new()
            } else {
                format!("{}/{kind}:{phash}:{}{}", entry.owner_id, shape(width, height), if kind == "video" { format!(":{}", crate::js_round(duration) as i64) } else { String::new() })
            };
            let asset = PhotoAsset {
                file_id: entry.id.clone(),
                owner_id: entry.owner_id.clone(),
                blob_id: entry.blob_id.clone(),
                time_key: photo_time_key(&entry.owner_id, taken_at, &entry.id),
                kind: kind.into(),
                taken_at,
                width,
                height,
                duration,
                exif,
                has_location: latitude.is_some() && longitude.is_some(),
                latitude: latitude.unwrap_or(0.0),
                longitude: longitude.unwrap_or(0.0),
                favorite: keep.as_ref().is_some_and(|k| k.favorite),
                archived: keep.as_ref().is_some_and(|k| k.archived),
                description: keep.as_ref().map(|k| k.description.clone()).unwrap_or_default(),
                thumbnail: thumbnail.into(),
                revision: keep.as_ref().map_or(0, |k| k.revision) + 1,
                indexed_at: at,
                index_version: INDEX_VERSION,
                phash,
                dup_key,
                playback: if kind == "video" { playback } else { String::new() },
                live_key: if content_id.is_empty() { String::new() } else { format!("{}/{content_id}", entry.owner_id) },
                stack_id: keep.as_ref().map(|k| k.stack_id.clone()).unwrap_or_default(),
                sha1,
                sync_seq: 0.0,
                sync_key: String::new(),
            };
            let stored = if keep.is_some() { update_asset(ctx, asset.clone(), false) } else { insert_asset(ctx, asset.clone()) };
            let stack_id = auto_stack(ctx, &asset, &entry, &burst_id, at);
            if stack_id != asset.stack_id {
                update_asset(ctx, PhotoAsset { stack_id, ..stored }, false);
            }
            Ok(json!({ "stored": true }))
        }
        _ => fail("notFound"),
    }
}

pub fn photos_search(ctx: &ReducerContext, user_id: &str, s: &mut crate::global_search::Search) {
    let r = Reader::new(ctx, "");
    let shared: Vec<PhotoAlbum> = ctx.db.photo_album_members().user_id().filter(user_id).filter_map(|m| ctx.db.photo_albums().id().find(m.album_id)).collect();
    for album in ctx.db.photo_albums().owner_id().filter(user_id).chain(shared) {
        if !s.visit() {
            return;
        }
        if s.matches(&album.name) || s.matches(&album.description) {
            let detail = if album.owner_id == user_id { "Photos album" } else { "Shared Photos album" };
            s.found("Photos", format!("album-{}", album.id), album.name.clone(), detail.into(), format!("/photos?album={}", album.id), String::new());
        }
    }
    for asset in ctx.db.photo_assets().owner_id().filter(user_id) {
        if !s.visit() {
            return;
        }
        let Some(entry) = r.file_of(&asset) else { continue };
        if r.state_of(&entry) != State::Library {
            continue;
        }
        let exif = exif_of(&asset);
        let camera = format!("{} {}", exif["make"].as_str().unwrap_or_default(), exif["model"].as_str().unwrap_or_default());
        if s.matches(&entry.name) || s.matches(&asset.description) || s.matches(&camera) {
            let date = crate::iso_string(asset.taken_at)[..10].to_owned();
            let detail = [date, asset.description.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            s.found("Photos", asset.file_id.clone(), entry.name.clone(), detail, format!("/photos?view=library&asset={}", asset.file_id), format!("/api/v1/photos/assets/{}/thumbnail", asset.file_id));
        }
    }
}

pub fn delete_photos_account(ctx: &ReducerContext, owner_id: &str) {
    for album in ctx.db.photo_albums().owner_id().filter(owner_id).collect::<Vec<_>>() {
        for item in ctx.db.photo_album_items().album_id().filter(album.id.as_str()).collect::<Vec<_>>() {
            delete_album_item(ctx, &item.id);
        }
        for member in ctx.db.photo_album_members().album_id().filter(album.id.as_str()).collect::<Vec<_>>() {
            delete_album_member(ctx, &member.id);
        }
        delete_album(ctx, &album.id);
    }
    for member in ctx.db.photo_album_members().user_id().filter(owner_id).collect::<Vec<_>>() {
        delete_album_member(ctx, &member.id);
    }
    for l in ctx.db.photo_links().owner_id().filter(owner_id).collect::<Vec<_>>() {
        for item in ctx.db.photo_link_items().link_id().filter(l.id.as_str()).collect::<Vec<_>>() {
            ctx.db.photo_link_items().id().delete(item.id);
        }
        for pass in ctx.db.photo_link_passes().link_id().filter(l.id.as_str()).collect::<Vec<_>>() {
            ctx.db.photo_link_passes().digest().delete(pass.digest);
        }
        ctx.db.photo_links().id().delete(l.id);
    }
    for asset in ctx.db.photo_assets().owner_id().filter(owner_id).collect::<Vec<_>>() {
        forget(ctx, &asset.file_id);
    }
    for stack in ctx.db.photo_stacks().owner_id().filter(owner_id).collect::<Vec<_>>() {
        delete_stack(ctx, &stack.id);
    }
    for session in ctx.db.immich_sessions().user_id().filter(owner_id).collect::<Vec<_>>() {
        for c in ctx.db.immich_checkpoints().immich_id().filter(session.id.as_str()).collect::<Vec<_>>() {
            ctx.db.immich_checkpoints().id().delete(c.id);
        }
        ctx.db.immich_sessions().digest().delete(session.digest);
    }
    ctx.db.immich_connections().user_id().delete(owner_id.to_owned());
    for remote in ctx.db.immich_remote().user_id().filter(owner_id).collect::<Vec<_>>() {
        ctx.db.immich_remote().id().delete(remote.id);
    }
}

pub fn photos_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "assets": ctx.db.photo_assets().iter().map(|a| { let mut v = row(&a); v.as_object_mut().expect("row").remove("timeKey"); v }).collect::<Vec<_>>(),
        "albums": ctx.db.photo_albums().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "items": ctx.db.photo_album_items().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "stacks": ctx.db.photo_stacks().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "members": ctx.db.photo_album_members().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "links": ctx.db.photo_links().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "linkItems": ctx.db.photo_link_items().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "immichConnections": ctx.db.immich_connections().iter().map(|r| row(&r)).collect::<Vec<_>>(),
        "immichRemote": ctx.db.immich_remote().iter().map(|r| row(&r)).collect::<Vec<_>>(),
    })
}

/// Fills columns older backups lack with their table defaults.
fn with_defaults(value: &Value, defaults: &[(&str, Value)]) -> Value {
    let mut v = value.clone();
    if let Some(o) = v.as_object_mut() {
        for (k, d) in defaults {
            o.entry(k.to_string()).or_insert_with(|| d.clone());
        }
    }
    v
}

/// V8 backups carry Photos metadata; derived images are regenerated. Later columns are optional.
pub fn restore_photos(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    if !value["assets"].is_array() || !value["albums"].is_array() || !value["items"].is_array() {
        return fail("invalidInput");
    }
    for k in ["stacks", "members", "links", "linkItems", "immichConnections", "immichRemote"] {
        if !value[k].is_null() && !value[k].is_array() {
            return fail("invalidInput");
        }
    }
    let list = |k: &str| value[k].as_array().cloned().unwrap_or_default();
    let parse = |v: Value| -> Res<_> { Ok(v) };
    for v in list("stacks") {
        let s: PhotoStack = serde_json::from_value(with_defaults(&parse(v)?, &[("syncSeq", json!(0))])).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(s.owner_id.clone()).is_none() {
            return fail("invalidInput");
        }
        crate::opaque_id(&s.id)?;
        insert_stack(ctx, s);
    }
    for v in list("assets") {
        let v = with_defaults(&v, &[("indexVersion", json!(1)), ("phash", json!("")), ("dupKey", json!("")), ("playback", json!("")), ("liveKey", json!("")), ("stackId", json!("")), ("sha1", json!("")), ("syncSeq", json!(0)), ("syncKey", json!("")), ("timeKey", json!(""))]);
        let mut a: PhotoAsset = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(a.owner_id.clone()).is_none() {
            return fail("invalidInput");
        }
        crate::opaque_id(&a.file_id)?;
        if !a.stack_id.is_empty() && ctx.db.photo_stacks().id().find(a.stack_id.clone()).is_none() {
            a.stack_id = String::new();
        }
        let taken = num(Some(&json!(a.taken_at)), 0.0, 8.64e15)?;
        a.time_key = photo_time_key(&a.owner_id, taken, &a.file_id);
        insert_asset(ctx, a);
    }
    for v in list("albums") {
        let a: PhotoAlbum = serde_json::from_value(with_defaults(&v, &[("syncSeq", json!(0))])).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(a.owner_id.clone()).is_none() {
            return fail("invalidInput");
        }
        crate::opaque_id(&a.id)?;
        insert_album(ctx, a);
    }
    for v in list("items") {
        let i: PhotoAlbumItem = serde_json::from_value(with_defaults(&v, &[("syncSeq", json!(0))])).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.photo_albums().id().find(i.album_id.clone()).is_none() || i.id != format!("{}/{}", i.album_id, i.file_id) {
            return fail("invalidInput");
        }
        insert_album_item(ctx, i);
    }
    for v in list("members") {
        let m: PhotoAlbumMember = serde_json::from_value(with_defaults(&v, &[("syncSeq", json!(0))])).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.photo_albums().id().find(m.album_id.clone()).is_none() || ctx.db.accounts().id().find(m.user_id.clone()).is_none() || m.id != format!("{}/{}", m.album_id, m.user_id) {
            return fail("invalidInput");
        }
        insert_album_member(ctx, m);
    }
    for v in list("links") {
        let l: PhotoLink = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(l.owner_id.clone()).is_none() || !l.album_id.is_empty() && ctx.db.photo_albums().id().find(l.album_id.clone()).is_none() {
            return fail("invalidInput");
        }
        crate::opaque_id(&l.id)?;
        ctx.db.photo_links().insert(l);
    }
    for v in list("immichConnections") {
        let c: ImmichConnection = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(c.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.immich_connections().insert(c);
    }
    for v in list("immichRemote") {
        let c: ImmichRemote = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.accounts().id().find(c.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.immich_remote().insert(c);
    }
    for v in list("linkItems") {
        let i: PhotoLinkItem = serde_json::from_value(v).map_err(|_| "invalidInput".to_owned())?;
        if ctx.db.photo_links().id().find(i.link_id.clone()).is_none() || i.id != format!("{}/{}", i.link_id, i.file_id) {
            return fail("invalidInput");
        }
        ctx.db.photo_link_items().insert(i);
    }
    Ok(())
}

/// A shared link by the digest of its token: unexpired, owner enabled, Photos on. `passed`
/// is whether the visitor's pass (digest) satisfies a link password.
pub fn open_photo_link(ctx: &ReducerContext, digest: &str, pass: &str) -> Res<(PhotoLink, bool)> {
    let at = now(ctx);
    let hex64 = |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let link = if hex64(digest) { ctx.db.photo_links().digest().find(digest.to_owned()) } else { None };
    let Some(link) = link.filter(|l| (l.expires_at == 0.0 || l.expires_at > at) && ctx.db.accounts().id().find(l.owner_id.clone()).is_some_and(|a| a.enabled) && app_enabled(ctx, "photos")) else {
        return fail("notFound");
    };
    let row = if !pass.is_empty() && hex64(pass) { ctx.db.photo_link_passes().digest().find(pass.to_owned()) } else { None };
    let passed = link.password_verifier.is_empty() || row.is_some_and(|r| r.link_id == link.id && r.expires_at > at);
    Ok((link, passed))
}

fn link_rows(ctx: &ReducerContext, r: &Reader, link: &PhotoLink) -> Vec<(PhotoAsset, FileEntry)> {
    if !link.album_id.is_empty() {
        let Some(album) = ctx.db.photo_albums().id().find(link.album_id.clone()) else { return Vec::new() };
        let mut rows = album_rows(ctx, r, &album);
        rows.sort_by(|x, y| newest(&x.0, &y.0));
        return rows;
    }
    let mut rows = Vec::new();
    for item in ctx.db.photo_link_items().link_id().filter(link.id.as_str()) {
        if let Some(asset) = ctx.db.photo_assets().file_id().find(item.file_id)
            && let Some(entry) = r.file_of(&asset)
            && asset.owner_id == link.owner_id
            && r.state_of(&entry) == State::Library
        {
            rows.push((asset, entry));
        }
    }
    rows.sort_by(|x, y| newest(&x.0, &y.0));
    rows
}

/// Visitor operations of a shared link (no account). Uploads are handled with Files.
pub fn photos_link_operation(ctx: &ReducerContext, digest: &str, pass: &str, operation: &str, p: &Doc) -> Res<Value> {
    let at = now(ctx);
    let (link, passed) = open_photo_link(ctx, digest, pass)?;
    let r = Reader::new(ctx, "");
    if operation == "verifier" {
        keys(p, &[])?;
        return Ok(json!({ "verifier": link.password_verifier }));
    }
    if operation == "grantPass" {
        // After the runtime checked the password: a pass for half a day, at most 1,000 per link.
        keys(p, &["digest"])?;
        let value = text(p.get("digest"), 64, false)?;
        if !(value.len() == 64 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))) || link.password_verifier.is_empty() {
            return fail("invalidInput");
        }
        let passes: Vec<PhotoLinkPass> = ctx.db.photo_link_passes().link_id().filter(link.id.as_str()).collect();
        for old in &passes {
            if old.expires_at <= at {
                ctx.db.photo_link_passes().digest().delete(old.digest.clone());
            }
        }
        if passes.iter().filter(|x| x.expires_at > at).count() >= 1000 {
            return fail("operationTooLarge");
        }
        ctx.db.photo_link_passes().insert(PhotoLinkPass { digest: value, link_id: link.id.clone(), expires_at: at + PASSWORD_DAYS * 86_400_000.0 });
        return Ok(json!({ "granted": true }));
    }
    if operation == "info" && !passed {
        return Ok(json!({ "needsPassword": true }));
    }
    if !passed {
        return fail("forbidden");
    }
    let rows = link_rows(ctx, &r, &link);
    let album = if link.album_id.is_empty() { None } else { ctx.db.photo_albums().id().find(link.album_id.clone()) };
    let item = |value: Option<&Value>| -> Res<(PhotoAsset, FileEntry)> {
        let id = opaque(value)?;
        rows.iter().find(|x| x.0.file_id == id).cloned().ok_or_else(|| "notFound".to_owned())
    };
    let plain = |asset: &PhotoAsset, entry: &FileEntry| {
        let mut v = r.expose(asset, entry, Some(State::Library));
        v["favorite"] = json!(false);
        v["archived"] = json!(false);
        v.as_object_mut().expect("asset").remove("stack");
        v
    };
    match operation {
        "info" => {
            keys(p, &[])?;
            let title = album.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| if rows.len() == 1 { rows[0].1.name.clone() } else { format!("{} photos", rows.len()) });
            Ok(json!({ "needsPassword": false, "title": title, "description": album.as_ref().map(|a| a.description.clone()).unwrap_or_default(),
                "owner": ctx.db.accounts().id().find(link.owner_id.clone()).map(|a| a.display_name).unwrap_or_default(), "count": rows.len(), "album": album.is_some(),
                "expiresAt": link.expires_at, "allowDownload": link.allow_download, "allowUpload": link.allow_upload && album.is_some(), "showMetadata": link.show_metadata }))
        }
        "page" => {
            keys(p, &["cursor", "limit"])?;
            let offset = match p.get("cursor") {
                None => 0,
                Some(Value::String(s)) if s.is_empty() => 0,
                Some(v) => int(Some(&json!(crate::text_of(Some(v)).parse::<f64>().unwrap_or(f64::NAN))), 0.0, 200000.0)? as usize,
            };
            let limit = int(Some(p.get("limit").unwrap_or(&json!(120))), 1.0, 500.0)? as usize;
            Ok(json!({ "items": rows.iter().skip(offset).take(limit).map(|x| plain(&x.0, &x.1)).collect::<Vec<_>>(), "cursor": if offset + limit < rows.len() { (offset + limit).to_string() } else { String::new() } }))
        }
        "detail" => {
            keys(p, &["id"])?;
            let (asset, entry) = item(p.get("id"))?;
            let mut base = json!({ "id": asset.file_id, "name": entry.name, "kind": if asset.kind == "video" { "video" } else { "image" }, "width": asset.width, "height": asset.height, "duration": asset.duration });
            if link.show_metadata {
                let o = base.as_object_mut().expect("detail");
                o.insert("takenAt".into(), json!(asset.taken_at));
                o.insert("description".into(), json!(asset.description));
                o.insert("exif".into(), exif_of(&asset));
                o.insert("latitude".into(), if asset.has_location { json!(asset.latitude) } else { Value::Null });
                o.insert("longitude".into(), if asset.has_location { json!(asset.longitude) } else { Value::Null });
            }
            Ok(base)
        }
        "asset" => {
            keys(p, &["id"])?;
            let (asset, entry) = item(p.get("id"))?;
            Ok(json!({ "id": asset.file_id, "revision": entry.revision, "blobId": entry.blob_id, "kind": asset.kind, "mediaType": entry.media_type, "name": entry.name,
                "playback": asset.playback, "audioCodec": exif_of(&asset)["audioCodec"].as_str().unwrap_or_default(), "duration": asset.duration, "size": entry.size, "allowDownload": link.allow_download }))
        }
        _ => fail("notFound"),
    }
}

/// Adds a visitor's finished upload to the link's album.
pub fn add_link_upload(ctx: &ReducerContext, link: &PhotoLink, file_id: &str) -> Res<()> {
    if link.album_id.is_empty() || !link.allow_upload {
        return fail("forbidden");
    }
    let id = format!("{}/{file_id}", link.album_id);
    if ctx.db.photo_album_items().id().find(id.clone()).is_none() {
        insert_album_item(ctx, PhotoAlbumItem { id, album_id: link.album_id.clone(), file_id: file_id.into(), added_at: now(ctx), sync_seq: 0.0 });
    }
    Ok(())
}
