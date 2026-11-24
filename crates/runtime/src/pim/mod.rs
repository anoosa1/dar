//! Contacts and Calendar for the browser (`runtime/pim/service.ts` and `http.ts`): address
//! books, calendars, sharing, items, people search, occurrences, import and export. Every
//! response carries the account's change revision (`x-dar-pim-revision`).
//! Not yet: calendar subscription sync, reminders, Tasks in Calendar.

pub mod calendar;
pub mod contacts;
pub mod notes;
pub mod tasks;
pub mod vobject;
pub mod workspace;

use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde_json::{Map, Value, json};

use crate::db::Database;
use crate::error::{AppError, Result};
use vobject::{field, invalid};

/// One request's PIM calls for a session; remembers the newest change revision seen.
pub struct Pim<'a> {
    pub db: &'a Database,
    pub session: &'a str,
    revision: AtomicU64,
}

pub struct Parsed {
    pub uid: String,
    pub content: String,
    pub data: Value,
}

/// The standards text of a contact or calendar resource, read and projected.
pub fn read(kind: &str, content: &str) -> Result<Parsed> {
    if kind == "contacts" {
        let c = contacts::read_contact(content)?;
        Ok(Parsed {
            uid: c.uid,
            content: c.content,
            data: c.data,
        })
    } else {
        let e = calendar::read_any(content)?;
        Ok(Parsed {
            uid: e.uid,
            content: e.content,
            data: e.data,
        })
    }
}

impl<'a> Pim<'a> {
    pub fn new(db: &'a Database, session: &'a str) -> Self {
        Pim {
            db,
            session,
            revision: AtomicU64::new(0),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    pub async fn call(&self, operation: &str, payload: Value) -> Result<Value> {
        let read = [
            "collections",
            "list",
            "read",
            "grants",
            "page",
            "search",
            "resource",
            "preferences",
            "reminders",
        ]
        .contains(&operation);
        let args = json!([self.session, operation, payload.to_string()]);
        let result = if read {
            self.db.read("service_pim", args).await?
        } else {
            self.db.call("service_pim", args).await?
        };
        self.revision
            .fetch_max(result["revision"].as_u64().unwrap_or(0), Ordering::Relaxed);
        Ok(result["value"].clone())
    }

    pub async fn collections(&self) -> Result<Vec<Value>> {
        Ok(self
            .call("collections", json!({}))
            .await?
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    pub async fn list(&self, collection_id: &str) -> Result<Value> {
        self.call("list", json!({ "collectionId": collection_id }))
            .await
    }

    pub async fn read(&self, collection_id: &str, href: &str) -> Result<Value> {
        self.call(
            "read",
            json!({ "collectionId": collection_id, "href": href }),
        )
        .await
    }

    /// Writes a resource from standards text (`content`) or form data (`data`).
    pub async fn put(
        &self,
        collection: &Value,
        href: &str,
        input: &Value,
        revision: u64,
        previous: Option<&str>,
    ) -> Result<Value> {
        let kind = collection["kind"].as_str().unwrap_or_default();
        if kind == "notes" {
            let old = if previous.is_some() {
                Some(
                    self.read(collection["id"].as_str().unwrap_or_default(), href)
                        .await?,
                )
            } else {
                None
            };
            let old_data = old.as_ref().and_then(|o| {
                serde_json::from_str::<Value>(o["projection"].as_str().unwrap_or("{}")).ok()
            });
            let note = notes::read_note(input, old_data.as_ref())?;
            let uid = old
                .as_ref()
                .and_then(|o| o["uid"].as_str().map(str::to_owned))
                .unwrap_or_else(crate::auth::uuid);
            let payload = json!({ "collectionId": collection["id"], "href": href, "revision": revision, "id": crate::auth::uuid(), "uid": uid, "content": note.0, "projection": note.1.to_string() });
            return self.call("put", payload).await;
        }
        let task = kind == "calendar"
            && match input["content"].as_str() {
                Some(content) => tasks::is_task(content),
                None => input["component"] == "task" || previous.is_some_and(tasks::is_task),
            };
        if kind == "calendar" && previous.is_some_and(|p| task != tasks::is_task(p)) {
            return Err(invalid(
                "A resource cannot change between VEVENT and VTODO; create a new resource.",
            ));
        }
        let parsed = if task {
            let t = match input["content"].as_str() {
                Some(content) => tasks::read_task(content)?,
                None => {
                    let occurrence = input["occurrenceId"]
                        .as_str()
                        .map(|o| field(&json!(o), 4096))
                        .transpose()?;
                    tasks::write_task(&input["data"], previous, occurrence.as_deref())?
                }
            };
            Parsed {
                uid: t.uid,
                content: t.content,
                data: t.data,
            }
        } else if let Some(content) = input["content"].as_str() {
            read(kind, content)?
        } else if kind == "contacts" {
            let c = contacts::write_contact(&input["data"], previous)?;
            Parsed {
                uid: c.uid,
                content: c.content,
                data: c.data,
            }
        } else {
            if previous.is_some_and(dar_dav::ical::is_task) {
                return Err(invalid("Tasks are edited in Tasks."));
            }
            let event = if let Some(occurrence) = input["cancelOccurrence"].as_str() {
                calendar::cancel_occurrence(
                    previous.unwrap_or_default(),
                    &field(&json!(occurrence), 4096)?,
                )?
            } else {
                let occurrence = input["occurrenceId"]
                    .as_str()
                    .map(|o| field(&json!(o), 4096))
                    .transpose()?;
                calendar::write_event(calendar::Write {
                    input: &input["data"],
                    previous,
                    occurrence: occurrence.as_deref(),
                })?
            };
            Parsed {
                uid: event.uid,
                content: event.content,
                data: event.data,
            }
        };
        if let Some(previous) = previous
            && read(kind, previous)?.uid != parsed.uid
        {
            return Err(invalid(
                "A resource UID cannot change; create another resource instead.",
            ));
        }
        let payload = json!({ "collectionId": collection["id"], "href": href, "revision": revision, "id": crate::auth::uuid(), "uid": parsed.uid, "content": parsed.content, "projection": parsed.data.to_string() });
        self.call("put", payload).await
    }

    pub async fn import(&self, collection: &Value, text: &str) -> Result<Value> {
        let kind = collection["kind"].as_str().unwrap_or_default();
        let input = vobject::resources(
            text,
            if kind == "contacts" {
                "vcard"
            } else {
                "vcalendar"
            },
        )?;
        let parts: Vec<String> = if kind == "contacts" {
            input
        } else {
            let mut all = Vec::new();
            for part in input {
                all.extend(calendar::split_import(&part)?);
            }
            all
        };
        let mut items: Vec<(usize, Value)> = Vec::new();
        let mut imported = 0;
        // Tasks are imported parents first.
        let mut pending: Vec<(usize, String, String, String)> = Vec::new();
        for (index, content) in parts.into_iter().enumerate() {
            if tasks::is_task(&content) {
                match tasks::read_task(&content) {
                    Ok(t) => pending.push((
                        index,
                        content,
                        t.uid,
                        t.data["parentUid"].as_str().unwrap_or_default().to_owned(),
                    )),
                    Err(e) => items.push((index, json!({ "index": index, "error": e.message }))),
                }
            } else {
                pending.push((index, content, String::new(), String::new()));
            }
        }
        while !pending.is_empty() {
            let mut ready = Vec::new();
            let mut rest = Vec::new();
            let mut wave_uids = std::collections::HashSet::new();
            let pending_uids: std::collections::HashSet<String> = pending
                .iter()
                .filter(|(_, _, uid, _)| !uid.is_empty())
                .map(|(_, _, uid, _)| uid.clone())
                .collect();
            for item in pending {
                let (_, _, ref uid, ref parent) = item;
                let parent_ready = parent.is_empty() || !pending_uids.contains(parent);
                let uid_distinct = uid.is_empty() || !wave_uids.contains(uid);
                if parent_ready && uid_distinct && ready.len() < 16 {
                    if !uid.is_empty() {
                        wave_uids.insert(uid.clone());
                    }
                    ready.push(item);
                } else {
                    rest.push(item);
                }
            }
            if ready.is_empty() {
                for (index, ..) in &rest {
                    items.push((*index, json!({ "index": index, "error": "Task parent relationships form a cycle." })));
                }
                break;
            }
            pending = rest;
            use futures_util::StreamExt;
            let wave_results: Vec<(usize, String, Result<Value>)> =
                futures_util::stream::iter(ready.into_iter().map(|(index, content, _, _)| {
                    let href = format!(
                        "{}.{}",
                        crate::auth::uuid(),
                        if kind == "contacts" { "vcf" } else { "ics" }
                    );
                    async move {
                        let res = self
                            .put(collection, &href, &json!({ "content": content }), 0, None)
                            .await;
                        (index, href, res)
                    }
                }))
                .buffered(16)
                .collect()
                .await;
            for (index, href, res) in wave_results {
                match res {
                    Ok(_) => {
                        imported += 1;
                        items.push((index, json!({ "index": index, "href": href })));
                    }
                    Err(e) => items.push((index, json!({ "index": index, "error": e.message }))),
                }
            }
        }
        items.sort_by_key(|(i, _)| *i);
        Ok(
            json!({ "items": items.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), "imported": imported }),
        )
    }

    pub async fn export(&self, collection_id: &str) -> Result<String> {
        let list = self.list(collection_id).await?;
        let items = list["items"].as_array().cloned().unwrap_or_default();
        let contents: Vec<&str> = items.iter().filter_map(|r| r["content"].as_str()).collect();
        if contents.iter().map(|c| c.len()).sum::<usize>() > 8 * 1024 * 1024 {
            return Err(invalid(
                "Export exceeds 8 MiB; export individual resources or use DAV.",
            ));
        }
        if list["collection"]["kind"] == "contacts" {
            return Ok(contents.concat());
        }
        calendar::export(&contents)
    }

    async fn occurrences(
        &self,
        collection_id: &str,
        zone: &str,
        from: i64,
        to: i64,
        include_cancelled: bool,
        include_tasks: bool,
    ) -> Result<Vec<Value>> {
        let list = self.list(collection_id).await?;
        if list["collection"]["kind"] != "calendar" {
            return Err(invalid("This is not a calendar."));
        }
        let mut result = Vec::new();
        for row in list["items"].as_array().into_iter().flatten() {
            if dar_dav::ical::is_task(row["content"].as_str().unwrap_or_default()) {
                continue;
            }
            result.extend(calendar::expand(
                row,
                zone,
                from,
                to,
                false,
                include_cancelled,
            )?);
        }
        // Dated tasks sit at their due time (date-only tasks on their due day); undated tasks stay in Tasks.
        if include_tasks {
            for row in list["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| tasks::is_task(r["content"].as_str().unwrap_or_default()))
            {
                for task in tasks::task_occurrences(row, zone, from, to, false, false)? {
                    if task["status"] == "CANCELLED" && !include_cancelled {
                        continue;
                    }
                    let end = chrono::DateTime::parse_from_rfc3339(
                        task["endAt"].as_str().unwrap_or_default(),
                    )
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(0);
                    let all_day = task["allDay"] == true;
                    let has_due = task["due"] != "";
                    let start_at = if all_day {
                        json!(calendar::iso(end - 86_400_000))
                    } else if has_due {
                        task["endAt"].clone()
                    } else {
                        task["startAt"].clone()
                    };
                    let end_at = if all_day || has_due {
                        task["endAt"].clone()
                    } else {
                        task["startAt"].clone()
                    };
                    result.push(json!({ "resourceId": task["resourceId"], "collectionId": task["collectionId"], "href": task["href"], "revision": task["revision"], "recurrenceId": task["recurrenceId"], "startAt": start_at, "endAt": end_at, "task": true, "priority": task["priority"],
                        "title": task["title"], "start": task["start"], "end": task["due"], "timezone": task["timezone"], "allDay": task["allDay"], "description": task["description"], "location": "", "recurrence": task["recurrence"], "recurring": task["recurring"], "attendees": [], "reminderMinutes": task["reminderMinutes"], "status": task["status"] }));
                }
            }
        }
        if result.len() > 5000 {
            return Err(invalid("Calendar window contains too many occurrences."));
        }
        result.sort_by(|a, b| a["startAt"].as_str().cmp(&b["startAt"].as_str()));
        Ok(result)
    }
}

/// `modifiedAt` and `dueAt` leave as ISO strings, as the Node runtime sends them.
fn isoify(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if (key == "modifiedAt" || key == "dueAt") && v.is_number() {
                    *v = json!(calendar::iso(v.as_f64().unwrap_or(0.0) as i64));
                } else {
                    isoify(v);
                }
            }
        }
        Value::Array(list) => list.iter_mut().for_each(isoify),
        _ => {}
    }
}

pub fn etag(row: &Value) -> String {
    format!(
        "\"{}:{}\"",
        row["id"].as_str().unwrap_or_default(),
        row["revision"]
    )
}

/// The row's revision when `If-Match` names it (`requireMatch`).
pub(crate) fn require_match(headers: &HeaderMap, row: &Value) -> Result<u64> {
    let supplied = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::new("preconditionRequired", 428))?;
    if supplied != etag(row) {
        return Err(AppError::new("revisionConflict", 412));
    }
    Ok(row["revision"].as_u64().unwrap_or(0))
}

fn respond(pim: &Pim<'_>, mut value: Value, status: StatusCode, row: Option<&Value>) -> Response {
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
    if pim.revision() > 0 {
        response
            .headers_mut()
            .insert("x-dar-pim-revision", HeaderValue::from(pim.revision()));
    }
    response
}

fn download(text: String, kind: &str, filename: &str) -> Response {
    let media = if kind == "contacts" {
        "text/vcard; charset=utf-8"
    } else {
        "text/calendar; charset=utf-8"
    };
    let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', ""));
    (
        [
            (header::CONTENT_TYPE, media.to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        text,
    )
        .into_response()
}

/// A time as `new Date(value)` reads it: RFC 3339, a bare date (UTC midnight) or a date and
/// time without an offset (local time); anything else is 0, which callers reject.
pub(crate) fn date_param(value: Option<&String>) -> i64 {
    let Some(v) = value.map(|v| v.trim()) else {
        return 0;
    };
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(v) {
        return t.timestamp_millis();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        return d
            .and_hms_opt(0, 0, 0)
            .expect("midnight")
            .and_utc()
            .timestamp_millis();
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(v, format) {
            use chrono::TimeZone;
            return chrono::Local
                .from_local_datetime(&t)
                .earliest()
                .map_or(0, |t| t.timestamp_millis());
        }
    }
    0
}

fn with_data(mut row: Value) -> Value {
    let data: Value =
        serde_json::from_str(row["projection"].as_str().unwrap_or("{}")).unwrap_or_default();
    row["data"] = data;
    row
}

/// `/api/v1/contacts/…` and `/api/v1/calendar/…`.
#[allow(clippy::too_many_arguments)]
pub async fn handle(
    db: &Database,
    session: &str,
    method: &Method,
    path: &str,
    query: &std::collections::HashMap<String, String>,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response> {
    let pim = Pim::new(db, session);
    let q = |name: &str| query.get(name).cloned().unwrap_or_default();
    let json_body = |body: Body, limit: usize| async move {
        let bytes = axum::body::to_bytes(body, limit)
            .await
            .map_err(|_| AppError::new("bodyTooLarge", 413))?;
        if bytes.is_empty() {
            return Ok(Map::new());
        }
        serde_json::from_slice::<Map<String, Value>>(&bytes)
            .map_err(|_| AppError::with("invalidJson", 400, "The request body is not valid JSON."))
    };
    if path == "/api/v1/pim/reminders" && *method == Method::GET {
        let items = pim.call("reminders", json!({})).await?;
        return Ok(respond(
            &pim,
            json!({ "items": items }),
            StatusCode::OK,
            None,
        ));
    }
    if let Some(id) = path
        .strip_prefix("/api/v1/pim/reminders/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
        && *method == Method::DELETE
    {
        let id = dar_dav::decode_component(id)
            .ok()
            .ok_or_else(|| AppError::new("notFound", 404))?;
        let result = pim.call("acknowledge", json!({ "id": id })).await?;
        return Ok(respond(&pim, result, StatusCode::OK, None));
    }
    if path == "/api/v1/contacts/people" && *method == Method::GET {
        let cursor =
            String::from_utf8(B64.decode(q("cursor")).unwrap_or_default()).unwrap_or_default();
        let payload = json!({ "kind": "contacts", "collection": q("book"), "q": q("q"), "tag": q("group"), "favorite": q("favorite") == "1", "sort": q("sort"), "cursor": cursor });
        let mut result = pim.call("search", payload).await?;
        let next = result["nextCursor"].as_str().unwrap_or_default().to_owned();
        result["nextCursor"] = json!(if next.is_empty() {
            String::new()
        } else {
            B64.encode(next)
        });
        return Ok(respond(&pim, result, StatusCode::OK, None));
    }
    if path == "/api/v1/calendar/subscriptions" && *method == Method::POST {
        let p = json_body(body, 3 * 1024 * 1024).await?;
        let payload = json!({ "id": crate::auth::uuid(), "name": field(&p.get("name").cloned().unwrap_or_default(), 255)?, "color": field(&p.get("color").cloned().unwrap_or(json!("#2563eb")), 7)?, "url": field(&p.get("url").cloned().unwrap_or_default(), 2048)?, "refreshMinutes": p.get("refreshMinutes").cloned().unwrap_or(json!(360)) });
        let row = pim.call("subscribe", payload).await?;
        return Ok(respond(&pim, row.clone(), StatusCode::CREATED, Some(&row)));
    }
    // /api/v1/{contacts/books|calendar/calendars}[/id[/sub[/href]]]
    let rest = path.strip_prefix("/api/v1/").unwrap_or_default();
    let parts: Vec<&str> = rest.split('/').collect();
    let kind = parts[0];
    let valid_base = matches!(
        (kind, parts.get(1)),
        ("contacts", Some(&"books")) | ("calendar", Some(&"calendars"))
    );
    let id = parts.get(2).copied();
    let sub = parts.get(3).copied();
    let href = parts.get(4).copied();
    let id_ok = id.is_none_or(|i| {
        i.len() == 36
            && i.bytes()
                .all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-'))
    });
    let sub_ok = sub.is_none_or(|s| {
        [
            "items",
            "export",
            "import",
            "occurrences",
            "grants",
            "subscription",
            "refresh",
        ]
        .contains(&s)
    });
    let href_ok = href.is_none_or(|h| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    });
    if !valid_base
        || !id_ok
        || !sub_ok
        || !href_ok
        || parts.len() > 5
        || href.is_some() && sub != Some("items")
    {
        return Err(AppError::not_found());
    }
    let Some(id) = id else {
        if *method == Method::GET {
            let items: Vec<Value> = pim
                .collections()
                .await?
                .into_iter()
                .filter(|c| c["kind"] == kind)
                .collect();
            return Ok(respond(
                &pim,
                json!({ "items": items, "nextCursor": "" }),
                StatusCode::OK,
                None,
            ));
        }
        if *method == Method::POST {
            let p = json_body(body, 64 * 1024).await?;
            let payload = json!({ "id": crate::auth::uuid(), "kind": kind, "name": field(&p.get("name").cloned().unwrap_or_default(), 255)?, "color": field(&p.get("color").cloned().unwrap_or(json!("#2563eb")), 7)? });
            let row = pim.call("createCollection", payload).await?;
            return Ok(respond(&pim, row.clone(), StatusCode::CREATED, Some(&row)));
        }
        return Err(AppError::new("methodNotAllowed", 405));
    };
    let collection = pim
        .collections()
        .await?
        .into_iter()
        .find(|c| c["id"] == id && c["kind"] == kind)
        .ok_or_else(AppError::not_found)?;
    match (sub, method.clone()) {
        (None, Method::GET) => {
            let list = pim.list(id).await?;
            let mut value = collection.clone();
            value["count"] = json!(list["items"].as_array().map_or(0, Vec::len));
            return Ok(respond(&pim, value, StatusCode::OK, Some(&collection)));
        }
        (None, Method::PUT | Method::DELETE) => {
            let revision = require_match(headers, &collection)?;
            let mut payload = if *method == Method::PUT {
                json_body(body, 64 * 1024).await?
            } else {
                Map::new()
            };
            payload.insert("collectionId".into(), json!(id));
            payload.insert("revision".into(), json!(revision));
            let operation = if *method == Method::PUT {
                "renameCollection"
            } else {
                "deleteCollection"
            };
            let value = pim.call(operation, Value::Object(payload)).await?;
            return Ok(respond(&pim, value, StatusCode::OK, None));
        }
        (Some("grants"), Method::GET) => {
            let value = pim.call("grants", json!({ "collectionId": id })).await?;
            return Ok(respond(
                &pim,
                json!({ "items": value }),
                StatusCode::OK,
                None,
            ));
        }
        (Some("grants"), Method::POST | Method::DELETE) => {
            let revision = require_match(headers, &collection)?;
            let mut payload = json_body(body, 64 * 1024).await?;
            payload.insert("collectionId".into(), json!(id));
            payload.insert("revision".into(), json!(revision));
            let operation = if *method == Method::POST {
                "share"
            } else {
                "revoke"
            };
            let value = pim.call(operation, Value::Object(payload)).await?;
            return Ok(respond(&pim, value, StatusCode::OK, None));
        }
        (Some("subscription"), Method::PUT) if kind == "calendar" => {
            let p = json_body(body, 64 * 1024).await?;
            let mut payload = json!({ "collectionId": id });
            if let Some(url) = p.get("url") {
                payload["url"] = json!(field(url, 2048)?);
            }
            if let Some(minutes) = p.get("refreshMinutes") {
                payload["refreshMinutes"] = minutes.clone();
            }
            let value = pim.call("updateSubscription", payload).await?;
            return Ok(respond(&pim, value, StatusCode::OK, None));
        }
        (Some("refresh"), Method::POST) if kind == "calendar" => {
            let value = pim
                .call("refreshSubscription", json!({ "collectionId": id }))
                .await?;
            return Ok(respond(&pim, value, StatusCode::OK, None));
        }
        (Some("occurrences"), Method::GET) if kind == "calendar" => {
            let zone = query
                .get("timezone")
                .cloned()
                .unwrap_or_else(|| "UTC".into());
            let items = pim
                .occurrences(
                    id,
                    &zone,
                    date_param(query.get("from")),
                    date_param(query.get("to")),
                    q("includeCancelled") == "1",
                    q("includeTasks") == "1",
                )
                .await?;
            return Ok(respond(
                &pim,
                json!({ "items": items, "nextCursor": "" }),
                StatusCode::OK,
                None,
            ));
        }
        (Some("export"), Method::GET) => {
            let text = pim.export(id).await?;
            return Ok(download(
                text,
                kind,
                if kind == "contacts" {
                    "contacts.vcf"
                } else {
                    "calendar.ics"
                },
            ));
        }
        (Some("import"), Method::POST) => {
            let p = json_body(body, 3 * 1024 * 1024).await?;
            let text = field(
                &p.get("content").cloned().unwrap_or_default(),
                2 * 1024 * 1024,
            )?;
            let value = pim.import(&collection, &text).await?;
            return Ok(respond(&pim, value, StatusCode::OK, None));
        }
        (Some("items"), _) => {}
        _ => return Err(AppError::new("methodNotAllowed", 405)),
    }
    let extension = if kind == "contacts" { "vcf" } else { "ics" };
    if let Some(href) = href {
        let old = pim.read(id, href).await?;
        match method.clone() {
            Method::GET => {
                if q("export") == "1" {
                    return Ok(download(
                        old["content"].as_str().unwrap_or_default().to_owned(),
                        kind,
                        href,
                    ));
                }
                return Ok(respond(
                    &pim,
                    with_data(old.clone()),
                    StatusCode::OK,
                    Some(&old),
                ));
            }
            Method::DELETE => {
                let revision = require_match(headers, &old)?;
                let value = pim
                    .call(
                        "delete",
                        json!({ "collectionId": id, "href": href, "revision": revision }),
                    )
                    .await?;
                return Ok(respond(&pim, value, StatusCode::OK, None));
            }
            Method::PUT => {
                let revision = require_match(headers, &old)?;
                let input = Value::Object(json_body(body, 3 * 1024 * 1024).await?);
                let row = pim
                    .put(&collection, href, &input, revision, old["content"].as_str())
                    .await?;
                return Ok(respond(
                    &pim,
                    with_data(row.clone()),
                    StatusCode::OK,
                    Some(&row),
                ));
            }
            _ => return Err(AppError::new("methodNotAllowed", 405)),
        }
    }
    match method.clone() {
        Method::POST => {
            let input = Value::Object(json_body(body, 3 * 1024 * 1024).await?);
            let href = format!("{}.{extension}", crate::auth::uuid());
            let row = pim.put(&collection, &href, &input, 0, None).await?;
            Ok(respond(
                &pim,
                with_data(row.clone()),
                StatusCode::CREATED,
                Some(&row),
            ))
        }
        Method::GET => {
            let list = pim.list(id).await?;
            let needle = q("q").to_lowercase();
            let (cursor, group, favorite) = (q("cursor"), q("group"), q("favorite") == "1");
            let all = list["items"].as_array().cloned().unwrap_or_default();
            let mut groups: Vec<String> = all
                .iter()
                .flat_map(|r| {
                    with_data(r.clone())["data"]["groups"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|g| g.as_str().map(str::to_owned))
                .collect();
            groups.sort();
            groups.dedup();
            let mut items: Vec<Value> = all
                .into_iter()
                .map(|mut r| {
                    r.as_object_mut().map(|m| m.remove("content"));
                    with_data(r)
                })
                .filter(|r| {
                    let data = &r["data"];
                    r["id"].as_str().unwrap_or_default() > cursor.as_str()
                        && (needle.is_empty() || data.to_string().to_lowercase().contains(&needle))
                        && (group.is_empty()
                            || data["groups"]
                                .as_array()
                                .is_some_and(|g| g.iter().any(|v| v == group.as_str())))
                        && (!favorite || data["favorite"] == json!(true))
                })
                .collect();
            items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            let next = if items.len() > 200 {
                items[199]["id"].clone()
            } else {
                json!("")
            };
            items.truncate(200);
            Ok(respond(
                &pim,
                json!({ "items": items, "nextCursor": next, "groups": groups }),
                StatusCode::OK,
                None,
            ))
        }
        _ => Err(AppError::new("methodNotAllowed", 405)),
    }
}
