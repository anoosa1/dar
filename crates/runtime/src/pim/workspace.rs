//! Notes and Tasks over the PIM service (`runtime/pim/workspace-http.ts`): notebooks and task
//! lists, their items, the notes trash, task subtrees (complete, delete, move, copy) and the
//! task views (today, upcoming, important, inbox, completed).

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Map, Value, json};

use super::vobject::{field, invalid, serialize};
use super::{Pim, date_param, etag, require_match, tasks};
use crate::db::Database;
use crate::error::{AppError, Result};

const READS: [&str; 1] = ["trash"];

/// `modifiedAt` and `trashedAt` leave as ISO strings.
fn isoify(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if (key == "modifiedAt" || key == "trashedAt") && v.is_number() {
                    *v = json!(super::calendar::iso(v.as_f64().unwrap_or(0.0) as i64));
                } else {
                    isoify(v);
                }
            }
        }
        Value::Array(list) => list.iter_mut().for_each(isoify),
        _ => {}
    }
}

fn projection(row: &Value) -> Value {
    serde_json::from_str(row["projection"].as_str().unwrap_or("{}")).unwrap_or_default()
}

struct Request<'a> {
    db: &'a Database,
    session: &'a str,
    pim: Pim<'a>,
    headers: &'a HeaderMap,
}

impl Request<'_> {
    fn json(&self, mut value: Value, status: StatusCode, row: Option<&Value>) -> Response {
        isoify(&mut value);
        let mut response = (
            status,
            [
                (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            value.to_string(),
        )
            .into_response();
        if let Some(row) = row
            && let Ok(tag) = HeaderValue::from_str(&etag(row))
        {
            response.headers_mut().insert(header::ETAG, tag);
        }
        if self.pim.revision() > 0 {
            response
                .headers_mut()
                .insert("x-dar-pim-revision", HeaderValue::from(self.pim.revision()));
        }
        response
    }
    async fn workspace(&self, operation: &str, p: Value) -> Result<Value> {
        self.db
            .operation(
                "service_workspace",
                operation,
                &READS,
                json!([self.session, operation, p.to_string()]),
            )
            .await
    }
    async fn body(&self, body: Body) -> Result<Map<String, Value>> {
        crate::http::body_object(body, 3 * 1024 * 1024, self.headers).await
    }
}

fn send(content: String, name: &str, media: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, media.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        content,
    )
        .into_response()
}

fn put_operation(collection_id: &str, row: &Value, content: &str, data: &Value) -> Value {
    json!({ "operation": "put", "collectionId": collection_id, "href": row["href"], "id": row["id"], "uid": row["uid"], "revision": row["revision"], "content": content, "projection": data.to_string() })
}

/// The task and its descendants in the same list, breadth first (at most `limit`).
fn subtree<'a>(row: &'a Value, rows: &'a [Value], limit: usize) -> Result<Vec<&'a Value>> {
    let mut selected = vec![row];
    let mut seen = vec![row["uid"].as_str().unwrap_or_default().to_owned()];
    let mut i = 0;
    while i < selected.len() {
        let parent = selected[i]["uid"].as_str().unwrap_or_default().to_owned();
        for child in rows {
            let uid = child["uid"].as_str().unwrap_or_default();
            if !seen.iter().any(|s| s == uid) && projection(child)["parentUid"] == json!(parent) {
                seen.push(uid.to_owned());
                selected.push(child);
                if selected.len() > limit {
                    return Err(invalid("Task subtree exceeds 500 items."));
                }
            }
        }
        i += 1;
    }
    Ok(selected)
}

fn today(zone: chrono_tz::Tz) -> String {
    chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y-%m-%d")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    db: &Database,
    session: &str,
    method: &Method,
    path_full: &str,
    query: &HashMap<String, String>,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response> {
    let q = |name: &str| query.get(name).cloned();
    let viewer_zone = q("timezone").unwrap_or_else(|| "UTC".into());
    let viewer = super::calendar::zone(&viewer_zone)?;
    let notes = path_full.starts_with("/api/v1/notes");
    let (base, kind) = if notes {
        ("/api/v1/notes", "notes")
    } else {
        ("/api/v1/tasks", "calendar")
    };
    let path = &path_full[base.len()..];
    let r = Request {
        db,
        session,
        pim: Pim::new(db, session),
        headers,
    };
    if notes && path == "/trash" && *method == Method::GET {
        return Ok(r.json(
            r.workspace(
                "trash",
                json!({ "cursor": q("cursor").unwrap_or_default() }),
            )
            .await?,
            StatusCode::OK,
            None,
        ));
    }
    if notes && let Some(rest) = path.strip_prefix("/trash/") {
        let (id, restore) = match rest.split_once('/') {
            Some((id, "restore")) => (id, true),
            None => (rest, false),
            _ => return Err(AppError::not_found()),
        };
        if !id.is_empty() {
            let rows = r.workspace("trash", json!({ "id": id })).await?;
            let row = rows["items"]
                .as_array()
                .and_then(|i| i.iter().find(|x| x["id"] == json!(id)).cloned())
                .ok_or_else(AppError::not_found)?;
            if *method == Method::GET && !restore {
                return Ok(r.json(row.clone(), StatusCode::OK, Some(&row["resource"])));
            }
            if *method == Method::DELETE && !restore || *method == Method::POST && restore {
                let mut p = if restore {
                    r.body(body).await?
                } else {
                    Map::new()
                };
                p.insert("id".into(), row["id"].clone());
                p.insert(
                    "revision".into(),
                    json!(require_match(headers, &row["resource"])?),
                );
                return Ok(r.json(
                    r.workspace(if restore { "restore" } else { "purge" }, Value::Object(p))
                        .await?,
                    StatusCode::OK,
                    None,
                ));
            }
        }
    }
    let collections: Vec<Value> = r
        .pim
        .collections()
        .await?
        .into_iter()
        .filter(|c| c["kind"] == kind && c["subscription"].is_null())
        .collect();
    let group = if notes { "/notebooks" } else { "/lists" };
    if let Some(rest) = path
        .strip_prefix(group)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
    {
        let parts: Vec<&str> = rest.split('/').skip(1).collect();
        let (id, sub) = (parts.first().copied(), parts.get(1).copied());
        let valid = parts.len() <= 2
            && sub.is_none_or(|s| ["grants", "export", "import"].contains(&s))
            && id.is_none_or(|i| !i.is_empty());
        if valid {
            let Some(id) = id else {
                if *method == Method::GET {
                    return Ok(r.json(
                        json!({ "items": collections, "nextCursor": "" }),
                        StatusCode::OK,
                        None,
                    ));
                }
                if *method == Method::POST {
                    let p = r.body(body).await?;
                    let color = p
                        .get("color")
                        .cloned()
                        .filter(|c| !c.is_null())
                        .unwrap_or(json!("#2563eb"));
                    let row = r.pim.call("createCollection", json!({ "id": crate::auth::uuid(), "kind": kind, "name": field(p.get("name").unwrap_or(&Value::Null), 255)?, "color": field(&color, 7)? })).await?;
                    return Ok(r.json(row.clone(), StatusCode::CREATED, Some(&row)));
                }
                return Err(AppError::new("methodNotAllowed", 405));
            };
            let c = collections
                .iter()
                .find(|c| c["id"] == json!(id))
                .cloned()
                .ok_or_else(AppError::not_found)?;
            match (sub, method.clone()) {
                (Some("grants"), Method::GET) => return Ok(r.json(json!({ "items": r.pim.call("grants", json!({ "collectionId": id })).await?, "nextCursor": "" }), StatusCode::OK, None)),
                (Some("grants"), Method::POST | Method::DELETE) => {
                    let revision = require_match(headers, &c)?;
                    let mut p = r.body(body).await?;
                    p.insert("collectionId".into(), json!(id));
                    p.insert("revision".into(), json!(revision));
                    let operation = if *method == Method::POST { "share" } else { "revoke" };
                    return Ok(r.json(r.pim.call(operation, Value::Object(p)).await?, StatusCode::OK, None));
                }
                (Some("export"), Method::GET) => {
                    if !notes {
                        return Ok(send(r.pim.export(id).await?, "tasks.ics", "text/calendar; charset=utf-8"));
                    }
                    let rows = r.pim.list(id).await?["items"].as_array().cloned().unwrap_or_default();
                    let items: Vec<Value> = rows
                        .iter()
                        .map(|row| {
                            let mut v = projection(row);
                            v["content"] = row["content"].clone();
                            v
                        })
                        .collect();
                    let archive = json!({ "format": "dar-notebook-v1", "name": c["name"], "items": items }).to_string();
                    if rows.len() > 500 || archive.len() > 2 * 1024 * 1024 {
                        return Err(invalid("Notebook export exceeds 500 notes or 2 MiB; export individual notes."));
                    }
                    return Ok(send(archive, "notebook.json", "application/json; charset=utf-8"));
                }
                (Some("import"), Method::POST) => {
                    let p = r.body(body).await?;
                    if !notes {
                        return Ok(r.json(r.pim.import(&c, &field(p.get("content").unwrap_or(&Value::Null), 2 * 1024 * 1024)?).await?, StatusCode::OK, None));
                    }
                    let items = p.get("items").and_then(Value::as_array).filter(|i| i.len() <= 500).cloned().ok_or_else(|| invalid("Import at most 500 notes."))?;
                    use futures_util::StreamExt;
                    let result: Vec<Value> = futures_util::stream::iter(items.into_iter().map(|input| {
                        let pim = &r.pim;
                        let c = &c;
                        async move {
                            let outcome = async {
                                let (content, data) = super::notes::read_note(&input, None)?;
                                let mut note = data.as_object().cloned().unwrap_or_default();
                                note.insert("content".into(), json!(content));
                                pim.put(c, &format!("{}.md", crate::auth::uuid()), &Value::Object(note), 0, None).await
                            }
                            .await;
                            match outcome {
                                Ok(row) => json!({ "id": row["id"] }),
                                Err(e) => json!({ "error": e.message }),
                            }
                        }
                    }))
                    .buffered(16)
                    .collect()
                    .await;
                    return Ok(r.json(json!({ "items": result }), StatusCode::OK, None));
                }
                (None, Method::GET) => return Ok(r.json(c.clone(), StatusCode::OK, Some(&c))),
                (None, Method::PUT | Method::DELETE) => {
                    let revision = require_match(headers, &c)?;
                    let mut p = if *method == Method::PUT { r.body(body).await? } else { Map::new() };
                    p.insert("collectionId".into(), json!(id));
                    p.insert("revision".into(), json!(revision));
                    let operation = if *method == Method::PUT { "renameCollection" } else { "deleteCollection" };
                    return Ok(r.json(r.pim.call(operation, Value::Object(p)).await?, StatusCode::OK, None));
                }
                _ => {}
            }
        }
    }
    let rest = path
        .strip_prefix("/items")
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .ok_or_else(AppError::not_found)?;
    let parts: Vec<&str> = rest.split('/').skip(1).collect();
    let (id, action) = (
        parts.first().copied().filter(|i| !i.is_empty()),
        parts.get(1).copied(),
    );
    if parts.len() > 2
        || action
            .is_some_and(|a| !["export", "move", "copy", "complete", "occurrences"].contains(&a))
        || (!parts.is_empty() && id.is_none())
    {
        return Err(AppError::not_found());
    }
    let Some(id) = id else {
        if *method == Method::GET {
            return list(&r, notes, &collections, query, viewer, &viewer_zone).await;
        }
        if *method == Method::POST {
            let p = r.body(body).await?;
            let c = collections
                .iter()
                .find(|c| Some(&c["id"]) == p.get("collectionId"))
                .cloned()
                .ok_or_else(AppError::not_found)?;
            let href = format!(
                "{}.{}",
                crate::auth::uuid(),
                if notes { "md" } else { "ics" }
            );
            let input = if notes {
                Value::Object(p)
            } else {
                let data = p
                    .get("data")
                    .cloned()
                    .filter(|d| !d.is_null())
                    .unwrap_or(Value::Object(p.clone()));
                json!({ "component": "task", "data": data })
            };
            let row = r.pim.put(&c, &href, &input, 0, None).await?;
            let mut value = row.clone();
            value["data"] = projection(&row);
            return Ok(r.json(value, StatusCode::CREATED, Some(&row)));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    };
    let row = r.pim.call("resource", json!({ "id": id })).await?;
    let c = collections
        .iter()
        .find(|c| c["id"] == row["collectionId"])
        .cloned();
    let content = row["content"].as_str().unwrap_or_default().to_owned();
    let Some(c) = c.filter(|_| notes || tasks::is_task(&content)) else {
        return Err(AppError::not_found());
    };
    let collection_id = c["id"].as_str().unwrap_or_default().to_owned();
    let needs_rows = !notes
        && (action.is_none() && *method == Method::GET
            || *method == Method::DELETE
            || action.is_some_and(|a| ["move", "copy", "complete"].contains(&a)));
    let rows: Vec<Value> = if needs_rows {
        r.pim.list(&collection_id).await?["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|x| tasks::is_task(x["content"].as_str().unwrap_or_default()))
            .collect()
    } else {
        Vec::new()
    };
    let data = projection(&row);
    match (action, method.clone()) {
        (None, Method::GET) => {
            let mut value = row.clone();
            value["data"] = data.clone();
            value["role"] = c["role"].clone();
            if !notes {
                if data["recurring"] == true
                    && let Some(next) = tasks::next_task_occurrence(&row, "UTC")?
                {
                    value["nextOccurrence"] = next;
                }
                value["children"] = Value::Array(
                    rows.iter()
                        .filter(|x| projection(x)["parentUid"] == row["uid"])
                        .map(|x| {
                            let d = projection(x);
                            json!({ "id": x["id"], "uid": x["uid"], "title": d["title"], "status": d["status"] })
                        })
                        .collect(),
                );
                if let Some(parent) = rows.iter().find(|x| x["uid"] == data["parentUid"]) {
                    value["parentId"] = parent["id"].clone();
                }
            }
            Ok(r.json(value, StatusCode::OK, Some(&row)))
        }
        (Some("complete"), Method::POST) if !notes => {
            require_match(headers, &row)?;
            let p = r.body(body).await?;
            let selected = subtree(&row, &rows, 500)?;
            if selected.len() > 1 && p.get("children") != Some(&json!("complete")) {
                return Err(AppError::with(
                    "conflict",
                    409,
                    "Choose whether to complete all subtasks.",
                ));
            }
            let mut operations = Vec::new();
            for entry in selected.iter().rev() {
                let next = tasks::write_task(
                    &json!({ "status": "COMPLETED", "progress": 100 }),
                    entry["content"].as_str(),
                    None,
                )?;
                operations.push(put_operation(
                    &collection_id,
                    entry,
                    &next.content,
                    &next.data,
                ));
            }
            let mut revisions = Map::new();
            revisions.insert(collection_id.clone(), c["revision"].clone());
            Ok(r.json(
                r.workspace(
                    "batch",
                    json!({ "collections": revisions, "operations": operations }),
                )
                .await?,
                StatusCode::OK,
                None,
            ))
        }
        (Some("occurrences"), Method::GET) if !notes => {
            let zone_name = q("timezone").unwrap_or_else(|| "UTC".into());
            let items = tasks::task_occurrences(
                &row,
                &zone_name,
                date_param(query.get("from")),
                date_param(query.get("to")),
                false,
                false,
            )?;
            Ok(r.json(
                json!({ "items": items, "nextCursor": "" }),
                StatusCode::OK,
                None,
            ))
        }
        (Some("export"), Method::GET) => Ok(if notes {
            send(content, "note.md", "text/markdown; charset=utf-8")
        } else {
            send(content, "task.ics", "text/calendar; charset=utf-8")
        }),
        (None, Method::PUT) => {
            let p = r.body(body).await?;
            let revision = require_match(headers, &row)?;
            let input = if notes {
                Value::Object(p)
            } else if p
                .get("data")
                .is_some_and(|d| !d.is_null() && d != &json!(false))
                || p.get("content")
                    .is_some_and(|d| !d.is_null() && d != &json!(""))
            {
                let mut m = p.clone();
                m.insert("component".into(), json!("task"));
                Value::Object(m)
            } else {
                json!({ "component": "task", "data": Value::Object(p) })
            };
            let href = row["href"].as_str().unwrap_or_default();
            let updated = r
                .pim
                .put(&c, href, &input, revision, Some(&content))
                .await?;
            let mut value = updated.clone();
            value["data"] = projection(&updated);
            Ok(r.json(value, StatusCode::OK, Some(&updated)))
        }
        (None, Method::DELETE) => {
            require_match(headers, &row)?;
            let delete = json!({ "collectionId": collection_id, "href": row["href"], "revision": row["revision"] });
            if notes {
                return Ok(r.json(r.pim.call("delete", delete).await?, StatusCode::OK, None));
            }
            let direct: Vec<&Value> = rows
                .iter()
                .filter(|x| projection(x)["parentUid"] == row["uid"])
                .collect();
            if direct.is_empty() {
                return Ok(r.json(r.pim.call("delete", delete).await?, StatusCode::OK, None));
            }
            if headers.get(header::CONTENT_LENGTH).is_none()
                && headers.get(header::TRANSFER_ENCODING).is_none()
            {
                return Err(AppError::with(
                    "conflict",
                    409,
                    "Choose whether to detach or delete subtasks.",
                ));
            }
            let p = r.body(body).await?;
            let mut operations = Vec::new();
            match p.get("children").and_then(Value::as_str) {
                Some("detach") => {
                    for child in &direct {
                        let next = tasks::write_task(
                            &json!({ "parentUid": "" }),
                            child["content"].as_str(),
                            None,
                        )?;
                        operations.push(put_operation(
                            &collection_id,
                            child,
                            &next.content,
                            &next.data,
                        ));
                    }
                }
                Some("delete") => {
                    let selected = subtree(&row, &rows, 499)?;
                    for entry in selected.iter().skip(1).rev() {
                        operations.push(json!({ "operation": "delete", "collectionId": collection_id, "href": entry["href"], "revision": entry["revision"] }));
                    }
                }
                _ => return Err(invalid("Choose detach or delete for subtasks.")),
            }
            operations.push(json!({ "operation": "delete", "collectionId": collection_id, "href": row["href"], "revision": row["revision"] }));
            let mut revisions = Map::new();
            revisions.insert(collection_id.clone(), c["revision"].clone());
            Ok(r.json(
                r.workspace(
                    "batch",
                    json!({ "collections": revisions, "operations": operations }),
                )
                .await?,
                StatusCode::OK,
                None,
            ))
        }
        (Some(action @ ("move" | "copy")), Method::POST) => {
            let p = r.body(body).await?;
            require_match(headers, &row)?;
            let target = collections
                .iter()
                .find(|t| Some(&t["id"]) == p.get("collectionId"))
                .cloned()
                .ok_or_else(AppError::not_found)?;
            if action == "move" && target["ownerId"] != c["ownerId"] {
                return Err(AppError::new("forbidden", 403));
            }
            let mut operations = Vec::new();
            if notes {
                if action == "copy" {
                    let title: String = data["title"]
                        .as_str()
                        .unwrap_or_default()
                        .encode_utf16()
                        .take(248)
                        .collect::<Vec<u16>>()
                        .pipe_string();
                    let mut note = data.as_object().cloned().unwrap_or_default();
                    note.insert("title".into(), json!(format!("{title} (copy)")));
                    note.insert("content".into(), json!(content));
                    let result = r
                        .pim
                        .put(
                            &target,
                            &format!("{}.md", crate::auth::uuid()),
                            &Value::Object(note),
                            0,
                            None,
                        )
                        .await?;
                    return Ok(r.json(result.clone(), StatusCode::CREATED, Some(&result)));
                }
                operations.push(json!({ "operation": "move", "collectionId": collection_id, "targetCollectionId": target["id"], "href": row["href"], "revision": row["revision"] }));
            } else {
                let selected = subtree(&row, &rows, 500)?;
                let ids: HashMap<String, String> = selected
                    .iter()
                    .map(|x| {
                        let uid = x["uid"].as_str().unwrap_or_default().to_owned();
                        let next = if action == "copy" {
                            crate::auth::uuid()
                        } else {
                            uid.clone()
                        };
                        (uid, next)
                    })
                    .collect();
                for (index, entry) in selected.iter().enumerate() {
                    let old = tasks::read_task(entry["content"].as_str().unwrap_or_default())?;
                    let mut input = old.data.clone();
                    let parent = old.data["parentUid"].as_str().unwrap_or_default();
                    input["parentUid"] = json!(if index == 0 {
                        String::new()
                    } else {
                        ids.get(parent).cloned().unwrap_or_default()
                    });
                    let next = tasks::write_task(&input, entry["content"].as_str(), None)?;
                    if action == "copy" {
                        let new_uid = ids[&old.uid].clone();
                        let mut root = next.root.clone();
                        for component in root.children.iter_mut().filter(|c| c.name == "VTODO") {
                            super::vobject::set_text(component, "UID", &new_uid);
                        }
                        operations.push(json!({ "operation": "put", "collectionId": target["id"], "id": crate::auth::uuid(), "href": format!("{}.ics", crate::auth::uuid()), "revision": 0, "uid": new_uid, "content": serialize(&root), "projection": next.data.to_string() }));
                    } else {
                        operations.push(json!({ "operation": "move", "collectionId": collection_id, "targetCollectionId": target["id"], "href": entry["href"], "revision": entry["revision"], "content": next.content, "projection": next.data.to_string() }));
                    }
                }
            }
            let mut revisions = Map::new();
            revisions.insert(collection_id.clone(), c["revision"].clone());
            revisions.insert(
                target["id"].as_str().unwrap_or_default().to_owned(),
                target["revision"].clone(),
            );
            Ok(r.json(
                r.workspace(
                    "batch",
                    json!({ "collections": revisions, "operations": operations }),
                )
                .await?,
                StatusCode::OK,
                None,
            ))
        }
        _ => Err(AppError::new("methodNotAllowed", 405)),
    }
}

trait PipeString {
    fn pipe_string(self) -> String;
}
impl PipeString for Vec<u16> {
    fn pipe_string(self) -> String {
        String::from_utf16_lossy(&self)
    }
}

/// Notes by search; tasks by view, sorted and paged over every list.
async fn list(
    r: &Request<'_>,
    notes: bool,
    collections: &[Value],
    query: &HashMap<String, String>,
    viewer: chrono_tz::Tz,
    viewer_zone: &str,
) -> Result<Response> {
    let q = |name: &str| query.get(name).cloned();
    let cursor_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(q("cursor").unwrap_or_default().trim_end_matches('='))
        .unwrap_or_default();
    let cursor = String::from_utf8_lossy(&cursor_bytes).into_owned();
    let cid = q("collection").filter(|s| !s.is_empty());
    let view = q("view").filter(|s| !s.is_empty());
    let sort = q("sort")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "modified".into());
    let tag = q("tag").unwrap_or_default();
    let term = {
        use unicode_normalization_shim::nfc_lower;
        nfc_lower(&q("q").unwrap_or_default())
    };
    let encode = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes());
    if notes {
        let result = r.pim.call("search", json!({ "kind": "notes", "collection": cid, "q": term, "tag": tag, "favorite": view.as_deref() == Some("favorites"), "sort": sort, "cursor": cursor })).await?;
        let next = result["nextCursor"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(encode)
            .unwrap_or_default();
        return Ok(r.json(
            json!({ "items": result["items"], "nextCursor": next }),
            StatusCode::OK,
            None,
        ));
    }
    let today_day = today(viewer);
    let week_end = chrono::NaiveDate::parse_from_str(&today_day, "%Y-%m-%d")
        .map(|d| {
            (d + chrono::Duration::days(7))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| today_day.clone());
    let mut results: Vec<(String, Value)> = Vec::new();
    for c in collections {
        if cid.as_ref().is_some_and(|id| c["id"] != json!(id)) {
            continue;
        }
        let mut col_rows: Vec<(Value, Value)> = Vec::new();
        let mut scan = String::new();
        loop {
            let page = r
                .pim
                .call("page", json!({ "collectionId": c["id"], "cursor": scan }))
                .await?;
            scan = page["nextCursor"].as_str().unwrap_or_default().to_owned();
            for row in page["items"].as_array().into_iter().flatten() {
                let data = projection(row);
                if data["kind"] != "task" {
                    continue;
                }
                col_rows.push((row.clone(), data));
            }
            if scan.is_empty() {
                break;
            }
        }
        let mut uid_to_id: HashMap<String, String> = HashMap::new();
        let mut children_by_parent: HashMap<String, Vec<Value>> = HashMap::new();
        for (row, data) in &col_rows {
            if let (Some(u), Some(id)) = (row["uid"].as_str(), row["id"].as_str()) {
                uid_to_id.insert(u.to_owned(), id.to_owned());
            }
            if let Some(puid) = data["parentUid"].as_str().filter(|s| !s.is_empty()) {
                children_by_parent
                    .entry(puid.to_owned())
                    .or_default()
                    .push(json!({
                        "id": row["id"],
                        "uid": row["uid"],
                        "title": data["title"],
                        "status": data["status"],
                    }));
            }
        }
        for (row, data) in col_rows {
            if !term.is_empty()
                && !unicode_normalization_shim::nfc_lower(
                    row["projection"].as_str().unwrap_or_default(),
                )
                .contains(&term)
                || !tag.is_empty()
                    && !data["categories"]
                        .as_array()
                        .is_some_and(|cats| cats.iter().any(|t| t == &json!(tag)))
            {
                continue;
            }
            let next = if data["recurring"] == true {
                tasks::next_task_occurrence(&row, viewer_zone)?
            } else {
                None
            };
            let d = next.as_ref().unwrap_or(&data);
            let (day, due_sort) = tasks::task_due(&row, viewer_zone, next.as_ref())?;
            let status = d["status"].as_str().unwrap_or_default();
            let done = status == "COMPLETED";
            let has_due = d["due"] != "";
            let is_starred = d["starred"] == json!(true)
                || d["priority"].as_i64().is_some_and(|p| (1..=4).contains(&p));
            let keep = match view.as_deref() {
                Some("completed") => done,
                Some("all") => true,
                other => {
                    !done
                        && status != "CANCELLED"
                        && match other {
                            Some("today") => has_due && day <= today_day,
                            Some("week") => has_due && day <= week_end,
                            Some("upcoming") => has_due && day > today_day,
                            Some("important") | Some("starred") => is_starred,
                            Some("inbox") => {
                                (d["parentUid"] == "" || d["parentUid"].is_null()) && !has_due
                            }
                            _ => true,
                        }
                }
            };
            if !keep {
                continue;
            }
            let modified_desc = format!(
                "{:013}",
                9_999_999_999_999i64 - row["modifiedAt"].as_f64().unwrap_or(0.0) as i64
            );
            let order = match sort.as_str() {
                "title" => data["title"].as_str().unwrap_or_default().to_lowercase(),
                "due" => due_sort.clone(),
                "start" => {
                    let s = d["start"].as_str().unwrap_or_default();
                    if s.is_empty() {
                        format!("9999\u{1}{modified_desc}")
                    } else {
                        format!("{s}\u{1}{modified_desc}")
                    }
                }
                "priority" => {
                    let p = d["priority"].as_i64().unwrap_or(0);
                    let rank = if (1..=9).contains(&p) { p } else { 99 };
                    let star_rank = if is_starred { 0 } else { 1 };
                    format!("{star_rank}{rank:02}\u{1}{due_sort}")
                }
                "manual" => {
                    let ord = data["sortOrder"].as_i64().unwrap_or(0);
                    let biased = (ord as i128 + 1_000_000_000_000i128).max(0);
                    format!("{biased:016}\u{1}{modified_desc}")
                }
                _ => modified_desc,
            };
            let key = format!("{order}\u{0}{}", row["id"].as_str().unwrap_or_default());
            if key <= cursor {
                continue;
            }
            let position = results.partition_point(|(k, _)| *k < key);
            if position > 200 {
                continue;
            }
            let mut item = row.clone();
            item.as_object_mut().expect("row").remove("content");
            let state = data["status"].as_str().unwrap_or_default();
            item["data"] = data.clone();
            item["role"] = c["role"].clone();
            item["collectionName"] = c["name"].clone();
            item["overdue"] = json!(
                !day.is_empty() && day < today_day && state != "COMPLETED" && state != "CANCELLED"
            );
            if let Some(puid) = data["parentUid"].as_str().filter(|s| !s.is_empty())
                && let Some(pid) = uid_to_id.get(puid)
            {
                item["parentId"] = json!(pid);
            }
            if let Some(uid) = row["uid"].as_str()
                && let Some(kids) = children_by_parent.get(uid)
            {
                item["children"] = Value::Array(kids.clone());
            }
            if let Some(next) = &next {
                item["nextOccurrence"] = next.clone();
            }
            results.insert(position, (key, item));
            if results.len() > 201 {
                results.pop();
            }
        }
    }
    let next = if results.len() > 200 {
        encode(&results[199].0)
    } else {
        String::new()
    };
    let items: Vec<Value> = results.into_iter().take(200).map(|(_, v)| v).collect();
    Ok(r.json(
        json!({ "items": items, "nextCursor": next }),
        StatusCode::OK,
        None,
    ))
}

/// NFC then lower case, as `normalize("NFC").toLowerCase()` (the runtime has no Unicode
/// normalization crate; text from browsers arrives composed, so lower case is what matters).
mod unicode_normalization_shim {
    pub fn nfc_lower(value: &str) -> String {
        value.to_lowercase()
    }
}
