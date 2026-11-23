//! Mail's HTTP API at `/api/v1/mail` (browser sessions only; app passwords cannot use it). The
//! database checks the account, the app switch and ownership; this layer reads message files,
//! talks to mail servers through `MailRuntime` and shapes requests.

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use super::message::{self, Attachment};
use super::runtime::{MailRuntime, url_encode};
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, ok, string};

pub const READ_OPERATIONS: &[&str] = &["overview", "messages", "message", "providers", "locate"];
const BODY_LIMIT: usize = 64 * 1024;
const SEND_LIMIT: usize = 36 * 1024 * 1024;
const ATTACHMENTS_LIMIT: usize = 25 * 1024 * 1024;

fn friendly(operation: &str, e: AppError) -> AppError {
    let message = match (operation, e.code.as_str()) {
        ("createAccount", "alreadyExists") => "This address is already added for that provider.",
        ("createAccount", "quotaExceeded") => "You can add up to 20 mail accounts.",
        ("saveProvider", "alreadyExists") => "A provider with that ID already exists.",
        ("saveProvider", "forbidden") => "Providers from the host configuration are changed in Nix, not here.",
        ("saveProvider", "invalidInput") => "Check the provider's servers, sign-in and OAuth details.",
        ("deleteProvider", "conflict") => "Accounts still use this provider; turn it off instead, or remove those accounts first.",
        ("deleteProvider", "forbidden") => "Providers from the host configuration are removed in Nix, not here.",
        _ => return e,
    };
    AppError::with(&e.code, e.status, message)
}

async fn input(body: Body, headers: &HeaderMap, limit: usize, allowed: &[&str]) -> Result<Map<String, Value>> {
    let map = body_object(body, limit, headers).await?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    Ok(map)
}

fn ids(p: &Map<String, Value>) -> Result<Vec<String>> {
    let list = p.get("ids").and_then(Value::as_array).ok_or_else(AppError::invalid)?;
    if list.is_empty() || list.len() > 500 {
        return Err(AppError::invalid());
    }
    list.iter().map(|v| string(Some(v), 100)).collect()
}

fn optional_bool(p: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => Err(AppError::invalid()),
    }
}

fn disposition(kind: &str, name: &str, inline: bool) -> String {
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' }).collect();
    format!("{}; filename=\"{}\"; filename*=UTF-8''{}", if inline && kind.starts_with("image/") && kind != "image/svg+xml" { "inline" } else { "attachment" }, safe, url_encode(name))
}

fn download(kind: &str, name: &str, data: Vec<u8>, inline: bool) -> Result<Response> {
    // Only plain images display in place; everything else downloads, never as active content.
    let shown = inline && matches!(kind, "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif");
    let mut res = (StatusCode::OK, data).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(if shown { kind } else { "application/octet-stream" }).map_err(|_| AppError::internal())?);
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disposition(kind, name, shown)).map_err(|_| AppError::internal())?);
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    Ok(res)
}

/// Accounts' credentials as the server will want them.
fn credential(p: &Map<String, Value>, auth: &str) -> Result<Value> {
    match auth {
        "password" => Ok(json!({ "kind": "password", "password": string(p.get("password"), 1024).and_then(|s| if s.is_empty() { Err(AppError::invalid()) } else { Ok(s) })? })),
        "token" => Ok(json!({ "kind": "token", "token": string(p.get("token"), 4096).and_then(|s| if s.trim().is_empty() { Err(AppError::invalid()) } else { Ok(s.trim().to_owned()) })? })),
        _ => Err(AppError::invalid()),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_http(runtime: &Runtime, method: &Method, path: &str, query: &HashMap<String, String>, headers: &HeaderMap, session_id: &str, mail: &MailRuntime, body: Body) -> Result<Response> {
    let rest = path.strip_prefix("/api/v1/mail").unwrap_or_default();
    let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let call = |operation: &'static str, value: Value| async move { mail.user(session_id, operation, value).await.map_err(|e| friendly(operation, e)) };
    let q = |k: &str| query.get(k).cloned().unwrap_or_default();
    match (method.clone(), parts.as_slice()) {
        (Method::GET, ["overview"]) => ok(call("overview", json!({})).await?),
        (Method::GET, ["messages"]) => {
            let mut p = json!({ "view": q("view") });
            for k in ["id", "cursor"] {
                if !q(k).is_empty() {
                    p[k] = json!(q(k));
                }
            }
            ok(call("messages", p).await?)
        }
        (Method::GET, ["messages", id]) => {
            let detail = call("message", json!({ "id": id })).await?;
            let data = mail.read_blob_app(detail["blobId"].as_str().unwrap_or_default()).await?;
            let mut value = detail;
            value["body"] = render(runtime, id, &data, q("images") == "1");
            ok(value)
        }
        (Method::GET, ["messages", id, "body"]) => {
            let detail = call("message", json!({ "id": id })).await?;
            let data = mail.read_blob_app(detail["blobId"].as_str().unwrap_or_default()).await?;
            ok(render(runtime, id, &data, q("images") == "1"))
        }
        (Method::GET, ["messages", id, "raw"]) => {
            let detail = call("message", json!({ "id": id })).await?;
            let data = mail.read_blob_app(detail["blobId"].as_str().unwrap_or_default()).await?;
            let name = detail["fileName"].as_str().filter(|n| !n.is_empty()).unwrap_or("message.eml").to_owned();
            download("message/rfc822", &name, data, false)
        }
        (Method::GET, ["messages", id, "attachments", index]) => {
            let index: usize = index.parse().map_err(|_| AppError::invalid())?;
            let detail = call("message", json!({ "id": id })).await?;
            let data = mail.read_blob_app(detail["blobId"].as_str().unwrap_or_default()).await?;
            let (name, kind, bytes) = message::part(&data, Some(index), None).ok_or_else(AppError::not_found)?;
            download(&kind, &name, bytes, q("inline") == "1")
        }
        (Method::GET, ["messages", id, "cid", cid]) => {
            let detail = call("message", json!({ "id": id })).await?;
            let data = mail.read_blob_app(detail["blobId"].as_str().unwrap_or_default()).await?;
            let cid = percent_decode(cid);
            let (name, kind, bytes) = message::part(&data, None, Some(&cid)).ok_or_else(AppError::not_found)?;
            download(&kind, &name, bytes, true)
        }
        (Method::GET, ["messages", id, "location"]) => ok(call("locate", json!({ "id": id })).await?),
        (Method::GET, ["image"]) => image(&q("url")).await,
        (Method::POST, ["messages", "flags"]) => {
            let p = input(body, headers, BODY_LIMIT, &["ids", "seen", "flagged"]).await?;
            let (seen, flagged) = (optional_bool(&p, "seen")?, optional_bool(&p, "flagged")?);
            if seen.is_none() && flagged.is_none() {
                return Err(AppError::invalid());
            }
            mail.set_flags(session_id, ids(&p)?, seen, flagged).await?;
            ok(json!({ "ok": true }))
        }
        (Method::POST, ["messages", "move"]) => {
            let p = input(body, headers, BODY_LIMIT, &["ids", "role", "mailboxId"]).await?;
            let role = p.get("role").and_then(Value::as_str).filter(|r| matches!(*r, "inbox" | "archive" | "trash" | "junk"));
            let mailbox = p.get("mailboxId").and_then(Value::as_str);
            if role.is_none() && mailbox.is_none() {
                return Err(AppError::invalid());
            }
            ok(mail.move_messages(session_id, ids(&p)?, role, mailbox).await?)
        }
        (Method::POST, ["send"]) | (Method::POST, ["drafts"]) => {
            let p = input(body, headers, SEND_LIMIT, &["accountId", "to", "cc", "bcc", "subject", "text", "inReplyTo", "attachments", "replaces", "format"]).await?;
            if p.get("format").is_some_and(|f| !matches!(f.as_str(), Some("plain" | "markdown"))) {
                return Err(AppError::invalid());
            }
            string(p.get("accountId"), 100)?;
            for k in ["to", "cc", "bcc"] {
                if p.get(k).is_some_and(|v| !v.is_array() || v.as_array().is_some_and(|a| a.len() > 100)) {
                    return Err(AppError::invalid());
                }
            }
            if p.get("subject").is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 998)) || p.get("text").is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 2 * 1024 * 1024)) {
                return Err(AppError::invalid());
            }
            let mut attachments = Vec::new();
            let mut total = 0;
            for a in p.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default() {
                let data = super::net::unb64(a["data"].as_str().unwrap_or_default()).ok_or_else(AppError::invalid)?;
                total += data.len();
                if total > ATTACHMENTS_LIMIT {
                    return Err(AppError::with("requestTooLarge", 413, "Attachments are limited to 25 MB in total."));
                }
                let kind = a["type"].as_str().filter(|t| t.contains('/') && t.len() < 200 && !t.contains(['\r', '\n'])).unwrap_or("application/octet-stream").to_owned();
                attachments.push(Attachment { name: a["name"].as_str().unwrap_or("attachment").chars().take(200).collect(), kind, data });
            }
            ok(mail.send(session_id, &Value::Object(p), attachments, parts[0] == "drafts").await?)
        }
        (Method::POST, ["accounts"]) => {
            let p = input(body, headers, BODY_LIMIT, &["providerId", "name", "address", "displayName", "username", "password", "token"]).await?;
            let provider_id = string(p.get("providerId"), 64)?;
            let provider = mail.provider(&provider_id).await.map_err(|_| AppError::with("notFound", 404, "That mail provider is not available."))?;
            if !provider.enabled {
                return Err(AppError::with("notFound", 404, "That mail provider is not available."));
            }
            let address = string(p.get("address"), 320)?.trim().to_owned();
            let username = p.get("username").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            let mut secret = credential(&p, provider.document["auth"].as_str().unwrap_or_default())?;
            mail.verify(&provider, &address, &username, &mut secret).await?;
            let account = call("createAccount", json!({
                "providerId": provider_id, "name": p.get("name").cloned().unwrap_or(json!(address)), "address": address,
                "displayName": p.get("displayName").cloned().unwrap_or(json!("")), "username": username, "credential": mail.seal_credential(&secret),
            })).await?;
            mail.poke();
            ok(account)
        }
        (Method::PATCH, ["accounts", id]) => {
            let p = input(body, headers, BODY_LIMIT, &["name", "displayName", "signature", "color", "ordering", "password", "token"]).await?;
            let mut update = json!({ "id": id });
            for k in ["name", "displayName", "signature", "color", "ordering"] {
                if let Some(v) = p.get(k) {
                    update[k] = v.clone();
                }
            }
            if p.contains_key("password") || p.contains_key("token") {
                let groups = mail.user(session_id, "prepare", json!({ "accountId": id })).await?;
                let work: super::runtime::Work = serde_json::from_value(groups["groups"][0].clone()).map_err(|_| AppError::not_found())?;
                let provider = work.provider.clone().ok_or_else(AppError::not_found)?;
                let mut secret = credential(&p, provider.document["auth"].as_str().unwrap_or_default())?;
                mail.verify(&provider, &work.address, &work.username, &mut secret).await?;
                update["credential"] = json!(mail.seal_credential(&secret));
            }
            let account = call("updateAccount", update).await?;
            mail.poke();
            ok(account)
        }
        (Method::DELETE, ["accounts", id]) => ok(call("removeAccount", json!({ "id": id, "keepFiles": q("keepFiles") != "0" })).await?),
        (Method::POST, ["sync"]) => {
            let p = input(body, headers, BODY_LIMIT, &["id"]).await?;
            let one = p.get("id").and_then(Value::as_str).map(str::to_owned);
            call("syncNow", Value::Object(p)).await?;
            // One account runs now and answers when its pass is done; all accounts run in the background.
            match one {
                Some(id) => mail.sync_now(&id).await?,
                None => mail.poke(),
            }
            ok(json!({ "ok": true }))
        }
        (Method::POST, ["oauth", "start"]) => {
            let p = input(body, headers, BODY_LIMIT, &["providerId", "name", "address", "displayName", "accountId"]).await?;
            // The session must be able to use Mail before anything leaves for the provider.
            call("overview", json!({})).await?;
            let address = string(p.get("address"), 320)?.trim().to_owned();
            if !address.contains('@') {
                return Err(AppError::with("invalidInput", 400, "Enter the email address of the account."));
            }
            let get = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            let name = if get("name").trim().is_empty() { address.clone() } else { get("name") };
            let url = mail.oauth_start(session_id, &string(p.get("providerId"), 64)?, &name, &address, &get("displayName"), &get("accountId")).await?;
            ok(json!({ "url": url }))
        }
        (Method::GET, ["admin", "providers"]) => ok(call("providers", json!({})).await?),
        (Method::PUT, ["admin", "providers", id]) => {
            let p = input(body, headers, BODY_LIMIT, &["name", "document", "secret", "enabled", "create"]).await?;
            let mut value = json!({ "id": id, "name": p.get("name").cloned().unwrap_or(Value::Null), "document": p.get("document").cloned().unwrap_or(Value::Null) });
            for k in ["enabled", "create"] {
                if let Some(v) = p.get(k) {
                    value[k] = v.clone();
                }
            }
            // A new secret is sealed here; leaving it out keeps the saved one, "" clears it.
            if let Some(secret) = p.get("secret").filter(|s| !s.is_null()) {
                let secret = string(Some(secret), 4096)?;
                value["secret"] = json!(if secret.is_empty() { String::new() } else { mail.seal_provider_secret(&secret) });
            }
            ok(call("saveProvider", value).await?)
        }
        (Method::DELETE, ["admin", "providers", id]) => ok(call("deleteProvider", json!({ "id": id })).await?),
        _ => Err(AppError::not_found()),
    }
}

fn render(runtime: &Runtime, id: &str, data: &[u8], images: bool) -> Value {
    let origin = &runtime.config.origin;
    message::render(data, &format!("{origin}/api/v1/mail/messages/{id}"), &format!("{origin}/api/v1/mail/image?url="), images)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&value[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Remote images in messages, fetched by the server so senders do not see the reader's address.
async fn image(url: &str) -> Result<Response> {
    if url.is_empty() || url.len() > 4096 {
        return Err(AppError::invalid());
    }
    let response = crate::net::safe_fetch(url, &crate::net::FetchOptions {
        accept: "image/avif,image/webp,image/png,image/jpeg,image/gif;q=0.9",
        etag: None,
        last_modified: None,
        max_bytes: 8 * 1024 * 1024,
        timeout: Duration::from_secs(15),
        allow_private: false,
    })
    .await?;
    let media_type = response.content_type.split(';').next().unwrap_or("").trim().to_owned();
    if response.status != 200 || !matches!(media_type.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif") {
        return Err(AppError::not_found());
    }
    let mut res = (StatusCode::OK, response.body).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&media_type).map_err(|_| AppError::internal())?);
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    Ok(res)
}
