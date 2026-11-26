//! Native-session News HTTP API (`next/runtime/news/http.ts`).
//! The database re-checks the account and app availability.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response, ok};
use crate::news::runtime::NewsRuntime;

pub const READ_OPERATIONS: &[&str] = &[
    "access",
    "overview",
    "folders",
    "feeds",
    "items",
    "itemsUpdated",
    "item",
    "exportData",
    "feedInfo",
];

const BODY_LIMIT: usize = 3 * 1024 * 1024;

fn friendly_message(operation: &str, code: &str) -> Option<&'static str> {
    match (operation, code) {
        ("subscribe", "alreadyExists") => Some("You are already subscribed to that feed."),
        ("subscribe", "quotaExceeded") => Some("You have reached the limit of 500 feeds."),
        ("importOpml", "quotaExceeded") => {
            Some("Importing would exceed the limit of 500 feeds or 100 folders.")
        }
        ("createFolder", "alreadyExists") => Some("A folder with that name already exists."),
        ("createFolder", "quotaExceeded") => Some("You have reached the limit of 100 folders."),
        ("renameFolder", "alreadyExists") => Some("A folder with that name already exists."),
        _ => None,
    }
}

fn map_error(operation: &str, error: AppError) -> AppError {
    if let Some(msg) = friendly_message(operation, &error.code) {
        AppError::with(&error.code, error.status, msg)
    } else {
        error
    }
}

async fn call(runtime: &Runtime, session_id: &str, operation: &str, value: Value) -> Result<Value> {
    runtime
        .db
        .operation(
            "service_news",
            operation,
            READ_OPERATIONS,
            json!([session_id, operation, value.to_string()]),
        )
        .await
        .map_err(|e| map_error(operation, e))
}

async fn input(body: Body, headers: &HeaderMap, allowed: &[&str]) -> Result<Map<String, Value>> {
    let map = body_object(body, BODY_LIMIT, headers).await?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    Ok(map)
}

fn query_integer(value: Option<&String>, fallback: i64) -> Result<i64> {
    static INT_RE: OnceLock<Regex> = OnceLock::new();
    let re = INT_RE.get_or_init(|| Regex::new(r"^-?\d{1,10}$").expect("int regex"));
    match value.map(String::as_str) {
        None | Some("") => Ok(fallback),
        Some(s) if re.is_match(s) => s.parse::<i64>().map_err(|_| AppError::invalid()),
        _ => Err(AppError::invalid()),
    }
}

fn scope(query: &HashMap<String, String>) -> Result<Map<String, Value>> {
    let kind = query.get("type").map(String::as_str).unwrap_or("unread");
    if !matches!(kind, "all" | "unread" | "starred" | "feed" | "folder") {
        return Err(AppError::invalid());
    }
    let mut map = Map::new();
    map.insert("type".into(), json!(kind));
    if kind == "feed" || kind == "folder" {
        map.insert("id".into(), json!(query_integer(query.get("id"), 0)?));
    }
    Ok(map)
}

fn is_http_url_ci(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn pretty_json_one_space(value: &Value) -> Result<String> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value
        .serialize(&mut ser)
        .map_err(|_| AppError::internal())?;
    String::from_utf8(buf).map_err(|_| AppError::internal())
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_http(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    username: &str,
    news: &NewsRuntime,
    body: Body,
) -> Result<Response> {
    static ITEM_RE: OnceLock<Regex> = OnceLock::new();
    static FEED_RE: OnceLock<Regex> = OnceLock::new();
    static FOLDER_RE: OnceLock<Regex> = OnceLock::new();
    let item_re = ITEM_RE.get_or_init(|| {
        Regex::new(r"^/items/(\d{1,10})(/(position|fulltext))?$").expect("item regex")
    });
    let feed_re = FEED_RE.get_or_init(|| Regex::new(r"^/feeds/(\d{1,10})$").expect("feed regex"));
    let folder_re =
        FOLDER_RE.get_or_init(|| Regex::new(r"^/folders/(\d{1,10})$").expect("folder regex"));

    let sub = path.strip_prefix("/api/v1/news").unwrap_or("");

    if sub == "/overview" && *method == Method::GET {
        return ok(call(runtime, session_id, "overview", json!({})).await?);
    }
    if sub == "/items" && *method == Method::GET {
        let mut params = scope(query)?;
        params.insert(
            "offset".into(),
            json!(query_integer(query.get("offset"), 0)?),
        );
        params.insert(
            "batch".into(),
            json!(query_integer(query.get("batch"), 50)?),
        );
        params.insert(
            "getRead".into(),
            json!(query.get("getRead").map(String::as_str) != Some("0")),
        );
        params.insert(
            "oldestFirst".into(),
            json!(query.get("oldestFirst").map(String::as_str) == Some("1")),
        );
        return ok(call(runtime, session_id, "items", Value::Object(params)).await?);
    }
    if sub == "/items/state" && *method == Method::POST {
        let value = input(body, headers, &["ids", "unread", "starred"]).await?;
        return ok(call(runtime, session_id, "setItems", Value::Object(value)).await?);
    }
    if let Some(caps) = item_re.captures(sub) {
        let id: u64 = caps[1].parse().map_err(|_| AppError::invalid())?;
        if caps.get(2).is_none() && *method == Method::GET {
            return ok(call(runtime, session_id, "item", json!({ "id": id })).await?);
        }
        if caps.get(3).map(|m| m.as_str()) == Some("position") && *method == Method::POST {
            let value = input(body, headers, &["position"]).await?;
            let mut params = Map::new();
            params.insert("id".into(), json!(id));
            if let Some(pos) = value.get("position") {
                params.insert("position".into(), pos.clone());
            }
            return ok(call(runtime, session_id, "setPosition", Value::Object(params)).await?);
        }
        if caps.get(3).map(|m| m.as_str()) == Some("fulltext") && *method == Method::POST {
            let item = call(runtime, session_id, "item", json!({ "id": id })).await?;
            let target_url = item["url"].as_str().unwrap_or("");
            let extracted = news.extract_url(target_url).await?;
            return ok(json!({ "id": id, "body": extracted }));
        }
    }
    if sub == "/read" && *method == Method::POST {
        let value = input(body, headers, &["type", "id", "newestItemId"]).await?;
        return ok(call(runtime, session_id, "markRead", Value::Object(value)).await?);
    }
    if sub == "/discover" && *method == Method::POST {
        let value = input(body, headers, &["url"]).await?;
        let Some(url) = value
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| !u.trim().is_empty() && u.encode_utf16().count() <= 2048)
        else {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Enter a site or feed address.",
            ));
        };
        call(runtime, session_id, "access", json!({})).await?;
        return ok(json!({ "items": news.discover(url).await? }));
    }
    if sub == "/feeds" && *method == Method::POST {
        let value = input(body, headers, &["url", "folderId", "title", "fullText"]).await?;
        let Some(url) = value
            .get("url")
            .and_then(Value::as_str)
            .filter(|u| is_http_url_ci(u))
        else {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Choose a feed address.",
            ));
        };
        let title = match value.get("title") {
            None => None,
            Some(Value::String(s)) => Some(s.as_str()),
            _ => return Err(AppError::invalid()),
        };
        let full_text = match value.get("fullText") {
            None => false,
            Some(Value::Bool(b)) => *b,
            _ => return Err(AppError::invalid()),
        };
        let folder_id = match value.get("folderId") {
            None | Some(Value::Null) => 0i64,
            Some(Value::Number(n)) => match n.as_f64() {
                Some(f) if f.is_finite() && f.fract() == 0.0 => f as i64,
                _ => return Err(AppError::invalid()),
            },
            _ => return Err(AppError::invalid()),
        };
        call(runtime, session_id, "access", json!({})).await?;
        let created = news
            .subscribe(session_id, url, folder_id, title, full_text)
            .await
            .map_err(|e| map_error("subscribe", e))?;
        return Ok(json_response(&created, StatusCode::CREATED));
    }
    if let Some(caps) = feed_re.captures(sub) {
        let id: u64 = caps[1].parse().map_err(|_| AppError::invalid())?;
        if *method == Method::GET {
            return ok(call(runtime, session_id, "feedInfo", json!({ "id": id })).await?);
        }
        if *method == Method::PATCH {
            let mut value = input(
                body,
                headers,
                &[
                    "title",
                    "folderId",
                    "fullText",
                    "pinned",
                    "paused",
                    "ordering",
                    "updateNow",
                ],
            )
            .await?;
            let update_now = value.get("updateNow") == Some(&Value::Bool(true));
            let unpaused = value.get("paused") == Some(&Value::Bool(false));
            value.insert("id".into(), json!(id));
            let result = call(runtime, session_id, "updateFeed", Value::Object(value)).await?;
            if update_now || unpaused {
                let _ = tokio::time::timeout(Duration::from_secs(15), news.tick()).await;
            }
            if update_now {
                return ok(call(runtime, session_id, "feedInfo", json!({ "id": id })).await?);
            }
            return ok(result);
        }
        if *method == Method::DELETE {
            return ok(call(runtime, session_id, "deleteFeed", json!({ "id": id })).await?);
        }
    }
    if sub == "/folders" && *method == Method::POST {
        let value = input(body, headers, &["name"]).await?;
        let created = call(runtime, session_id, "createFolder", Value::Object(value)).await?;
        return Ok(json_response(&created, StatusCode::CREATED));
    }
    if let Some(caps) = folder_re.captures(sub) {
        let id: u64 = caps[1].parse().map_err(|_| AppError::invalid())?;
        if *method == Method::PATCH {
            let value = input(body, headers, &["name", "opened"]).await?;
            let mut result: Option<Value> = None;
            if let Some(name) = value.get("name") {
                result = Some(
                    call(
                        runtime,
                        session_id,
                        "renameFolder",
                        json!({ "id": id, "name": name }),
                    )
                    .await?,
                );
            }
            if let Some(opened) = value.get("opened") {
                result = Some(
                    call(
                        runtime,
                        session_id,
                        "openFolder",
                        json!({ "id": id, "opened": opened }),
                    )
                    .await?,
                );
            }
            return ok(result.unwrap_or_else(|| json!({})));
        }
        if *method == Method::DELETE {
            return ok(call(runtime, session_id, "deleteFolder", json!({ "id": id })).await?);
        }
    }
    if sub == "/refresh" && *method == Method::POST {
        call(runtime, session_id, "access", json!({})).await?;
        let _ = tokio::time::timeout(Duration::from_secs(15), news.tick()).await;
        return ok(call(runtime, session_id, "overview", json!({})).await?);
    }
    if sub == "/opml" && *method == Method::POST {
        let value = input(body, headers, &["text"]).await?;
        let Some(text) = value
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| t.encode_utf16().count() <= 2 * 1024 * 1024)
        else {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Choose an OPML file up to 2 MiB.",
            ));
        };
        let imported = news
            .import_opml(session_id, text)
            .await
            .map_err(|e| map_error("importOpml", e))?;
        return ok(imported);
    }
    if sub == "/opml" && *method == Method::GET {
        let text = news.export_opml(session_id, username).await?;
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/x-opml; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"subscriptions.opml\"",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
            text,
        )
            .into_response());
    }
    if sub == "/starred" && *method == Method::GET {
        let data = call(
            runtime,
            session_id,
            "exportData",
            json!({ "starred": true }),
        )
        .await?;
        let text = pretty_json_one_space(&data)?;
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"starred-articles.json\"",
                ),
                (header::CACHE_CONTROL, "no-store"),
            ],
            text,
        )
            .into_response());
    }
    if sub == "/image" && *method == Method::GET {
        let target = query.get("url").map(String::as_str).unwrap_or("");
        if !(target.starts_with("http://") || target.starts_with("https://"))
            || target.encode_utf16().count() > 2048
        {
            return Err(AppError::invalid());
        }
        call(runtime, session_id, "access", json!({})).await?;
        return news.image(target).await;
    }

    Err(AppError::not_found())
}
