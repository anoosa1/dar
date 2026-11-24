//! Native-session Photos HTTP API (`next/runtime/photos/http.ts`) and local-file range
//! streamer (`send_file`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::auth::{self, is_uuid};
use crate::blobs::BlobFile;
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response, ok};
use crate::photos::immich_connector::ImmichConnector;
use crate::photos::runtime::{PhotoJob, PhotosRuntime, VideoRequest};
use crate::photos::{PHOTOS_READ_OPERATIONS, valid_link_password};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssetInfo {
    pub id: String,
    pub revision: u32,
    pub blob_id: String,
    pub kind: String,
    pub media_type: String,
    pub name: String,
    pub playback: String,
    pub audio_codec: String,
    pub duration: f64,
    pub size: f64,
}

impl AssetInfo {
    pub(crate) fn to_blob_file(&self) -> BlobFile {
        BlobFile {
            id: self.id.clone(),
            name: self.name.clone(),
            blob_id: self.blob_id.clone(),
            size: self.size,
            media_type: self.media_type.clone(),
            revision: self.revision,
            locked: false,
        }
    }
}

fn is_renderable_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/avif" | "image/bmp"
    )
}

fn friendly_message(operation: &str, code: &str) -> Option<&'static str> {
    match (operation, code) {
        ("createAlbum", "quotaExceeded") => Some("You have reached the limit of 500 albums."),
        ("editAlbum", "conflict") => Some("This album changed elsewhere. Reload it and try again."),
        ("describe", "conflict") => {
            Some("This description changed elsewhere. Reload it and try again.")
        }
        ("uploadTarget", "conflict") | ("unlock", "conflict") => Some(
            "A file is in the way of the Photos folder (or a year or month inside it). Rename it in Files or choose another folder in Photos settings.",
        ),
        ("restore", "conflict") => Some(
            "A file with the same name is already where this photo was. Rename it in Files and try again.",
        ),
        ("resolveDuplicates", "conflict") => {
            Some("These duplicates changed. Reload the list and try again.")
        }
        ("shareAlbum", "notFound") => Some("No active account has that username."),
        ("shareAlbum", "quotaExceeded") => Some("An album can be shared with up to 100 people."),
        ("createLink", "quotaExceeded") => {
            Some("You can have up to 200 shared links. Remove one first.")
        }
        ("editLink", "conflict") => Some("This link changed elsewhere. Reload it and try again."),
        ("saveImmichConnection", "conflict") => {
            Some("The Immich connection changed elsewhere. Reload and try again.")
        }
        ("saveImmichConnection", "invalidInput") => {
            Some("Check the server address (http:// or https://) and the interval.")
        }
        ("addToAlbum", "quotaExceeded") => Some("An album can hold up to 50,000 items."),
        ("addToAlbum", "forbidden") => Some("You can view this album but not add to it."),
        _ => None,
    }
}

/// Streams a local file with byte ranges (video seeking) and a strong ETag (`sendFile` in `photos/http.ts`).
pub async fn send_file(
    method: &Method,
    headers: &HeaderMap,
    file: &Path,
    content_type: &str,
    tag: &str,
    cache: &str,
) -> std::io::Result<Response> {
    let mut f = tokio::fs::File::open(file).await?;
    let size = f.metadata().await?.len();
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(tag)
    {
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, tag)
            .header(header::CACHE_CONTROL, cache)
            .body(Body::empty())
            .expect("response"));
    }
    let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), StatusCode::OK);
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let if_range = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok());
    let mut content_range = None;
    if let Some(range) = range.filter(|_| if_range.is_none_or(|r| r == tag)) {
        let invalid = || {
            Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Body::empty())
                .expect("response")
        };
        let Some(spec) = range.strip_prefix("bytes=") else {
            return Ok(invalid());
        };
        let Some((from, to)) = spec.split_once('-') else {
            return Ok(invalid());
        };
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if !digits(from) || !digits(to) || (from.is_empty() && to.is_empty()) || size == 0 {
            return Ok(invalid());
        }
        if from.is_empty() {
            let Ok(suffix) = to.parse::<u64>() else {
                return Ok(invalid());
            };
            if suffix == 0 {
                return Ok(invalid());
            }
            start = size.saturating_sub(suffix);
        } else {
            let Ok(s) = from.parse::<u64>() else {
                return Ok(invalid());
            };
            start = s;
            if !to.is_empty() {
                let Ok(e) = to.parse::<u64>() else {
                    return Ok(invalid());
                };
                end = end.min(e);
            }
        }
        if start > end || start >= size {
            return Ok(invalid());
        }
        status = StatusCode::PARTIAL_CONTENT;
        content_range = Some(format!("bytes {start}-{end}/{size}"));
    }
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ETAG, tag)
        .header(header::CACHE_CONTROL, cache)
        .header(header::ACCEPT_RANGES, "bytes")
        .header("x-content-type-options", "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
        .header(header::CONTENT_LENGTH, length);
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    if *method == Method::HEAD || size == 0 {
        return Ok(builder.body(Body::empty()).expect("response"));
    }
    f.seek(std::io::SeekFrom::Start(start)).await?;
    let stream = tokio_util::io::ReaderStream::new(f.take(length));
    Ok(builder.body(Body::from_stream(stream)).expect("response"))
}

async fn call_photos(
    runtime: &Runtime,
    session_id: &str,
    operation: &str,
    value: Value,
) -> Result<Value> {
    runtime
        .db
        .operation(
            "service_photos",
            operation,
            PHOTOS_READ_OPERATIONS,
            json!([session_id, operation, value.to_string()]),
        )
        .await
        .map_err(|err| {
            if let Some(msg) = friendly_message(operation, &err.code) {
                AppError::with(&err.code, err.status, msg)
            } else {
                err
            }
        })
}

async fn input_object(
    body: Body,
    headers: &HeaderMap,
    allowed: &[&str],
) -> Result<Map<String, Value>> {
    let map = body_object(body, 128 * 1024, headers).await?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    Ok(map)
}

fn present_link(photos: &PhotosRuntime, origin: &str, mut link: Value) -> Value {
    if let Some(obj) = link.as_object_mut() {
        let sealed = obj
            .remove("sealed")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let token = photos.open_link(&sealed);
        let url = if token.is_empty() {
            String::new()
        } else {
            format!("{origin}/s/{token}")
        };
        obj.insert("url".into(), json!(url));
    }
    link
}

async fn link_input(
    photos: &PhotosRuntime,
    mut map: Map<String, Value>,
) -> Result<Map<String, Value>> {
    if let Some(password) = map.remove("password") {
        match password {
            Value::Null => {
                map.insert("passwordVerifier".into(), json!(""));
            }
            Value::String(s) if s.is_empty() => {
                map.insert("passwordVerifier".into(), json!(""));
            }
            Value::String(s) if valid_link_password(&s) => {
                let verifier = photos.hash_link_password(s).await?;
                map.insert("passwordVerifier".into(), json!(verifier));
            }
            _ => {
                return Err(AppError::with(
                    "invalidInput",
                    400,
                    "Use a link password of 4 to 200 characters.",
                ));
            }
        }
    }
    Ok(map)
}

/// Native-session Photos API (`handlePhotosHttp` in `next/runtime/photos/http.ts`).
#[allow(clippy::too_many_arguments)]
pub async fn handle_http(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    user_id: &str,
    photos: &Arc<PhotosRuntime>,
    immich: &ImmichConnector,
    _root: &Path,
    body: Body,
) -> Result<Response> {
    let route = path.strip_prefix("/api/v1/photos").unwrap_or("");
    let origin = &runtime.config.origin;

    if route == "/overview" && *method == Method::GET {
        let mut value = call_photos(runtime, session_id, "overview", json!({})).await?;
        photos.kick();
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "immich".into(),
                immich
                    .progress(user_id)
                    .map(|p| json!(p))
                    .unwrap_or(Value::Null),
            );
        }
        return ok(value);
    }

    if route == "/assets" && *method == Method::GET {
        let limit: i64 = match query.get("limit") {
            None => 120,
            Some(s) if s.trim().is_empty() => 0,
            Some(s) => s.trim().parse::<i64>().map_err(|_| AppError::invalid())?,
        };
        let mut payload = Map::new();
        payload.insert(
            "view".into(),
            json!(query.get("view").map(String::as_str).unwrap_or("library")),
        );
        if let Some(album) = query.get("album").filter(|a| !a.is_empty()) {
            payload.insert("album".into(), json!(album));
        }
        payload.insert(
            "cursor".into(),
            json!(query.get("cursor").map(String::as_str).unwrap_or("")),
        );
        payload.insert("limit".into(), json!(limit));
        return ok(call_photos(runtime, session_id, "page", Value::Object(payload)).await?);
    }

    if route == "/assets" && *method == Method::PATCH {
        let map = input_object(body, headers, &["ids", "favorite", "archived"]).await?;
        return ok(call_photos(runtime, session_id, "update", Value::Object(map)).await?);
    }

    if let Some(action) = route.strip_prefix("/assets/")
        && ["trash", "restore", "delete", "lock", "unlock"].contains(&action)
        && *method == Method::POST
    {
        let allowed: &[&str] = if action == "unlock" {
            &["ids", "to"]
        } else {
            &["ids"]
        };
        let map = input_object(body, headers, allowed).await?;
        return ok(call_photos(runtime, session_id, action, Value::Object(map)).await?);
    }

    if route == "/trash/empty" && *method == Method::POST {
        return ok(call_photos(runtime, session_id, "emptyTrash", json!({})).await?);
    }

    if route == "/upload-target" && *method == Method::POST {
        let map = input_object(body, headers, &["year", "month"]).await?;
        return ok(call_photos(runtime, session_id, "uploadTarget", Value::Object(map)).await?);
    }

    if route == "/albums" && *method == Method::POST {
        let mut map = input_object(body, headers, &["name", "ids"]).await?;
        map.insert("id".into(), json!(auth::uuid()));
        let created = call_photos(runtime, session_id, "createAlbum", Value::Object(map)).await?;
        return Ok(json_response(&created, StatusCode::CREATED));
    }

    if route == "/duplicates" && *method == Method::GET {
        let cursor = query.get("cursor").map(String::as_str).unwrap_or("");
        return ok(call_photos(
            runtime,
            session_id,
            "duplicates",
            json!({ "cursor": cursor }),
        )
        .await?);
    }

    if route == "/duplicates/resolve" && *method == Method::POST {
        let map = input_object(body, headers, &["key", "keep"]).await?;
        return ok(
            call_photos(runtime, session_id, "resolveDuplicates", Value::Object(map)).await?,
        );
    }

    if route == "/duplicates/dismiss" && *method == Method::POST {
        let map = input_object(body, headers, &["key"]).await?;
        return ok(
            call_photos(runtime, session_id, "dismissDuplicates", Value::Object(map)).await?,
        );
    }

    // Immich: devices signed in with the mobile app, and Dar's connection to an Immich server.
    if route == "/immich" && *method == Method::GET {
        let (devices, connection) = tokio::try_join!(
            call_photos(runtime, session_id, "immichDevices", json!({})),
            call_photos(runtime, session_id, "immichConnection", json!({}))
        )?;
        let mut out = Map::new();
        out.insert("serverUrl".into(), json!(origin));
        if let Some(obj) = devices.as_object() {
            for (k, v) in obj {
                out.insert(k.clone(), v.clone());
            }
        }
        out.insert("connection".into(), connection);
        out.insert(
            "progress".into(),
            immich
                .progress(user_id)
                .map(|p| json!(p))
                .unwrap_or(Value::Null),
        );
        return ok(Value::Object(out));
    }

    if let Some(device_id) = route.strip_prefix("/immich/devices/")
        && is_uuid(device_id)
        && *method == Method::DELETE
    {
        return ok(call_photos(
            runtime,
            session_id,
            "revokeImmichDevice",
            json!({ "id": device_id }),
        )
        .await?);
    }

    if route == "/immich/connection" && *method == Method::PUT {
        let mut map = input_object(
            body,
            headers,
            &["url", "apiKey", "intervalMinutes", "enabled", "revision"],
        )
        .await?;
        let api_key = map.remove("apiKey");
        let current = call_photos(runtime, session_id, "immichConnection", json!({})).await?;
        let url = map
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_owned();
        if let Some(Value::String(key)) = api_key
            && !key.is_empty()
        {
            if key.len() > 500 {
                return Err(AppError::invalid());
            }
            if let Err(msg) = immich.test(&url, &key).await {
                let text = if msg.is_empty() {
                    "The Immich server could not be reached."
                } else {
                    msg.as_str()
                };
                return Err(AppError::with("immichUnavailable", 400, text));
            }
            let enabled = map.get("enabled") != Some(&json!(false));
            map.insert("sealedKey".into(), json!(photos.seal_immich_key(&key)));
            let saved = call_photos(
                runtime,
                session_id,
                "saveImmichConnection",
                Value::Object(map),
            )
            .await?;
            if enabled {
                immich.sync_now(user_id);
            }
            return ok(saved);
        }
        let connected = current.get("connected") == Some(&json!(true));
        let current_url = current.get("url").and_then(Value::as_str).unwrap_or("");
        if !connected || url != current_url {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Enter the API key for this Immich server.",
            ));
        }
        return ok(call_photos(
            runtime,
            session_id,
            "saveImmichConnection",
            Value::Object(map),
        )
        .await?);
    }

    if route == "/immich/connection" && *method == Method::DELETE {
        return ok(call_photos(runtime, session_id, "deleteImmichConnection", json!({})).await?);
    }

    if route == "/immich/connection/sync" && *method == Method::POST {
        immich.sync_now(user_id);
        return Ok(json_response(
            &json!({ "started": true }),
            StatusCode::ACCEPTED,
        ));
    }

    if route == "/stacks" && *method == Method::POST {
        let map = input_object(body, headers, &["ids", "primaryId"]).await?;
        let created = call_photos(runtime, session_id, "stack", Value::Object(map)).await?;
        return Ok(json_response(&created, StatusCode::CREATED));
    }

    // Sharing: people on this Dar and shared links.
    if route == "/links" && *method == Method::GET {
        let value = call_photos(runtime, session_id, "links", json!({})).await?;
        let links: Vec<Value> = value
            .get("links")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|l| present_link(photos, origin, l))
            .collect();
        return ok(json!({ "links": links }));
    }

    if route == "/links" && *method == Method::POST {
        let map = input_object(
            body,
            headers,
            &[
                "albumId",
                "ids",
                "expiresAt",
                "password",
                "allowDownload",
                "allowUpload",
                "showMetadata",
            ],
        )
        .await?;
        let mut map = link_input(photos, map).await?;
        let (secret, digest) = auth::opaque_token();
        map.insert("id".into(), json!(auth::uuid()));
        map.insert("digest".into(), json!(digest));
        map.insert("sealed".into(), json!(photos.seal_link(&secret)));
        let created = call_photos(runtime, session_id, "createLink", Value::Object(map)).await?;
        return Ok(json_response(
            &present_link(photos, origin, created),
            StatusCode::CREATED,
        ));
    }

    if let Some(id) = route.strip_prefix("/links/")
        && !id.contains('/')
    {
        if !is_uuid(id) {
            return Err(AppError::not_found());
        }
        if *method == Method::PATCH {
            let map = input_object(
                body,
                headers,
                &[
                    "revision",
                    "expiresAt",
                    "password",
                    "allowDownload",
                    "allowUpload",
                    "showMetadata",
                ],
            )
            .await?;
            let mut map = link_input(photos, map).await?;
            map.insert("id".into(), json!(id));
            let edited = call_photos(runtime, session_id, "editLink", Value::Object(map)).await?;
            return ok(present_link(photos, origin, edited));
        }
        if *method == Method::DELETE {
            return ok(call_photos(runtime, session_id, "deleteLink", json!({ "id": id })).await?);
        }
    }

    // Album sharing & members: /albums/:id/(sharing|members)(/:userId)?
    if let Some(rest) = route.strip_prefix("/albums/") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 2 && matches!(parts[1], "sharing" | "members") && parts.len() <= 3 {
            let album_id = parts[0];
            let kind = parts[1];
            let member_id = parts.get(2).copied();
            if !is_uuid(album_id) || member_id.is_some_and(|u| !is_uuid(u)) {
                return Err(AppError::not_found());
            }
            if kind == "sharing" && member_id.is_none() && *method == Method::GET {
                let value = call_photos(
                    runtime,
                    session_id,
                    "albumSharing",
                    json!({ "id": album_id }),
                )
                .await?;
                let links: Vec<Value> = value
                    .get("links")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| present_link(photos, origin, l))
                    .collect();
                return ok(json!({
                    "members": value.get("members").cloned().unwrap_or_else(|| json!([])),
                    "links": links,
                }));
            }
            if kind == "members" && member_id.is_none() && *method == Method::POST {
                let mut map = input_object(body, headers, &["username", "role"]).await?;
                map.insert("id".into(), json!(album_id));
                return ok(
                    call_photos(runtime, session_id, "shareAlbum", Value::Object(map)).await?,
                );
            }
            if kind == "members"
                && let Some(uid) = member_id
                && *method == Method::DELETE
            {
                return ok(call_photos(
                    runtime,
                    session_id,
                    "unshareAlbum",
                    json!({ "id": album_id, "userId": uid }),
                )
                .await?);
            }
        }
    }

    // Stacks: /stacks/:id(/remove)?
    if let Some(rest) = route.strip_prefix("/stacks/") {
        let (id, remove) = match rest.split_once('/') {
            Some((id, "remove")) => (id, true),
            None => (rest, false),
            _ => return Err(AppError::not_found()),
        };
        if !is_uuid(id) {
            return Err(AppError::not_found());
        }
        if !remove && *method == Method::GET {
            return ok(call_photos(runtime, session_id, "stackDetail", json!({ "id": id })).await?);
        }
        if !remove && *method == Method::PATCH {
            let mut map = input_object(body, headers, &["primaryId"]).await?;
            map.insert("id".into(), json!(id));
            return ok(call_photos(runtime, session_id, "stackCover", Value::Object(map)).await?);
        }
        if !remove && *method == Method::DELETE {
            return ok(call_photos(runtime, session_id, "unstack", json!({ "id": id })).await?);
        }
        if remove && *method == Method::POST {
            let mut map = input_object(body, headers, &["ids"]).await?;
            map.insert("id".into(), json!(id));
            return ok(call_photos(runtime, session_id, "unstack", Value::Object(map)).await?);
        }
    }

    // Assets: /assets/:id(/[a-z]+)?
    if let Some(rest) = route.strip_prefix("/assets/") {
        let (id, action) = match rest.split_once('/') {
            Some((id, act)) if !act.is_empty() && act.bytes().all(|b| b.is_ascii_lowercase()) => {
                (id, Some(act))
            }
            None => (rest, None),
            _ => return Err(AppError::not_found()),
        };
        if !is_uuid(id) {
            return Err(AppError::not_found());
        }
        if action.is_none() && *method == Method::GET {
            return ok(call_photos(runtime, session_id, "detail", json!({ "id": id })).await?);
        }
        if action == Some("description") && *method == Method::PUT {
            let mut map = input_object(body, headers, &["description", "revision"]).await?;
            map.insert("id".into(), json!(id));
            return ok(call_photos(runtime, session_id, "describe", Value::Object(map)).await?);
        }
        if action == Some("index") && *method == Method::POST {
            let res = call_photos(runtime, session_id, "indexJob", json!({ "id": id })).await?;
            let job: Option<PhotoJob> = res
                .get("job")
                .filter(|v| !v.is_null())
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
            return ok(json!({ "indexed": photos.index_now(job).await? }));
        }
        if action == Some("original") && (*method == Method::GET || *method == Method::HEAD) {
            let info: AssetInfo = serde_json::from_value(
                call_photos(runtime, session_id, "asset", json!({ "id": id })).await?,
            )
            .map_err(|_| AppError::internal())?;
            return runtime
                .blobs
                .download(method, headers, &info.to_blob_file(), false)
                .await;
        }
        if action == Some("thumbnail") && (*method == Method::GET || *method == Method::HEAD) {
            let size = if query.get("size").map(String::as_str) == Some("m") {
                "m"
            } else {
                "s"
            };
            let info: AssetInfo = serde_json::from_value(
                call_photos(runtime, session_id, "asset", json!({ "id": id })).await?,
            )
            .map_err(|_| AppError::internal())?;
            let tag = format!("\"{}-{size}\"", info.blob_id);
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                == Some(tag.as_str())
            {
                return Ok((
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, tag),
                        (header::CACHE_CONTROL, "private, max-age=86400".into()),
                    ],
                )
                    .into_response());
            }
            let Some(file) = photos.thumbnail_file(&info.blob_id, &info.kind, size).await else {
                if info.kind == "image" && is_renderable_image(&info.media_type) {
                    let entry: BlobFile = serde_json::from_value(
                        runtime
                            .db
                            .read("service_download", json!([session_id, id, ""]))
                            .await?,
                    )
                    .map_err(|_| AppError::internal())?;
                    return runtime.blobs.download(method, headers, &entry, true).await;
                }
                return Err(AppError::with(
                    "notFound",
                    404,
                    "No preview is available for this file.",
                ));
            };
            return send_file(
                method,
                headers,
                &file,
                "image/webp",
                &tag,
                "private, max-age=86400",
            )
            .await
            .map_err(|_| AppError::internal());
        }
        if action == Some("playback") && *method == Method::POST {
            let info: AssetInfo = serde_json::from_value(
                call_photos(runtime, session_id, "asset", json!({ "id": id })).await?,
            )
            .map_err(|_| AppError::internal())?;
            if info.kind != "video" {
                return Err(AppError::invalid());
            }
            if info.playback == "original" {
                return ok(json!({
                    "state": "ready",
                    "progress": 1,
                    "url": format!("/api/v1/photos/assets/{id}/video"),
                }));
            }
            let state = photos
                .video(VideoRequest {
                    blob_id: info.blob_id.clone(),
                    mode: if info.playback == "remux" {
                        "remux".into()
                    } else {
                        "transcode".into()
                    },
                    audio: info.audio_codec,
                    duration: info.duration,
                })
                .await;
            let url = if state.state == "ready" {
                format!("/api/v1/photos/assets/{id}/video?v={}", info.blob_id)
            } else {
                String::new()
            };
            return ok(json!({ "state": state.state, "progress": state.progress, "url": url }));
        }
        if action == Some("video") && (*method == Method::GET || *method == Method::HEAD) {
            let info: AssetInfo = serde_json::from_value(
                call_photos(runtime, session_id, "asset", json!({ "id": id })).await?,
            )
            .map_err(|_| AppError::internal())?;
            if info.kind != "video" {
                return Err(AppError::not_found());
            }
            if info.playback == "original"
                || query.get("source").map(String::as_str) == Some("original")
            {
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
            let file = photos
                .video_file(&info.blob_id)
                .map_err(|_| AppError::not_found())?;
            return send_file(
                method,
                headers,
                &file,
                "video/mp4",
                &format!("\"{}-v\"", info.blob_id),
                "private, max-age=86400",
            )
            .await
            .map_err(|_| AppError::with("notFound", 404, "This video is still being prepared."));
        }
    }

    // Albums: /albums/:id(/[a-z]+)?
    if let Some(rest) = route.strip_prefix("/albums/") {
        let (id, action) = match rest.split_once('/') {
            Some((id, act)) if !act.is_empty() && act.bytes().all(|b| b.is_ascii_lowercase()) => {
                (id, Some(act))
            }
            None => (rest, None),
            _ => return Err(AppError::not_found()),
        };
        if !is_uuid(id) {
            return Err(AppError::not_found());
        }
        if action.is_none() && *method == Method::PATCH {
            let mut map = input_object(
                body,
                headers,
                &["name", "description", "coverId", "revision"],
            )
            .await?;
            map.insert("id".into(), json!(id));
            return ok(call_photos(runtime, session_id, "editAlbum", Value::Object(map)).await?);
        }
        if action.is_none() && *method == Method::DELETE {
            return ok(call_photos(runtime, session_id, "deleteAlbum", json!({ "id": id })).await?);
        }
        if action == Some("add") && *method == Method::POST {
            let mut map = input_object(body, headers, &["ids"]).await?;
            map.insert("id".into(), json!(id));
            return ok(call_photos(runtime, session_id, "addToAlbum", Value::Object(map)).await?);
        }
        if action == Some("remove") && *method == Method::POST {
            let mut map = input_object(body, headers, &["ids"]).await?;
            map.insert("id".into(), json!(id));
            return ok(
                call_photos(runtime, session_id, "removeFromAlbum", Value::Object(map)).await?,
            );
        }
    }

    Err(AppError::not_found())
}
