//! Contacts and Calendar records (`next/module/src/pim.ts`): collections, sharing, resources
//! (vCard/iCalendar text with the runtime's JSON projection), preferences and the change
//! counter pages watch. The runtime parses and writes the standards text; the module checks
//! access, revisions, UIDs and limits, plans reminders and stores subscription syncs. Notes
//! collections hold Markdown; deleted notes go to the notes trash (`workspace.rs`).

use std::ops::Bound;

use serde_json::{Value, json};
use spacetimedb::{ProcedureContext, ReducerContext, Table, ViewContext};
use unicode_normalization::UnicodeNormalization;

use crate::scopes::password_allows;
use crate::tables::*;
use crate::{Res, fail, from_session, now, run, service, unused_uuid, user_for_view};

const REFRESH_CHOICES: [f64; 4] = [15.0, 60.0, 360.0, 1440.0];

/// A non-empty string of at most `max` UTF-16 units without NUL (`text`).
fn text(value: &Value, max: usize) -> Res<String> {
    match value.as_str() {
        Some(s) if !s.is_empty() && s.encode_utf16().count() <= max && !s.contains('\0') => Ok(s.to_owned()),
        _ => fail("invalidInput"),
    }
}

fn id(value: &Value) -> Res<String> {
    let result = text(value, 36)?;
    if result.len() == 36 && result.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9' | b'-')) { Ok(result) } else { fail("invalidInput") }
}

fn expected(actual: u32, value: &Value) -> Res<()> {
    if value.as_f64() == Some(actual as f64) { Ok(()) } else { fail("conflict") }
}

fn color(value: &Value) -> Res<String> {
    let c = text(value, 7)?;
    if c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit()) { Ok(c) } else { fail("invalidInput") }
}

fn subscription_url(value: &Value) -> Res<String> {
    let raw = text(value, 2048)?;
    let trimmed = raw.trim();
    let url = if trimmed.to_lowercase().starts_with("webcals://") { format!("https://{}", &trimmed[10..]) } else { trimmed.to_owned() };
    let lower = url.to_lowercase();
    let scheme = ["http://", "https://", "webcal://"].iter().find(|s| lower.starts_with(**s));
    match scheme {
        Some(s) if url.len() > s.len() && !url.chars().any(char::is_whitespace) => Ok(url),
        _ => fail("invalidInput"),
    }
}

fn refresh_choice(value: &Value) -> Res<u32> {
    match value.as_f64() {
        Some(v) if REFRESH_CHOICES.contains(&v) => Ok(v as u32),
        _ => fail("invalidInput"),
    }
}

fn due_key(at: f64) -> String {
    format!("{:016}", at.floor() as i64)
}

fn bump(ctx: &ReducerContext, user_id: &str) {
    match ctx.db.pim_changes().user_id().find(user_id.to_owned()) {
        Some(old) => {
            ctx.db.pim_changes().user_id().update(PimChange { revision: old.revision + 1, ..old });
        }
        None => {
            ctx.db.pim_changes().insert(PimChange { user_id: user_id.into(), revision: 1 });
        }
    }
}

fn notify(ctx: &ReducerContext, collection: &PimCollection) {
    bump(ctx, &collection.owner_id);
    for grant in ctx.db.pim_grants().collection_id().filter(collection.id.as_str()).collect::<Vec<_>>() {
        bump(ctx, &grant.user_id);
    }
}

/// A collection the user may use, with their role; writes need owner or write access.
fn collection(ctx: &ReducerContext, user_id: &str, value: &Value, write: bool) -> Res<(PimCollection, String)> {
    let row = ctx.db.pim_collections().id().find(id(value)?).ok_or("notFound")?;
    if row.kind == "notes" && !crate::app_settings::app_enabled(ctx, "notes") {
        return fail("notFound");
    }
    let role = if row.owner_id == user_id { Some("owner".to_owned()) } else { ctx.db.pim_grants().id().find(format!("{}/{user_id}", row.id)).map(|g| g.role) };
    let Some(role) = role else { return fail("notFound") };
    if write && role == "read" {
        return fail("forbidden");
    }
    Ok((row, role))
}

fn invalidate_reminders(ctx: &ReducerContext, resource_id: &str) {
    ctx.db.pim_reminder_cursors().resource_id().delete(resource_id.to_owned());
    for reminder in ctx.db.pim_reminders().resource_id().filter(resource_id).collect::<Vec<_>>() {
        ctx.db.pim_reminders().id().delete(reminder.id);
    }
}

fn log(ctx: &ReducerContext, owner_id: &str, resource_id: &str, action: &str) {
    ctx.db.activity().insert(Activity { id: unused_uuid(ctx), owner_id: owner_id.into(), file_id: resource_id.into(), action: action.into(), at: now(ctx) });
}

fn collection_json(row: &PimCollection, role: &str) -> Value {
    let mut value = serde_json::to_value(row).unwrap_or_default();
    value["role"] = json!(role);
    value
}

fn projection(row: &PimResource) -> Value {
    serde_json::from_str(&row.projection).unwrap_or_default()
}

/// A task's parent chain stays within its calendar and ends (`validateTaskParent`).
fn validate_task_parent(ctx: &ReducerContext, collection_id: &str, uid: &str, projection_text: &str) -> Res<()> {
    let data: Value = serde_json::from_str(projection_text).unwrap_or_default();
    if data["kind"] != "task" {
        return Ok(());
    }
    let mut parent = data["parentUid"].as_str().unwrap_or_default().to_owned();
    let mut seen = vec![uid.to_owned()];
    while !parent.is_empty() {
        if seen.contains(&parent) || seen.len() > 32 {
            return fail("invalidInput");
        }
        seen.push(parent.clone());
        let Some(row) = ctx.db.pim_resources().uid_key().find(format!("{collection_id}/{parent}")) else { return fail("invalidInput") };
        let data = projection(&row);
        if data["kind"] != "task" {
            return fail("invalidInput");
        }
        parent = data["parentUid"].as_str().unwrap_or_default().to_owned();
    }
    Ok(())
}

fn collections(ctx: &ReducerContext, user_id: &str, allows: &dyn Fn(&str) -> bool) -> Res<Vec<Value>> {
    let mut rows: Vec<(PimCollection, String)> = ctx.db.pim_collections().owner_id().filter(user_id).map(|r| (r, "owner".to_owned())).collect();
    for grant in ctx.db.pim_grants().user_id().filter(user_id) {
        if let Some(row) = ctx.db.pim_collections().id().find(grant.collection_id) {
            rows.push((row, grant.role));
        }
    }
    if rows.len() > 200 {
        return fail("operationTooLarge");
    }
    Ok(rows
        .into_iter()
        .filter(|(row, _)| allows(&row.kind) && (row.kind != "notes" || crate::app_settings::app_enabled(ctx, "notes")))
        .map(|(row, role)| {
            let mut value = collection_json(&row, &role);
            if row.kind == "calendar"
                && let Some(s) = ctx.db.pim_subscriptions().collection_id().find(row.id.clone()) {
                    value["subscription"] = json!({ "url": if role == "owner" { s.url } else { String::new() }, "refreshMinutes": s.refresh_minutes, "lastSyncAt": s.last_sync_at, "lastError": s.last_error });
                }
            value
        })
        .collect())
}

/// Contacts or notes matching a query, sorted by name (or newest note), 200 at a time.
fn search(ctx: &ReducerContext, user_id: &str, p: &Value) -> Res<Value> {
    let kind = p["kind"].as_str().unwrap_or_default();
    if kind != "notes" && kind != "contacts" {
        return fail("invalidInput");
    }
    let q = p["q"].as_str().unwrap_or_default().nfc().collect::<String>().to_lowercase();
    let cursor = p["cursor"].as_str().unwrap_or_default().to_owned();
    let wanted = p["collection"].as_str().unwrap_or_default();
    let tag = p["tag"].as_str().unwrap_or_default();
    let favorite = p["favorite"] == json!(true);
    let by_last = p["sort"] == "lastName";
    let mut groups = std::collections::BTreeSet::new();
    let mut results: Vec<(String, Value)> = Vec::new();
    for c in collections(ctx, user_id, &|_| true)? {
        if c["kind"] != kind || !wanted.is_empty() && c["id"] != wanted {
            continue;
        }
        let cid = c["id"].as_str().unwrap_or_default().to_owned();
        for row in ctx.db.pim_resources().collection_id().filter(cid.as_str()) {
            let data = projection(&row);
            let contact = data.get("displayName").is_some();
            if contact {
                for g in data["groups"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    groups.insert(g.to_owned());
                }
            }
            if !q.is_empty() {
                let haystack = format!("{}\n{}", row.projection, if contact { "" } else { row.content.as_str() });
                if !haystack.nfc().collect::<String>().to_lowercase().contains(&q) {
                    continue;
                }
            }
            if favorite && data["favorite"] != json!(true) {
                continue;
            }
            if !tag.is_empty() && !data[if contact { "groups" } else { "tags" }].as_array().is_some_and(|l| l.iter().any(|v| v == tag)) {
                continue;
            }
            let name = if contact {
                let preferred = data[if by_last { "lastName" } else { "firstName" }].as_str().unwrap_or_default();
                if preferred.is_empty() { data["displayName"].as_str().unwrap_or_default().to_owned() } else { preferred.to_owned() }
            } else {
                data["title"].as_str().unwrap_or_default().to_owned()
            };
            let key = if !contact && p["sort"] != "title" {
                format!("{:013}\u{0}{}", 9_999_999_999_999i64 - row.modified_at as i64, row.id)
            } else {
                format!("{}\u{0}{}", name.nfc().collect::<String>().to_lowercase(), row.id)
            };
            if key <= cursor {
                continue;
            }
            let at = results.partition_point(|(k, _)| *k < key);
            if at > 200 {
                continue;
            }
            let mut value = serde_json::to_value(&row).unwrap_or_default();
            let map = value.as_object_mut().expect("row");
            let snippet: String = row.content.chars().take(180).collect();
            map.remove("content");
            map.insert("data".into(), data);
            map.insert("role".into(), c["role"].clone());
            if contact {
                map.insert("bookName".into(), c["name"].clone());
            } else {
                map.insert("collectionName".into(), c["name"].clone());
                map.insert("snippet".into(), json!(snippet));
            }
            results.insert(at, (key, value));
            if results.len() > 201 {
                results.pop();
            }
        }
    }
    let next = if results.len() > 200 { results[199].0.clone() } else { String::new() };
    Ok(json!({ "items": results.into_iter().take(200).map(|(_, v)| v).collect::<Vec<_>>(), "groups": groups.into_iter().collect::<Vec<_>>(), "nextCursor": next }))
}

/// One operation for a checked session (`pimOperation`); `scope` limits app passwords.
pub fn pim_operation(ctx: &ReducerContext, user_id: &str, operation: &str, p: &Value, scope: Option<&str>, batch: bool) -> Res<Value> {
    let at = now(ctx);
    let allows = |kind: &str| scope.is_none_or(|s| password_allows(s, kind, "read") || password_allows(s, kind, "write"));
    if scope.is_some() && !["collections", "list", "read", "put", "delete"].contains(&operation) {
        return fail("forbidden");
    }
    match operation {
        "resource" => {
            let row = ctx.db.pim_resources().id().find(p["id"].as_str().unwrap_or_default().to_owned()).ok_or("notFound")?;
            collection(ctx, user_id, &json!(row.collection_id), false)?;
            return Ok(serde_json::to_value(row).unwrap_or_default());
        }
        "search" => return search(ctx, user_id, p),
        "collections" => return Ok(Value::Array(collections(ctx, user_id, &allows)?)),
        "subscribe" | "createCollection" => {
            let kind = if operation == "subscribe" { "calendar".to_owned() } else { text(&p["kind"], 255)? };
            if kind == "notes" && !crate::app_settings::app_enabled(ctx, "notes") {
                return fail("notFound");
            }
            if !["contacts", "calendar", "notes"].contains(&kind.as_str()) {
                return fail("invalidInput");
            }
            if ctx.db.pim_collections().owner_id().filter(user_id).count() >= 100 {
                return fail("operationTooLarge");
            }
            let row = PimCollection { id: id(&p["id"])?, owner_id: user_id.into(), kind: kind.clone(), name: text(&json!(text(&p["name"], 255)?.trim()), 255)?, color: color(&p["color"])?, revision: 1, modified_at: at };
            if operation == "subscribe" {
                let subscription = PimSubscription {
                    collection_id: row.id.clone(), owner_id: user_id.into(), url: subscription_url(&p["url"])?, refresh_minutes: refresh_choice(&p["refreshMinutes"])?,
                    next_at: due_key(at), last_sync_at: 0.0, last_error: String::new(), etag: String::new(), last_modified: String::new(),
                };
                ctx.db.pim_subscriptions().insert(subscription);
            }
            ctx.db.pim_collections().insert(row.clone());
            notify(ctx, &row);
            log(ctx, user_id, &row.id, if operation == "subscribe" { "calendar.subscribed".into() } else { format!("{kind}.collectionCreated") }.as_str());
            return Ok(collection_json(&row, "owner"));
        }
        "preferences" => {
            return Ok(match ctx.db.pim_preferences().user_id().find(user_id.to_owned()) {
                Some(r) => json!({ "userId": r.user_id, "value": r.value, "revision": r.revision }),
                None => json!({ "userId": user_id, "value": "{}", "revision": 0 }),
            });
        }
        "savePreferences" => {
            let old = ctx.db.pim_preferences().user_id().find(user_id.to_owned());
            expected(old.as_ref().map_or(0, |o| o.revision), &p["revision"])?;
            let row = PimPreference { user_id: user_id.into(), value: text(&p["value"], 16_384)?, revision: old.as_ref().map_or(0, |o| o.revision) + 1 };
            let value = json!({ "userId": row.user_id, "value": row.value, "revision": row.revision });
            if old.is_some() {
                ctx.db.pim_preferences().user_id().update(row);
            } else {
                ctx.db.pim_preferences().insert(row);
            }
            bump(ctx, user_id);
            return Ok(value);
        }
        "reminders" => {
            let rows: Vec<PimReminder> = ctx.db.pim_reminders().user_id().filter(user_id).filter(|r| !r.acknowledged && r.due_at <= at).take(200).collect();
            return Ok(serde_json::to_value(rows).unwrap_or_default());
        }
        "acknowledge" => {
            let reminder = ctx.db.pim_reminders().id().find(text(&p["id"], 255)?).filter(|r| r.user_id == user_id).ok_or("notFound")?;
            ctx.db.pim_reminders().id().update(PimReminder { acknowledged: true, ..reminder });
            bump(ctx, user_id);
            return Ok(json!({ "ok": true }));
        }
        _ => {}
    }
    let (c, role) = collection(ctx, user_id, &p["collectionId"], !["list", "read", "grants", "page"].contains(&operation))?;
    if !allows(&c.kind) {
        return fail("forbidden");
    }
    if operation == "page" {
        let cursor = p["cursor"].as_str().unwrap_or_default().to_owned();
        let lower = if cursor.is_empty() { Bound::Unbounded } else { Bound::Excluded(cursor.as_str()) };
        let items: Vec<PimResource> = ctx.db.pim_resources().by_collection_href().filter((c.id.as_str(), (lower, Bound::Unbounded))).take(201).collect();
        let next = if items.len() > 200 { items[199].href.clone() } else { String::new() };
        return Ok(json!({ "items": items.into_iter().take(200).collect::<Vec<_>>(), "nextCursor": next }));
    }
    let metadata_only = operation == "list" && p["metadataOnly"] == json!(true);
    if let Some(s) = scope {
        let permission = if ["list", "read"].contains(&operation) && !metadata_only { "read" } else { "write" };
        if !password_allows(s, &c.kind, permission) {
            return fail("forbidden");
        }
    }
    if operation == "list" {
        let mut rows = Vec::new();
        for mut row in ctx.db.pim_resources().collection_id().filter(c.id.as_str()) {
            if rows.len() >= 10_000 {
                return fail("operationTooLarge");
            }
            if metadata_only {
                row.content.clear();
                row.projection.clear();
            }
            rows.push(row);
        }
        return Ok(json!({ "collection": collection_json(&c, &role), "items": rows }));
    }
    if operation == "updateSubscription" || operation == "refreshSubscription" {
        let subscription = ctx.db.pim_subscriptions().collection_id().find(c.id.clone());
        let Some(subscription) = subscription.filter(|_| role == "owner") else { return fail("forbidden") };
        let update = operation == "updateSubscription";
        let url = if update && !p["url"].is_null() { subscription_url(&p["url"])? } else { subscription.url.clone() };
        let refresh = if update && !p["refreshMinutes"].is_null() { refresh_choice(&p["refreshMinutes"])? } else { subscription.refresh_minutes };
        let same = url == subscription.url;
        let next = PimSubscription {
            url,
            refresh_minutes: refresh,
            next_at: due_key(at),
            etag: if same { subscription.etag.clone() } else { String::new() },
            last_modified: if same { subscription.last_modified.clone() } else { String::new() },
            ..subscription
        };
        let value = json!({ "url": next.url, "refreshMinutes": next.refresh_minutes, "lastSyncAt": next.last_sync_at, "lastError": next.last_error });
        ctx.db.pim_subscriptions().collection_id().update(next);
        notify(ctx, &c);
        return Ok(value);
    }
    if ["renameCollection", "deleteCollection", "share", "revoke", "grants"].contains(&operation) {
        if role != "owner" {
            return fail("forbidden");
        }
        if operation == "grants" {
            let rows: Vec<Value> = ctx
                .db
                .pim_grants()
                .collection_id()
                .filter(c.id.as_str())
                .map(|g| {
                    let mut value = serde_json::to_value(&g).unwrap_or_default();
                    value["username"] = json!(ctx.db.accounts().id().find(g.user_id.clone()).map(|a| a.username));
                    value
                })
                .collect();
            return Ok(Value::Array(rows));
        }
        expected(c.revision, &p["revision"])?;
        match operation {
            "share" => {
                let user = ctx.db.accounts().username().find(text(&p["username"], 255)?.to_lowercase());
                let role = text(&p["role"], 255)?;
                let Some(user) = user.filter(|u| u.enabled && u.id != user_id && (role == "read" || role == "write")) else { return fail("invalidInput") };
                let grant = PimGrant { id: format!("{}/{}", c.id, user.id), collection_id: c.id.clone(), user_id: user.id.clone(), role };
                if ctx.db.pim_grants().id().find(grant.id.clone()).is_some() {
                    ctx.db.pim_grants().id().update(grant);
                } else {
                    ctx.db.pim_grants().insert(grant);
                }
                bump(ctx, &user.id);
            }
            "revoke" => {
                let grant = ctx.db.pim_grants().id().find(text(&p["id"], 255)?).filter(|g| g.collection_id == c.id).ok_or("notFound")?;
                ctx.db.pim_grants().id().delete(grant.id.clone());
                bump(ctx, &grant.user_id);
            }
            "deleteCollection" => {
                notify(ctx, &c);
                let rows: Vec<PimResource> = ctx.db.pim_resources().collection_id().filter(c.id.as_str()).collect();
                if rows.len() > 10_000 {
                    return fail("operationTooLarge");
                }
                for row in rows {
                    if c.kind == "notes" {
                        crate::workspace::trash_note(ctx, &row)?;
                    }
                    invalidate_reminders(ctx, &row.id);
                    ctx.db.pim_resources().id().delete(row.id);
                }
                for grant in ctx.db.pim_grants().collection_id().filter(c.id.as_str()).collect::<Vec<_>>() {
                    ctx.db.pim_grants().id().delete(grant.id);
                }
                ctx.db.pim_subscriptions().collection_id().delete(c.id.clone());
                ctx.db.pim_collections().id().delete(c.id.clone());
                log(ctx, user_id, &c.id, &format!("{}.collectionDeleted", c.kind));
                return Ok(json!({ "ok": true }));
            }
            _ => {}
        }
        let new_color = if p["color"].is_null() { c.color.clone() } else { color(&p["color"])? };
        let name = if operation == "renameCollection" { text(&json!(text(&p["name"], 255)?.trim()), 255)? } else { c.name.clone() };
        let updated = PimCollection { name, color: new_color, revision: c.revision + 1, modified_at: at, ..c.clone() };
        ctx.db.pim_collections().id().update(updated.clone());
        notify(ctx, &c);
        log(ctx, user_id, &c.id, &format!("{}.{operation}", c.kind));
        return Ok(serde_json::to_value(updated).unwrap_or_default());
    }
    let href = text(&p["href"], 255)?;
    let extension = match c.kind.as_str() {
        "contacts" => ".vcf",
        "notes" => ".md",
        _ => ".ics",
    };
    let stem_ok = href.len() > extension.len() && href[..href.len() - extension.len()].bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if !href.ends_with(extension) || !stem_ok {
        return fail("invalidInput");
    }
    let old = ctx.db.pim_resources().href_key().find(format!("{}/{href}", c.id));
    if operation == "read" {
        return old.map(|o| serde_json::to_value(o).unwrap_or_default()).ok_or_else(|| "notFound".to_owned());
    }
    // Subscribed calendars change only through their sync.
    let subscribed = ctx.db.pim_subscriptions().collection_id().find(c.id.clone()).is_some();
    if ["move", "put", "delete"].contains(&operation) && subscribed {
        return fail("forbidden");
    }
    if operation == "move" {
        let Some(old) = old.filter(|_| batch) else { return fail("invalidInput") };
        expected(old.revision, &p["revision"])?;
        let (target, target_role) = collection(ctx, user_id, &p["targetCollectionId"], true)?;
        if ctx.db.pim_subscriptions().collection_id().find(target.id.clone()).is_some() || target.kind != c.kind || target.owner_id != c.owner_id {
            return fail("forbidden");
        }
        if target.id == c.id {
            return Ok(serde_json::to_value(old).unwrap_or_default());
        }
        if ctx.db.pim_resources().uid_key().find(format!("{}/{}", target.id, old.uid)).is_some() || ctx.db.pim_resources().href_key().find(format!("{}/{href}", target.id)).is_some() {
            return fail("conflict");
        }
        let moved = PimResource {
            collection_id: target.id.clone(),
            href_key: format!("{}/{href}", target.id),
            uid_key: format!("{}/{}", target.id, old.uid),
            content: if p["content"].is_null() { old.content.clone() } else { text(&p["content"], 262_144)? },
            projection: if p["projection"].is_null() { old.projection.clone() } else { text(&p["projection"], 65_536)? },
            revision: old.revision + 1,
            modified_at: at,
            ..old.clone()
        };
        invalidate_reminders(ctx, &old.id);
        ctx.db.pim_resources().id().delete(old.id.clone());
        ctx.db.pim_resources().insert(moved.clone());
        validate_task_parent(ctx, &target.id, &old.uid, &moved.projection)?;
        if target.kind == "calendar" {
            ctx.db.pim_reminder_cursors().insert(PimReminderCursor { resource_id: old.id.clone(), revision: moved.revision, at, run_at: due_key(at + 1000.0) });
        }
        for (item, _) in [(&c, &role), (&target, &target_role)] {
            ctx.db.pim_collections().id().update(PimCollection { revision: item.revision + 1, modified_at: at, ..item.clone() });
            notify(ctx, item);
        }
        log(ctx, &c.owner_id, &old.id, &format!("{}.moved", c.kind));
        return Ok(serde_json::to_value(moved).unwrap_or_default());
    }
    if operation != "put" && operation != "delete" {
        return fail("invalidInput");
    }
    expected(old.as_ref().map_or(0, |o| o.revision), &p["revision"])?;
    let resource_id;
    if operation == "delete" {
        let Some(old) = old else { return fail("notFound") };
        if c.kind == "notes" {
            crate::workspace::trash_note(ctx, &old)?;
        }
        if projection(&old)["kind"] == "task" && ctx.db.pim_resources().collection_id().filter(c.id.as_str()).any(|child| projection(&child)["parentUid"] == old.uid.as_str()) {
            return fail("conflict");
        }
        invalidate_reminders(ctx, &old.id);
        ctx.db.pim_resources().id().delete(old.id.clone());
        resource_id = old.id;
    } else {
        let uid = text(&p["uid"], 1024)?;
        let uid_key = format!("{}/{uid}", c.id);
        let duplicate = ctx.db.pim_resources().uid_key().find(uid_key.clone());
        if old.as_ref().is_some_and(|o| o.uid != uid) {
            return fail("invalidInput");
        }
        let new_projection = text(&p["projection"], 65_536)?;
        let data: Value = serde_json::from_str(&new_projection).unwrap_or_default();
        if let Some(o) = &old
            && c.kind == "calendar" && (projection(o)["kind"] == "task") != (data["kind"] == "task") {
                return fail("invalidInput");
            }
        if duplicate.as_ref().is_some_and(|d| Some(&d.id) != old.as_ref().map(|o| &o.id)) {
            return fail("conflict");
        }
        if old.is_none() && ctx.db.pim_resources().collection_id().filter(c.id.as_str()).count() >= 10_000 {
            return fail("operationTooLarge");
        }
        let content = match p["content"].as_str() {
            Some(s) if s.encode_utf16().count() <= 262_144 && !s.contains('\0') && (c.kind == "notes" || !s.is_empty()) => s.to_owned(),
            _ => return fail("invalidInput"),
        };
        let row = PimResource {
            id: match &old {
                Some(o) => o.id.clone(),
                None => id(&p["id"])?,
            },
            collection_id: c.id.clone(),
            href: href.clone(),
            href_key: format!("{}/{href}", c.id),
            uid: uid.clone(),
            uid_key,
            content,
            projection: new_projection.clone(),
            revision: old.as_ref().map_or(0, |o| o.revision) + 1,
            modified_at: at,
        };
        validate_task_parent(ctx, &c.id, &uid, &row.projection)?;
        if data["kind"] == "task" && data["status"] == "COMPLETED" {
            let open_child = ctx.db.pim_resources().collection_id().filter(c.id.as_str()).any(|child| {
                let d = projection(&child);
                d["parentUid"] == uid.as_str() && !["COMPLETED", "CANCELLED"].contains(&d["status"].as_str().unwrap_or_default())
            });
            if open_child {
                return fail("conflict");
            }
        }
        resource_id = row.id.clone();
        if old.is_some() {
            invalidate_reminders(ctx, &row.id);
            ctx.db.pim_resources().id().update(row.clone());
        } else {
            ctx.db.pim_resources().insert(row.clone());
        }
        if c.kind == "calendar" {
            ctx.db.pim_reminder_cursors().insert(PimReminderCursor { resource_id: row.id.clone(), revision: row.revision, at, run_at: due_key(at + 1000.0) });
        }
    }
    ctx.db.pim_collections().id().update(PimCollection { revision: c.revision + 1, modified_at: at, ..c.clone() });
    notify(ctx, &c);
    log(ctx, &c.owner_id, &resource_id, &format!("{}.{operation}", c.kind));
    if operation == "delete" {
        return Ok(json!({ "ok": true }));
    }
    Ok(ctx.db.pim_resources().href_key().find(format!("{}/{href}", c.id)).map(|r| serde_json::to_value(r).unwrap_or_default()).unwrap_or_default())
}

/// A Contacts or Calendar operation for a browser session or an app password's protocol
/// session, with the account's change counter as of the result.
#[spacetimedb::procedure]
pub fn service_pim(ctx: &mut ProcedureContext, sessionId: String, operation: String, payload: String) -> String {
    let session_id = sessionId;
    run(ctx, |tx| {
        service(tx)?;
        let user = from_session(tx, &session_id)?;
        let password = tx.db.app_passwords().session_id().find(session_id.clone());
        if password.as_ref().is_some_and(|p| p.revoked || p.expires_at <= now(tx)) {
            return fail("unauthenticated");
        }
        let p: Value = serde_json::from_str(&payload).map_err(|_| "invalidInput")?;
        let value = pim_operation(tx, &user.id, &operation, &p, password.as_ref().map(|p| p.scope.as_str()), false)?;
        let revision = tx.db.pim_changes().user_id().find(user.id.clone()).map_or(0, |r| r.revision);
        Ok(json!({ "value": value, "revision": revision }).to_string())
    })
}

#[spacetimedb::view(accessor = my_pim_changes, public)]
pub fn my_pim_changes(ctx: &ViewContext) -> Option<PimChange> {
    let user = user_for_view(ctx)?;
    Some(ctx.db.pim_changes().user_id().find(user.id.clone()).unwrap_or(PimChange { user_id: user.id, revision: 0 }))
}

/// Calendar resources due reminder planning (`reminderWork`): the runtime expands each one's
/// recurrences in the owner's time zone and commits what fell due.
pub fn reminder_work(ctx: &ReducerContext) -> Value {
    let at = now(ctx);
    let mut result = Vec::new();
    let upper = due_key(at);
    for cursor in ctx.db.pim_reminder_cursors().run_at().filter((Bound::Unbounded, Bound::Included(upper.as_str()))) {
        let Some(row) = ctx.db.pim_resources().id().find(cursor.resource_id.clone()) else { continue };
        let Some(c) = ctx.db.pim_collections().id().find(row.collection_id.clone()) else { continue };
        if c.kind != "calendar" || !ctx.db.accounts().id().find(c.owner_id.clone()).is_some_and(|a| a.enabled) || ctx.db.pim_subscriptions().collection_id().find(c.id.clone()).is_some() {
            continue;
        }
        let from = if cursor.revision == row.revision { cursor.at } else { row.modified_at };
        if from >= at - 1000.0 {
            continue;
        }
        let preferences: Value = ctx.db.pim_preferences().user_id().find(c.owner_id.clone()).and_then(|p| serde_json::from_str(&p.value).ok()).unwrap_or(json!({}));
        let timezone = preferences.get("timezone").cloned().filter(|t| !t.is_null()).unwrap_or(json!("UTC"));
        result.push(json!({ "resource": crate::row(&row), "from": from, "to": at.min(from + 7.0 * 86_400_000.0), "userId": c.owner_id, "timezone": timezone }));
        if result.len() >= 200 {
            break;
        }
    }
    Value::Array(result)
}

pub fn commit_reminders(ctx: &ReducerContext, resource_id: &str, revision: u32, from: f64, to: f64, reminders: &Value) -> Res<()> {
    let resource = ctx.db.pim_resources().id().find(resource_id.to_owned());
    let cursor = ctx.db.pim_reminder_cursors().resource_id().find(resource_id.to_owned());
    let Some(resource) = resource.filter(|r| r.revision == revision) else { return fail("conflict") };
    let start = match &cursor {
        Some(c) if c.revision == revision => c.at,
        _ => resource.modified_at,
    };
    if start != from {
        return fail("conflict");
    }
    let Some(reminders) = reminders.as_array() else { return fail("invalidInput") };
    if !to.is_finite() || to <= from || to > now(ctx) || to - from > 7.0 * 86_400_000.0 || reminders.len() > 2000 {
        return fail("invalidInput");
    }
    let c = ctx.db.pim_collections().id().find(resource.collection_id.clone()).expect("collection");
    for row in reminders {
        let due_at = row.get("dueAt").and_then(Value::as_f64);
        let rid = row.get("id").and_then(Value::as_str);
        let title = row.get("title").and_then(Value::as_str);
        let (Some(due_at), Some(rid), Some(title)) = (due_at, rid, title) else { return fail("invalidInput") };
        if !due_at.is_finite() || due_at < from || due_at > to || rid.encode_utf16().count() > 200 {
            return fail("invalidInput");
        }
        let id = format!("{resource_id}/{revision}/{rid}");
        if ctx.db.pim_reminders().id().find(id.clone()).is_none() {
            ctx.db.pim_reminders().insert(PimReminder { id, user_id: c.owner_id.clone(), resource_id: resource_id.into(), resource_revision: revision, due_at, title: title.into(), acknowledged: false });
        }
    }
    let next = PimReminderCursor { resource_id: resource_id.into(), revision, at: to, run_at: due_key(now(ctx) + 60_000.0) };
    if cursor.is_some() {
        ctx.db.pim_reminder_cursors().resource_id().update(next);
    } else {
        ctx.db.pim_reminder_cursors().insert(next);
    }
    if !reminders.is_empty() {
        bump(ctx, &c.owner_id);
    }
    Ok(())
}

pub fn defer_reminder(ctx: &ReducerContext, resource_id: &str, revision: u32, from: f64) {
    if let Some(cursor) = ctx.db.pim_reminder_cursors().resource_id().find(resource_id.to_owned())
        && cursor.revision == revision
        && cursor.at == from
    {
        let run_at = due_key(now(ctx) + 300_000.0);
        ctx.db.pim_reminder_cursors().resource_id().update(PimReminderCursor { run_at, ..cursor });
    }
}

fn clip(value: &str, max: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(max).collect::<Vec<_>>())
}

/// The subscription worker (service only): pick due calendars, upsert fetched events by UID,
/// then remove events the source no longer lists. Each call is bounded.
pub fn pim_subscription_work(ctx: &ReducerContext, operation: &str, p: &Value) -> Res<Value> {
    let at = now(ctx);
    if operation == "due" {
        let mut rows = Vec::new();
        let upper = due_key(at);
        for row in ctx.db.pim_subscriptions().next_at().filter((Bound::Unbounded, Bound::Included(upper.as_str()))).collect::<Vec<_>>() {
            if !ctx.db.accounts().id().find(row.owner_id.clone()).is_some_and(|a| a.enabled) {
                continue;
            }
            // A lease: a crashed or slow sync is retried later instead of running twice.
            rows.push(json!({ "collectionId": row.collection_id, "url": row.url, "etag": row.etag, "lastModified": row.last_modified }));
            ctx.db.pim_subscriptions().collection_id().update(PimSubscription { next_at: due_key(at + 15.0 * 60_000.0), ..row });
            if rows.len() >= 10 {
                break;
            }
        }
        return Ok(Value::Array(rows));
    }
    let c = ctx.db.pim_collections().id().find(id(&p["collectionId"])?);
    let subscription = c.as_ref().and_then(|c| ctx.db.pim_subscriptions().collection_id().find(c.id.clone()));
    let (Some(c), Some(subscription)) = (c, subscription) else { return Ok(json!({ "ok": false })) };
    match operation {
        "put" => {
            let Some(items) = p.get("items").and_then(Value::as_array).filter(|i| i.len() <= 200) else { return fail("invalidInput") };
            let mut changed = 0;
            let mut count = ctx.db.pim_resources().collection_id().filter(c.id.as_str()).count();
            for input in items {
                let uid = text(&input["uid"], 1024)?;
                let href = text(&input["href"], 255)?;
                let old = ctx.db.pim_resources().uid_key().find(format!("{}/{uid}", c.id));
                let valid_href = href.len() == 44 && href.ends_with(".ics") && href[..40].bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9'));
                let content = input.get("content").and_then(Value::as_str).filter(|s| s.encode_utf16().count() <= 262_144);
                let (true, Some(content)) = (valid_href, content) else { return fail("invalidInput") };
                if old.as_ref().is_some_and(|o| o.content == content) {
                    continue;
                }
                let row = PimResource {
                    id: match &old {
                        Some(o) => o.id.clone(),
                        None => id(&input["id"])?,
                    },
                    collection_id: c.id.clone(),
                    href_key: format!("{}/{href}", c.id),
                    href,
                    uid_key: format!("{}/{uid}", c.id),
                    uid,
                    content: content.into(),
                    projection: text(&input["projection"], 65_536)?,
                    revision: old.as_ref().map_or(0, |o| o.revision) + 1,
                    modified_at: at,
                };
                if old.is_some() {
                    ctx.db.pim_resources().id().update(row);
                } else {
                    count += 1;
                    if count > 10_000 {
                        return fail("operationTooLarge");
                    }
                    if ctx.db.pim_resources().href_key().find(row.href_key.clone()).is_some() {
                        continue;
                    }
                    ctx.db.pim_resources().insert(row);
                }
                changed += 1;
            }
            if changed > 0 {
                ctx.db.pim_collections().id().update(PimCollection { revision: c.revision + 1, modified_at: at, ..c.clone() });
                notify(ctx, &c);
            }
            Ok(json!({ "changed": changed }))
        }
        "finish" => {
            let mut changed = false;
            if let Some(uids) = p.get("uids").and_then(Value::as_array) {
                if uids.len() > 10_000 {
                    return fail("operationTooLarge");
                }
                let keep: std::collections::HashSet<String> = uids.iter().map(|v| crate::text_of(Some(v))).collect();
                for row in ctx.db.pim_resources().collection_id().filter(c.id.as_str()).collect::<Vec<_>>() {
                    if !keep.contains(&row.uid) {
                        invalidate_reminders(ctx, &row.id);
                        ctx.db.pim_resources().id().delete(row.id);
                        changed = true;
                    }
                }
            }
            let error = p.get("error").and_then(Value::as_str).map(|e| clip(e, 500)).unwrap_or_default();
            // Failures retry within the hour; successes wait for the chosen interval.
            let wait = subscription.refresh_minutes.min(if error.is_empty() { subscription.refresh_minutes } else { 60 }) as f64 * 60_000.0;
            let header = |key: &str, old: &str| if !error.is_empty() { old.to_owned() } else { p.get(key).and_then(Value::as_str).map(|s| clip(s, 500)).unwrap_or_default() };
            let next = PimSubscription {
                last_sync_at: if error.is_empty() { at } else { subscription.last_sync_at },
                next_at: due_key(at + wait),
                etag: header("etag", &subscription.etag),
                last_modified: header("lastModified", &subscription.last_modified),
                last_error: error.clone(),
                ..subscription
            };
            ctx.db.pim_subscriptions().collection_id().update(next);
            if changed {
                ctx.db.pim_collections().id().update(PimCollection { revision: c.revision + 1, modified_at: at, ..c.clone() });
            }
            notify(ctx, &c);
            Ok(json!({ "ok": true }))
        }
        _ => fail("invalidInput"),
    }
}

pub fn pim_snapshot(ctx: &ReducerContext) -> Value {
    json!({
        "collections": ctx.db.pim_collections().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "resources": ctx.db.pim_resources().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "grants": ctx.db.pim_grants().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "preferences": ctx.db.pim_preferences().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "reminders": ctx.db.pim_reminders().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "cursors": ctx.db.pim_reminder_cursors().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
        "subscriptions": ctx.db.pim_subscriptions().iter().map(|r| crate::row(&r)).collect::<Vec<_>>(),
    })
}

pub fn restore_pim(ctx: &ReducerContext, data: Option<&Value>) -> Res<()> {
    let Some(data) = data.filter(|d| !d.is_null()) else { return Ok(()) };
    let rows = |key: &str| crate::identity::snapshot_rows::<Value>(data, key);
    for row in crate::identity::snapshot_rows::<PimCollection>(data, "collections")? {
        if ctx.db.accounts().id().find(row.owner_id.clone()).is_none() || !["contacts", "calendar", "notes"].contains(&row.kind.as_str()) {
            return fail("invalidInput");
        }
        ctx.db.pim_collections().insert(row);
    }
    for row in crate::identity::snapshot_rows::<PimResource>(data, "resources")? {
        if ctx.db.pim_collections().id().find(row.collection_id.clone()).is_none() || row.href_key != format!("{}/{}", row.collection_id, row.href) || row.uid_key != format!("{}/{}", row.collection_id, row.uid) {
            return fail("invalidInput");
        }
        ctx.db.pim_resources().insert(row);
    }
    for row in crate::identity::snapshot_rows::<PimGrant>(data, "grants")? {
        if ctx.db.pim_collections().id().find(row.collection_id.clone()).is_none() || ctx.db.accounts().id().find(row.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.pim_grants().insert(row);
    }
    for row in crate::identity::snapshot_rows::<PimPreference>(data, "preferences")? {
        if !["instance:appearance", "instance:apps"].contains(&row.user_id.as_str()) && ctx.db.accounts().id().find(row.user_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.pim_preferences().insert(row);
    }
    for row in crate::identity::snapshot_rows::<PimReminder>(data, "reminders")? {
        if ctx.db.accounts().id().find(row.user_id.clone()).is_none() || ctx.db.pim_resources().id().find(row.resource_id.clone()).is_none() {
            return fail("invalidInput");
        }
        ctx.db.pim_reminders().insert(row);
    }
    for row in crate::identity::snapshot_rows::<PimSubscription>(data, "subscriptions")? {
        let c = ctx.db.pim_collections().id().find(row.collection_id.clone());
        if !c.is_some_and(|c| c.kind == "calendar" && c.owner_id == row.owner_id) {
            return fail("invalidInput");
        }
        ctx.db.pim_subscriptions().insert(row);
    }
    for row in rows("cursors")? {
        let resource_id = crate::text_of(row.get("resourceId"));
        let (Some(revision), Some(at)) = (row.get("revision").and_then(Value::as_f64), row.get("at").and_then(Value::as_f64)) else { return fail("invalidInput") };
        if ctx.db.pim_resources().id().find(resource_id.clone()).is_none() {
            return fail("invalidInput");
        }
        let run_at = row.get("runAt").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| due_key(at));
        ctx.db.pim_reminder_cursors().insert(PimReminderCursor { resource_id, revision: revision as u32, at, run_at });
    }
    for row in ctx.db.pim_resources().iter().collect::<Vec<_>>() {
        let calendar = ctx.db.pim_collections().id().find(row.collection_id.clone()).is_some_and(|c| c.kind == "calendar");
        if calendar && ctx.db.pim_reminder_cursors().resource_id().find(row.id.clone()).is_none() {
            ctx.db.pim_reminder_cursors().insert(PimReminderCursor { resource_id: row.id.clone(), revision: row.revision, at: row.modified_at, run_at: due_key(row.modified_at) });
        }
    }
    Ok(())
}

#[spacetimedb::procedure]
pub fn service_pim_reminder_work(ctx: &mut ProcedureContext) -> String {
    crate::run_json(ctx, |tx| {
        service(tx)?;
        Ok(reminder_work(tx))
    })
}

#[spacetimedb::reducer]
pub fn service_pim_commit_reminders(ctx: &ReducerContext, resourceId: String, revision: u32, from: f64, to: f64, reminders: String) -> Res<()> {
    service(ctx)?;
    let reminders: Value = serde_json::from_str(&reminders).map_err(|_| "invalidInput")?;
    commit_reminders(ctx, &resourceId, revision, from, to, &reminders)
}

/// Calendar subscription sync; the runtime fetches, the module validates and stores.
#[spacetimedb::procedure]
pub fn service_pim_subscriptions(ctx: &mut ProcedureContext, operation: String, payload: String) -> String {
    crate::run_json(ctx, |tx| {
        service(tx)?;
        let p: Value = serde_json::from_str(&payload).map_err(|_| "invalidInput")?;
        pim_subscription_work(tx, &operation, &p)
    })
}

#[spacetimedb::reducer]
pub fn service_pim_defer_reminder(ctx: &ReducerContext, resourceId: String, revision: u32, from: f64) -> Res<()> {
    service(ctx)?;
    defer_reminder(ctx, &resourceId, revision, from);
    Ok(())
}
