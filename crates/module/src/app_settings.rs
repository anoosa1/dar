//! Instance app availability (`app-settings.ts`) and installed apps' per-user data (`apps.ts`).

use serde_json::{Value, json};
use spacetimedb::{ReducerContext, Table};

use crate::tables::*;
use crate::{Res, fail, now, row};

/// `{revision, disabled}` from the shared revisioned settings store.
pub fn app_settings(ctx: &ReducerContext) -> (u32, Vec<String>) {
    match ctx
        .db
        .pim_preferences()
        .user_id()
        .find("instance:apps".to_owned())
    {
        Some(r) => {
            let disabled = serde_json::from_str::<Value>(&r.value)
                .ok()
                .and_then(|v| {
                    v["disabled"].as_array().map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                })
                .unwrap_or_default();
            (r.revision, disabled)
        }
        None => (0, Vec::new()),
    }
}

pub fn app_settings_json(ctx: &ReducerContext) -> Value {
    let (revision, disabled) = app_settings(ctx);
    json!({ "revision": revision, "disabled": disabled })
}

pub fn app_enabled(ctx: &ReducerContext, id: &str) -> bool {
    !app_settings(ctx).1.iter().any(|d| d == id)
}

/// Apps an administrator cannot turn off (`essentialApps`).
pub const ESSENTIAL_APPS: [&str; 3] = ["files", "contacts", "calendar"];

pub fn valid_app_key(key: &str) -> bool {
    let b = key.as_bytes();
    !b.is_empty()
        && b.len() <= 100
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

pub fn valid_app_id(id: &str) -> bool {
    if id.len() > 120 {
        return false;
    }
    let parts: Vec<&str> = id.split('.').collect();
    (3..=8).contains(&parts.len())
        && parts.iter().all(|p| {
            p.as_bytes().first().is_some_and(|c| c.is_ascii_lowercase())
                && p.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// `encodeURIComponent(s).replace(/%[A-F\d]{2}/g, "x").length`: UTF-8 bytes, one per escape.
pub fn bytes(s: &str) -> usize {
    s.len()
}

fn tuple(items: &[&str]) -> String {
    serde_json::to_string(items).expect("tuple")
}

fn expose(r: &AppData) -> Value {
    json!({ "key": r.key, "revision": r.revision, "schemaVersion": r.schema_version, "value": serde_json::from_str::<Value>(&r.value).unwrap_or(Value::Null), "modifiedAt": r.modified_at })
}

#[allow(clippy::too_many_arguments)]
pub fn app_operation(
    ctx: &ReducerContext,
    user_id: &str,
    app_id: &str,
    operation: &str,
    key: &str,
    revision: u32,
    schema_version: u32,
    value: &str,
) -> Res<Value> {
    let permissions: Vec<String> = if let Some(app) = ctx
        .db
        .installed_apps()
        .id()
        .find(app_id.to_owned())
    {
        if !app_enabled(ctx, app_id) {
            return fail("notFound");
        }
        serde_json::from_str::<Value>(&app.document)
            .ok()
            .and_then(|d| {
                d["permissions"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    } else if app_id == "org.dar.prayer-times" && app_enabled(ctx, "prayers") {
        vec!["ownUserData:read".into(), "ownUserData:write".into()]
    } else {
        return fail("notFound");
    };
    let needed = if operation == "put" || operation == "delete" {
        "ownUserData:write"
    } else {
        "ownUserData:read"
    };
    if !permissions.iter().any(|p| p == needed) {
        return fail("forbidden");
    }
    let owner_key = tuple(&[app_id, user_id]);
    let id = tuple(&[app_id, user_id, key]);
    if operation == "list" {
        let mut rows: Vec<AppData> = ctx
            .db
            .app_data()
            .owner_key()
            .filter(&owner_key)
            .filter(|r| r.key.as_str() > key)
            .collect();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        let cursor = if rows.len() > 50 {
            json!(rows[49].key)
        } else {
            Value::Null
        };
        return Ok(
            json!({ "items": rows.iter().take(50).map(expose).collect::<Vec<_>>(), "cursor": cursor }),
        );
    }
    if !valid_app_key(key) {
        return fail("invalidInput");
    }
    let old = ctx.db.app_data().id().find(id.clone());
    if operation == "get" {
        return Ok(old.as_ref().map_or(Value::Null, expose));
    }
    if old.as_ref().map_or(0, |o| o.revision) != revision {
        return fail("conflict");
    }
    if operation == "delete" {
        if old.is_some() {
            ctx.db.app_data().id().delete(id);
        }
        return Ok(json!({ "ok": true }));
    }
    if operation != "put" || !(1..=1_000_000).contains(&schema_version) {
        return fail("invalidInput");
    }
    if serde_json::from_str::<Value>(value).is_err() {
        return fail("invalidInput");
    }
    if bytes(value) > 65536 {
        return fail("operationTooLarge");
    }
    let rows: Vec<AppData> = ctx.db.app_data().owner_key().filter(&owner_key).collect();
    let total: usize = rows.iter().map(|r| bytes(&r.value)).sum::<usize>()
        - old.as_ref().map_or(0, |o| bytes(&o.value))
        + bytes(value);
    if old.is_none() && rows.len() >= 128 || total > 1_048_576 {
        return fail("quotaExceeded");
    }
    let next = AppData {
        id: id.clone(),
        app_id: app_id.into(),
        owner_id: user_id.into(),
        owner_key,
        key: key.into(),
        revision: revision + 1,
        schema_version,
        value: value.into(),
        modified_at: now(ctx),
    };
    if old.is_some() {
        ctx.db.app_data().id().update(next.clone());
    } else {
        ctx.db.app_data().insert(next.clone());
    }
    Ok(expose(&next))
}

pub fn app_snapshot(ctx: &ReducerContext) -> Value {
    json!({ "inventory": ctx.db.installed_apps().iter().map(|r| row(&r)).collect::<Vec<_>>(), "data": ctx.db.app_data().iter().map(|r| row(&r)).collect::<Vec<_>>() })
}

/// Inventory is informational on restore: only configured packages can activate code.
pub fn restore_apps(ctx: &ReducerContext, value: Option<&Value>) -> Res<()> {
    let Some(value) = value else { return Ok(()) };
    for r in crate::identity::snapshot_rows::<AppData>(value, "data")? {
        let ok = valid_app_id(&r.app_id)
            && valid_app_key(&r.key)
            && ctx.db.accounts().id().find(r.owner_id.clone()).is_some()
            && r.id == tuple(&[&r.app_id, &r.owner_id, &r.key])
            && r.owner_key == tuple(&[&r.app_id, &r.owner_id])
            && r.revision >= 1
            && r.schema_version >= 1
            && bytes(&r.value) <= 65536
            && serde_json::from_str::<Value>(&r.value).is_ok();
        if !ok {
            return fail("invalidInput");
        }
        ctx.db.app_data().insert(r);
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn service_installed_apps(ctx: &ReducerContext, document: String) -> Res<()> {
    crate::service(ctx)?;
    let Ok(Value::Array(apps)) = serde_json::from_str::<Value>(&document) else {
        return fail("invalidInput");
    };
    if apps.len() > 100
        || apps
            .iter()
            .any(|a| !a["id"].as_str().is_some_and(valid_app_id))
    {
        return fail("invalidInput");
    }
    for old in ctx.db.installed_apps().iter().collect::<Vec<_>>() {
        ctx.db.installed_apps().id().delete(old.id);
    }
    for app in apps {
        ctx.db.installed_apps().insert(InstalledApp {
            id: app["id"].as_str().unwrap_or_default().into(),
            document: crate::stringify(&app),
        });
    }
    Ok(())
}

#[allow(non_snake_case)]
#[spacetimedb::procedure]
pub fn service_app_settings(
    ctx: &mut spacetimedb::ProcedureContext,
    sessionId: String,
    appId: String,
    enabled: bool,
    revision: u32,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let (current, disabled) = app_settings(tx);
        if appId.is_empty() {
            return Ok(app_settings_json(tx));
        }
        let user = crate::from_session(tx, &sessionId)?;
        if !user.administrator
            || tx
                .db
                .app_passwords()
                .session_id()
                .find(sessionId.clone())
                .is_some()
        {
            return fail("forbidden");
        }
        if ESSENTIAL_APPS.contains(&appId.as_str()) {
            return fail("forbidden");
        }
        if ![
            "notes", "tasks", "agent", "news", "mail", "chat", "photos", "books", "git", "prayers",
        ]
        .contains(&appId.as_str())
            && !valid_app_id(&appId)
        {
            return fail("invalidInput");
        }
        if current != revision {
            return fail("conflict");
        }
        let mut next: Vec<String> = disabled.into_iter().filter(|d| *d != appId).collect();
        if !enabled {
            next.push(appId.clone());
        }
        if next.len() > 200 {
            return fail("operationTooLarge");
        }
        let value = json!({ "disabled": next }).to_string();
        let record = PimPreference {
            user_id: "instance:apps".into(),
            revision: current + 1,
            value,
        };
        if tx
            .db
            .pim_preferences()
            .user_id()
            .find("instance:apps".to_owned())
            .is_some()
        {
            tx.db.pim_preferences().user_id().update(record);
        } else {
            tx.db.pim_preferences().insert(record);
        }
        Ok(json!({ "revision": current + 1, "disabled": next }))
    })
}

#[allow(non_snake_case, clippy::too_many_arguments)]
#[spacetimedb::procedure]
pub fn service_app_data(
    ctx: &mut spacetimedb::ProcedureContext,
    sessionId: String,
    appId: String,
    operation: String,
    key: String,
    revision: u32,
    schemaVersion: u32,
    value: String,
) -> String {
    crate::run_json(ctx, |tx| {
        crate::service(tx)?;
        let user = crate::from_session(tx, &sessionId)?;
        if tx
            .db
            .app_passwords()
            .session_id()
            .find(sessionId.clone())
            .is_some()
        {
            return fail("forbidden");
        }
        app_operation(
            tx,
            &user.id,
            &appId,
            &operation,
            &key,
            revision,
            schemaVersion,
            &value,
        )
    })
}
