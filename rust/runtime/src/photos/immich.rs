//! The Immich mobile-app API (server v3.4.1 contract, `next/runtime/photos/immich.ts`)
//! at `/dav/immich/api`, found through `/.well-known/immich`. Sign-in: the app's OAuth
//! button ("Sign in with Dar") through Dar's own OIDC provider, which gives the device
//! its own Photos app password; or Dar username followed by "@" and anything, and an app
//! password with Photos access.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::auth::{self, is_token, is_uuid};
use crate::db::Database;
use crate::digests::Sha1;
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object};
use crate::photos::http::{AssetInfo, send_file};
use crate::photos::runtime::{PhotoJob, PhotosRuntime, VideoRequest};
use crate::photos::{
    IMMICH_OAUTH_CLIENT_ID, IMMICH_OAUTH_REDIRECT_URI, MAX_FILE_BYTES, PHOTOS_READ_OPERATIONS,
    cookie_value, iso_ms, now_ms, parse_iso_ms, photo_kind, sha1_hex_to_base64,
};

pub const IMMICH_BASE: &str = "/dav/immich/api";

pub fn immich_well_known() -> Value {
    json!({ "api": { "endpoint": IMMICH_BASE } })
}

const MEDIA_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("avif", "image/avif"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("dng", "image/x-adobe-dng"),
    ("cr2", "image/x-canon-cr2"),
    ("cr3", "image/x-canon-cr3"),
    ("nef", "image/x-nikon-nef"),
    ("arw", "image/x-sony-arw"),
    ("orf", "image/x-olympus-orf"),
    ("rw2", "image/x-panasonic-rw2"),
    ("raf", "image/x-fuji-raf"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("mkv", "video/x-matroska"),
    ("3gp", "video/3gpp"),
    ("avi", "video/x-msvideo"),
];

fn media_type_for_ext(ext: &str) -> Option<&'static str> {
    MEDIA_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

fn status_text(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

fn send_json(value: &Value, status: StatusCode, cookies: &[String]) -> Response {
    let mut resp = (
        status,
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        value.to_string(),
    )
        .into_response();
    for c in cookies {
        if let Ok(hv) = HeaderValue::from_str(c) {
            resp.headers_mut().append(header::SET_COOKIE, hv);
        }
    }
    resp
}

fn immich_error(failure: AppError) -> Response {
    let status = if failure.status == 423 { 404 } else { failure.status };
    let text = status_text(status);
    let message = if failure.message == failure.code {
        if text != "Error" { text } else { failure.code.as_str() }
    } else {
        failure.message.as_str()
    };
    let corr = &auth::uuid()[..8];
    let body = json!({
        "message": message,
        "error": text,
        "statusCode": status,
        "correlationId": corr,
    });
    send_json(
        &body,
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        &[],
    )
}

fn auth_cookies(token: &str, auth_type: &str, secure: bool) -> Vec<String> {
    let attributes = format!(
        "Path=/; Max-Age={}; SameSite=Lax{}",
        400 * 86_400,
        if secure { "; Secure" } else { "" }
    );
    vec![
        format!("immich_access_token={token}; {attributes}; HttpOnly"),
        format!("immich_auth_type={auth_type}; {attributes}; HttpOnly"),
        format!("immich_is_authenticated=true; {attributes}"),
    ]
}

fn cleared_cookies() -> Vec<String> {
    ["immich_access_token", "immich_auth_type", "immich_is_authenticated"]
        .into_iter()
        .map(|name| format!("{name}=; Path=/; Max-Age=0; SameSite=Lax"))
        .collect()
}

fn device_header(headers: &HeaderMap, name: &str, alternative: &str, fallback: &str) -> String {
    let val = headers
        .get(name)
        .or_else(|| headers.get(alternative))
        .and_then(|v| v.to_str().ok())
        .unwrap_or(fallback);
    val.chars().take(100).collect()
}

fn login_rate_key(headers: &HeaderMap) -> String {
    let peer = headers
        .get("x-dar-client-ip")
        .and_then(|v| v.to_str().ok())
        .filter(|ip| ip.parse::<std::net::IpAddr>().is_ok())
        .unwrap_or("local");
    format!("immich-login:{peer}")
}

struct PartFile {
    path: PathBuf,
    size: u64,
    sha1: String,
}

struct Part {
    name: String,
    filename: Option<String>,
    media_type: Option<String>,
    value: Option<String>,
    file: Option<PartFile>,
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_disposition_param(header: &str, key: &str) -> Option<String> {
    let pat = format!(r#"\b{}="([^"]*)""#, regex::escape(key));
    let re = Regex::new(&pat).ok()?;
    re.captures(header)?.get(1).map(|m| m.as_str().to_owned())
}

/// A streaming `multipart/form-data` reader: small fields in memory (64 KiB in all),
/// one file written to a private temporary directory while its SHA-1 is computed.
async fn read_multipart(body: Body, boundary: &str, dir: &Path, max_file: u64) -> Result<Vec<Part>> {
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut parts: Vec<Part> = Vec::new();
    let mut buffer: Vec<u8> = b"\r\n".to_vec();
    let mut fields: usize = 0;

    let mut current: Option<Part> = None;
    let mut in_headers = true;
    let mut file: Option<tokio::fs::File> = None;
    let mut hash: Option<Sha1> = None;
    let mut file_size: u64 = 0;
    let mut done = false;
    let mut chunks: Vec<u8> = Vec::new();

    let mut stream = body.into_data_stream();
    while let Some(next) = tokio::time::timeout(Duration::from_secs(60), stream.next())
        .await
        .map_err(|_| AppError::with("invalidInput", 400, "Upload stalled."))?
    {
        let chunk = next.map_err(|_| AppError::with("invalidInput", 400, "The upload ended early."))?;
        if done {
            continue;
        }
        buffer.extend_from_slice(&chunk);
        loop {
            if in_headers {
                let Some(start) = find_bytes(&buffer, &delimiter) else {
                    break;
                };
                if buffer.len() < start + delimiter.len() + 2 {
                    break;
                }
                if &buffer[start + delimiter.len()..start + delimiter.len() + 2] == b"--" {
                    done = true;
                    break;
                }
                let search_from = start + delimiter.len();
                let Some(rel_end) = find_bytes(&buffer[search_from..], b"\r\n\r\n") else {
                    break;
                };
                let end = search_from + rel_end;
                let header_bytes = &buffer[start + delimiter.len() + 2..end];
                let header_text = String::from_utf8_lossy(header_bytes);
                let mut disposition = String::new();
                let mut content_type: Option<String> = None;
                for line in header_text.split("\r\n") {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("content-disposition") {
                            disposition = line.to_owned();
                        } else if k.eq_ignore_ascii_case("content-type") {
                            content_type = Some(v.trim().to_owned());
                        }
                    }
                }
                let name = parse_disposition_param(&disposition, "name").unwrap_or_default();
                let filename = parse_disposition_param(&disposition, "filename");
                let mut part = Part {
                    name,
                    filename: filename.clone(),
                    media_type: content_type,
                    value: None,
                    file: None,
                };
                if filename.is_some() {
                    let target = dir.join(format!("{}.part", auth::uuid()));
                    let f = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&target)
                        .await
                        .map_err(|_| AppError::internal())?;
                    file = Some(f);
                    hash = Some(Sha1::default());
                    file_size = 0;
                    part.file = Some(PartFile { path: target, size: 0, sha1: String::new() });
                }
                current = Some(part);
                chunks.clear();
                buffer.drain(..end + 4);
                in_headers = false;
            } else if let Some(at) = find_bytes(&buffer, &delimiter) {
                let data = &buffer[..at];
                if !data.is_empty() && current.is_some() {
                    if let Some(f) = file.as_mut() {
                        file_size += data.len() as u64;
                        if file_size > max_file {
                            return Err(AppError::new("uploadTooLarge", 413));
                        }
                        if let Some(h) = hash.as_mut() {
                            h.update(data);
                        }
                        f.write_all(data).await.map_err(|_| AppError::internal())?;
                    } else {
                        fields += data.len();
                        if fields > 64 * 1024 {
                            return Err(AppError::invalid());
                        }
                        chunks.extend_from_slice(data);
                    }
                }
                if let Some(mut part) = current.take() {
                    if let Some(mut f) = file.take() {
                        f.flush().await.map_err(|_| AppError::internal())?;
                        drop(f);
                        let sha1 = hash
                            .take()
                            .map(|h| sha1_hex_to_base64(&h.hex()))
                            .unwrap_or_default();
                        if let Some(pf) = part.file.as_mut() {
                            pf.size = file_size;
                            pf.sha1 = sha1;
                        }
                    } else {
                        part.value = Some(String::from_utf8_lossy(&chunks).into_owned());
                    }
                    parts.push(part);
                    chunks.clear();
                }
                buffer.drain(..at);
                in_headers = true;
            } else {
                let keep = buffer.len().saturating_sub(delimiter.len());
                if keep > 0 && current.is_some() {
                    let data = &buffer[..keep];
                    if let Some(f) = file.as_mut() {
                        file_size += data.len() as u64;
                        if file_size > max_file {
                            return Err(AppError::new("uploadTooLarge", 413));
                        }
                        if let Some(h) = hash.as_mut() {
                            h.update(data);
                        }
                        f.write_all(data).await.map_err(|_| AppError::internal())?;
                    } else {
                        fields += data.len();
                        if fields > 64 * 1024 {
                            return Err(AppError::invalid());
                        }
                        chunks.extend_from_slice(data);
                    }
                    buffer.drain(..keep);
                }
                break;
            }
        }
    }
    if !done {
        return Err(AppError::with("invalidInput", 400, "The upload ended early."));
    }
    Ok(parts)
}

async fn free_disk_bytes(root: &Path) -> u64 {
    let out = tokio::process::Command::new("stat")
        .args(["-f", "-c", "%a %S"])
        .arg(root)
        .output()
        .await;
    let Ok(out) = out else { return 0 };
    if !out.status.success() {
        return 0;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace();
    let avail: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let bsize: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    avail.saturating_mul(bsize)
}

fn seq_of(ack: Option<&str>) -> f64 {
    let id = ack.and_then(|a| a.split('|').nth(1)).unwrap_or("");
    if is_uuid(id) {
        i64::from_str_radix(&id[24..], 16).map(|n| n as f64).unwrap_or(-1.0)
    } else {
        -1.0
    }
}

fn asset_response(d: &Value, viewer: &str) -> Value {
    let e = d.get("exif").and_then(Value::as_object);
    let exif_field = |k: &str| e.and_then(|m| m.get(k)).cloned().unwrap_or(Value::Null);
    let exposure = e.and_then(|m| m.get("exposure")).and_then(Value::as_f64).map(|x| {
        if x < 1.0 && x > 0.0 {
            format!("1/{}", (1.0 / x).round() as i64)
        } else {
            format!("{x}")
        }
    });
    let taken_at = d.get("takenAt").and_then(Value::as_f64).unwrap_or(0.0);
    let modified_at = d.get("modifiedAt").and_then(Value::as_f64).unwrap_or(0.0);
    let kind = d.get("kind").and_then(Value::as_str).unwrap_or("image");
    let archived = d.get("archived") == Some(&json!(true));
    let trashed = d.get("trashedAt").and_then(Value::as_f64).is_some_and(|t| t != 0.0);
    let duration = d.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    let width = d.get("width").and_then(Value::as_f64).map(|n| n as u64).unwrap_or(0);
    let height = d.get("height").and_then(Value::as_f64).map(|n| n as u64).unwrap_or(0);
    let has_owner = d.get("owner").and_then(Value::as_str).is_some_and(|s| !s.is_empty());

    json!({
        "id": d["id"],
        "deviceAssetId": d["id"],
        "deviceId": "dar",
        "ownerId": if has_owner { "" } else { viewer },
        "originalFileName": d["name"],
        "originalMimeType": d["mediaType"],
        "originalPath": "",
        "checksum": d.get("checksum").and_then(Value::as_str).unwrap_or(""),
        "thumbhash": null,
        "type": if kind == "video" { "VIDEO" } else { "IMAGE" },
        "fileCreatedAt": iso_ms(taken_at),
        "fileModifiedAt": iso_ms(modified_at),
        "localDateTime": iso_ms(taken_at),
        "createdAt": iso_ms(modified_at),
        "updatedAt": iso_ms(modified_at),
        "isFavorite": d.get("favorite") == Some(&json!(true)),
        "isArchived": archived,
        "isTrashed": trashed,
        "isOffline": false,
        "isEdited": false,
        "visibility": if archived { "archive" } else { "timeline" },
        "hasMetadata": true,
        "duration": if kind == "video" { json!((duration * 1000.0).round() as i64) } else { Value::Null },
        "width": if width > 0 { json!(width) } else { Value::Null },
        "height": if height > 0 { json!(height) } else { Value::Null },
        "livePhotoVideoId": d.get("live").cloned().unwrap_or(Value::Null),
        "people": [],
        "tags": [],
        "exifInfo": {
            "make": exif_field("make"),
            "model": exif_field("model"),
            "lensModel": exif_field("lens"),
            "fNumber": exif_field("fNumber"),
            "focalLength": exif_field("focalLength"),
            "iso": exif_field("iso"),
            "exposureTime": exposure.map(|s| json!(s)).unwrap_or(Value::Null),
            "latitude": d.get("latitude").cloned().unwrap_or(Value::Null),
            "longitude": d.get("longitude").cloned().unwrap_or(Value::Null),
            "description": d.get("description").cloned().unwrap_or_else(|| json!("")),
            "dateTimeOriginal": iso_ms(taken_at),
            "exifImageWidth": if width > 0 { json!(width) } else { Value::Null },
            "exifImageHeight": if height > 0 { json!(height) } else { Value::Null },
            "fileSizeInByte": d["size"],
            "orientation": null,
            "timeZone": null,
            "city": null,
            "state": null,
            "country": null,
        }
    })
}

fn preferences() -> Value {
    json!({
        "albums": { "defaultAssetOrder": "desc" },
        "cast": { "gCastEnabled": false },
        "download": { "archiveSize": 4294967296u64, "includeEmbeddedVideos": false },
        "emailNotifications": { "enabled": false, "albumInvite": false, "albumUpdate": false },
        "folders": { "enabled": false, "sidebarWeb": false },
        "memories": { "enabled": false, "duration": 5, "sidebarWeb": false },
        "people": { "enabled": false, "sidebarWeb": false, "updateStrategy": "self" },
        "purchase": { "showSupportBadge": false, "hideBuyButtonUntil": "2099-01-01T00:00:00.000Z" },
        "ratings": { "enabled": false },
        "recentlyAdded": { "enabled": true, "sidebarWeb": false },
        "sharedLinks": { "enabled": false, "sidebarWeb": false },
        "tags": { "enabled": false, "sidebarWeb": false },
    })
}

async fn call_immich(db: &Database, token_digest: &str, op: &str, val: Value) -> Result<Value> {
    db.call("service_immich", json!([token_digest, op, val.to_string()]))
        .await
}

async fn call_photos_op(
    db: &Database,
    protocol_session_id: &str,
    op: &str,
    val: Value,
) -> Result<Value> {
    db.operation(
        "service_photos",
        op,
        PHOTOS_READ_OPERATIONS,
        json!([protocol_session_id, op, val.to_string()]),
    )
    .await
}

async fn update_assets(
    db: &Database,
    protocol_session_id: &str,
    ids: &[Value],
    p: &Map<String, Value>,
) -> Result<()> {
    let mut change = Map::new();
    if let Some(Value::Bool(fav)) = p.get("isFavorite") {
        change.insert("favorite".into(), json!(*fav));
    }
    if let Some(Value::String(vis)) = p.get("visibility")
        && (vis == "archive" || vis == "timeline")
    {
        change.insert("archived".into(), json!(vis == "archive"));
    }
    if !change.is_empty() {
        change.insert("ids".into(), Value::Array(ids.to_vec()));
        call_photos_op(db, protocol_session_id, "update", Value::Object(change)).await?;
    }
    if let Some(Value::String(desc)) = p.get("description")
        && ids.len() == 1
    {
        let id = &ids[0];
        let detail =
            call_photos_op(db, protocol_session_id, "detail", json!({ "id": id })).await?;
        call_photos_op(
            db,
            protocol_session_id,
            "describe",
            json!({ "id": id, "description": desc, "revision": detail["revision"] }),
        )
        .await?;
    }
    Ok(())
}

async fn album_response(
    db: &Database,
    protocol_session_id: &str,
    user: &Value,
    a: &Value,
    with_assets: bool,
) -> Result<Value> {
    let album_id = a.get("id").and_then(Value::as_str).unwrap_or("");
    let owner_obj = a.get("owner").and_then(Value::as_object);
    let owner_user = if let Some(o) = owner_obj {
        json!({
            "id": o.get("id").cloned().unwrap_or(Value::Null),
            "name": o.get("name").cloned().unwrap_or_else(|| json!("")),
            "email": "",
            "profileImagePath": "",
            "avatarColor": "primary",
            "profileChangedAt": iso_ms(0.0),
        })
    } else {
        user.clone()
    };
    let mut people = vec![json!({ "user": owner_user, "role": "owner" })];
    let shared = a.get("shared") == Some(&json!(true));
    if a.get("role").and_then(Value::as_str) == Some("owner") && shared {
        let sharing =
            call_photos_op(db, protocol_session_id, "albumSharing", json!({ "id": album_id }))
                .await?;
        if let Some(members) = sharing.get("members").and_then(Value::as_array) {
            for m in members {
                let username = m.get("username").and_then(Value::as_str).unwrap_or("");
                people.push(json!({
                    "user": {
                        "id": m["userId"],
                        "name": m["displayName"],
                        "email": format!("{username}@dar"),
                        "profileImagePath": "",
                        "avatarColor": "primary",
                        "profileChangedAt": iso_ms(0.0),
                    },
                    "role": m["role"],
                }));
            }
        }
    }
    let updated_at = a.get("updatedAt").and_then(Value::as_f64).unwrap_or(0.0);
    let cover_id = a
        .get("coverId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let owner_id = owner_obj
        .and_then(|o| o.get("id"))
        .cloned()
        .unwrap_or_else(|| user["id"].clone());

    let mut out = json!({
        "id": a["id"],
        "albumName": a["name"],
        "description": a["description"],
        "albumThumbnailAssetId": cover_id.map(|s| json!(s)).unwrap_or(Value::Null),
        "albumUsers": people,
        "assetCount": a["count"],
        "createdAt": iso_ms(updated_at),
        "updatedAt": iso_ms(updated_at),
        "hasSharedLink": false,
        "shared": shared || owner_obj.is_some(),
        "isActivityEnabled": false,
        "order": "desc",
        "ownerId": owner_id,
    });
    if with_assets {
        let page = call_photos_op(
            db,
            protocol_session_id,
            "page",
            json!({ "view": "album", "album": album_id, "limit": 500 }),
        )
        .await?;
        let items: Vec<Value> = page
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|i| {
                let kind = i.get("kind").and_then(Value::as_str).unwrap_or("image");
                let taken = i.get("takenAt").and_then(Value::as_f64).unwrap_or(0.0);
                json!({
                    "id": i["id"],
                    "type": if kind == "video" { "VIDEO" } else { "IMAGE" },
                    "originalFileName": i["name"],
                    "fileCreatedAt": iso_ms(taken),
                    "isFavorite": i["favorite"],
                })
            })
            .collect();
        out.as_object_mut().expect("obj").insert("assets".into(), Value::Array(items));
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_immich(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    photos: &Arc<PhotosRuntime>,
    root: &Path,
    body: Body,
) -> Result<Response> {
    match dispatch(runtime, method, path, query, headers, photos, root, body).await {
        Ok(resp) => Ok(resp),
        Err(err) => Ok(immich_error(err)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    photos: &Arc<PhotosRuntime>,
    root: &Path,
    body: Body,
) -> Result<Response> {
    let stripped = path.strip_prefix(IMMICH_BASE).unwrap_or("");
    let route = if stripped.is_empty() { "/" } else { stripped };
    let origin = &runtime.config.origin;
    let secure = origin.starts_with("https://");

    // Public server information the app reads before sign-in.
    if *method == Method::GET && route == "/server/ping" {
        return Ok(send_json(&json!({ "res": "pong" }), StatusCode::OK, &[]));
    }
    if *method == Method::GET && route == "/server/version" {
        return Ok(send_json(
            &json!({ "major": 3, "minor": 4, "patch": 1, "prerelease": null, "featureLevel": 1 }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET && route == "/server/features" {
        return Ok(send_json(
            &json!({
                "backups": false,
                "configFile": false,
                "duplicateDetection": true,
                "email": false,
                "facialRecognition": false,
                "importFaces": false,
                "map": false,
                "oauth": true,
                "oauthAutoLaunch": false,
                "ocr": false,
                "passwordLogin": true,
                "realtimeTranscoding": false,
                "reverseGeocoding": false,
                "search": false,
                "sidecar": false,
                "smartSearch": false,
                "trash": true,
            }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET && route == "/server/config" {
        return Ok(send_json(
            &json!({
                "externalDomain": origin,
                "isInitialized": true,
                "isOnboarded": true,
                "loginPageMessage": "Use Sign in with Dar, or your directory email (or your Dar username followed by @dar) and an app password with Photos access.",
                "maintenanceMode": false,
                "mapDarkStyleUrl": "",
                "mapLightStyleUrl": "",
                "minFaces": 3,
                "oauthButtonText": "Sign in with Dar",
                "publicUsers": false,
                "trashDays": 0,
                "userDeleteDelay": 7,
            }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET && route == "/server/media-types" {
        let image: Vec<String> = MEDIA_TYPES
            .iter()
            .filter(|(_, t)| t.starts_with("image/"))
            .map(|(e, _)| format!(".{e}"))
            .collect();
        let video: Vec<String> = MEDIA_TYPES
            .iter()
            .filter(|(_, t)| t.starts_with("video/"))
            .map(|(e, _)| format!(".{e}"))
            .collect();
        return Ok(send_json(
            &json!({ "image": image, "video": video, "sidecar": [] }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET && route == "/server/about" {
        return Ok(send_json(
            &json!({
                "version": "v3.4.1",
                "versionUrl": "https://github.com/immich-app/immich/releases/tag/v3.4.1",
                "licensed": false,
                "repository": "Dar (Immich-compatible)",
            }),
            StatusCode::OK,
            &[],
        ));
    }

    if *method == Method::POST && route == "/auth/login" {
        runtime.attempt_up_to(&login_rate_key(headers), 10)?;
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let email = p.get("email").and_then(Value::as_str).map(str::trim).unwrap_or("");
        let password = p.get("password").and_then(Value::as_str).unwrap_or("");
        if email.split('@').next().unwrap_or("").is_empty()
            || email.len() > 320
            || password.is_empty()
            || password.len() > 1024
        {
            return Err(AppError::with("unauthenticated", 401, "Incorrect email or password"));
        }
        let (secret, token_digest) = auth::opaque_token();
        let user = runtime
            .db
            .call(
                "service_immich_login",
                json!([
                    email,
                    auth::digest_token(password),
                    token_digest,
                    device_header(headers, "devicetype", "device-type", "Mobile"),
                    device_header(headers, "devicemodel", "device-model", "Immich"),
                ]),
            )
            .await
            .map_err(|err| {
                if err.code == "unauthenticated" || err.code == "forbidden" {
                    AppError::with(
                        "unauthenticated",
                        401,
                        if err.code == "forbidden" {
                            "This app password has no Photos access."
                        } else {
                            "Incorrect email or password"
                        },
                    )
                } else {
                    err
                }
            })?;
        return Ok(send_json(
            &json!({
                "accessToken": secret,
                "userId": user["id"],
                "userEmail": user["email"],
                "name": user["name"],
                "isAdmin": user["isAdmin"],
                "isOnboarded": true,
                "profileImagePath": "",
                "shouldChangePassword": false,
            }),
            StatusCode::CREATED,
            &auth_cookies(&secret, "password", secure),
        ));
    }

    // OAuth: the app opens Dar's OIDC sign-in in a browser tab, then hands the callback here.
    if *method == Method::POST && route == "/oauth/authorize" {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let state = p.get("state").and_then(Value::as_str).unwrap_or("");
        let challenge = p.get("codeChallenge").and_then(Value::as_str).unwrap_or("");
        if p.get("redirectUri").and_then(Value::as_str) != Some(IMMICH_OAUTH_REDIRECT_URI) {
            return Err(AppError::with(
                "invalidInput",
                400,
                &format!("Use the redirect URI {IMMICH_OAUTH_REDIRECT_URI}."),
            ));
        }
        let valid_state = (8..=512).contains(&state.len())
            && state.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if !is_token(challenge) || !valid_state {
            return Err(AppError::with(
                "invalidInput",
                400,
                "A state and an S256 code challenge are required.",
            ));
        }
        let mut target =
            url::Url::parse(&format!("{origin}/oidc/auth")).map_err(|_| AppError::internal())?;
        target.query_pairs_mut()
            .append_pair("client_id", IMMICH_OAUTH_CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid")
            .append_pair("redirect_uri", IMMICH_OAUTH_REDIRECT_URI)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        return Ok(send_json(&json!({ "url": target.as_str() }), StatusCode::CREATED, &[]));
    }

    if *method == Method::POST && route == "/oauth/callback" {
        runtime.attempt_up_to(&login_rate_key(headers), 10)?;
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let raw = p.get("url").and_then(Value::as_str).unwrap_or("");
        let cb_re = Regex::new(r"^app\.immich:/(?://)?oauth-callback(?:\?|$)").expect("regex");
        if raw.len() > 4096 || !cb_re.is_match(raw) {
            return Err(AppError::with("invalidInput", 400, "Unexpected OAuth callback URL."));
        }
        let qs = raw.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: HashMap<String, String> = url::form_urlencoded::parse(qs.as_bytes())
            .into_owned()
            .collect();
        if params.get("error").is_some_and(|s| !s.is_empty()) {
            let desc = params
                .get("error_description")
                .filter(|s| !s.is_empty())
                .map(String::as_str)
                .unwrap_or("Sign-in was cancelled.");
            return Err(AppError::with("unauthenticated", 401, desc));
        }
        let expected_state = p.get("state").and_then(Value::as_str);
        if expected_state.is_none() || params.get("state").map(String::as_str) != expected_state {
            return Err(AppError::with(
                "unauthenticated",
                401,
                "The sign-in state does not match. Try again.",
            ));
        }
        let code = params.get("code").map(String::as_str).unwrap_or("");
        let verifier = p.get("codeVerifier").and_then(Value::as_str).unwrap_or("");
        let user_id = runtime
            .oidc
            .redeem_built_in_code(IMMICH_OAUTH_CLIENT_ID, code, IMMICH_OAUTH_REDIRECT_URI, verifier)
            .map_err(|_| {
                AppError::with("unauthenticated", 401, "Sign-in could not be completed. Try again.")
            })?;
        let (_, dummy_password_digest) = auth::opaque_token();
        let (secret, token_digest) = auth::opaque_token();
        let user = runtime
            .db
            .call(
                "service_immich_oauth_login",
                json!([
                    user_id,
                    dummy_password_digest,
                    token_digest,
                    device_header(headers, "devicetype", "device-type", "Mobile"),
                    device_header(headers, "devicemodel", "device-model", "Immich"),
                ]),
            )
            .await
            .map_err(|err| {
                if err.code == "forbidden" {
                    AppError::with("forbidden", 403, "Photos is turned off on this server.")
                } else {
                    err
                }
            })?;
        return Ok(send_json(
            &json!({
                "accessToken": secret,
                "userId": user["id"],
                "userEmail": user["email"],
                "name": user["name"],
                "isAdmin": user["isAdmin"],
                "isOnboarded": true,
                "profileImagePath": "",
                "shouldChangePassword": false,
            }),
            StatusCode::CREATED,
            &auth_cookies(&secret, "oauth", secure),
        ));
    }

    if *method == Method::GET && route == "/oauth/mobile-redirect" {
        let mut location = IMMICH_OAUTH_REDIRECT_URI.to_owned();
        if !query.is_empty() {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in query {
                ser.append_pair(k, v);
            }
            location.push('?');
            location.push_str(&ser.finish());
        }
        return Ok(Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, location)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::empty())
            .expect("response"));
    }

    // Everything else needs an Immich access token.
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            let prefix = s.get(..7)?;
            if prefix.eq_ignore_ascii_case("bearer ") { Some(s[7..].to_owned()) } else { None }
        });
    let token = headers
        .get("x-immich-user-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or(bearer)
        .or_else(|| cookie_value(headers, "immich_access_token"))
        .unwrap_or_default();
    if !is_token(&token) {
        return Err(AppError::with("unauthenticated", 401, "Authentication required"));
    }
    let token_digest = auth::digest_token(&token);
    let session = call_immich(&runtime.db, &token_digest, "session", json!({})).await?;
    let user = session["user"].clone();
    let user_id = user.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
    let protocol_session_id = session
        .get("protocolSessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    if *method == Method::POST && route == "/auth/validateToken" {
        return Ok(send_json(&json!({ "authStatus": true }), StatusCode::OK, &[]));
    }
    if *method == Method::GET && route == "/auth/status" {
        return Ok(send_json(
            &json!({ "password": true, "pinCode": false, "isElevated": false }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::POST && route == "/auth/logout" {
        call_immich(&runtime.db, &token_digest, "logout", json!({})).await?;
        return Ok(send_json(
            &json!({ "successful": true, "redirectUri": "/auth/login?autoLaunch=0" }),
            StatusCode::OK,
            &cleared_cookies(),
        ));
    }
    if *method == Method::GET && (route == "/users/me" || route == "/users/me/") {
        return Ok(send_json(&user, StatusCode::OK, &[]));
    }
    if *method == Method::GET && route == "/users" {
        return Ok(send_json(&json!([user]), StatusCode::OK, &[]));
    }
    if route == "/users/me/preferences" {
        return Ok(send_json(&preferences(), StatusCode::OK, &[]));
    }
    if *method == Method::GET && route == "/server/storage" {
        let used = user.get("quotaUsageInBytes").and_then(Value::as_f64).unwrap_or(0.0);
        let quota = user.get("quotaSizeInBytes").and_then(Value::as_f64).filter(|q| *q > 0.0);
        let size = match quota {
            Some(q) => q.max(1.0),
            None => (used + free_disk_bytes(root).await as f64).max(1.0),
        };
        let gib = 1024.0 * 1024.0 * 1024.0;
        let pct = ((used / size) * 10_000.0).round() / 100.0;
        return Ok(send_json(
            &json!({
                "diskSize": format!("{:.1} GiB", size / gib),
                "diskUse": format!("{:.1} GiB", used / gib),
                "diskAvailable": format!("{:.1} GiB", (size - used) / gib),
                "diskSizeRaw": size,
                "diskUseRaw": used,
                "diskAvailableRaw": size - used,
                "diskUsagePercentage": pct,
            }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET && route == "/server/version-check" {
        return Ok(send_json(
            &json!({ "checkedAt": iso_ms(now_ms()), "releaseVersion": "v3.4.1" }),
            StatusCode::OK,
            &[],
        ));
    }
    if *method == Method::GET
        && [
            "/partners",
            "/memories",
            "/tags",
            "/notifications",
            "/timeline/buckets",
            "/stacks",
            "/shared-links",
            "/sessions",
        ]
        .contains(&route)
    {
        return Ok(send_json(&json!([]), StatusCode::OK, &[]));
    }
    if *method == Method::GET && route == "/people" {
        return Ok(send_json(
            &json!({ "people": [], "total": 0, "hidden": 0, "hasNextPage": false }),
            StatusCode::OK,
            &[],
        ));
    }

    // Sync stream: checkpoints, then each requested type's channels as JSON lines, then completion.
    if *method == Method::POST && route == "/sync/stream" {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let types: Vec<String> = p
            .get("types")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let reset_req = p.get("reset") == Some(&json!(true));
        let mut start = Value::Null;
        for rounds in 0..200 {
            start = call_immich(
                &runtime.db,
                &token_digest,
                "syncStart",
                json!({ "reset": reset_req && rounds == 0 }),
            )
            .await?;
            if start.get("backfilling") != Some(&json!(true)) {
                break;
            }
        }
        let db = runtime.db.clone();
        let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(32);
        tokio::spawn(async move {
            let emit = |tx: &mpsc::Sender<std::result::Result<Bytes, std::io::Error>>, val: Value| {
                let mut s = val.to_string();
                s.push('\n');
                let tx = tx.clone();
                async move { tx.send(Ok(Bytes::from(s))).await.is_ok() }
            };
            if start.get("reset") == Some(&json!(true)) {
                let _ = emit(&tx, json!({ "type": "SyncResetV1", "data": {}, "ack": "SyncResetV1|reset" })).await;
                return;
            }
            let checkpoints = start.get("checkpoints").and_then(Value::as_object);
            let up_to = start.get("upTo").and_then(Value::as_f64).unwrap_or(0.0);
            let now_id = start.get("nowId").and_then(Value::as_str).unwrap_or("");
            let order: &[(&str, &[(&str, &str)])] = &[
                ("AuthUsersV1", &[("authUserV1", "AuthUserV1")]),
                ("AuthUsersV2", &[("authUser", "AuthUserV2")]),
                ("UsersV1", &[("users", "UserV1")]),
                ("PartnersV1", &[]),
                ("AssetsV2", &[("assetDeletes", "AssetDeleteV1"), ("assets", "AssetV2")]),
                ("StacksV1", &[("stackDeletes", "StackDeleteV1"), ("stacks", "StackV1")]),
                ("PartnerAssetsV2", &[]),
                ("PartnerStacksV1", &[]),
                ("AlbumAssetsV2", &[("albumAssets", "AlbumAssetCreateV2")]),
                ("AlbumsV2", &[("albumDeletes", "AlbumDeleteV1"), ("albums", "AlbumV2")]),
                (
                    "AlbumUsersV1",
                    &[("albumUserDeletes", "AlbumUserDeleteV1"), ("albumUsers", "AlbumUserV1")],
                ),
                (
                    "AlbumToAssetsV1",
                    &[
                        ("albumToAssetDeletes", "AlbumToAssetDeleteV1"),
                        ("albumToAssets", "AlbumToAssetV1"),
                    ],
                ),
                ("AssetExifsV1", &[("exifs", "AssetExifV1")]),
                ("AlbumAssetExifsV1", &[("albumAssetExifs", "AlbumAssetExifCreateV1")]),
            ];
            let cp = |ack_name: &str| -> f64 {
                seq_of(checkpoints.and_then(|m| m.get(ack_name)).and_then(Value::as_str))
            };
            for (ty, channels) in order {
                if !types.iter().any(|t| t == ty) || channels.is_empty() {
                    continue;
                }
                for (channel, ack_name) in *channels {
                    let mut after = cp(ack_name);
                    if *channel == "assets" && types.iter().any(|t| t == "AssetExifsV1") {
                        after = after.min(cp("AssetExifV1"));
                    } else if *channel == "albumAssets"
                        && types.iter().any(|t| t == "AlbumAssetExifsV1")
                    {
                        after = after.min(cp("AlbumAssetExifCreateV1"));
                    }
                    for _ in 0..10_000 {
                        if tx.is_closed() {
                            return;
                        }
                        let Ok(result) = call_immich(
                            &db,
                            &token_digest,
                            "syncPage",
                            json!({ "channel": channel, "after": after, "limit": 500, "upTo": up_to }),
                        )
                        .await
                        else {
                            return;
                        };
                        if let Some(lines) = result.get("lines").and_then(Value::as_array) {
                            for line in lines {
                                if !emit(&tx, line.clone()).await {
                                    return;
                                }
                            }
                        }
                        after = result.get("last").and_then(Value::as_f64).unwrap_or(after);
                        if result.get("more") != Some(&json!(true)) {
                            break;
                        }
                    }
                }
            }
            let _ = emit(
                &tx,
                json!({ "type": "SyncCompleteV1", "data": {}, "ack": format!("SyncCompleteV1|{now_id}") }),
            )
            .await;
        });
        let stream = async_stream_from_mpsc(rx);
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/jsonlines+json; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from_stream(stream))
            .expect("response"));
    }

    if route == "/sync/ack" {
        if *method == Method::GET {
            let acks = call_immich(&runtime.db, &token_digest, "acks", json!({})).await?;
            return Ok(send_json(&acks, StatusCode::OK, &[]));
        }
        if *method == Method::POST {
            let p = body_object(body, 2 * 1024 * 1024, headers).await?;
            call_immich(
                &runtime.db,
                &token_digest,
                "setAcks",
                json!({ "acks": p.get("acks") }),
            )
            .await?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        if *method == Method::DELETE {
            let has_body = headers
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v != "0");
            let types = if has_body {
                body_object(body, 2 * 1024 * 1024, headers)
                    .await?
                    .get("types")
                    .cloned()
            } else {
                None
            };
            let payload = match types {
                Some(v) if !v.is_null() => json!({ "types": v }),
                _ => json!({}),
            };
            call_immich(&runtime.db, &token_digest, "deleteAcks", payload).await?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }

    // Backup: duplicate check by SHA-1, then multipart uploads into the Photos folder.
    if *method == Method::POST && route == "/assets/bulk-upload-check" {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let res = call_immich(
            &runtime.db,
            &token_digest,
            "bulkCheck",
            json!({ "assets": p.get("assets") }),
        )
        .await?;
        return Ok(send_json(&res, StatusCode::OK, &[]));
    }

    if *method == Method::POST && route == "/assets" {
        let ct = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let boundary_re = Regex::new(r#"(?i)boundary=(?:"([^"]+)"|([^;\s]+))"#).expect("regex");
        let boundary = boundary_re
            .captures(ct)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| AppError::with("invalidInput", 400, "Expected multipart form data."))?;
        let dir = root.join("uploads").join(format!("immich-{}", auth::uuid()));
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .await
            .map_err(|_| AppError::internal())?;
        let upload_result = async {
            let parts = read_multipart(body, &boundary, &dir, MAX_FILE_BYTES).await?;
            let field = |name: &str| -> Option<&str> {
                parts
                    .iter()
                    .find(|p| p.name == name && p.value.is_some())
                    .and_then(|p| p.value.as_deref())
            };
            let data = parts
                .iter()
                .find(|p| p.name == "assetData" && p.file.is_some())
                .ok_or_else(|| AppError::with("invalidInput", 400, "assetData is required."))?;
            let data_file = data.file.as_ref().expect("file");
            let raw_name = field("filename")
                .filter(|s| !s.is_empty())
                .or(data.filename.as_deref().filter(|s| !s.is_empty()))
                .unwrap_or("upload");
            let name: String = raw_name
                .chars()
                .map(|c| if c == '\\' || c == '/' { '_' } else { c })
                .take(255)
                .collect();
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let media_type = match data.media_type.as_deref() {
                Some(mt) if !mt.is_empty() && mt != "application/octet-stream" => mt.to_owned(),
                _ => media_type_for_ext(&ext)
                    .unwrap_or("application/octet-stream")
                    .to_owned(),
            };
            if photo_kind(&media_type, &name).is_none() {
                return Err(AppError::with(
                    "invalidInput",
                    400,
                    "Only photos and videos can be backed up.",
                ));
            }
            let created_src = field("fileCreatedAt").unwrap_or("");
            let ym_re = Regex::new(r"^(\d{4})-(\d{2})").expect("regex");
            let now_iso = iso_ms(now_ms());
            let caps = ym_re
                .captures(created_src)
                .or_else(|| ym_re.captures(&now_iso))
                .expect("ym");
            let year: i64 = caps[1].parse().unwrap_or(1970);
            let month: i64 = caps[2].parse().unwrap_or(1);

            let begun = call_immich(
                &runtime.db,
                &token_digest,
                "uploadBegin",
                json!({
                    "sha1": data_file.sha1,
                    "name": name,
                    "size": data_file.size,
                    "mediaType": media_type,
                    "year": year,
                    "month": month,
                }),
            )
            .await?;
            if let Some(dup) = begun.get("duplicate").and_then(Value::as_str).filter(|s| !s.is_empty())
            {
                return Ok(send_json(
                    &json!({ "id": dup, "status": "duplicate" }),
                    StatusCode::OK,
                    &[],
                ));
            }
            let f = tokio::fs::File::open(&data_file.path)
                .await
                .map_err(|_| AppError::internal())?;
            let digest = runtime
                .blobs
                .receive(tokio_util::io::ReaderStream::new(f), data_file.size)
                .await?;
            let mut finish_arg = json!({ "id": begun["id"], "digest": digest });
            if let Some(ft) = parse_iso_ms(created_src) {
                finish_arg
                    .as_object_mut()
                    .expect("obj")
                    .insert("fileTime".into(), json!(ft));
            }
            let finished =
                call_immich(&runtime.db, &token_digest, "uploadFinish", finish_arg).await?;
            let file_id = finished
                .get("fileId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let job: Option<PhotoJob> = finished
                .get("job")
                .filter(|v| !v.is_null())
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
            let _ = photos.index_now(job).await;
            let favorite = field("isFavorite") == Some("true");
            let archived = field("visibility") == Some("archive");
            if favorite || archived {
                let mut upd = Map::new();
                upd.insert("ids".into(), json!([file_id]));
                if favorite {
                    upd.insert("favorite".into(), json!(true));
                }
                if archived {
                    upd.insert("archived".into(), json!(true));
                }
                let _ = call_photos_op(&runtime.db, &protocol_session_id, "update", Value::Object(upd)).await;
            }
            Ok(send_json(
                &json!({ "id": file_id, "status": "created" }),
                StatusCode::CREATED,
                &[],
            ))
        }
        .await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return upload_result;
    }

    if route == "/assets" && (*method == Method::PUT || *method == Method::DELETE) {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let ids: Vec<Value> = p
            .get("ids")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if *method == Method::PUT {
            update_assets(&runtime.db, &protocol_session_id, &ids, &p).await?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        // Immich moves to the trash; force deletes for good.
        match call_photos_op(&runtime.db, &protocol_session_id, "trash", json!({ "ids": ids })).await
        {
            Ok(_) => {}
            Err(e) if e.code == "notFound" => {}
            Err(e) => return Err(e),
        }
        if p.get("force") == Some(&json!(true)) {
            call_photos_op(&runtime.db, &protocol_session_id, "delete", json!({ "ids": ids }))
                .await?;
        }
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    if *method == Method::POST && route == "/trash/restore/assets" {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let count = p.get("ids").and_then(Value::as_array).map_or(0, Vec::len);
        call_photos_op(
            &runtime.db,
            &protocol_session_id,
            "restore",
            json!({ "ids": p.get("ids") }),
        )
        .await?;
        return Ok(send_json(&json!({ "count": count }), StatusCode::OK, &[]));
    }

    if *method == Method::POST && (route == "/trash/restore" || route == "/trash/empty") {
        let page = call_photos_op(
            &runtime.db,
            &protocol_session_id,
            "page",
            json!({ "view": "trash", "limit": 500 }),
        )
        .await?;
        let ids: Vec<Value> = page
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|i| i.get("id").cloned()).collect())
            .unwrap_or_default();
        if !ids.is_empty() {
            let op = if route == "/trash/restore" { "restore" } else { "delete" };
            call_photos_op(&runtime.db, &protocol_session_id, op, json!({ "ids": ids })).await?;
        }
        return Ok(send_json(&json!({ "count": ids.len() }), StatusCode::OK, &[]));
    }

    // Assets: /assets/:id(/[a-z]+(/[a-z]+)?)?
    if let Some(rest) = route.strip_prefix("/assets/") {
        let (id, action) = match rest.split_once('/') {
            Some((id, act)) => (id, Some(act)),
            None => (rest, None),
        };
        let valid_action = action.is_none_or(|a| {
            let segs: Vec<&str> = a.split('/').collect();
            segs.len() <= 2
                && segs
                    .iter()
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()))
        });
        if is_uuid(id) && valid_action {
            if action.is_none() && *method == Method::GET {
                let detail =
                    call_photos_op(&runtime.db, &protocol_session_id, "detail", json!({ "id": id }))
                        .await?;
                return Ok(send_json(&asset_response(&detail, &user_id), StatusCode::OK, &[]));
            }
            if action.is_none() && *method == Method::PUT {
                let p = body_object(body, 2 * 1024 * 1024, headers).await?;
                update_assets(&runtime.db, &protocol_session_id, &[json!(id)], &p).await?;
                let detail =
                    call_photos_op(&runtime.db, &protocol_session_id, "detail", json!({ "id": id }))
                        .await?;
                return Ok(send_json(&asset_response(&detail, &user_id), StatusCode::OK, &[]));
            }
            let info: AssetInfo = serde_json::from_value(
                call_photos_op(&runtime.db, &protocol_session_id, "asset", json!({ "id": id }))
                    .await?,
            )
            .map_err(|_| AppError::internal())?;
            if action == Some("thumbnail") && (*method == Method::GET || *method == Method::HEAD) {
                let size = match query.get("size").map(String::as_str) {
                    None | Some("") | Some("thumbnail") => "s",
                    _ => "m",
                };
                if let Some(file) = photos.thumbnail_file(&info.blob_id, &info.kind, size).await {
                    return send_file(
                        method,
                        headers,
                        &file,
                        "image/webp",
                        &format!("\"{}-{size}\"", info.blob_id),
                        "private, max-age=86400",
                    )
                    .await
                    .map_err(|_| AppError::internal());
                }
                if info.kind == "image"
                    && matches!(
                        info.media_type.as_str(),
                        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                    )
                {
                    let path = runtime.blobs.path(&info.blob_id)?;
                    return send_file(
                        method,
                        headers,
                        &path,
                        &info.media_type,
                        &format!("\"{}\"", info.blob_id),
                        "private, max-age=86400",
                    )
                    .await
                    .map_err(|_| AppError::new("contentUnavailable", 503));
                }
                return Err(AppError::with(
                    "notFound",
                    404,
                    "No preview is available for this file.",
                ));
            }
            if action == Some("original") && (*method == Method::GET || *method == Method::HEAD) {
                return runtime
                    .blobs
                    .download(method, headers, &info.to_blob_file(), false)
                    .await;
            }
            if action == Some("video/playback")
                && (*method == Method::GET || *method == Method::HEAD)
            {
                if info.kind != "video" {
                    return Err(AppError::not_found());
                }
                if info.playback == "remux" || info.playback == "transcode" {
                    let state = photos
                        .video(VideoRequest {
                            blob_id: info.blob_id.clone(),
                            mode: info.playback.clone(),
                            audio: info.audio_codec.clone(),
                            duration: info.duration,
                        })
                        .await;
                    if state.state == "ready"
                        && let Some(file) = state.file
                    {
                        return send_file(
                            method,
                            headers,
                            &file,
                            "video/mp4",
                            &format!("\"{}-v\"", info.blob_id),
                            "private, max-age=86400",
                        )
                        .await
                        .map_err(|_| AppError::internal());
                    }
                }
                let path = runtime.blobs.path(&info.blob_id)?;
                let ct = if info.media_type == "video/webm" {
                    "video/webm"
                } else {
                    "video/mp4"
                };
                return send_file(
                    method,
                    headers,
                    &path,
                    ct,
                    &format!("\"{}\"", info.blob_id),
                    "private, no-store",
                )
                .await
                .map_err(|_| AppError::new("contentUnavailable", 503));
            }
        }
    }

    // Albums.
    if route == "/albums" && *method == Method::GET {
        let overview =
            call_photos_op(&runtime.db, &protocol_session_id, "overview", json!({})).await?;
        let own = overview
            .get("albums")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let shared_albums = overview
            .get("sharedAlbums")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let list: Vec<Value> = match query.get("shared").map(String::as_str) {
            Some("true") => own
                .into_iter()
                .filter(|a| a.get("shared") == Some(&json!(true)))
                .chain(shared_albums)
                .collect(),
            Some("false") => own
                .into_iter()
                .filter(|a| a.get("shared") != Some(&json!(true)))
                .collect(),
            _ => own.into_iter().chain(shared_albums).collect(),
        };
        let mut out = Vec::with_capacity(list.len());
        for a in &list {
            out.push(album_response(&runtime.db, &protocol_session_id, &user, a, false).await?);
        }
        return Ok(send_json(&Value::Array(out), StatusCode::OK, &[]));
    }

    if route == "/albums" && *method == Method::POST {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let name = match p.get("albumName") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => "Untitled".into(),
            Some(other) => other.to_string(),
        };
        let mut arg = json!({ "id": auth::uuid(), "name": name });
        if let Some(ids) = p.get("assetIds").and_then(Value::as_array)
            && !ids.is_empty()
        {
            arg.as_object_mut()
                .expect("obj")
                .insert("ids".into(), Value::Array(ids.clone()));
        }
        let album = call_photos_op(&runtime.db, &protocol_session_id, "createAlbum", arg).await?;
        let resp = album_response(&runtime.db, &protocol_session_id, &user, &album, false).await?;
        return Ok(send_json(&resp, StatusCode::CREATED, &[]));
    }

    if *method == Method::PUT && route == "/albums/assets" {
        let p = body_object(body, 2 * 1024 * 1024, headers).await?;
        let album_ids = p
            .get("albumIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let ids = p
            .get("assetIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for album_id in album_ids {
            call_photos_op(
                &runtime.db,
                &protocol_session_id,
                "addToAlbum",
                json!({ "id": album_id, "ids": ids }),
            )
            .await?;
        }
        return Ok(send_json(&json!({ "success": true }), StatusCode::OK, &[]));
    }

    if let Some(rest) = route.strip_prefix("/albums/") {
        let (id, assets) = match rest.split_once('/') {
            Some((id, "assets")) => (id, true),
            None => (rest, false),
            _ => {
                return Err(AppError::with(
                    "notFound",
                    404,
                    &format!("{method} {route} is not available on this server."),
                ));
            }
        };
        if is_uuid(id) {
            let find_album = || async {
                let o =
                    call_photos_op(&runtime.db, &protocol_session_id, "overview", json!({})).await?;
                let own = o.get("albums").and_then(Value::as_array);
                let shared = o.get("sharedAlbums").and_then(Value::as_array);
                own.into_iter()
                    .flatten()
                    .chain(shared.into_iter().flatten())
                    .find(|x| x.get("id").and_then(Value::as_str) == Some(id))
                    .cloned()
                    .ok_or_else(AppError::not_found)
            };
            if !assets && *method == Method::GET {
                let found = find_album().await?;
                let with_assets = query.get("withoutAssets").map(String::as_str) != Some("true");
                let resp =
                    album_response(&runtime.db, &protocol_session_id, &user, &found, with_assets)
                        .await?;
                return Ok(send_json(&resp, StatusCode::OK, &[]));
            }
            if !assets && *method == Method::PATCH {
                let p = body_object(body, 2 * 1024 * 1024, headers).await?;
                let current = find_album().await?;
                let mut edit = Map::new();
                edit.insert("id".into(), json!(id));
                edit.insert("revision".into(), current["revision"].clone());
                if let Some(Value::String(name)) = p.get("albumName") {
                    edit.insert("name".into(), json!(name));
                }
                if let Some(Value::String(desc)) = p.get("description") {
                    edit.insert("description".into(), json!(desc));
                }
                if let Some(Value::String(cover)) = p.get("albumThumbnailAssetId") {
                    edit.insert("coverId".into(), json!(cover));
                }
                call_photos_op(&runtime.db, &protocol_session_id, "editAlbum", Value::Object(edit))
                    .await?;
                let updated = find_album().await?;
                let resp =
                    album_response(&runtime.db, &protocol_session_id, &user, &updated, false)
                        .await?;
                return Ok(send_json(&resp, StatusCode::OK, &[]));
            }
            if !assets && *method == Method::DELETE {
                call_photos_op(
                    &runtime.db,
                    &protocol_session_id,
                    "deleteAlbum",
                    json!({ "id": id }),
                )
                .await?;
                return Ok(StatusCode::NO_CONTENT.into_response());
            }
            if assets && (*method == Method::PUT || *method == Method::DELETE) {
                let p = body_object(body, 2 * 1024 * 1024, headers).await?;
                let ids: Vec<String> = p
                    .get("ids")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let page = call_photos_op(
                    &runtime.db,
                    &protocol_session_id,
                    "page",
                    json!({ "view": "album", "album": id, "limit": 500 }),
                )
                .await?;
                let before: HashSet<String> = page
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i.get("id").and_then(Value::as_str).map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let op = if *method == Method::PUT { "addToAlbum" } else { "removeFromAlbum" };
                call_photos_op(
                    &runtime.db,
                    &protocol_session_id,
                    op,
                    json!({ "id": id, "ids": ids }),
                )
                .await?;
                let out: Vec<Value> = ids
                    .iter()
                    .map(|asset_id| {
                        let existed = before.contains(asset_id);
                        if *method == Method::PUT {
                            if existed {
                                json!({ "id": asset_id, "success": false, "error": "duplicate" })
                            } else {
                                json!({ "id": asset_id, "success": true })
                            }
                        } else if existed {
                            json!({ "id": asset_id, "success": true })
                        } else {
                            json!({ "id": asset_id, "success": false, "error": "not_found" })
                        }
                    })
                    .collect();
                return Ok(send_json(&Value::Array(out), StatusCode::OK, &[]));
            }
        }
    }

    Err(AppError::with(
        "notFound",
        404,
        &format!("{method} {route} is not available on this server."),
    ))
}

fn async_stream_from_mpsc(
    rx: mpsc::Receiver<std::result::Result<Bytes, std::io::Error>>,
) -> impl futures_util::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}
