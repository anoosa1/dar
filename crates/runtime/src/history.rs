//! Folder history under `/api/v1/files/history/<folderId>` (`runtime/files/history.ts`); the
//! database checks access and revisions.

use axum::body::Body;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::blobs::BlobFile;
use crate::error::{AppError, Result};
use crate::http::{Api, json_response};

pub const READ_OPERATIONS: [&str; 4] = ["overview", "changeset", "content", "branch"];

fn uuid_like(value: &str) -> bool {
    value.len() == 36 && value.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'))
}

pub async fn handle(r: &Api<'_>, body: Body) -> Result<Response> {
    let rest = r.path.strip_prefix("/api/v1/files/history/").ok_or_else(AppError::not_found)?;
    let (folder_id, rest) = rest.split_at(rest.len().min(36));
    if !uuid_like(folder_id) || !(rest.is_empty() || rest.starts_with('/')) {
        return Err(AppError::not_found());
    }
    let db = &r.runtime.db;
    let session_id = r.session_id();
    let call = async |operation: &str, document: Value| -> Result<Value> {
        let mut document = match document {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        document.insert("folderId".into(), json!(folder_id));
        db.operation("service_history", operation, &READ_OPERATIONS, json!([session_id, operation, Value::Object(document).to_string()])).await
    };
    let input = async |body: Body, allowed: &[&str]| -> Result<Value> {
        let p = r.body(body, 1024 * 1024).await?;
        if p.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(AppError::invalid());
        }
        Ok(Value::Object(p))
    };
    let send = |value: Value, status: StatusCode| Ok(json_response(&value, status));
    let method = r.method;
    if rest.is_empty() && *method == Method::GET {
        let before = r.q_opt("before");
        if let Some(b) = &before
            && !(b.len() <= 10 && !b.starts_with('0') && !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err(AppError::invalid());
        }
        let document = match before {
            Some(b) => json!({ "before": b.parse::<u64>().unwrap_or(0) }),
            None => json!({}),
        };
        return send(call("overview", document).await?, StatusCode::OK);
    }
    if rest.is_empty() && *method == Method::POST {
        let p = input(body, &["action", "revision"]).await?;
        return match p["action"].as_str() {
            Some("track") => send(call("track", json!({})).await?, StatusCode::OK),
            Some("untrack") => {
                let mut document = json!({});
                if let Some(revision) = p.get("revision") {
                    document["revision"] = revision.clone();
                }
                send(call("untrack", document).await?, StatusCode::OK)
            }
            _ => Err(AppError::invalid()),
        };
    }
    if rest == "/commits" && *method == Method::POST {
        let p = input(body, &["message", "token", "paths", "snapshot"]).await?;
        return send(call("commit", p).await?, StatusCode::CREATED);
    }
    if let Some(id) = rest.strip_prefix("/commits/").filter(|id| uuid_like(id))
        && *method == Method::GET
    {
        return send(call("changeset", json!({ "id": id })).await?, StatusCode::OK);
    }
    if rest == "/restore" && *method == Method::POST {
        let p = input(body, &["id", "revision"]).await?;
        return send(call("restore", p).await?, StatusCode::CREATED);
    }
    if let Some(action) = rest.strip_prefix("/branches/").filter(|a| *a == "merge" || *a == "discard")
        && *method == Method::POST
    {
        let p = input(body, &["name", "revision"]).await?;
        return send(call(action, p).await?, StatusCode::OK);
    }
    if let Some(version) = rest.strip_prefix("/versions/").and_then(|v| v.strip_suffix("/content")).filter(|v| uuid_like(v))
        && (*method == Method::GET || *method == Method::HEAD)
    {
        let entry: BlobFile = serde_json::from_value(call("content", json!({ "versionId": version })).await?).map_err(|_| AppError::not_found())?;
        return r.runtime.blobs.download(method, r.headers, &entry, false).await;
    }
    // The same commits a clone gets (docs/spacetimedb/folder-history.md#git).
    if rest == "/bundle" && *method == Method::GET {
        let overview = call("overview", json!({})).await?;
        let name = overview["folder"]["name"].as_str().unwrap_or("folder").to_owned();
        return r.runtime.git.bundle(session_id, folder_id, &name).await;
    }
    Err(AppError::not_found())
}
