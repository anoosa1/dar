//! Public shared links for Photos (`next/runtime/photos/public.ts`), served at
//! `/public/photos/<token>/...` (the page itself is `/s/<token>`). Only the token's digest
//! reaches the database. A link password, once entered, becomes an HttpOnly pass cookie
//! limited to the link's path.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::auth::{self, is_token, is_uuid};
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response, ok};
use crate::photos::http::{AssetInfo, send_file};
use crate::photos::runtime::{PhotoJob, PhotosRuntime, VideoRequest};
use crate::photos::{PASSWORD_DAYS, cookie_value};

const PASS_COOKIE: &str = "dar_link_pass";

async fn call_link(
    runtime: &Runtime,
    digest: &str,
    pass: &str,
    operation: &str,
    value: Value,
) -> Result<Value> {
    runtime
        .db
        .call(
            "service_photo_link",
            json!([digest, pass, operation, value.to_string()]),
        )
        .await
}

async fn input_object(
    body: Body,
    headers: &HeaderMap,
    allowed: &[&str],
) -> Result<Map<String, Value>> {
    let map = body_object(body, 16 * 1024, headers).await?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    Ok(map)
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_public(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    peer: &str,
    photos: &Arc<PhotosRuntime>,
    _root: &Path,
    body: Body,
) -> Result<Response> {
    let Some(rest) = path.strip_prefix("/public/photos/") else {
        return Err(AppError::not_found());
    };
    let (token, route) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    if !is_token(token) {
        return Err(AppError::not_found());
    }
    let digest = auth::digest_token(token);
    let base = format!("/public/photos/{token}");
    let pass = cookie_value(headers, PASS_COOKIE)
        .filter(|c| is_token(c))
        .map(|c| auth::digest_token(&c))
        .unwrap_or_default();

    let result = dispatch(
        runtime, method, route, &base, &digest, &pass, query, headers, peer, photos, body,
    )
    .await;

    match result {
        Err(err) if err.code == "notFound" && route.is_empty() => Err(AppError::with(
            "notFound",
            404,
            "This link is no longer available.",
        )),
        other => other,
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    runtime: &Runtime,
    method: &Method,
    route: &str,
    base: &str,
    digest: &str,
    pass: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    peer: &str,
    photos: &Arc<PhotosRuntime>,
    body: Body,
) -> Result<Response> {
    if route.is_empty() && *method == Method::GET {
        return ok(call_link(runtime, digest, pass, "info", json!({})).await?);
    }

    if route == "/password" && *method == Method::POST {
        runtime.attempt_up_to(&format!("photo-link:{digest}:{peer}"), 10)?;
        let map = input_object(body, headers, &["password"]).await?;
        let password = match map.get("password") {
            Some(Value::String(s)) if s.encode_utf16().count() <= 200 && s.len() <= 1024 => {
                s.clone()
            }
            _ => return Err(AppError::invalid()),
        };
        let v = call_link(runtime, digest, pass, "verifier", json!({})).await?;
        let verifier = v.get("verifier").and_then(Value::as_str).unwrap_or("");
        if verifier.is_empty() {
            return Err(AppError::with(
                "invalidInput",
                400,
                "This link has no password.",
            ));
        }
        if !photos
            .check_link_password(password, verifier.to_owned())
            .await?
        {
            return Err(AppError::with(
                "invalidCredentials",
                403,
                "That password is not correct.",
            ));
        }
        let (grant_secret, grant_digest) = auth::opaque_token();
        call_link(
            runtime,
            digest,
            pass,
            "grantPass",
            json!({ "digest": grant_digest }),
        )
        .await?;
        let age = (PASSWORD_DAYS * 86_400.0).round() as u64;
        let secure = if !runtime.config.development {
            "; Secure"
        } else {
            ""
        };
        let cookie = format!(
            "{PASS_COOKIE}={grant_secret}; Path={base}; HttpOnly; SameSite=Strict; Max-Age={age}{secure}"
        );
        let mut resp = json_response(&json!({ "ok": true }), StatusCode::OK);
        resp.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("cookie"),
        );
        return Ok(resp);
    }

    if route == "/assets" && *method == Method::GET {
        let cursor = query.get("cursor").map(String::as_str).unwrap_or("");
        let limit: Value = match query.get("limit") {
            None => json!(120),
            Some(s) if s.trim().is_empty() => json!(0),
            Some(s) => match s.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => json!(n),
                _ => Value::Null,
            },
        };
        return ok(call_link(
            runtime,
            digest,
            pass,
            "page",
            json!({ "cursor": cursor, "limit": limit }),
        )
        .await?);
    }

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
            return ok(call_link(runtime, digest, pass, "detail", json!({ "id": id })).await?);
        }
        let raw_info = call_link(runtime, digest, pass, "asset", json!({ "id": id })).await?;
        let allow_download = raw_info.get("allowDownload") == Some(&json!(true));
        let info: AssetInfo = serde_json::from_value(raw_info).map_err(|_| AppError::internal())?;

        if action == Some("thumbnail") && (*method == Method::GET || *method == Method::HEAD) {
            let size = if query.get("size").map(String::as_str) == Some("m") {
                "m"
            } else {
                "s"
            };
            let Some(file) = photos.thumbnail_file(&info.blob_id, &info.kind, size).await else {
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
                &format!("\"{}-{size}\"", info.blob_id),
                "private, max-age=3600",
            )
            .await
            .map_err(|_| AppError::internal());
        }

        if action == Some("playback") && *method == Method::POST {
            if info.kind != "video" {
                return Err(AppError::invalid());
            }
            if info.playback == "original" {
                return ok(json!({
                    "state": "ready",
                    "progress": 1,
                    "url": format!("{base}/assets/{id}/video"),
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
                format!("{base}/assets/{id}/video?v={}", info.blob_id)
            } else {
                String::new()
            };
            return ok(json!({ "state": state.state, "progress": state.progress, "url": url }));
        }

        if action == Some("video") && (*method == Method::GET || *method == Method::HEAD) {
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
                "private, max-age=3600",
            )
            .await
            .map_err(|_| AppError::with("notFound", 404, "This video is still being prepared."));
        }

        if action == Some("original") && (*method == Method::GET || *method == Method::HEAD) {
            if !allow_download {
                return Err(AppError::with(
                    "forbidden",
                    403,
                    "The owner turned off downloads for this link.",
                ));
            }
            return runtime
                .blobs
                .download(method, headers, &info.to_blob_file(), false)
                .await;
        }
    }

    // Visitor uploads go into the owner's Photos folder and this link's album.
    if route == "/uploads" && *method == Method::POST {
        let map = input_object(
            body,
            headers,
            &["name", "size", "mediaType", "year", "month"],
        )
        .await?;
        let created = call_link(runtime, digest, pass, "beginUpload", Value::Object(map)).await?;
        return Ok(json_response(&created, StatusCode::CREATED));
    }

    if let Some(id) = route.strip_prefix("/uploads/")
        && is_uuid(id)
    {
        if *method == Method::DELETE {
            return ok(
                call_link(runtime, digest, pass, "cancelUpload", json!({ "id": id })).await?,
            );
        }
        if *method == Method::PUT {
            let reserved = call_link(runtime, digest, pass, "upload", json!({ "id": id })).await?;
            let state = reserved.get("state").and_then(Value::as_str).unwrap_or("");
            if state == "complete" {
                return ok(json!({ "id": reserved["fileId"] }));
            }
            if state != "uploading" {
                return Err(AppError::new("conflict", 409));
            }
            let size = reserved.get("size").and_then(Value::as_f64).unwrap_or(0.0) as u64;
            let content_digest = runtime.blobs.receive(body.into_data_stream(), size).await?;
            let finished = call_link(
                runtime,
                digest,
                pass,
                "finishUpload",
                json!({ "id": reserved["id"], "digest": content_digest }),
            )
            .await?;
            let job: Option<PhotoJob> = finished
                .get("job")
                .filter(|v| !v.is_null())
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
            let _ = photos.index_now(job).await;
            return ok(json!({ "id": finished["fileId"] }));
        }
    }

    Err(AppError::not_found())
}
