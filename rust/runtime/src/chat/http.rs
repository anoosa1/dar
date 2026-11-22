//! Native-session Chat HTTP API (`next/runtime/chat/http.ts`); the database re-checks
//! the account and that Chat is enabled.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response, ok};

use super::{
    CHAT_LIMIT_ACCOUNTS, CHAT_LIMIT_NAME, CHAT_LIMIT_TEXT, ChatRuntime, is_account_id,
};

const BODY_LIMIT: usize = 512 * 1024;

fn friendly_message(code: &str) -> Option<String> {
    match code {
        "quotaExceeded" => Some(format!("You can link up to {CHAT_LIMIT_ACCOUNTS} accounts.")),
        "conflict" => Some("This account is not linked. Link it again to send messages.".into()),
        "notFound" => Some("This chat or account no longer exists.".into()),
        _ => None,
    }
}

async fn input(body: Body, headers: &HeaderMap, allowed: &[&str]) -> Result<Map<String, Value>> {
    let value = body_object(body, BODY_LIMIT, headers).await?;
    if value.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::invalid());
    }
    Ok(value)
}

fn str_req(value: Option<&Value>, max: usize) -> Result<String> {
    match value {
        Some(Value::String(s)) if s.encode_utf16().count() <= max => Ok(s.clone()),
        _ => Err(AppError::invalid()),
    }
}

fn str_opt(value: Option<&Value>, max: usize) -> Result<Option<String>> {
    match value {
        None => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) if s.encode_utf16().count() <= max => Ok(Some(s.clone())),
        _ => Err(AppError::invalid()),
    }
}

fn str_default(value: Option<&Value>, max: usize) -> Result<String> {
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) if s.encode_utf16().count() <= max => Ok(s.clone()),
        _ => Err(AppError::invalid()),
    }
}

fn str_query(value: Option<&String>, max: usize) -> Result<String> {
    let s = value.map(String::as_str).unwrap_or("");
    if s.encode_utf16().count() > max {
        return Err(AppError::invalid());
    }
    Ok(s.to_owned())
}

fn valid_link_phone(phone: &str) -> bool {
    let rest = phone.strip_prefix('+').unwrap_or(phone);
    (7..=30).contains(&rest.len())
        && rest
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b' ' | b'(' | b')' | b'-'))
}

pub async fn handle_http(
    _runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    chat: &ChatRuntime,
    body: Body,
) -> Result<Response> {
    let sub = path.strip_prefix("/api/v1/chat").unwrap_or(path);
    match dispatch(method, sub, query, headers, session_id, chat, body).await {
        Ok(Some(response)) => Ok(response),
        Ok(None) => Err(AppError::not_found()),
        Err(error) => {
            if error.message == error.code
                && let Some(msg) = friendly_message(&error.code)
            {
                return Err(AppError::with(&error.code, error.status, &msg));
            }
            Err(error)
        }
    }
}

async fn dispatch(
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    chat: &ChatRuntime,
    body: Body,
) -> Result<Option<Response>> {
    if path == "/overview" && *method == Method::GET {
        return Ok(Some(ok(chat.overview(session_id).await?)?));
    }
    if path == "/threads" && *method == Method::GET {
        let mut params = Map::new();
        params.insert(
            "accountId".to_owned(),
            json!(query.get("accountId").map(String::as_str).unwrap_or("")),
        );
        params.insert(
            "view".to_owned(),
            json!(query.get("view").map(String::as_str).unwrap_or("chats")),
        );
        if let Some(cursor) = query.get("cursor").filter(|s| !s.is_empty()) {
            params.insert("cursor".to_owned(), json!(cursor));
        }
        return Ok(Some(ok(
            chat.call(session_id, "threads", Value::Object(params))
                .await?,
        )?));
    }
    if path == "/messages" && *method == Method::GET {
        let mut params = Map::new();
        params.insert(
            "threadId".to_owned(),
            json!(query.get("threadId").map(String::as_str).unwrap_or("")),
        );
        if let Some(before) = query.get("before").filter(|s| !s.is_empty()) {
            params.insert("before".to_owned(), json!(before));
        }
        return Ok(Some(ok(
            chat.call(session_id, "messages", Value::Object(params))
                .await?,
        )?));
    }
    if path == "/chats" && *method == Method::POST {
        let v = input(body, headers, &["accountId", "phone", "user"]).await?;
        let account_id = str_req(v.get("accountId"), 64)?;
        let phone = str_opt(v.get("phone"), 40)?;
        let user = str_opt(v.get("user"), 25)?;
        let value = chat
            .open_chat(session_id, &account_id, phone.as_deref(), user.as_deref())
            .await?;
        return Ok(Some(json_response(&value, StatusCode::CREATED)));
    }
    if path == "/voice/join" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "muted", "deafened"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let muted = v.get("muted") == Some(&Value::Bool(true));
        let deafened = v.get("deafened") == Some(&Value::Bool(true));
        return Ok(Some(ok(
            chat.voice_join(session_id, &thread_id, muted, deafened)
                .await?,
        )?));
    }
    if path == "/voice/leave" && *method == Method::POST {
        let v = input(body, headers, &["accountId"]).await?;
        let account_id = str_req(v.get("accountId"), 64)?;
        return Ok(Some(ok(chat.voice_leave(session_id, &account_id).await?)?));
    }
    if path == "/voice/set" && *method == Method::POST {
        let v = input(body, headers, &["accountId", "muted", "deafened"]).await?;
        let muted = v
            .get("muted")
            .and_then(Value::as_bool)
            .ok_or_else(AppError::invalid)?;
        let deafened = v
            .get("deafened")
            .and_then(Value::as_bool)
            .ok_or_else(AppError::invalid)?;
        let account_id = str_req(v.get("accountId"), 64)?;
        return Ok(Some(ok(
            chat.voice_set(session_id, &account_id, muted, deafened)
                .await?,
        )?));
    }
    if path == "/voice/code" && *method == Method::GET {
        let account_id = str_query(query.get("accountId"), 64)?;
        return Ok(Some(ok(chat.voice_code(session_id, &account_id).await?)?));
    }
    if path == "/servers" && *method == Method::GET {
        let account_id = str_query(query.get("accountId"), 64)?;
        return Ok(Some(ok(chat.servers(session_id, &account_id).await?)?));
    }
    if path == "/friends" && *method == Method::GET {
        let account_id = str_query(query.get("accountId"), 64)?;
        return Ok(Some(ok(chat.friends(session_id, &account_id).await?)?));
    }
    if path == "/catchUp" && *method == Method::POST {
        let v = input(body, headers, &["threadId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        return Ok(Some(ok(chat.catch_up(session_id, &thread_id).await?)?));
    }
    if path == "/media" && (*method == Method::GET || *method == Method::HEAD) {
        let thread_id = str_query(query.get("threadId"), 300)?;
        let message_id = str_query(query.get("messageId"), 128)?;
        let attachment = query.get("download").map(String::as_str) == Some("1");
        return Ok(Some(
            chat.serve_media(method, headers, session_id, &thread_id, &message_id, attachment)
                .await?,
        ));
    }
    if path == "/save" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId", "folderId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        let folder_id = str_default(v.get("folderId"), 100)?;
        let value = chat
            .save_media(session_id, &thread_id, &message_id, &folder_id)
            .await?;
        return Ok(Some(json_response(&value, StatusCode::CREATED)));
    }
    if path == "/react" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId", "emoji", "remove"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        let emoji = str_default(v.get("emoji"), 120)?;
        let remove = v.get("remove") == Some(&Value::Bool(true));
        return Ok(Some(ok(
            chat.act(
                session_id,
                &thread_id,
                &message_id,
                "react",
                Some(emoji),
                None,
                remove,
            )
            .await?,
        )?));
    }
    if path == "/edit" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId", "text"]).await?;
        let text = str_req(v.get("text"), CHAT_LIMIT_TEXT)?;
        if text.trim().is_empty() {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Write the new text first.",
            ));
        }
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        return Ok(Some(ok(
            chat.act(
                session_id,
                &thread_id,
                &message_id,
                "edit",
                None,
                Some(text),
                false,
            )
            .await?,
        )?));
    }
    if path == "/revoke" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        return Ok(Some(ok(
            chat.act(session_id, &thread_id, &message_id, "revoke", None, None, false)
                .await?,
        )?));
    }
    if path == "/deleteForMe" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        return Ok(Some(ok(
            chat.call(
                session_id,
                "deleteForMe",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?,
        )?));
    }
    if path == "/chatSetting" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "pinned", "archived", "mutedUntil"]).await?;
        let (mut pinned, mut archived, mut muted_until) = (None, None, None);
        if let Some(b) = v.get("pinned").and_then(Value::as_bool) {
            pinned = Some(b);
        } else if let Some(b) = v.get("archived").and_then(Value::as_bool) {
            archived = Some(b);
        } else if let Some(n) = v.get("mutedUntil").and_then(Value::as_f64)
            && n.is_finite()
            && n.fract() == 0.0
            && (-1.0..=9_007_199_254_740_991.0).contains(&n)
        {
            muted_until = Some(n as i64);
        } else {
            return Err(AppError::invalid());
        }
        let thread_id = str_req(v.get("threadId"), 300)?;
        return Ok(Some(ok(
            chat.chat_setting(session_id, &thread_id, pinned, archived, muted_until)
                .await?,
        )?));
    }
    if path == "/info" && *method == Method::GET {
        let thread_id = str_query(query.get("threadId"), 300)?;
        return Ok(Some(ok(chat.info(session_id, &thread_id).await?)?));
    }
    if path == "/history" && *method == Method::POST {
        let v = input(body, headers, &["threadId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        return Ok(Some(ok(chat.request_history(session_id, &thread_id).await?)?));
    }
    if path == "/contacts" && *method == Method::GET {
        let account_id = query.get("accountId").map(String::as_str).unwrap_or("");
        let q = query.get("q").map(String::as_str).unwrap_or("");
        return Ok(Some(ok(
            chat.call(
                session_id,
                "contactSearch",
                json!({ "accountId": account_id, "q": q }),
            )
            .await?,
        )?));
    }
    if path == "/read" && *method == Method::POST {
        let v = input(body, headers, &["threadId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        return Ok(Some(ok(chat.mark_read(session_id, &thread_id).await?)?));
    }
    if path == "/send" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "text", "quotedId"]).await?;
        let text = str_req(v.get("text"), CHAT_LIMIT_TEXT)?;
        if text.trim().is_empty() {
            return Err(AppError::with("invalidInput", 400, "Write a message first."));
        }
        let thread_id = str_req(v.get("threadId"), 300)?;
        let quoted_id = str_opt(v.get("quotedId"), 128)?;
        let value = chat
            .send(session_id, &thread_id, &text, quoted_id.as_deref())
            .await?;
        return Ok(Some(json_response(&value, StatusCode::CREATED)));
    }
    if path == "/retry" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        return Ok(Some(ok(
            chat.retry(session_id, &thread_id, &message_id).await?,
        )?));
    }
    if path == "/discard" && *method == Method::POST {
        let v = input(body, headers, &["threadId", "messageId"]).await?;
        let thread_id = str_req(v.get("threadId"), 300)?;
        let message_id = str_req(v.get("messageId"), 128)?;
        return Ok(Some(ok(
            chat.call(
                session_id,
                "deleteFailed",
                json!({ "threadId": thread_id, "messageId": message_id }),
            )
            .await?,
        )?));
    }
    if path == "/accounts/link" && *method == Method::POST {
        let v = input(body, headers, &["name", "phone", "accountId", "platform"]).await?;
        let phone = str_opt(v.get("phone"), 40)?;
        if let Some(ref p) = phone
            && !valid_link_phone(p)
        {
            return Err(AppError::with(
                "invalidInput",
                400,
                "Enter the full number with its country code.",
            ));
        }
        let platform = match v.get("platform") {
            None => None,
            Some(Value::String(s)) if s == "whatsapp" || s == "discord" => Some(s.as_str()),
            _ => return Err(AppError::invalid()),
        };
        let name = str_opt(v.get("name"), CHAT_LIMIT_NAME)?;
        let account_id = str_opt(v.get("accountId"), 64)?;
        let value = chat
            .link(
                session_id,
                name.as_deref(),
                phone.as_deref(),
                account_id.as_deref(),
                platform,
            )
            .await?;
        return Ok(Some(json_response(&value, StatusCode::CREATED)));
    }

    if let Some(rest) = path.strip_prefix("/accounts/") {
        let (id, suffix) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        };
        if is_account_id(id) && matches!(suffix, "" | "/link" | "/unlink") {
            if suffix == "/link" && *method == Method::GET {
                return Ok(Some(ok(chat.link_state(session_id, id).await?)?));
            }
            if suffix == "/link" && *method == Method::DELETE {
                return Ok(Some(ok(chat.cancel_link(session_id, id).await?)?));
            }
            if suffix == "/unlink" && *method == Method::POST {
                return Ok(Some(ok(chat.unlink(session_id, id).await?)?));
            }
            if suffix.is_empty() && *method == Method::PATCH {
                let v = input(body, headers, &["name"]).await?;
                let name = str_req(v.get("name"), CHAT_LIMIT_NAME)?;
                return Ok(Some(ok(
                    chat.call(session_id, "rename", json!({ "accountId": id, "name": name }))
                        .await?,
                )?));
            }
            if suffix.is_empty() && *method == Method::DELETE {
                return Ok(Some(ok(chat.remove(session_id, id).await?)?));
            }
        }
    }

    Ok(None)
}
