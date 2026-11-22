//! Installed apps at run time (`runtime/apps/http.ts`, `apps/server.ts` and the app routes of
//! `server.ts`): the enabled packages' descriptors, their immutable assets, per-user app data
//! and the administrator's switches for built-in and installed apps.
//!
//! App server hooks are Node modules and this runtime does not load them: a package that
//! declares one reports a startup failure, as the Node runtime does when a hook fails.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::apps::{InventoryItem, Package, Registry, content_type};
use crate::db::Database;
use crate::error::{AppError, Result};

const BUILT_IN: [&str; 12] = ["files", "photos", "books", "contacts", "calendar", "notes", "tasks", "news", "mail", "chat", "agent", "git"];
const TOGGLES: [&str; 9] = ["notes", "tasks", "news", "mail", "chat", "photos", "books", "agent", "git"];
const STARTUP_FAILED: &str = "App startup failed. Disable or replace the package, then restart.";

pub struct Installed {
    /// Packages the host enabled at startup, by id; settings switch them on and off.
    packages: HashMap<String, Package>,
    host_enabled: HashSet<String>,
    state: RwLock<State>,
    /// Revisioned writes and their effects happen in request order.
    update: tokio::sync::Mutex<()>,
}

struct State {
    apps: HashMap<String, Package>,
    inventory: Vec<InventoryItem>,
}

impl Installed {
    pub fn new(registry: Registry) -> Self {
        let host_enabled = registry.apps.keys().cloned().collect();
        Installed { packages: registry.apps.clone(), host_enabled, state: RwLock::new(State { apps: registry.apps, inventory: registry.inventory }), update: tokio::sync::Mutex::new(()) }
    }

    fn set_enabled(&self, id: &str, enabled: bool) {
        let Some(app) = self.packages.get(id) else { return };
        let mut state = self.state.write().expect("apps");
        if !enabled {
            state.apps.remove(id);
            return;
        }
        let item = state.inventory.iter_mut().find(|i| i.id == id);
        if app.manifest.server.is_some() {
            if let Some(item) = item {
                item.status = STARTUP_FAILED.into();
            }
            state.apps.remove(id);
            return;
        }
        if let Some(item) = item {
            item.status = "Ready".into();
        }
        state.apps.insert(id.to_owned(), app.clone());
    }

    fn descriptors(&self) -> Value {
        let state = self.state.read().expect("apps");
        let mut apps: Vec<&Package> = state.apps.values().collect();
        apps.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Value::Array(apps.iter().map(|a| a.descriptor()).collect())
    }

    async fn publish(&self, db: &Database) -> Result<()> {
        db.call("service_installed_apps", json!([self.descriptors().to_string()])).await?;
        Ok(())
    }

    /// Startup: apps the settings turned off stay off; the module learns what is installed.
    pub async fn start(&self, db: &Database) -> Result<()> {
        let settings = settings(db).await?;
        let disabled = disabled_of(&settings);
        let ids: Vec<String> = self.packages.keys().cloned().collect();
        for id in ids {
            self.set_enabled(&id, !disabled.contains(&id));
        }
        self.publish(db).await
    }

    fn managed(&self, settings: &Value) -> Value {
        let disabled = disabled_of(settings);
        let state = self.state.read().expect("apps");
        let mut items: Vec<Value> = BUILT_IN
            .iter()
            .map(|id| {
                let mut name = id.to_string();
                name[..1].make_ascii_uppercase();
                json!({ "id": id, "name": name, "version": "", "permissions": [], "status": "", "enabled": !disabled.contains(*id), "canToggle": TOGGLES.contains(id) })
            })
            .collect();
        for item in &state.inventory {
            let mut v = serde_json::to_value(item).expect("inventory");
            v["enabled"] = json!(state.apps.contains_key(&item.id) && !disabled.contains(&item.id));
            v["canToggle"] = json!(self.host_enabled.contains(&item.id));
            items.push(v);
        }
        Value::Array(items)
    }

    /// An enabled package's file at its digest; server code is never served.
    pub fn asset(&self, method: &Method, path: &str) -> Result<Response> {
        let parts: Vec<&str> = path.split('/').collect();
        let state = self.state.read().expect("apps");
        let app = parts.get(2).and_then(|id| state.apps.get(*id));
        let name = parts.get(4..).map(|p| p.join("/")).unwrap_or_default();
        let bytes = app.filter(|a| parts.get(3) == Some(&a.digest.as_str())).and_then(|a| a.assets.get(&name));
        let (Some(app), Some(bytes)) = (app, bytes) else { return Err(AppError::not_found()) };
        if app.manifest.server.as_deref() == Some(name.as_str()) || (method != Method::GET && method != Method::HEAD) {
            return Err(AppError::not_found());
        }
        let body = if method == Method::HEAD { Bytes::new() } else { Bytes::from(bytes.clone()) };
        Ok(([(header::CONTENT_TYPE, content_type(&name)), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], body).into_response())
    }
}

pub async fn settings(db: &Database) -> Result<Value> {
    db.read("service_app_settings", json!(["", "", false, 0])).await
}

fn disabled_of(settings: &Value) -> HashSet<String> {
    settings["disabled"].as_array().map(|d| d.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// Whether the administrator switched this built-in app off.
pub async fn app_disabled(db: &Database, id: &str) -> Result<bool> {
    Ok(disabled_of(&settings(db).await?).contains(id))
}

fn send(value: &Value) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], value.to_string()).into_response()
}

/// `/api/v1/admin/installedApps`: the list, and switching one app on or off at a revision.
#[allow(clippy::too_many_arguments)]
pub async fn admin(installed: &Installed, db: &Database, method: &Method, headers: &HeaderMap, body: Body, session_id: &str, administrator: bool, on_change: impl AsyncFnOnce(&str, bool) -> Result<()>) -> Result<Response> {
    if !administrator {
        return Err(AppError::new("forbidden", 403));
    }
    if *method == Method::GET {
        let mut s = settings(db).await?;
        s["items"] = installed.managed(&s);
        return Ok(crate::http::json_ok(&s));
    }
    if *method != Method::PUT {
        return Err(AppError::new("methodNotAllowed", 405));
    }
    let p = crate::http::body_object(body, 16 * 1024, headers).await?;
    let id = crate::http::string(p.get("id"), 100)?;
    let revision = crate::http::integer(p.get("revision"), u32::MAX as f64)? as u32;
    let Some(enabled) = p.get("enabled").and_then(Value::as_bool) else { return Err(AppError::invalid()) };
    if headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) != Some(format!("\"apps:{revision}\"").as_str()) {
        return Err(AppError::new("preconditionRequired", 428));
    }
    let _turn = installed.update.lock().await;
    let current = settings(db).await?;
    if !installed.managed(&current).as_array().expect("items").iter().any(|a| a["id"] == json!(id) && a["canToggle"] == json!(true)) {
        return Err(AppError::new("forbidden", 403));
    }
    let mut next = db.call("service_app_settings", json!([session_id, id, enabled, revision])).await?;
    on_change(&id, enabled).await?;
    installed.set_enabled(&id, enabled);
    installed.publish(db).await?;
    next["items"] = installed.managed(&next);
    Ok(crate::http::json_ok(&next))
}

/// `/api/v1/apps` and `/api/v1/apps/<id>/userData[/<key>]` (`handleAppHttp`).
pub async fn handle(installed: &Installed, db: &Database, method: &Method, path: &str, query: &HashMap<String, String>, headers: &HeaderMap, body: Body, session_id: &str) -> Result<Response> {
    if path == "/api/v1/apps" && *method == Method::GET {
        return Ok(send(&json!({ "items": installed.descriptors() })));
    }
    let rest = path.strip_prefix("/api/v1/apps/").ok_or_else(AppError::not_found)?;
    let (app_id, tail) = rest.split_once('/').ok_or_else(AppError::not_found)?;
    if tail == "api" || tail.starts_with("api/") {
        // Only app server hooks answer here, and this runtime runs none.
        return Err(AppError::not_found());
    }
    let key = match tail.strip_prefix("userData") {
        Some("") => None,
        Some(k) if k.starts_with('/') && k.len() > 1 && !k[1..].contains('/') => Some(k[1..].to_owned()),
        _ => return Err(AppError::not_found()),
    };
    let manifest_id = {
        let state = installed.state.read().expect("apps");
        state.apps.get(app_id).map(|a| a.manifest.id.clone()).ok_or_else(AppError::not_found)?
    };
    let key_value = key.clone().or_else(|| query.get("cursor").cloned()).unwrap_or_default();
    let operation = match (method.clone(), key.is_some()) {
        (Method::GET, true) => "get",
        (Method::GET, false) => "list",
        (Method::PUT, true) => "put",
        (Method::DELETE, true) => "delete",
        _ => return Err(AppError::new("methodNotAllowed", 405)),
    };
    let (mut revision, mut schema_version, mut value) = (0u32, 1u32, String::new());
    if operation == "put" || operation == "delete" {
        let header = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).unwrap_or_default();
        let digits = header.strip_prefix('"').and_then(|h| h.strip_suffix('"')).filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
        let Some(digits) = digits else { return Err(AppError::new("preconditionRequired", 428)) };
        revision = digits.parse::<u64>().ok().filter(|r| *r <= u32::MAX as u64).ok_or_else(AppError::invalid)? as u32;
        if operation == "put" {
            let input = crate::http::body_object(body, 80 * 1024, headers).await?;
            let version = input.get("schemaVersion").and_then(Value::as_f64).filter(|v| v.fract() == 0.0 && *v >= 1.0 && *v <= u32::MAX as f64);
            if version.is_none() || !input.contains_key("value") || input.keys().any(|k| k != "schemaVersion" && k != "value") {
                return Err(AppError::invalid());
            }
            schema_version = version.expect("version") as u32;
            value = input["value"].to_string();
        }
    }
    let result = db.call("service_app_data", json!([session_id, manifest_id, operation, key_value, revision, schema_version, value])).await?;
    let mut response = send(&result);
    if let Some(r) = result.get("revision").and_then(Value::as_u64).filter(|r| *r != 0) {
        response.headers_mut().insert(header::ETAG, format!("\"{r}\"").parse().expect("etag"));
    }
    Ok(response)
}

