//! Nextcloud News API v1.2 and v1.3 for existing mobile and desktop clients
//! (`next/runtime/news/nextcloud.ts`). Clients use `<origin>/dav/news` as the server address,
//! the Dar username and an app password with News access.

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::auth::{digest_token, is_token};
use crate::error::{AppError, Result};
use crate::http::{Runtime, ok};
use crate::news::http::READ_OPERATIONS;
use crate::news::runtime::NewsRuntime;

pub const NEXTCLOUD_BASE: &str = "/dav/news";

const TYPE_NAMES: [&str; 4] = ["feed", "folder", "starred", "all"];

fn str_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

fn non_empty_or_null(value: &Value) -> Value {
    match value.as_str() {
        Some(s) if !s.is_empty() => Value::String(s.to_owned()),
        _ => Value::Null,
    }
}

fn num(value: Option<&Value>, fallback: i64) -> Result<i64> {
    match value {
        None | Some(Value::Null) => Ok(fallback),
        Some(Value::String(s)) if s.is_empty() => Ok(fallback),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if f.is_finite() => Ok(f.trunc() as i64),
            _ => Err(AppError::new("invalidInput", 422)),
        },
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(0);
            }
            match trimmed.parse::<f64>() {
                Ok(f) if f.is_finite() => Ok(f.trunc() as i64),
                _ => Err(AppError::new("invalidInput", 422)),
            }
        }
        Some(Value::Bool(b)) => Ok(if *b { 1 } else { 0 }),
        _ => Err(AppError::new("invalidInput", 422)),
    }
}

fn bool_param(value: Option<&Value>, fallback: bool) -> bool {
    match value {
        None | Some(Value::Null) => fallback,
        Some(Value::String(s)) if s.is_empty() => fallback,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true" || s == "1",
        Some(Value::Number(n)) => n.as_f64() == Some(1.0),
        _ => false,
    }
}

fn parse_ids(value: Option<&Value>) -> Result<Vec<i64>> {
    let Some(arr) = value.and_then(Value::as_array) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for v in arr {
        let n = num(Some(v), 0)?;
        if n > 0 {
            out.push(n);
            if out.len() >= 1000 {
                break;
            }
        }
    }
    Ok(out)
}

fn nc_folder(row: &Value) -> Value {
    json!({
        "id": row["id"],
        "name": row["name"],
        "opened": row["opened"],
        "feeds": [],
    })
}

fn nc_feed(row: &Value, v13: bool) -> Value {
    let folder_id = match row["folderId"].as_i64().unwrap_or(0) {
        0 => {
            if v13 {
                Value::Null
            } else {
                json!(0)
            }
        }
        n => json!(n),
    };
    let added = (row["added"].as_f64().unwrap_or(0.0) / 1000.0).floor() as i64;
    json!({
        "id": row["id"],
        "url": row["url"],
        "title": row["title"],
        "faviconLink": Value::Null,
        "added": added,
        "folderId": folder_id,
        "unreadCount": row["unreadCount"],
        "ordering": row["ordering"],
        "link": non_empty_or_null(&row["link"]),
        "pinned": row["pinned"],
        "updateErrorCount": row["updateErrorCount"],
        "lastUpdateError": non_empty_or_null(&row["lastUpdateError"]),
    })
}

fn nc_item(row: &Value) -> Value {
    let pub_date = (row["pubDate"].as_f64().unwrap_or(0.0) / 1000.0).floor() as i64;
    let updated_date = (row["updatedDate"].as_f64().unwrap_or(0.0) / 1000.0).floor() as i64;
    let last_modified = (row["lastModified"].as_f64().unwrap_or(0.0) / 1e6).floor() as i64;
    json!({
        "id": row["id"],
        "guid": row["guid"],
        "guidHash": row["guidHash"],
        "url": non_empty_or_null(&row["url"]),
        "title": row["title"],
        "author": row["author"],
        "pubDate": pub_date,
        "updatedDate": updated_date,
        "body": row["body"],
        "enclosureMime": non_empty_or_null(&row["enclosureMime"]),
        "enclosureLink": non_empty_or_null(&row["enclosureLink"]),
        "mediaThumbnail": non_empty_or_null(&row["mediaThumbnail"]),
        "mediaDescription": non_empty_or_null(&row["mediaDescription"]),
        "feedId": row["feedId"],
        "unread": row["unread"],
        "starred": row["starred"],
        "rtl": row["rtl"],
        "lastModified": last_modified,
        "fingerprint": row["fingerprint"],
        "contentHash": row["contentHash"],
    })
}

async fn call(runtime: &Runtime, session_id: &str, operation: &str, value: Value) -> Result<Value> {
    runtime.db.operation("service_news", operation, READ_OPERATIONS, json!([session_id, operation, value.to_string()])).await
}

pub async fn handle_nextcloud(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    news: &NewsRuntime,
    body: Body,
) -> Result<Response> {
    static OCS_USER_RE: OnceLock<Regex> = OnceLock::new();
    static OCS_CAP_RE: OnceLock<Regex> = OnceLock::new();
    static API_RE: OnceLock<Regex> = OnceLock::new();
    static FOLDER_RE: OnceLock<Regex> = OnceLock::new();
    static FEED_RE: OnceLock<Regex> = OnceLock::new();
    static MULTI_RE: OnceLock<Regex> = OnceLock::new();
    static ITEM_STATE_RE: OnceLock<Regex> = OnceLock::new();
    static GUID_STAR_RE: OnceLock<Regex> = OnceLock::new();

    let ocs_user_re = OCS_USER_RE.get_or_init(|| Regex::new(r"^/ocs/v[12]\.php/cloud/user$").expect("ocs user regex"));
    let ocs_cap_re = OCS_CAP_RE.get_or_init(|| Regex::new(r"^/ocs/v[12]\.php/cloud/capabilities$").expect("ocs cap regex"));
    let api_re = API_RE.get_or_init(|| Regex::new(r"^(?:/index\.php)?/apps/news/api/v1-([23])(/.*)?$").expect("api regex"));
    let folder_re = FOLDER_RE.get_or_init(|| Regex::new(r"^/folders/(\d+)(/read)?$").expect("folder regex"));
    let feed_re = FEED_RE.get_or_init(|| Regex::new(r"^/feeds/(\d+)(?:/(move|rename|read))?$").expect("feed regex"));
    let multi_re = MULTI_RE.get_or_init(|| Regex::new(r"^/items/(read|unread|star|unstar)/multiple$").expect("multi regex"));
    let item_state_re = ITEM_STATE_RE.get_or_init(|| Regex::new(r"^/items/(\d+)/(read|unread|star|unstar)$").expect("item state regex"));
    let guid_star_re = GUID_STAR_RE.get_or_init(|| Regex::new(r"^/items/(\d+)/([a-f0-9]{32})/(star|unstar)$").expect("guid star regex"));

    let rest = path.strip_prefix(NEXTCLOUD_BASE).unwrap_or("");
    let trimmed = rest.trim_end_matches('/');
    let sub = if trimmed.is_empty() { "/" } else { trimmed };

    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(runtime.config.origin.as_str())
    {
        return Err(AppError::new("invalidOrigin", 403));
    }
    if *method == Method::OPTIONS {
        return Ok((StatusCode::NO_CONTENT, [(header::ALLOW, "GET, POST, PUT, DELETE, OPTIONS")]).into_response());
    }
    if sub == "/status.php" {
        return ok(json!({
            "installed": true,
            "maintenance": false,
            "needsDbUpgrade": false,
            "version": "28.0.0.0",
            "versionstring": "28.0.0",
            "edition": "",
            "productname": "Nextcloud",
            "extendedSupport": false,
        }));
    }

    // HTTP Basic with an app password; the scope decides read or write access.
    let authorization = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
    let Some(encoded) = authorization.strip_prefix("Basic ").filter(|_| authorization.len() <= 2048) else {
        return Err(AppError::new("unauthenticated", 401));
    };
    let decoded_bytes = B64.decode(encoded).map_err(|_| AppError::new("unauthenticated", 401))?;
    let decoded = String::from_utf8(decoded_bytes).map_err(|_| AppError::new("unauthenticated", 401))?;
    let Some((username, secret)) = decoded.split_once(':') else {
        return Err(AppError::new("unauthenticated", 401));
    };
    if username.is_empty() || !is_token(secret) {
        return Err(AppError::new("unauthenticated", 401));
    }

    let write = !matches!(*method, Method::GET | Method::HEAD);
    let session = runtime.db.read("service_pim_authenticate", json!([username, digest_token(secret), "news", write])).await?;
    let session_id = session["sessionId"].as_str().unwrap_or_default();
    let session_username = session["username"].as_str().unwrap_or_default();

    if ocs_user_re.is_match(sub) {
        let v2 = sub.contains("/v2.");
        return ok(json!({
            "ocs": {
                "meta": { "status": "ok", "statuscode": if v2 { 200 } else { 100 }, "message": "OK" },
                "data": { "id": session_username, "display-name": session_username, "displayname": session_username, "email": Value::Null, "enabled": true },
            }
        }));
    }
    if ocs_cap_re.is_match(sub) {
        let v2 = sub.contains("/v2.");
        return ok(json!({
            "ocs": {
                "meta": { "status": "ok", "statuscode": if v2 { 200 } else { 100 }, "message": "OK" },
                "data": {
                    "version": { "major": 28, "minor": 0, "micro": 0, "string": "28.0.0", "edition": "", "extendedSupport": false },
                    "capabilities": { "news": { "version": "24.0.0" } },
                },
            }
        }));
    }

    let Some(api_caps) = api_re.captures(sub) else {
        return Err(AppError::not_found());
    };
    let v13 = &api_caps[1] == "3";
    let route = api_caps.get(2).map_or("/", |m| m.as_str());

    let mut input: Map<String, Value> = Map::new();
    if write {
        let bytes = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| AppError::new("requestTooLarge", 413))?;
        if !bytes.is_empty() {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| AppError::new("invalidJson", 400))?;
            if let Value::Object(map) = value {
                input = map;
            }
        }
    }

    let param = |name: &str| -> Option<Value> {
        input.get(name).filter(|v| !v.is_null()).cloned().or_else(|| query.get(name).map(|s| Value::String(s.clone())))
    };
    let scope = || -> Result<Map<String, Value>> {
        let idx = num(param("type").as_ref(), 3)?;
        let kind = if (0..4).contains(&idx) { TYPE_NAMES[idx as usize] } else { "all" };
        let id = num(param("id").as_ref(), 0)?;
        let mut map = Map::new();
        map.insert("type".into(), json!(kind));
        if kind == "feed" || kind == "folder" {
            map.insert("id".into(), json!(id));
        }
        Ok(map)
    };
    let empty_ok = || ok(json!({}));

    if route == "/version" && *method == Method::GET {
        return ok(json!({ "version": "24.0.0" }));
    }
    if route == "/status" && *method == Method::GET {
        return ok(json!({ "version": "24.0.0", "warnings": { "improperlyConfiguredCron": false, "incorrectDbCharset": false } }));
    }
    if route == "/user" && *method == Method::GET {
        return ok(json!({ "userId": session_username, "displayName": session_username, "lastLoginTimestamp": 0, "avatar": Value::Null }));
    }
    if route == "/folders" {
        if *method == Method::GET {
            let overview = call(runtime, session_id, "overview", json!({})).await?;
            let folders: Vec<Value> = overview["folders"].as_array().map(|a| a.iter().map(nc_folder).collect()).unwrap_or_default();
            return ok(json!({ "folders": folders }));
        }
        if *method == Method::POST {
            let created = call(runtime, session_id, "createFolder", json!({ "name": str_of(input.get("name")) })).await.map_err(|e| match e.code.as_str() {
                "alreadyExists" => AppError::with("alreadyExists", 409, "Folder exists"),
                "invalidInput" => AppError::with("invalidInput", 422, "Invalid folder name"),
                _ => e,
            })?;
            return ok(json!({ "folders": [nc_folder(&created)] }));
        }
    }
    if let Some(caps) = folder_re.captures(route) {
        let id: u64 = caps[1].parse().map_err(|_| AppError::new("invalidInput", 422))?;
        if caps.get(2).is_some() && (*method == Method::PUT || *method == Method::POST) {
            call(runtime, session_id, "markRead", json!({ "type": "folder", "id": id, "newestItemId": num(param("newestItemId").as_ref(), 0)? })).await?;
            return empty_ok();
        }
        if caps.get(2).is_none() && *method == Method::PUT {
            call(runtime, session_id, "renameFolder", json!({ "id": id, "name": str_of(input.get("name")) })).await.map_err(|e| {
                if e.code == "alreadyExists" { AppError::with("alreadyExists", 409, "Folder exists") } else { e }
            })?;
            return empty_ok();
        }
        if caps.get(2).is_none() && *method == Method::DELETE {
            call(runtime, session_id, "deleteFolder", json!({ "id": id })).await?;
            return empty_ok();
        }
    }
    if route == "/feeds" {
        if *method == Method::GET {
            let value = call(runtime, session_id, "overview", json!({})).await?;
            let feeds: Vec<Value> = value["feeds"].as_array().map(|a| a.iter().map(|r| nc_feed(r, v13)).collect()).unwrap_or_default();
            let mut body = Map::new();
            body.insert("feeds".into(), Value::Array(feeds));
            body.insert("starredCount".into(), value["starredCount"].clone());
            if value["newestItemId"].as_u64().unwrap_or(0) != 0 {
                body.insert("newestItemId".into(), value["newestItemId"].clone());
            }
            return ok(Value::Object(body));
        }
        if *method == Method::POST {
            let target = str_of(input.get("url"));
            let lower = target.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return Err(AppError::with("invalidInput", 422, "Invalid feed address"));
            }
            let folder_id = num(input.get("folderId"), 0)?;
            let created = news.subscribe(session_id, &target, folder_id, None, false).await.map_err(|e| {
                if e.code == "alreadyExists" {
                    AppError::with("alreadyExists", 409, "Feed exists")
                } else if !matches!(e.code.as_str(), "forbidden" | "unauthenticated" | "notFound") {
                    AppError::with(&e.code, 422, &e.message)
                } else {
                    e
                }
            })?;
            let overview = call(runtime, session_id, "overview", json!({})).await?;
            return ok(json!({ "feeds": [nc_feed(&created, v13)], "newestItemId": overview["newestItemId"] }));
        }
    }
    if let Some(caps) = feed_re.captures(route) {
        let id: u64 = caps[1].parse().map_err(|_| AppError::new("invalidInput", 422))?;
        let action = caps.get(2).map(|m| m.as_str());
        if action.is_none() && *method == Method::DELETE {
            call(runtime, session_id, "deleteFeed", json!({ "id": id })).await?;
            return empty_ok();
        }
        if action == Some("move") && (*method == Method::PUT || *method == Method::POST) {
            call(runtime, session_id, "updateFeed", json!({ "id": id, "folderId": num(input.get("folderId"), 0)? })).await?;
            return empty_ok();
        }
        if action == Some("rename") && (*method == Method::PUT || *method == Method::POST) {
            call(runtime, session_id, "updateFeed", json!({ "id": id, "title": str_of(input.get("feedTitle")) })).await?;
            return empty_ok();
        }
        if action == Some("read") && (*method == Method::PUT || *method == Method::POST) {
            call(runtime, session_id, "markRead", json!({ "type": "feed", "id": id, "newestItemId": num(param("newestItemId").as_ref(), 0)? })).await?;
            return empty_ok();
        }
    }
    if route == "/items" && *method == Method::GET {
        let batch = num(param("batchSize").as_ref(), -1)?;
        let batch_arg = if batch <= 0 { -1 } else { batch.min(2000) };
        let mut params = scope()?;
        params.insert("offset".into(), json!(num(param("offset").as_ref(), 0)?));
        params.insert("batch".into(), json!(batch_arg));
        params.insert("getRead".into(), json!(bool_param(param("getRead").as_ref(), true)));
        params.insert("oldestFirst".into(), json!(bool_param(param("oldestFirst").as_ref(), false)));
        params.insert("bodies".into(), json!(true));
        let result = call(runtime, session_id, "items", Value::Object(params)).await?;
        let items: Vec<Value> = result["items"].as_array().map(|a| a.iter().map(nc_item).collect()).unwrap_or_default();
        return ok(json!({ "items": items }));
    }
    if route == "/items/updated" && *method == Method::GET {
        let since = num(param("lastModified").as_ref(), 0)?;
        let last_modified = if since < 1_000_000_000_000 { since.saturating_mul(1_000_000) } else { since };
        let mut params = scope()?;
        params.insert("lastModified".into(), json!(last_modified));
        let result = call(runtime, session_id, "itemsUpdated", Value::Object(params)).await?;
        let items: Vec<Value> = result["items"].as_array().map(|a| a.iter().map(nc_item).collect()).unwrap_or_default();
        return ok(json!({ "items": items }));
    }
    if route == "/items/read" && (*method == Method::PUT || *method == Method::POST) {
        call(runtime, session_id, "markRead", json!({ "type": "all", "newestItemId": num(param("newestItemId").as_ref(), 0)? })).await?;
        return empty_ok();
    }
    if let Some(caps) = multi_re.captures(route)
        && (*method == Method::PUT || *method == Method::POST)
    {
        let action = &caps[1];
        let list = input.get("itemIds").filter(|v| !v.is_null()).or_else(|| input.get("items"));
        if action == "star" || action == "unstar" {
            let starred = action == "star";
            if let Some(arr) = list.and_then(Value::as_array)
                && arr.iter().any(Value::is_object)
            {
                let mut items = Vec::new();
                for v in arr.iter().take(1000) {
                    let feed_id = num(v.get("feedId"), 0)?;
                    let guid_hash = str_of(v.get("guidHash"));
                    items.push(json!({ "feedId": feed_id, "guidHash": guid_hash }));
                }
                call(runtime, session_id, "starByGuid", json!({ "items": items, "starred": starred })).await?;
            } else {
                call(runtime, session_id, "setItems", json!({ "ids": parse_ids(list)?, "starred": starred })).await?;
            }
        } else {
            call(runtime, session_id, "setItems", json!({ "ids": parse_ids(list)?, "unread": action == "unread" })).await?;
        }
        return empty_ok();
    }
    if let Some(caps) = item_state_re.captures(route)
        && (*method == Method::PUT || *method == Method::POST)
    {
        let id: u64 = caps[1].parse().map_err(|_| AppError::new("invalidInput", 422))?;
        let action = &caps[2];
        let payload = if action == "read" || action == "unread" {
            json!({ "ids": [id], "unread": action == "unread" })
        } else {
            json!({ "ids": [id], "starred": action == "star" })
        };
        let result = call(runtime, session_id, "setItems", payload).await?;
        if result["items"].as_array().is_none_or(Vec::is_empty) {
            return Err(AppError::with("notFound", 404, "Item not found"));
        }
        return empty_ok();
    }
    if let Some(caps) = guid_star_re.captures(route)
        && (*method == Method::PUT || *method == Method::POST)
    {
        let feed_id: u64 = caps[1].parse().map_err(|_| AppError::new("invalidInput", 422))?;
        let guid_hash = &caps[2];
        let starred = &caps[3] == "star";
        let result = call(runtime, session_id, "starByGuid", json!({ "items": [{ "feedId": feed_id, "guidHash": guid_hash }], "starred": starred })).await?;
        if result["changed"].as_u64().unwrap_or(0) == 0 {
            return Err(AppError::with("notFound", 404, "Item not found"));
        }
        return empty_ok();
    }

    Err(AppError::with("notFound", 404, "Not found"))
}
