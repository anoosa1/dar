//! App folders in Files (`index.ts`): the Photos upload folder by month, the Notes and Books
//! libraries, free names, and the `service_notes` and `service_books` procedures.

use serde_json::{Value, json};
use spacetimedb::{ReducerContext, Table};
use unicode_normalization::UnicodeNormalization;

use crate::app_settings::{app_enabled, app_settings};
use crate::files::{find_key, is_locked_root, key};
use crate::tables::*;
use crate::{Res, fail, now, unused_uuid};

pub const DEFAULT_PHOTO_FOLDER: &str = "pics";
pub const DEFAULT_NOTES_FOLDER: &str = "docs/notes";
pub const DEFAULT_BOOKS_FOLDER: &str = "docs/books";
const RAW_EXTENSIONS: [&str; 8] = ["dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf"];

/// `photoKind`: the kind of media Photos indexes, by media type then extension.
pub fn photo_kind(media_type: &str, name: &str) -> Option<&'static str> {
    let kind = media_type.to_lowercase();
    let ext = name.to_lowercase().rsplit('.').next().unwrap_or_default().to_owned();
    if kind.starts_with("image/") && kind != "image/svg+xml" {
        return Some("image");
    }
    if kind.starts_with("video/") {
        return Some("video");
    }
    if ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "tif", "tiff"].contains(&ext.as_str()) || RAW_EXTENSIONS.contains(&ext.as_str()) {
        return Some("image");
    }
    if ["mp4", "mov", "m4v", "webm", "mkv", "3gp", "avi"].contains(&ext.as_str()) {
        return Some("video");
    }
    None
}

/// `nameParts`: the stem and lower-case extension.
pub fn name_parts(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (name[..dot].to_owned(), name[dot + 1..].to_lowercase()),
        _ => (name.to_owned(), String::new()),
    }
}

/// `photoFolderParts`: 1–8 folder names, no dot-only names or control characters.
pub fn photo_folder_parts(value: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = value.split('/').map(|p| p.nfc().collect::<String>().trim().to_owned()).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.len() > 8 || value.encode_utf16().count() > 500 || parts.iter().any(|p| p.encode_utf16().count() > 255 || p == "." || p == ".." || p.chars().any(|c| (c as u32) < 0x20 || c == '\\')) {
        return None;
    }
    Some(parts)
}

pub fn is_note_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    (lower.ends_with(".md") || lower.ends_with(".markdown")) && !name.starts_with('.')
}

/// `bookFormat`: the format by extension; hidden files are not books.
pub fn book_format(name: &str) -> Option<&'static str> {
    if name.starts_with('.') {
        return None;
    }
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 8 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    ["epub", "pdf", "cbz", "cbr", "cb7", "cbt"].into_iter().find(|f| *f == ext.to_lowercase())
}

fn preference(ctx: &ReducerContext, user_id: &str, name: &str) -> Option<String> {
    let value: Value = serde_json::from_str(&ctx.db.pim_preferences().user_id().find(user_id.to_owned())?.value).ok()?;
    value.get(name)?.as_str().map(str::to_owned)
}

fn preference_folder_parts(ctx: &ReducerContext, user_id: &str, name: &str, fallback: &str) -> Vec<String> {
    preference(ctx, user_id, name).and_then(|v| photo_folder_parts(&v)).unwrap_or_else(|| fallback.split('/').map(str::to_owned).collect())
}

pub fn notes_folder_parts(ctx: &ReducerContext, user_id: &str) -> Vec<String> {
    preference_folder_parts(ctx, user_id, "notesFolder", DEFAULT_NOTES_FOLDER)
}

pub fn books_folder_parts(ctx: &ReducerContext, user_id: &str) -> Vec<String> {
    preference_folder_parts(ctx, user_id, "booksFolder", DEFAULT_BOOKS_FOLDER)
}

/// The Photos folder from the account's preferences (default "pics").
pub fn photo_folder(ctx: &ReducerContext, user_id: &str) -> Vec<String> {
    preference(ctx, user_id, "photosUploadFolder").and_then(|v| photo_folder_parts(&v)).unwrap_or_else(|| vec![DEFAULT_PHOTO_FOLDER.to_owned()])
}

fn new_folder(ctx: &ReducerContext, user_id: &str, parent_id: &str, name: &str) -> String {
    let id = unused_uuid(ctx);
    ctx.db.file_entries().insert(FileEntry {
        id: id.clone(),
        owner_id: user_id.into(),
        parent_id: parent_id.into(),
        name: name.into(),
        kind: "folder".into(),
        blob_id: String::new(),
        size: 0.0,
        media_type: "inode/directory".into(),
        revision: 1,
        modified_at: now(ctx),
        trashed: false,
        favorite: false,
        list_key: key(user_id, parent_id, name),
    });
    crate::files::event(ctx, user_id, &id, "created", None);
    id
}

/// Finds (or, when asked, creates as the owner) an app's folder; never the Locked folder or a trashed one.
pub fn own_folder(ctx: &ReducerContext, user_id: &str, parts: &[String], create: bool) -> Res<Option<String>> {
    let mut parent_id = String::new();
    for name in parts {
        if let Some(existing) = find_key(ctx, &key(user_id, &parent_id, name)) {
            if existing.kind != "folder" || existing.trashed || is_locked_root(&existing) {
                return fail("conflict");
            }
            parent_id = existing.id;
            continue;
        }
        if !create {
            return Ok(None);
        }
        parent_id = new_folder(ctx, user_id, &parent_id, name);
    }
    Ok(Some(parent_id))
}

pub fn notes_root(ctx: &ReducerContext, user_id: &str, create: bool) -> Res<Option<String>> {
    own_folder(ctx, user_id, &notes_folder_parts(ctx, user_id), create)
}

/// Photos uploads go to `<Photos folder>/YYYY/MM`, creating missing folders as the owner.
pub fn photos_folder(ctx: &ReducerContext, user_id: &str, year: Option<&Value>, month: Option<&Value>) -> Res<String> {
    let int = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|f| f.fract() == 0.0);
    let (Some(year), Some(month)) = (int(year).filter(|y| (1900.0..=2999.0).contains(y)), int(month).filter(|m| (1.0..=12.0).contains(m))) else { return fail("invalidInput") };
    let mut parent_id = String::new();
    let mut names = photo_folder(ctx, user_id);
    names.push(format!("{}", year as i64));
    names.push(format!("{:02}", month as i64));
    for name in names {
        if let Some(existing) = find_key(ctx, &key(user_id, &parent_id, &name)) {
            if existing.kind != "folder" || existing.trashed {
                return fail("conflict");
            }
            parent_id = existing.id;
            continue;
        }
        parent_id = new_folder(ctx, user_id, &parent_id, &name);
    }
    Ok(parent_id)
}

pub fn photos_upload_target(ctx: &ReducerContext, user_id: &str, p: &serde_json::Map<String, Value>) -> Res<Value> {
    if app_settings(ctx).1.iter().any(|d| d == "photos") {
        return fail("notFound");
    }
    if p.keys().any(|k| k != "year" && k != "month") {
        return fail("invalidInput");
    }
    Ok(json!({ "parentId": photos_folder(ctx, user_id, p.get("year"), p.get("month"))? }))
}

/// The first free name in a folder: "IMG_1.jpg", then "IMG_1 (2).jpg" and so on.
pub fn free_name(ctx: &ReducerContext, owner_id: &str, parent_id: &str, name: &str) -> Res<String> {
    let (stem, _) = name_parts(name);
    let suffix = &name[stem.len()..];
    for n in 1..1000 {
        let next = if n == 1 { name.to_owned() } else { format!("{stem} ({n}){suffix}") };
        if find_key(ctx, &key(owner_id, parent_id, &next)).is_none() {
            return Ok(next);
        }
    }
    fail("conflict")
}

/// The account's Locked folder, created on first use outside the folder tree.
pub fn locked_folder_for(ctx: &ReducerContext, user_id: &str) -> String {
    if let Some(existing) = find_key(ctx, &format!("{user_id}/locked")) {
        return existing.id;
    }
    let id = unused_uuid(ctx);
    ctx.db.file_entries().insert(FileEntry {
        id: id.clone(),
        owner_id: user_id.into(),
        parent_id: String::new(),
        name: "Locked folder".into(),
        kind: "folder".into(),
        blob_id: String::new(),
        size: 0.0,
        media_type: crate::files::LOCKED_FOLDER_TYPE.into(),
        revision: 1,
        modified_at: now(ctx),
        trashed: false,
        favorite: false,
        list_key: format!("{user_id}/locked"),
    });
    id
}

/// The folder tree and files of a library, breadth first and bounded (Notes and Books).
struct Walk {
    folders: Vec<(String, String, String, f64)>,
    files: Vec<(FileEntry, String, String, String)>,
    truncated: bool,
    libraries: Vec<Value>,
}

fn walk(ctx: &ReducerContext, user_id: &str, root: &str, limit: usize, file_limit: usize, wanted: &dyn Fn(&str) -> bool, track_libraries: bool) -> Walk {
    let mut queue: Vec<(String, String, u32, String)> = vec![(root.to_owned(), String::new(), 0, String::new())];
    let mut out = Walk { folders: Vec::new(), files: Vec::new(), truncated: false, libraries: Vec::new() };
    let mut scanned = 0;
    let mut i = 0;
    while i < queue.len() && !out.truncated {
        let (id, path, depth, library) = queue[i].clone();
        i += 1;
        let window = crate::folder_window(&format!("{user_id}/{id}/"));
        for entry in ctx.db.file_entries().list_key().filter(crate::bounds(&window)) {
            scanned += 1;
            if scanned > limit {
                out.truncated = true;
                break;
            }
            if entry.trashed || entry.name.starts_with('.') {
                continue;
            }
            let child_path = if path.is_empty() { entry.name.clone() } else { format!("{path}/{}", entry.name) };
            if entry.kind == "folder" {
                if depth >= 16 {
                    out.truncated = true;
                    continue;
                }
                if track_libraries && depth == 0 {
                    out.libraries.push(json!({ "id": entry.id, "name": entry.name }));
                }
                out.folders.push((entry.id.clone(), id.clone(), entry.name.clone(), entry.modified_at));
                let next_library = if library.is_empty() { entry.id.clone() } else { library.clone() };
                queue.push((entry.id.clone(), child_path.clone(), depth + 1, next_library));
                if let Some(last) = out.folders.last_mut() {
                    last.2 = format!("{}\u{0}{child_path}", entry.name);
                }
            } else if wanted(&entry.name) && out.files.len() < file_limit {
                out.files.push((entry, id.clone(), child_path, library.clone()));
            }
        }
    }
    out
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_notes(ctx: &mut spacetimedb::ProcedureContext, sessionId: String, operation: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx.db.app_passwords().session_id().find(sessionId.clone()).is_some() {
            return fail("forbidden");
        }
        if !app_enabled(tx, "notes") {
            return fail("notFound");
        }
        if operation != "library" {
            return fail("invalidInput");
        }
        let root_id = notes_root(tx, &user.id, true)?.unwrap_or_default();
        let parts = notes_folder_parts(tx, &user.id);
        let found = walk(tx, &user.id, &root_id, 5000, 3000, &is_note_file, false);
        let folders: Vec<Value> = found
            .folders
            .iter()
            .map(|(id, parent, name_path, modified)| {
                let (name, path) = name_path.split_once('\u{0}').unwrap_or((name_path, name_path));
                json!({ "id": id, "parentId": parent, "name": name, "path": path, "modifiedAt": modified })
            })
            .collect();
        let files: Vec<Value> = found
            .files
            .iter()
            .map(|(e, parent, path, _)| json!({ "id": e.id, "parentId": parent, "name": e.name, "blobId": e.blob_id, "size": e.size, "revision": e.revision, "modifiedAt": e.modified_at, "favorite": e.favorite, "path": path }))
            .collect();
        let mut legacy = 0;
        for c in tx.db.pim_collections().owner_id().filter(&user.id) {
            if c.kind == "notes" {
                legacy += tx.db.pim_resources().collection_id().filter(&c.id).count();
            }
        }
        Ok(json!({ "rootId": root_id, "path": parts.join("/"), "folders": folders, "files": files, "truncated": found.truncated, "legacy": legacy }))
    })
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_books(ctx: &mut spacetimedb::ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx.db.app_passwords().session_id().find(sessionId.clone()).is_some() {
            return fail("forbidden");
        }
        if !app_enabled(tx, "books") {
            return fail("notFound");
        }
        if document.len() > 64 * 1024 {
            return fail("operationTooLarge");
        }
        let value = crate::document(if document.is_empty() { "{}" } else { &document })?;
        if operation == "library" {
            let root_id = own_folder(tx, &user.id, &books_folder_parts(tx, &user.id), true)?.unwrap_or_default();
            let parts = books_folder_parts(tx, &user.id);
            let found = walk(tx, &user.id, &root_id, 20000, 10000, &|n: &str| book_format(n).is_some(), true);
            let files: Vec<Value> = found
                .files
                .iter()
                .map(|(e, parent, path, library)| json!({ "id": e.id, "parentId": parent, "name": e.name, "blobId": e.blob_id, "size": e.size, "revision": e.revision, "modifiedAt": e.modified_at, "path": path, "library": library }))
                .collect();
            return Ok(json!({ "rootId": root_id, "path": parts.join("/"), "libraries": found.libraries, "files": files, "states": crate::books::book_states(tx, &user.id), "truncated": found.truncated }));
        }
        let session = sessionId.clone();
        let readable = |file_id: &str| -> bool {
            tx.db.file_entries().id().find(file_id.to_owned()).is_some_and(|e| e.kind == "file" && crate::files::access(tx, &e, &user.id, crate::files::Unlock::Session(&session)).is_some())
        };
        crate::books::books_operation(tx, &user.id, &operation, &value, &readable)
    })
}
