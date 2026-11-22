//! Photos, shared-link and Immich procedures (`index.ts`): Photos changes files through the
//! same checks as Files, acting for the caller's browser session.

use serde_json::{Value, json};
use spacetimedb::{ProcedureContext, ReducerContext, Table};

use crate::app_settings::app_enabled;
use crate::files::{Unlock, begin_upload_for, delete_for, find_key, finish_upload_for, key, move_for, restore_for, trash_for, unlocked};
use crate::folders::{free_name, locked_folder_for, photo_folder, photo_kind, photos_folder, photos_upload_target};
use crate::immich::*;
use crate::photos::*;
use crate::tables::*;
use crate::{Res, fail, from_session, name_of, now, run_json, service, unused_uuid};

fn upload_job(ctx: &ReducerContext, file_id: &str) -> Value {
    let entry = ctx.db.file_entries().id().find(file_id.to_owned()).expect("entry");
    json!({ "fileId": file_id, "job": { "fileId": file_id, "blobId": entry.blob_id, "name": entry.name, "mediaType": entry.media_type, "modifiedAt": entry.modified_at,
        "kind": photo_kind(&entry.media_type, &entry.name).unwrap_or("image") } })
}

fn cancel_upload(ctx: &ReducerContext, upload: Upload) {
    if upload.state == "uploading" {
        if let Some(meter) = ctx.db.usage().user_id().find(upload.owner_id.clone()) {
            ctx.db.usage().user_id().update(Usage { reserved: (meter.reserved - upload.size).max(0.0), ..meter });
        }
        ctx.db.uploads().id().update(Upload { state: "cancelled".into(), ..upload });
    }
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_photos(ctx: &mut ProcedureContext, sessionId: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &sessionId)?;
        let password = tx.db.app_passwords().session_id().find(sessionId.clone());
        // App passwords with Photos access (the Immich app) reach Photos too: reads with read, changes with write.
        if let Some(password) = &password {
            let permission = if READ_OPERATIONS.contains(&operation.as_str()) { "read" } else { "write" };
            if password.revoked || password.expires_at <= now(tx) || !crate::scopes::password_allows(&password.scope, "photos", permission) {
                return fail("forbidden");
            }
            if ["uploadTarget", "lock", "unlock", "createLink", "editLink", "deleteLink", "links", "immichDevices", "revokeImmichDevice", "immichConnection", "saveImmichConnection", "deleteImmichConnection"].contains(&operation.as_str()) {
                return fail("forbidden");
            }
        }
        if document.len() > 1024 * 1024 {
            return fail("operationTooLarge");
        }
        let value = crate::document(&document)?;
        match operation.as_str() {
            "uploadTarget" => return photos_upload_target(tx, &user.id, &value),
            // Immich app devices signed in with this account (Photos settings › Immich).
            "immichDevices" => return Ok(json!({ "devices": immich_sessions(tx, &user.id) })),
            "immichConnection" => return Ok(immich_connection_view(tx, &user.id)),
            "saveImmichConnection" => return save_immich_connection(tx, &user.id, &value),
            "deleteImmichConnection" => return Ok(delete_immich_connection(tx, &user.id)),
            "revokeImmichDevice" => {
                let row = value.get("id").and_then(Value::as_str).and_then(|id| tx.db.immich_sessions().id().find(id.to_owned()));
                let Some(row) = row.filter(|r| r.user_id == user.id) else { return fail("notFound") };
                delete_immich_session(tx, &row);
                return Ok(json!({ "revoked": true }));
            }
            _ => {}
        }
        let session = sessionId.as_str();
        let unlock = Unlock::Session(session);
        let user_id = user.id.clone();
        let trash = |e: &FileEntry| trash_for(tx, &user_id, &e.id, e.revision, unlock);
        let restore = |e: &FileEntry| restore_for(tx, &user_id, &e.id, e.revision, unlock);
        let remove = |e: &FileEntry| delete_for(tx, &user_id, &e.id, e.revision, unlock);
        let move_to = |e: &FileEntry, parent: &str| -> Res<()> {
            let name = free_name(tx, &user_id, parent, &e.name)?;
            move_for(tx, &user_id, &e.id, parent, e.revision, Some(&name), unlock)
        };
        let locked = || locked_folder_for(tx, &user_id);
        let month = |y: i64, m: u32| photos_folder(tx, &user_id, Some(&json!(y)), Some(&json!(m)));
        let folder = || photo_folder(tx, &user_id).join("/");
        let folder_id = || {
            let mut parent_id = String::new();
            for name in photo_folder(tx, &user_id) {
                match find_key(tx, &key(&user_id, &parent_id, &name)) {
                    Some(f) if f.kind == "folder" && !f.trashed => parent_id = f.id,
                    _ => return String::new(),
                }
            }
            parent_id
        };
        let files = PhotoFiles {
            unlocked: unlocked(tx, &user.id, unlock),
            trash: &trash,
            restore: &restore,
            remove: &remove,
            move_to: &move_to,
            locked_folder: &locked,
            month_folder: &month,
            upload_folder: &folder,
            upload_folder_id: &folder_id,
        };
        photos_operation(tx, &user.id, &operation, &value, &files)
    })
}

/// Shared-link visitors (no account): `digest` identifies the link and `pass` the visitor's
/// password pass. Uploads go into the owner's Photos folder and the link's album.
#[spacetimedb::procedure]
pub fn service_photo_link(ctx: &mut ProcedureContext, digest: String, pass: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if document.len() > 64 * 1024 {
            return fail("operationTooLarge");
        }
        let q = crate::document(&document)?;
        if !["beginUpload", "upload", "finishUpload", "cancelUpload"].contains(&operation.as_str()) {
            return photos_link_operation(tx, &digest, &pass, &operation, &q);
        }
        let (link, passed) = open_photo_link(tx, &digest, &pass)?;
        if !passed || link.album_id.is_empty() || !link.allow_upload || tx.db.photo_albums().id().find(link.album_id.clone()).is_none() {
            return fail("forbidden");
        }
        let upload_key = format!("link:{}", link.id);
        if operation == "beginUpload" {
            let parent_id = photos_folder(tx, &link.owner_id, q.get("year"), q.get("month"))?;
            let (Some(name), Some(size), Some(media_type)) = (q.get("name").and_then(Value::as_str), q.get("size").and_then(Value::as_f64), q.get("mediaType").and_then(Value::as_str)) else { return fail("invalidInput") };
            if photo_kind(media_type, name).is_none() {
                return fail("invalidInput");
            }
            let name = free_name(tx, &link.owner_id, &parent_id, &name_of(name)?)?;
            let upload = begin_upload_for(tx, &link.owner_id, &upload_key, &unused_uuid(tx), &unused_uuid(tx), &parent_id, &name, size, &media_type.to_lowercase(), 0, Unlock::Caller)?;
            return Ok(json!({ "id": upload.id, "size": upload.size, "state": upload.state }));
        }
        let upload = q.get("id").and_then(Value::as_str).and_then(|id| tx.db.uploads().id().find(id.to_owned()));
        let Some(upload) = upload.filter(|u| u.session_id == upload_key && u.expires_at > now(tx)) else { return fail("notFound") };
        match operation.as_str() {
            "upload" => Ok(json!({ "id": upload.id, "size": upload.size, "state": upload.state, "fileId": upload.file_id })),
            "cancelUpload" => {
                cancel_upload(tx, upload);
                Ok(json!({ "cancelled": true }))
            }
            _ => {
                let digest = q.get("digest").and_then(Value::as_str).unwrap_or_default();
                let file_id = finish_upload_for(tx, &link.owner_id, &upload_key, &upload.id, digest, Unlock::Caller, 0.0)?;
                add_link_upload(tx, &link, &file_id)?;
                Ok(upload_job(tx, &file_id))
            }
        }
    })
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Immich app sign-in: Dar username and an app password with Photos access.
#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_immich_login(ctx: &mut ProcedureContext, username: String, digest: String, tokenDigest: String, deviceType: String, deviceOS: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let password = tx.db.app_passwords().digest().find(digest.clone());
        let Some(password) = password.filter(|p| !p.revoked && p.expires_at > now(tx)) else { return fail("unauthenticated") };
        let user = tx.db.accounts().id().find(password.user_id.clone());
        let session = tx.db.sessions().id().find(password.session_id.clone());
        let Some(user) = user.filter(|u| u.enabled && immich_login_matches(tx, u, &username)) else { return fail("unauthenticated") };
        if session.is_none_or(|s| !s.active) {
            return fail("unauthenticated");
        }
        if !crate::scopes::password_allows(&password.scope, "photos", "read") || !app_enabled(tx, "photos") {
            return fail("forbidden");
        }
        if !hex64(&tokenDigest) || deviceType.encode_utf16().count() > 100 || deviceOS.encode_utf16().count() > 100 {
            return fail("invalidInput");
        }
        if tx.db.immich_sessions().user_id().filter(user.id.as_str()).count() >= 50 {
            return fail("quotaExceeded");
        }
        let id = unused_uuid(tx);
        tx.db.immich_sessions().insert(ImmichSession {
            digest: tokenDigest.clone(),
            id: id.clone(),
            user_id: user.id.clone(),
            session_id: password.session_id.clone(),
            device_type: deviceType.clone(),
            device_os: deviceOS.clone(),
            created_at: now(tx),
            used_at: now(tx),
            epoch_seen: crate::photo_sync::current_seq(tx),
        });
        let mut v = immich_user(tx, &user);
        v["sessionId"] = json!(id);
        Ok(v)
    })
}

/// Immich app sign-in with Dar (OAuth), after the runtime redeemed the code for this account.
/// The device gets its own Photos app password, revoked when the device signs out.
#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_immich_oauth_login(ctx: &mut ProcedureContext, userId: String, passwordDigest: String, tokenDigest: String, deviceType: String, deviceOS: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        let Some(user) = tx.db.accounts().id().find(userId.clone()).filter(|u| u.enabled) else { return fail("unauthenticated") };
        if !app_enabled(tx, "photos") {
            return fail("forbidden");
        }
        if !hex64(&tokenDigest) || deviceType.encode_utf16().count() > 100 || deviceOS.encode_utf16().count() > 100 {
            return fail("invalidInput");
        }
        if tx.db.immich_sessions().user_id().filter(user.id.as_str()).count() >= 50 {
            return fail("quotaExceeded");
        }
        let id = unused_uuid(tx);
        let device = if !deviceOS.trim().is_empty() { deviceOS.trim().to_owned() } else if !deviceType.trim().is_empty() { deviceType.trim().to_owned() } else { "a phone".to_owned() };
        let label: String = format!("Immich on {device}").encode_utf16().take(100).collect::<Vec<u16>>().iter().map(|u| char::from_u32(*u as u32).unwrap_or('?')).collect();
        let session_id = crate::auth::insert_app_password(tx, &user.id, &id, &passwordDigest, &label, "v2 photos:read photos:write")?;
        tx.db.immich_sessions().insert(ImmichSession {
            digest: tokenDigest.clone(),
            id: id.clone(),
            user_id: user.id.clone(),
            session_id,
            device_type: deviceType.clone(),
            device_os: deviceOS.clone(),
            created_at: now(tx),
            used_at: now(tx),
            epoch_seen: crate::photo_sync::current_seq(tx),
        });
        let mut v = immich_user(tx, &user);
        v["sessionId"] = json!(id);
        Ok(v)
    })
}

/// Requests from the Immich app, by the digest of its access token.
#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_immich(ctx: &mut ProcedureContext, tokenDigest: String, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if document.len() > 2 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        let q = crate::document(&document)?;
        let write = ["uploadBegin", "uploadFinish", "uploadCancel"].contains(&operation.as_str());
        let (session, user) = immich_session(tx, &tokenDigest, write)?;
        if !app_enabled(tx, "photos") {
            return fail("notFound");
        }
        let upload_key = format!("immich:{}", session.id);
        match operation.as_str() {
            "session" => Ok(json!({ "user": immich_user(tx, &user), "sessionId": session.id, "protocolSessionId": session.session_id })),
            "logout" => {
                delete_immich_session(tx, &session);
                Ok(json!({ "ok": true }))
            }
            "syncStart" => Ok(immich_sync_start(tx, &session, q.get("reset") == Some(&json!(true)))),
            "syncPage" => {
                let after = q.get("after").and_then(Value::as_f64).filter(|f| f.is_finite()).unwrap_or(-1.0);
                let limit = q.get("limit").and_then(Value::as_f64).map_or(500.0, |l| l.clamp(1.0, 2000.0)) as usize;
                let up_to = q.get("upTo").and_then(Value::as_f64).filter(|f| f.is_finite()).unwrap_or(f64::INFINITY);
                immich_sync_page(tx, &session, &user, &crate::text_of(q.get("channel")), after, limit, up_to)
            }
            "acks" => Ok(immich_acks(tx, &session)),
            "setAcks" => {
                immich_set_acks(tx, &session, q.get("acks"))?;
                Ok(json!({ "ok": true }))
            }
            "deleteAcks" => {
                immich_delete_acks(tx, &session, q.get("types"))?;
                Ok(json!({ "ok": true }))
            }
            "bulkCheck" => immich_bulk_check(tx, &user.id, q.get("assets")),
            "uploadBegin" => {
                // Backups land in the Photos folder by their creation month; a file already here is a duplicate.
                let sha1 = q.get("sha1").and_then(Value::as_str).map(checksum_to_base64).unwrap_or_default();
                let existing = immich_duplicate(tx, &user.id, &sha1);
                if !existing.is_empty() {
                    return Ok(json!({ "duplicate": existing }));
                }
                let (Some(name), Some(size), Some(media_type)) = (q.get("name").and_then(Value::as_str), q.get("size").and_then(Value::as_f64), q.get("mediaType").and_then(Value::as_str)) else { return fail("invalidInput") };
                if photo_kind(media_type, name).is_none() {
                    return fail("invalidInput");
                }
                let parent_id = photos_folder(tx, &user.id, q.get("year"), q.get("month"))?;
                let name = free_name(tx, &user.id, &parent_id, &name_of(name)?)?;
                let upload = begin_upload_for(tx, &user.id, &upload_key, &unused_uuid(tx), &unused_uuid(tx), &parent_id, &name, size, &media_type.to_lowercase(), 0, Unlock::Caller)?;
                Ok(json!({ "id": upload.id, "size": upload.size }))
            }
            "uploadFinish" => {
                let upload = q.get("id").and_then(Value::as_str).and_then(|id| tx.db.uploads().id().find(id.to_owned()));
                let Some(upload) = upload.filter(|u| u.session_id == upload_key) else { return fail("notFound") };
                let digest = q.get("digest").and_then(Value::as_str).unwrap_or_default();
                let file_time = q.get("fileTime").and_then(Value::as_f64).unwrap_or(0.0);
                let file_id = finish_upload_for(tx, &user.id, &upload_key, &upload.id, digest, Unlock::Caller, file_time)?;
                Ok(upload_job(tx, &file_id))
            }
            "uploadCancel" => {
                let upload = q.get("id").and_then(Value::as_str).and_then(|id| tx.db.uploads().id().find(id.to_owned()));
                let Some(upload) = upload.filter(|u| u.session_id == upload_key) else { return fail("notFound") };
                cancel_upload(tx, upload);
                Ok(json!({ "ok": true }))
            }
            _ => fail("notFound"),
        }
    })
}

/// The Immich server connector (runtime only): due connections, imports into Files as the owner,
/// mappings and reports.
#[spacetimedb::procedure]
pub fn service_immich_worker(ctx: &mut ProcedureContext, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if document.len() > 4 * 1024 * 1024 {
            return fail("operationTooLarge");
        }
        let q = crate::document(&document)?;
        let user_id = q.get("userId").and_then(Value::as_str).unwrap_or_default().to_owned();
        let s = |k: &str| q.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        if operation == "due" {
            return Ok(if app_enabled(tx, "photos") { immich_due(tx, &user_id) } else { json!({ "connections": [] }) });
        }
        if user_id.is_empty() || tx.db.immich_connections().user_id().find(user_id.clone()).is_none() || tx.db.accounts().id().find(user_id.clone()).is_none_or(|a| !a.enabled) {
            return fail("notFound");
        }
        let upload_key = format!("connector:{user_id}");
        match operation.as_str() {
            "report" => Ok(immich_report(tx, &user_id, &s("error"), q.get("summary"))),
            "known" => Ok(immich_known(tx, &user_id, &s("after"), q.get("limit").and_then(Value::as_f64).map_or(2000.0, |l| l.min(5000.0)) as usize)),
            "map" => immich_map(tx, &user_id, &s("remoteId"), &s("fileId"), &s("checksum")),
            "apply" => Ok(immich_apply(tx, &user_id, &s("fileId"), q.get("favorite") == Some(&json!(true)), q.get("archived") == Some(&json!(true)), &s("description"))),
            "album" => {
                let ids: Vec<String> = q.get("fileIds").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                immich_album(tx, &user_id, &s("remoteId"), &s("name"), &s("description"), &ids)
            }
            "importBegin" => {
                // Imported originals land in the Photos folder by their capture month, as uploads do.
                let existing = immich_duplicate(tx, &user_id, &checksum_to_base64(&s("sha1")));
                if !existing.is_empty() {
                    return Ok(json!({ "duplicate": existing }));
                }
                let name = s("name");
                let media_type = s("mediaType").to_lowercase();
                let Some(size) = q.get("size").and_then(Value::as_f64) else { return fail("invalidInput") };
                if photo_kind(&media_type, &name).is_none() {
                    return fail("invalidInput");
                }
                let parent_id = photos_folder(tx, &user_id, q.get("year"), q.get("month"))?;
                let token = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"!#$&^_.+-".contains(&b));
                let media_type = if media_type.split_once('/').is_some_and(|(a, b)| token(a) && token(b)) { media_type } else { "application/octet-stream".into() };
                let file_name = free_name(tx, &user_id, &parent_id, &name_of(&name)?)?;
                let upload = begin_upload_for(tx, &user_id, &upload_key, &unused_uuid(tx), &unused_uuid(tx), &parent_id, &file_name, size, &media_type, 0, Unlock::Caller)?;
                Ok(json!({ "id": upload.id, "size": upload.size }))
            }
            "importFinish" => {
                let Some(upload) = tx.db.uploads().id().find(s("id")).filter(|u| u.session_id == upload_key) else { return fail("notFound") };
                let file_time = q.get("fileTime").and_then(Value::as_f64).unwrap_or(0.0);
                let file_id = finish_upload_for(tx, &user_id, &upload_key, &upload.id, &s("digest"), Unlock::Caller, file_time)?;
                Ok(upload_job(tx, &file_id))
            }
            _ => fail("notFound"),
        }
    })
}

#[spacetimedb::procedure]
pub fn service_photos_worker(ctx: &mut ProcedureContext, operation: String, document: String) -> String {
    run_json(ctx, |tx| {
        service(tx)?;
        if document.len() > 1024 * 1024 {
            return fail("operationTooLarge");
        }
        photos_worker(tx, &operation, &crate::document(&document)?)
    })
}
