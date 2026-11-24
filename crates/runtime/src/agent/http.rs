//! Native-session HTTP gateway for Agent (`next/runtime/agent/http.ts`).
//! Account scope is resolved again inside the database.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

use crate::agent::{AGENT_READ_OPERATIONS, AgentRuntime, utf16_len};
use crate::error::{AppError, Result};
use crate::http::{Runtime, body_object, json_response};

const MAX_JSON_BODY: usize = 128 * 1024;
const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;

fn is_hex_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

fn parse_before(query: &HashMap<String, String>) -> Result<Option<u64>> {
    let Some(before) = query.get("before") else {
        return Ok(None);
    };
    let bytes = before.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 10
        || !(b'1'..=b'9').contains(&bytes[0])
        || !bytes[1..].iter().all(|b| b.is_ascii_digit())
    {
        return Err(AppError::invalid());
    }
    let num = before.parse::<u64>().map_err(|_| AppError::invalid())?;
    Ok(Some(num))
}

fn parse_revision_etag(headers: &HeaderMap) -> Result<u64> {
    let etag = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::new("preconditionRequired", 428))?;
    let inner = etag
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| AppError::new("preconditionRequired", 428))?;
    if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AppError::new("preconditionRequired", 428));
    }
    inner
        .parse::<u64>()
        .map_err(|_| AppError::new("preconditionRequired", 428))
}

async fn call_agent(runtime: &Runtime, session_id: &str, op: &str, arg: Value) -> Result<Value> {
    runtime
        .db
        .operation(
            "service_agent",
            op,
            &AGENT_READ_OPERATIONS,
            json!([session_id, op, arg.to_string()]),
        )
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_http(
    runtime: &Runtime,
    method: &Method,
    path: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    session_id: &str,
    user_id: &str,
    agent: &AgentRuntime,
    body: Body,
) -> Result<Response> {
    let sub = path.strip_prefix("/api/v1/agent").unwrap_or(path);
    let segments: Vec<&str> = sub.split('/').collect();

    if sub == "/shared" && *method == Method::GET {
        let value = call_agent(runtime, session_id, "sharedThreads", json!({})).await?;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let ["", "shared", thread_id, "messages"] = segments.as_slice()
        && is_hex_id(thread_id)
        && *method == Method::GET
    {
        let before = parse_before(query)?;
        let mut arg = Map::new();
        arg.insert("threadId".into(), json!(thread_id));
        if let Some(n) = before {
            arg.insert("before".into(), json!(n));
        }
        let value = call_agent(runtime, session_id, "sharedHistory", Value::Object(arg)).await?;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let Some((thread_id, share_id)) = match segments.as_slice() {
        ["", "threads", tid, "shares"] if is_hex_id(tid) => Some((*tid, None)),
        ["", "threads", tid, "shares", sid] if is_hex_id(tid) && is_hex_id(sid) => {
            Some((*tid, Some(*sid)))
        }
        _ => None,
    } {
        if *method == Method::GET && share_id.is_none() {
            let value = call_agent(
                runtime,
                session_id,
                "threadShares",
                json!({ "threadId": thread_id }),
            )
            .await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        if *method == Method::POST && share_id.is_none() {
            let mut input = body_object(body, MAX_JSON_BODY, headers).await?;
            if input.keys().any(|k| k != "id" && k != "username") {
                return Err(AppError::invalid());
            }
            input.insert("threadId".into(), json!(thread_id));
            let value =
                call_agent(runtime, session_id, "shareThread", Value::Object(input)).await?;
            return Ok(json_response(&value, StatusCode::CREATED));
        }
        if *method == Method::DELETE
            && let Some(sid) = share_id
        {
            let value = call_agent(
                runtime,
                session_id,
                "unshareThread",
                json!({ "threadId": thread_id, "id": sid }),
            )
            .await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    }

    if sub == "/provider/limits" && *method == Method::GET {
        let _ = call_agent(runtime, session_id, "projects", json!({})).await?;
        let value = agent.limits(user_id).await;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let ["", "threads", thread_id, "fork"] = segments.as_slice()
        && is_hex_id(thread_id)
        && *method == Method::POST
    {
        let mut input = body_object(body, MAX_JSON_BODY, headers).await?;
        if input.keys().any(|k| k != "id" && k != "sequence") {
            return Err(AppError::invalid());
        }
        input.insert("threadId".into(), json!(thread_id));
        let value = call_agent(runtime, session_id, "forkThread", Value::Object(input)).await?;
        return Ok(json_response(&value, StatusCode::CREATED));
    }

    if sub == "/provider" && *method == Method::GET {
        let _ = call_agent(runtime, session_id, "projects", json!({})).await?;
        let value = agent.status(user_id).await;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let ["", "threads", thread_id, "branch"] = segments.as_slice()
        && is_hex_id(thread_id)
    {
        if *method == Method::GET {
            let value = agent
                .branch(session_id, user_id, thread_id, false, &runtime.blobs)
                .await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        if *method == Method::POST {
            let value = agent
                .branch(session_id, user_id, thread_id, true, &runtime.blobs)
                .await?;
            return Ok(json_response(&value, StatusCode::CREATED));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    }

    if let ["", "threads", thread_id, "git"] = segments.as_slice()
        && is_hex_id(thread_id)
    {
        if *method == Method::GET {
            let mut input = Map::new();
            input.insert("action".into(), json!("status"));
            let value = agent
                .git_action(session_id, user_id, thread_id, input)
                .await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        if *method == Method::POST {
            let input = body_object(body, MAX_JSON_BODY, headers).await?;
            if input
                .keys()
                .any(|k| !matches!(k.as_str(), "action" | "name" | "message"))
                || input.get("action").and_then(Value::as_str) == Some("status")
            {
                return Err(AppError::invalid());
            }
            let value = agent
                .git_action(session_id, user_id, thread_id, input)
                .await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    }

    if let Some((thread_id, sub_op)) = match segments.as_slice() {
        ["", "threads", tid, "terminal"] if is_hex_id(tid) => Some((*tid, None)),
        ["", "threads", tid, "terminal", op @ ("input" | "resize")] if is_hex_id(tid) => {
            Some((*tid, Some(*op)))
        }
        _ => None,
    } {
        let operation = if *method == Method::GET && sub_op.is_none() {
            "read"
        } else if *method == Method::DELETE && sub_op.is_none() {
            "close"
        } else if *method == Method::POST {
            sub_op.unwrap_or("open")
        } else {
            return Err(AppError::new("methodNotAllowed", 405));
        };
        let input = match operation {
            "read" => {
                let mut m = Map::new();
                m.insert(
                    "after".into(),
                    json!(query.get("after").map(String::as_str).unwrap_or("0")),
                );
                m
            }
            "close" => Map::new(),
            _ => body_object(body, MAX_JSON_BODY, headers).await?,
        };
        let value = agent
            .terminal(session_id, user_id, thread_id, operation, input)
            .await?;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let ["", "runs", run_id, action @ ("steer" | "answer")] = segments.as_slice()
        && is_hex_id(run_id)
    {
        if *method != Method::POST {
            return Err(AppError::new("methodNotAllowed", 405));
        }
        let input = body_object(body, MAX_JSON_BODY, headers).await?;
        let (value, status) = if *action == "steer" {
            (
                agent.steer(session_id, run_id, input).await?,
                StatusCode::CREATED,
            )
        } else {
            (
                agent.answer(session_id, run_id, input).await?,
                StatusCode::OK,
            )
        };
        return Ok(json_response(&value, status));
    }

    if let ["", "threads", thread_id, "attachments", att_id] = segments.as_slice()
        && is_hex_id(thread_id)
        && is_hex_id(att_id)
    {
        if *method == Method::PUT {
            let name = query.get("name").map(String::as_str).unwrap_or("");
            let media_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let length: u64 = headers
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if length > MAX_ATTACHMENT_BYTES {
                return Err(AppError::with(
                    "requestTooLarge",
                    413,
                    "Images must be 8 MiB or smaller.",
                ));
            }
            let value = agent
                .upload(
                    session_id,
                    user_id,
                    thread_id,
                    att_id,
                    name,
                    &media_type,
                    body,
                )
                .await?;
            return Ok(json_response(&value, StatusCode::CREATED));
        }
        if *method == Method::GET {
            let (info, file) = agent
                .attachment(session_id, user_id, thread_id, att_id)
                .await?;
            let stream = ReaderStream::new(file);
            let mut resp = Response::new(Body::from_stream(stream));
            *resp.status_mut() = StatusCode::OK;
            let h = resp.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&info.media_type).map_err(|_| AppError::internal())?,
            );
            h.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&info.size.to_string()).map_err(|_| AppError::internal())?,
            );
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            );
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            h.insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; sandbox"),
            );
            h.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("inline; filename=\"attachment\""),
            );
            return Ok(resp);
        }
    }

    if let Some((run_id, is_cancel)) = match segments.as_slice() {
        ["", "runs", rid] if is_hex_id(rid) => Some((*rid, false)),
        ["", "runs", rid, "cancel"] if is_hex_id(rid) => Some((*rid, true)),
        _ => None,
    } {
        if !is_cancel && *method == Method::GET {
            let value = call_agent(runtime, session_id, "run", json!({ "id": run_id })).await?;
            let text = value.to_string();
            let tag = format!(
                "\"{}\"",
                URL_SAFE_NO_PAD.encode(Sha256::digest(text.as_bytes()))
            );
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                == Some(tag.as_str())
            {
                return Ok((
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, tag),
                        (header::CACHE_CONTROL, "no-store".to_owned()),
                    ],
                )
                    .into_response());
            }
            return Ok((
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        "application/json; charset=utf-8".to_owned(),
                    ),
                    (header::CACHE_CONTROL, "no-store".to_owned()),
                    (header::ETAG, tag),
                ],
                text,
            )
                .into_response());
        }
        if is_cancel && *method == Method::POST {
            let value = agent.cancel(session_id, run_id).await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    }

    if let ["", "threads", thread_id, "runs"] = segments.as_slice()
        && is_hex_id(thread_id)
        && *method == Method::POST
    {
        let mut input = body_object(body, MAX_JSON_BODY, headers).await?;
        let revision = parse_revision_etag(headers)?;
        if input.keys().any(|k| {
            !matches!(
                k.as_str(),
                "id" | "messageId" | "content" | "model" | "effort" | "mode" | "attachments"
            )
        }) {
            return Err(AppError::invalid());
        }
        input.insert("threadId".into(), json!(thread_id));
        input.insert("revision".into(), json!(revision));
        let value = agent.start(session_id, user_id, input).await?;
        return Ok(json_response(&value, StatusCode::CREATED));
    }

    if let ["", "threads", thread_id, "files"] = segments.as_slice()
        && is_hex_id(thread_id)
        && *method == Method::GET
    {
        let q = query.get("query").map(String::as_str).unwrap_or("");
        if utf16_len(q) > 200 {
            return Err(AppError::invalid());
        }
        let value = agent.files(session_id, user_id, thread_id, q).await?;
        return Ok(json_response(&value, StatusCode::OK));
    }

    if let ["", "threads", thread_id] = segments.as_slice()
        && is_hex_id(thread_id)
    {
        if *method == Method::DELETE {
            let value = agent.delete_thread(session_id, user_id, thread_id).await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
        if *method == Method::PATCH {
            let input = body_object(body, MAX_JSON_BODY, headers).await?;
            if input.keys().any(|k| k != "title") {
                return Err(AppError::invalid());
            }
            let mut arg = Map::new();
            arg.insert("threadId".into(), json!(thread_id));
            if let Some(title) = input.get("title") {
                arg.insert("title".into(), title.clone());
            }
            let value = call_agent(runtime, session_id, "renameThread", Value::Object(arg)).await?;
            return Ok(json_response(&value, StatusCode::OK));
        }
    }

    let (operation, payload, status): (&'static str, Value, StatusCode) = match (sub, method) {
        ("/folders", m) if *m == Method::GET => (
            "folders",
            json!({
                "parentId": query.get("parentId").map(String::as_str).unwrap_or(""),
                "cursor": query.get("cursor").map(String::as_str).unwrap_or(""),
            }),
            StatusCode::OK,
        ),
        ("/projects", m) if *m == Method::GET => ("projects", json!({}), StatusCode::OK),
        ("/projects", m) if *m == Method::POST => (
            "createProject",
            Value::Object(body_object(body, MAX_JSON_BODY, headers).await?),
            StatusCode::CREATED,
        ),
        ("/threads", m) if *m == Method::GET => {
            let mut arg = Map::new();
            if let Some(pid) = query.get("projectId") {
                arg.insert("projectId".into(), json!(pid));
            }
            ("threads", Value::Object(arg), StatusCode::OK)
        }
        ("/threads", m) if *m == Method::POST => (
            "createThread",
            Value::Object(body_object(body, MAX_JSON_BODY, headers).await?),
            StatusCode::CREATED,
        ),
        _ => {
            let ["", "threads", thread_id, "messages"] = segments.as_slice() else {
                return Err(AppError::not_found());
            };
            if !is_hex_id(thread_id) {
                return Err(AppError::not_found());
            }
            if *method == Method::GET {
                let before = parse_before(query)?;
                let mut arg = Map::new();
                arg.insert("threadId".into(), json!(thread_id));
                if let Some(n) = before {
                    arg.insert("before".into(), json!(n));
                }
                ("history", Value::Object(arg), StatusCode::OK)
            } else if *method == Method::POST {
                let mut input = body_object(body, MAX_JSON_BODY, headers).await?;
                let revision = parse_revision_etag(headers)?;
                if input.keys().any(|k| k != "id" && k != "content") {
                    return Err(AppError::invalid());
                }
                input.insert("threadId".into(), json!(thread_id));
                input.insert("revision".into(), json!(revision));
                ("appendMessage", Value::Object(input), StatusCode::CREATED)
            } else {
                return Err(AppError::new("methodNotAllowed", 405));
            }
        }
    };

    let result = call_agent(runtime, session_id, operation, payload).await?;
    Ok(json_response(&result, status))
}
