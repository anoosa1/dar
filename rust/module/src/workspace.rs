//! Notes trash and task relationships (`workspace.ts`), and the `service_workspace` batch.

use serde_json::{Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::tables::*;
use crate::{Res, fail, now, row};

fn changed(ctx: &ReducerContext, user_id: &str, id: &str, action: &str) {
    let old = ctx.db.pim_changes().user_id().find(user_id.to_owned());
    let next = PimChange { user_id: user_id.into(), revision: old.as_ref().map_or(0, |o| o.revision) + 1 };
    if old.is_some() {
        ctx.db.pim_changes().user_id().update(next);
    } else {
        ctx.db.pim_changes().insert(next);
    }
    let mut event_id = ctx.new_uuid_v4().expect("randomness").to_string();
    while ctx.db.activity().id().find(event_id.clone()).is_some() {
        event_id = ctx.new_uuid_v4().expect("randomness").to_string();
    }
    ctx.db.activity().insert(Activity { id: event_id, owner_id: user_id.into(), file_id: id.into(), action: action.into(), at: now(ctx) });
}

fn notify(ctx: &ReducerContext, collection_id: &str) {
    let Some(c) = ctx.db.pim_collections().id().find(collection_id.to_owned()) else { return };
    let mut ids = vec![c.owner_id.clone()];
    for g in ctx.db.pim_grants().collection_id().filter(&c.id) {
        if !ids.contains(&g.user_id) {
            ids.push(g.user_id);
        }
    }
    for id in ids {
        let old = ctx.db.pim_changes().user_id().find(id.clone());
        let next = PimChange { user_id: id, revision: old.as_ref().map_or(0, |o| o.revision) + 1 };
        if old.is_some() {
            ctx.db.pim_changes().user_id().update(next);
        } else {
            ctx.db.pim_changes().insert(next);
        }
    }
    let revision = c.revision + 1;
    ctx.db.pim_collections().id().update(PimCollection { revision, modified_at: now(ctx), ..c });
}

/// Trash is private to the notebook owner, even when a writer trashes a shared note.
pub fn trash_note(ctx: &ReducerContext, resource: &PimResource) -> Res<()> {
    let Some(c) = ctx.db.pim_collections().id().find(resource.collection_id.clone()) else { return fail("invalidInput") };
    if c.kind != "notes" {
        return fail("invalidInput");
    }
    if ctx.db.note_trash().count() >= 10000 {
        return fail("operationTooLarge");
    }
    ctx.db.note_trash().insert(NoteTrash { id: resource.id.clone(), owner_id: c.owner_id, document: crate::stringify(&row(resource)), notebook_name: c.name, trashed_at: now(ctx) });
    Ok(())
}

pub fn note_trash_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Value) -> Res<Value> {
    if !crate::app_settings::app_enabled(ctx, "notes") {
        return fail("notFound");
    }
    let id = p.get("id").filter(|v| !v.is_null() && v.as_str() != Some("")).map(|v| crate::text_of(Some(v)));
    if operation == "trash" {
        let cursor = p.get("cursor").filter(|v| !v.is_null()).map(|v| crate::text_of(Some(v))).unwrap_or_default();
        let mut rows: Vec<NoteTrash> = ctx.db.note_trash().owner_id().filter(user_id).filter(|r| match &id { Some(id) => &r.id == id, None => r.id > cursor }).collect();
        rows.sort_by(|a, b| crate::locale_compare(&a.id, &b.id));
        let take = if id.is_some() { 1 } else { 200 };
        let items: Vec<Value> = rows
            .iter()
            .take(take)
            .map(|r| {
                let mut resource: Value = serde_json::from_str(&r.document).unwrap_or(Value::Null);
                if id.is_none()
                    && let Some(o) = resource.as_object_mut()
                {
                    o.remove("content");
                }
                let mut v = row(r);
                let o = v.as_object_mut().expect("row");
                o.remove("document");
                o.insert("resource".into(), resource);
                v
            })
            .collect();
        let next = if id.is_none() && rows.len() > 200 { json!(rows[199].id) } else { json!("") };
        return Ok(json!({ "items": items, "nextCursor": next }));
    }
    let Some(old) = ctx.db.note_trash().id().find(crate::text_of(p.get("id"))).filter(|o| o.owner_id == user_id) else { return fail("notFound") };
    let resource: PimResource = serde_json::from_str(&old.document).map_err(|_| "invalidInput".to_owned())?;
    if p.get("revision").and_then(Value::as_f64) != Some(resource.revision as f64) {
        return fail("conflict");
    }
    if operation == "purge" {
        ctx.db.note_trash().id().delete(old.id);
        changed(ctx, user_id, &resource.id, "notes.purged");
        return Ok(json!({ "ok": true }));
    }
    if operation != "restore" {
        return fail("invalidInput");
    }
    let target = match p.get("collectionId").filter(|v| v.as_str().is_some_and(|s| !s.is_empty())) {
        Some(v) => crate::text_of(Some(v)),
        None => resource.collection_id.clone(),
    };
    let Some(c) = ctx.db.pim_collections().id().find(target).filter(|c| c.owner_id == user_id && c.kind == "notes") else { return fail("notFound") };
    let href_key = format!("{}/{}", c.id, resource.href);
    let uid_key = format!("{}/{}", c.id, resource.uid);
    if ctx.db.pim_resources().href_key().find(href_key.clone()).is_some() || ctx.db.pim_resources().uid_key().find(uid_key.clone()).is_some() {
        return fail("conflict");
    }
    let restored = PimResource { collection_id: c.id.clone(), href_key, uid_key, revision: resource.revision + 1, modified_at: now(ctx), ..resource.clone() };
    ctx.db.pim_resources().insert(restored.clone());
    ctx.db.note_trash().id().delete(old.id);
    notify(ctx, &c.id);
    changed(ctx, user_id, &resource.id, "notes.restored");
    Ok(ctx.db.pim_resources().id().find(restored.id).map_or(Value::Null, |r| row(&r)))
}

/// Relationship validation runs with the write, so concurrent reparenting cannot create cycles.
pub fn validate_task_parent(ctx: &ReducerContext, collection_id: &str, uid: &str, projection: &str) -> Res<()> {
    let data: Value = serde_json::from_str(projection).unwrap_or(Value::Null);
    if data["kind"] != "task" {
        return Ok(());
    }
    let parent_of = |v: &Value| v.get("parentUid").filter(|p| !p.is_null()).map(|p| crate::text_of(Some(p))).unwrap_or_default();
    let mut parent = parent_of(&data);
    let mut seen = vec![uid.to_owned()];
    let mut depth = 0;
    while !parent.is_empty() {
        depth += 1;
        if seen.contains(&parent) || depth > 32 {
            return fail("invalidInput");
        }
        seen.push(parent.clone());
        let Some(r) = ctx.db.pim_resources().uid_key().find(format!("{collection_id}/{parent}")) else { return fail("invalidInput") };
        let projection: Value = serde_json::from_str(&r.projection).unwrap_or(Value::Null);
        if projection["kind"] != "task" {
            return fail("invalidInput");
        }
        parent = parent_of(&projection);
    }
    Ok(())
}

pub fn workspace_snapshot(ctx: &ReducerContext) -> Value {
    json!({ "trash": ctx.db.note_trash().iter().map(|r| row(&r)).collect::<Vec<_>>() })
}

pub fn restore_workspace(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    if let Some(value) = value {
        for r in crate::identity::snapshot_rows::<NoteTrash>(value, "trash")? {
            let document: Value = serde_json::from_str(&r.document).unwrap_or(Value::Null);
            if ctx.db.accounts().id().find(r.owner_id.clone()).is_none() || document["id"].as_str() != Some(r.id.as_str()) {
                return fail("invalidInput");
            }
            ctx.db.note_trash().insert(r);
        }
    }
    for r in ctx.db.pim_resources().iter().collect::<Vec<_>>() {
        validate_task_parent(ctx, &r.collection_id, &r.uid, &r.projection)?;
    }
    Ok(())
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_workspace(ctx: &mut spacetimedb::ProcedureContext, sessionId: String, operation: String, payload: String) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx.db.app_passwords().session_id().find(sessionId.clone()).is_some() {
            return fail("forbidden");
        }
        let data: Value = serde_json::from_str(&payload).map_err(|_| "invalidInput".to_owned())?;
        if operation != "batch" {
            return note_trash_operation(tx, &user.id, &operation, &data);
        }
        let Some(operations) = data["operations"].as_array().filter(|o| o.len() <= 500) else { return fail("operationTooLarge") };
        if let Some(collections) = data["collections"].as_object() {
            for (id, revision) in collections {
                let c = tx.db.pim_collections().id().find(id.clone());
                if c.is_none_or(|c| revision.as_f64() != Some(c.revision as f64)) {
                    return fail("conflict");
                }
            }
        }
        let mut result = Vec::new();
        let mut affected: Vec<String> = Vec::new();
        for op in operations {
            let name = op["operation"].as_str().unwrap_or_default();
            if !["put", "delete", "move"].contains(&name) {
                return fail("invalidInput");
            }
            for key in ["collectionId", "targetCollectionId"] {
                if let Some(id) = op[key].as_str().filter(|s| !s.is_empty())
                    && !affected.iter().any(|a| a == id)
                {
                    affected.push(id.to_owned());
                }
            }
            result.push(crate::pim::pim_operation(tx, &user.id, name, op, None, true)?);
        }
        for id in affected {
            for r in tx.db.pim_resources().collection_id().filter(&id).collect::<Vec<_>>() {
                validate_task_parent(tx, &id, &r.uid, &r.projection)?;
            }
        }
        Ok(json!({ "items": result }))
    })
}
