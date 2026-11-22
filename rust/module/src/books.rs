//! Reading state and bookmarks (`books.ts`). Books never own bytes: rows point at Files entries.

use serde_json::{Map, Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::tables::*;
use crate::{Res, fail, now, row};

const STATUSES: [&str; 4] = ["", "want", "reading", "finished"];

fn is_uuid(value: &str) -> bool {
    crate::opaque_id(value).is_ok()
}

/// `bookPosition`: a validated position, or `None`.
pub fn book_position(value: &Value) -> Option<Value> {
    let v = value.as_object()?;
    if v.keys().any(|k| !["spine", "page", "fraction"].contains(&k.as_str())) {
        return None;
    }
    let index = |n: Option<&Value>| match n {
        None => true,
        Some(n) => n.as_f64().is_some_and(|f| f.fract() == 0.0 && (0.0..1_000_000.0).contains(&f)),
    };
    if !index(v.get("spine")) || !index(v.get("page")) || v.get("spine").is_none() == v.get("page").is_none() {
        return None;
    }
    let fraction = v.get("fraction")?.as_f64().filter(|f| (0.0..=1.0).contains(f))?;
    Some(match v.get("spine") {
        Some(spine) => json!({ "spine": spine, "fraction": fraction }),
        None => json!({ "page": v["page"], "fraction": fraction }),
    })
}

fn parse(text: &str) -> Value {
    if text.is_empty() {
        return Value::Null;
    }
    serde_json::from_str::<Value>(text).ok().and_then(|v| book_position(&v)).unwrap_or(Value::Null)
}

fn text(value: Option<&Value>, max: usize) -> Res<String> {
    let Some(s) = value.and_then(Value::as_str) else { return fail("invalidInput") };
    if s.encode_utf16().count() > max || s.chars().any(|c| matches!(c as u32, 0..=8 | 0x0b | 0x0c | 0x0e..=0x1f)) {
        return fail("invalidInput");
    }
    Ok(s.to_owned())
}

pub fn expose_state(r: &BookState) -> Value {
    let status = if STATUSES.contains(&r.status.as_str()) { r.status.clone() } else { String::new() };
    json!({ "fileId": r.file_id, "status": status, "position": parse(&r.position), "progress": r.progress, "device": r.device, "updatedAt": r.updated_at, "revision": r.revision })
}

fn expose_mark(r: &BookMark) -> Value {
    let position = match parse(&r.position) {
        Value::Null => json!({ "page": 0, "fraction": 0 }),
        p => p,
    };
    json!({ "id": r.id, "fileId": r.file_id, "position": position, "label": r.label, "createdAt": r.created_at })
}

/// Every reading state of an account, for the library.
pub fn book_states(ctx: &ReducerContext, user_id: &str) -> Value {
    Value::Array(ctx.db.book_states().owner_id().filter(user_id).map(|r| expose_state(&r)).collect())
}

pub fn books_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Map<String, Value>, readable: &dyn Fn(&str) -> bool) -> Res<Value> {
    let file_id = || -> Res<String> {
        let Some(id) = p.get("fileId").and_then(Value::as_str).filter(|id| is_uuid(id)) else { return fail("invalidInput") };
        if !readable(id) {
            return fail("notFound");
        }
        Ok(id.to_owned())
    };
    match operation {
        "detail" => {
            let id = file_id()?;
            let key = format!("{user_id}/{id}");
            let state = ctx.db.book_states().id().find(key.clone());
            let mut marks: Vec<Value> = ctx.db.book_marks().key().filter(&key).map(|r| expose_mark(&r)).collect();
            let order = |m: &Value| m["position"]["spine"].as_f64().or(m["position"]["page"].as_f64()).unwrap_or(0.0);
            marks.sort_by(|a, b| order(a).total_cmp(&order(b)).then(a["position"]["fraction"].as_f64().unwrap_or(0.0).total_cmp(&b["position"]["fraction"].as_f64().unwrap_or(0.0))));
            Ok(json!({ "state": state.as_ref().map_or(Value::Null, expose_state), "marks": marks }))
        }
        "saveState" => {
            if p.keys().any(|k| !["fileId", "revision", "status", "position", "progress", "device"].contains(&k.as_str())) {
                return fail("invalidInput");
            }
            let id = file_id()?;
            let key = format!("{user_id}/{id}");
            let old = ctx.db.book_states().id().find(key.clone());
            // A newer state from another device is never overwritten silently.
            if p.get("revision").and_then(Value::as_f64) != Some(old.as_ref().map_or(0, |o| o.revision) as f64) {
                return fail("conflict");
            }
            let mut status = old.as_ref().map(|o| o.status.clone()).unwrap_or_default();
            let mut position = old.as_ref().map(|o| o.position.clone()).unwrap_or_default();
            let mut progress = old.as_ref().map_or(0.0, |o| o.progress);
            let mut device = old.as_ref().map(|o| o.device.clone()).unwrap_or_default();
            if let Some(s) = p.get("status") {
                let Some(s) = s.as_str().filter(|s| STATUSES.contains(s)) else { return fail("invalidInput") };
                status = s.into();
            }
            if let Some(v) = p.get("position") {
                position = if v.is_null() {
                    String::new()
                } else {
                    let Some(value) = book_position(v) else { return fail("invalidInput") };
                    crate::stringify(&value)
                };
            }
            if let Some(v) = p.get("progress") {
                let Some(f) = v.as_f64().filter(|f| (0.0..=1.0).contains(f)) else { return fail("invalidInput") };
                progress = f;
            }
            if p.contains_key("device") {
                device = text(p.get("device"), 80)?;
            }
            if old.is_none() && ctx.db.book_states().owner_id().filter(user_id).count() >= 20000 {
                return fail("quotaExceeded");
            }
            let next = BookState { id: key, owner_id: user_id.into(), file_id: id, status, position, progress, device, updated_at: now(ctx), revision: old.as_ref().map_or(0, |o| o.revision) + 1 };
            if old.is_some() {
                ctx.db.book_states().id().update(next.clone());
            } else {
                ctx.db.book_states().insert(next.clone());
            }
            Ok(expose_state(&next))
        }
        "addMark" => {
            if p.keys().any(|k| !["fileId", "id", "position", "label"].contains(&k.as_str())) {
                return fail("invalidInput");
            }
            let id = file_id()?;
            let position = p.get("position").and_then(book_position);
            let (Some(mark_id), Some(position)) = (p.get("id").and_then(Value::as_str).filter(|m| is_uuid(m)), position) else { return fail("invalidInput") };
            if let Some(existing) = ctx.db.book_marks().id().find(mark_id.to_owned()) {
                if existing.owner_id != user_id {
                    return fail("conflict");
                }
                return Ok(expose_mark(&existing));
            }
            let key = format!("{user_id}/{id}");
            if ctx.db.book_marks().key().filter(&key).count() >= 1000 {
                return fail("quotaExceeded");
            }
            let label = text(Some(p.get("label").unwrap_or(&json!(""))), 200)?.trim().to_owned();
            let mark = BookMark { id: mark_id.into(), owner_id: user_id.into(), key, file_id: id, position: crate::stringify(&position), label, created_at: now(ctx) };
            ctx.db.book_marks().insert(mark.clone());
            Ok(expose_mark(&mark))
        }
        "deleteMark" => {
            let Some(id) = p.get("id").and_then(Value::as_str).filter(|m| is_uuid(m)) else { return fail("invalidInput") };
            if ctx.db.book_marks().id().find(id.to_owned()).is_none_or(|r| r.owner_id != user_id) {
                return fail("notFound");
            }
            ctx.db.book_marks().id().delete(id.to_owned());
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

pub fn books_snapshot(ctx: &ReducerContext) -> Value {
    json!({ "states": ctx.db.book_states().iter().map(|r| row(&r)).collect::<Vec<_>>(), "marks": ctx.db.book_marks().iter().map(|r| row(&r)).collect::<Vec<_>>() })
}

pub fn restore_books(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    for r in crate::identity::snapshot_rows::<BookState>(value, "states")? {
        if ctx.db.accounts().id().find(r.owner_id.clone()).is_none() || r.id != format!("{}/{}", r.owner_id, r.file_id) || !STATUSES.contains(&r.status.as_str()) || r.revision < 1 {
            return fail("invalidInput");
        }
        ctx.db.book_states().insert(r);
    }
    for r in crate::identity::snapshot_rows::<BookMark>(value, "marks")? {
        if ctx.db.accounts().id().find(r.owner_id.clone()).is_none() || r.key != format!("{}/{}", r.owner_id, r.file_id) || !is_uuid(&r.id) {
            return fail("invalidInput");
        }
        ctx.db.book_marks().insert(r);
    }
    Ok(())
}
