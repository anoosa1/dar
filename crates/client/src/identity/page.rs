//! Dar Settings and OIDC identity interaction pages (`identity.tsx`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use js_sys::Date;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wasm_bindgen::{JsCast, JsValue};

use crate::api::{self, BrowserSession, Req, format_bytes};
use crate::app::Brand;
use crate::dates::{local_date, local_date_time};
use crate::icons::Icon;
use crate::shell::WorkspaceHeader;
use crate::ui::{Dialog, Select, listen, unique_id, window};

use super::account::AccountSecurity;
use super::appearance::AppearanceSettings;
use super::installed_app_settings::InstalledAppsSettings;
use super::instance_appearance::InstanceAppearanceSettings;
use super::profile::ProfileSettings;
use super::regional::RegionalSettings;
use super::settings_overview::SettingsOverview;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const ENTERPRISE: &str = "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";

#[derive(Clone, Copy)]
struct Section {
    id: &'static str,
    label: &'static str,
    route: &'static str,
    icon: &'static str,
}

const OVERVIEW_SECTION: Section = Section {
    id: "Overview",
    label: "Overview",
    route: "overview",
    icon: "grid",
};

const PERSONAL_SECTIONS: [Section; 5] = [
    Section {
        id: "Appearance",
        label: "Appearance",
        route: "appearance",
        icon: "palette",
    },
    Section {
        id: "Profile",
        label: "Profile",
        route: "profile",
        icon: "user",
    },
    Section {
        id: "Account",
        label: "Security",
        route: "security",
        icon: "lock",
    },
    Section {
        id: "Regional settings",
        label: "Regional settings",
        route: "regional",
        icon: "globe",
    },
    Section {
        id: "Connected applications",
        label: "Connected applications",
        route: "connected",
        icon: "plug",
    },
];

const ADMIN_SECTIONS: [Section; 6] = [
    Section {
        id: "Instance appearance",
        label: "Appearance",
        route: "instanceAppearance",
        icon: "palette",
    },
    Section {
        id: "Installed apps",
        label: "Apps",
        route: "installedApps",
        icon: "grid",
    },
    Section {
        id: "Applications",
        label: "Applications",
        route: "applications",
        icon: "plug",
    },
    Section {
        id: "Directory",
        label: "Directory",
        route: "directory",
        icon: "contacts",
    },
    Section {
        id: "SCIM",
        label: "SCIM",
        route: "scim",
        icon: "refresh",
    },
    Section {
        id: "Signing keys",
        label: "Signing keys",
        route: "keys",
        icon: "key",
    },
];

fn all_sections(admin: bool) -> Vec<Section> {
    let mut out = vec![OVERVIEW_SECTION];
    out.extend_from_slice(&PERSONAL_SECTIONS);
    if admin {
        out.extend_from_slice(&ADMIN_SECTIONS);
    }
    out
}

fn initial_section(admin: bool) -> &'static str {
    let search = window().location().search().unwrap_or_default();
    let route = web_sys::UrlSearchParams::new_with_str(&search)
        .ok()
        .and_then(|p| p.get("section"));
    let Some(route) = route else {
        return "Overview";
    };
    all_sections(admin)
        .into_iter()
        .find(|s| s.route == route)
        .map(|s| s.id)
        .unwrap_or("Overview")
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Client {
    id: String,
    revision: u64,
    enabled: bool,
    name: String,
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    post_logout_redirect_uris: Vec<String>,
    #[serde(default)]
    scopes: String,
    #[serde(default)]
    require_pkce: bool,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Storage {
    user_id: String,
    used: f64,
    #[serde(default)]
    quota: f64,
}

#[derive(Default, Deserialize)]
struct Items<T> {
    #[serde(default)]
    items: Vec<T>,
}

#[derive(Clone, Debug)]
struct EditorState {
    kind: &'static str,
    item: Option<Value>,
}

#[derive(Clone)]
struct ConfirmationState {
    title: String,
    description: String,
    work: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone, Debug, Deserialize)]
struct SecretState {
    secret: String,
    #[serde(default)]
    id: Option<String>,
}

type ProofWork = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<(), api::RequestError>>>> + Send + Sync,
>;

#[derive(Clone)]
struct PendingState {
    work: ProofWork,
}

fn storage_summary(s: Option<&Storage>) -> String {
    let Some(s) = s else { return String::new() };
    if s.quota > 0.0 {
        format!("{} of {}", format_bytes(s.used), format_bytes(s.quota))
    } else {
        format!("{} used \u{00b7} Unlimited", format_bytes(s.used))
    }
}

fn format_gb(quota: f64) -> String {
    let gb = quota / GIB;
    let formatted = format!("{gb:.2}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn select_input(event: web_sys::FocusEvent) {
    if let Some(input) = event
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}

fn parse_ts(v: &Value) -> Option<f64> {
    v.as_str()
        .map(Date::parse)
        .or_else(|| v.as_f64())
        .filter(|t| !t.is_nan())
}

fn json_matches(value: &Value, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    value
        .to_string()
        .to_lowercase()
        .contains(&query.to_lowercase())
}

fn encode_uri(value: &str) -> String {
    format!("{}", js_sys::encode_uri_component(value))
}

fn first_char(s: &str) -> String {
    s.chars().next().map(|c| c.to_string()).unwrap_or_default()
}

/// Dar Settings and Identity administration page (`IdentitySettings`).
#[component]
pub fn IdentitySettings(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let admin = session.user.administrator;
    let current_user_id = StoredValue::new(session.user.id.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());

    let tab = RwSignal::new(initial_section(admin));
    let query = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);

    let clients: RwSignal<Vec<Client>> = RwSignal::new(Vec::new());
    let users: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let groups: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let credentials: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let connections: RwSignal<Vec<Value>> = RwSignal::new(Vec::new());
    let status: RwSignal<Option<Value>> = RwSignal::new(None);
    let storage: RwSignal<HashMap<String, Storage>> = RwSignal::new(HashMap::new());

    let editor: RwSignal<Option<EditorState>> = RwSignal::new(None);
    let confirmation: RwSignal<Option<ConfirmationState>> = RwSignal::new(None);
    let secret: RwSignal<Option<SecretState>> = RwSignal::new(None);
    let proof: RwSignal<Option<(String, f64)>> = RwSignal::new(None);
    let pending: RwSignal<Option<PendingState>> = RwSignal::new(None);
    let navigation = RwSignal::new(false);
    let navigation_id = unique_id();

    let refresh = move || async move {
        let connected = api::get::<Items<Value>>("/api/v1/identity/connections").await?;
        let _ = connections.try_set(connected.items);
        if admin {
            let (a, b, c, d, e, f) = futures::try_join!(
                api::get::<Items<Client>>("/api/v1/identity/clients"),
                api::get::<Items<Value>>("/api/v1/identity/users"),
                api::get::<Items<Value>>("/api/v1/identity/groups"),
                api::get::<Items<Value>>("/api/v1/identity/credentials"),
                api::get::<Value>("/api/v1/identity/status"),
                api::get::<Items<Storage>>("/api/v1/identity/storage"),
            )?;
            let map: HashMap<String, Storage> = f
                .items
                .into_iter()
                .map(|s| (s.user_id.clone(), s))
                .collect();
            let _ = storage.try_set(map);
            let _ = clients.try_set(a.items);
            let _ = users.try_set(b.items);
            let _ = groups.try_set(c.items);
            let _ = credentials.try_set(d.items);
            let _ = status.try_set(Some(e));
        }
        Ok::<(), api::RequestError>(())
    };

    spawn_local(async move {
        if let Err(cause) = refresh().await {
            let _ = error.try_set(cause.message);
        }
        let _ = loading.try_set(false);
    });

    listen(&window(), "popstate", false, move |_| {
        tab.set(initial_section(admin));
    });

    let run_work = move |fut: Pin<Box<dyn Future<Output = Result<(), api::RequestError>>>>| {
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        spawn_local(async move {
            match fut.await {
                Ok(()) => match refresh().await {
                    Ok(()) => {
                        let _ = notice.try_set("Changes saved.".to_owned());
                    }
                    Err(cause) => {
                        let _ = error.try_set(cause.message);
                    }
                },
                Err(cause) => {
                    if cause.status == 403 {
                        let _ = proof.try_set(None);
                    }
                    let _ = error.try_set(cause.message);
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let mutate = move |work: ProofWork| {
        if let Some((val, until)) = proof.get_untracked()
            && until > Date::now()
        {
            run_work(work(val));
        } else {
            pending.set(Some(PendingState { work }));
        }
    };

    let remove_item = move |path: String, version: String, title: String, description: String| {
        let work: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            confirmation.set(None);
            let path = path.clone();
            let version = version.clone();
            let token = csrf.get_value();
            let pf: ProofWork = Arc::new(move |p: String| {
                let path = path.clone();
                let version = version.clone();
                let token = token.clone();
                Box::pin(async move {
                    let mut hdrs = vec![("x-identity-proof", p.as_str())];
                    if !version.is_empty() {
                        hdrs.push(("if-match", version.as_str()));
                    }
                    api::request::<Value>(
                        &path,
                        Req {
                            method: "DELETE",
                            csrf: Some(&token),
                            headers: &hdrs,
                            ..Default::default()
                        },
                    )
                    .await
                    .map(|_| ())
                })
            });
            mutate(pf);
        });
        confirmation.set(Some(ConfirmationState {
            title,
            description,
            work,
        }));
    };

    let choose_section = Callback::new(move |id: &'static str| {
        tab.set(id);
        query.set(String::new());
        navigation.set(false);
        if let Some(sec) = all_sections(true).into_iter().find(|s| s.id == id)
            && let Ok(history) = window().history()
        {
            let _ = history.push_state_with_url(
                &JsValue::NULL,
                "",
                Some(&format!("/settings?section={}", sec.route)),
            );
        }
    });

    let save_editor = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(ed) = editor.get_untracked() else {
            return;
        };
        let Some(form_el) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        let Ok(form) = web_sys::FormData::new_with_form(&form_el) else {
            return;
        };
        let field = |name: &str| form.get(name).as_string().unwrap_or_default();
        let kind = ed.kind;
        let item = ed.item.clone();
        let token = csrf.get_value();
        let me_id = current_user_id.get_value();
        let storage_map = storage.get_untracked();

        // Collect all form values synchronously before spawning async work.
        let members: Vec<Value> = form
            .get_all("members")
            .iter()
            .filter_map(|v| v.as_string())
            .map(|value| json!({ "value": value }))
            .collect();
        let mut form_fields: HashMap<&'static str, String> = HashMap::new();
        for k in [
            "name",
            "type",
            "redirectUris",
            "postLogoutRedirectUris",
            "scopes",
            "enabled",
            "label",
            "scope",
            "days",
            "userName",
            "displayName",
            "givenName",
            "familyName",
            "emails",
            "organization",
            "department",
            "employeeNumber",
            "password",
            "storageLimit",
            "active",
        ] {
            form_fields.insert(k, field(k));
        }

        let work: ProofWork = Arc::new(move |p: String| {
            let item = item.clone();
            let token = token.clone();
            let me_id = me_id.clone();
            let storage_map = storage_map.clone();
            let members = members.clone();
            let fields = form_fields.clone();
            let get = move |k: &str| fields.get(k).cloned().unwrap_or_default();
            Box::pin(async move {
                if kind == "client" {
                    let ctype = get("type");
                    let require_pkce = ctype == "public"
                        || item.as_ref().and_then(|i| i["requirePkce"].as_bool()) != Some(false);
                    let split_lines = |s: String| -> Vec<String> {
                        s.lines()
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .collect()
                    };
                    let scopes: Vec<String> = get("scopes")
                        .split_whitespace()
                        .map(str::to_owned)
                        .collect();
                    let payload = json!({
                        "name": get("name"),
                        "type": ctype,
                        "requirePkce": require_pkce,
                        "redirectUris": split_lines(get("redirectUris")),
                        "postLogoutRedirectUris": split_lines(get("postLogoutRedirectUris")),
                        "scopes": scopes,
                        "enabled": get("enabled") == "on",
                    });
                    let (url, method, ver) = match &item {
                        Some(it) => {
                            let id = it["id"].as_str().unwrap_or("");
                            let rev = it["revision"].as_u64().unwrap_or(0);
                            (
                                format!("/api/v1/identity/clients/{id}"),
                                "PUT",
                                Some(format!("W/\"{rev}\"")),
                            )
                        }
                        None => ("/api/v1/identity/clients".to_owned(), "POST", None),
                    };
                    let mut hdrs = vec![("x-identity-proof", p.as_str())];
                    if let Some(ref v) = ver {
                        hdrs.push(("if-match", v.as_str()));
                    }
                    let value = api::request::<Value>(
                        &url,
                        Req {
                            method,
                            body: Some(payload.to_string()),
                            csrf: Some(&token),
                            headers: &hdrs,
                            ..Default::default()
                        },
                    )
                    .await?;
                    if let Some(sec) = value["secret"].as_str() {
                        let _ = secret.try_set(Some(SecretState {
                            id: value["id"].as_str().map(str::to_owned),
                            secret: sec.to_owned(),
                        }));
                    }
                } else if kind == "credential" {
                    let days: f64 = get("days").parse().unwrap_or(30.0);
                    let payload = json!({
                        "label": get("label"),
                        "scope": get("scope"),
                        "days": days,
                    });
                    let hdrs = [("x-identity-proof", p.as_str())];
                    let res = api::request::<SecretState>(
                        "/api/v1/identity/credentials",
                        Req {
                            method: "POST",
                            body: Some(payload.to_string()),
                            csrf: Some(&token),
                            headers: &hdrs,
                            ..Default::default()
                        },
                    )
                    .await?;
                    let _ = secret.try_set(Some(res));
                } else {
                    let mut previous = item
                        .as_ref()
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_else(Map::new);
                    for drop_key in ["id", "meta", "groups", "schemas"] {
                        previous.remove(drop_key);
                    }
                    if kind == "user" {
                        let is_self =
                            item.as_ref().and_then(|i| i["id"].as_str()) == Some(me_id.as_str());
                        let active = if is_self {
                            item.as_ref()
                                .and_then(|i| i["active"].as_bool())
                                .unwrap_or(true)
                        } else {
                            get("active") == "on"
                        };
                        let mut name_obj = previous
                            .get("name")
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                        name_obj.insert("givenName".into(), get("givenName").into());
                        name_obj.insert("familyName".into(), get("familyName").into());
                        let emails: Vec<Value> = get("emails")
                            .lines()
                            .map(|l| l.trim().to_owned())
                            .filter(|l| !l.is_empty())
                            .enumerate()
                            .map(|(i, value)| json!({ "value": value, "primary": i == 0 }))
                            .collect();
                        let mut ent_obj = previous
                            .get(ENTERPRISE)
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                        ent_obj.insert("department".into(), get("department").into());
                        ent_obj.insert("organization".into(), get("organization").into());
                        ent_obj.insert("employeeNumber".into(), get("employeeNumber").into());

                        previous.insert("userName".into(), get("userName").into());
                        previous.insert("displayName".into(), get("displayName").into());
                        previous.insert("active".into(), active.into());
                        previous.insert("name".into(), Value::Object(name_obj));
                        previous.insert("emails".into(), Value::Array(emails));
                        previous.insert(ENTERPRISE.into(), Value::Object(ent_obj));
                        let pw = get("password");
                        if !pw.is_empty() {
                            previous.insert("password".into(), pw.into());
                        }
                    } else {
                        previous.insert("displayName".into(), get("displayName").into());
                        previous.insert("members".into(), Value::Array(members));
                    }

                    let resource = if kind == "user" { "users" } else { "groups" };
                    let (url, method, ver) = match &item {
                        Some(it) => {
                            let id = it["id"].as_str().unwrap_or("");
                            let ver = it["meta"]["version"].as_str().map(str::to_owned);
                            (format!("/api/v1/identity/{resource}/{id}"), "PUT", ver)
                        }
                        None => (format!("/api/v1/identity/{resource}"), "POST", None),
                    };
                    let mut hdrs = vec![("x-identity-proof", p.as_str())];
                    if let Some(ref v) = ver {
                        hdrs.push(("if-match", v.as_str()));
                    }
                    let saved = api::request::<Value>(
                        &url,
                        Req {
                            method,
                            body: Some(Value::Object(previous).to_string()),
                            csrf: Some(&token),
                            headers: &hdrs,
                            ..Default::default()
                        },
                    )
                    .await?;

                    if kind == "user" {
                        let raw_limit = get("storageLimit");
                        let trimmed = raw_limit.trim();
                        let quota: f64 = if !trimmed.is_empty() {
                            (trimmed.parse::<f64>().unwrap_or(0.0) * GIB).round()
                        } else {
                            0.0
                        };
                        let prev_quota = item
                            .as_ref()
                            .and_then(|it| it["id"].as_str())
                            .and_then(|id| storage_map.get(id))
                            .map(|s| s.quota)
                            .unwrap_or(0.0);
                        if (quota - prev_quota).abs() > 0.5
                            && let Some(saved_id) = saved["id"].as_str()
                        {
                            let body = if quota > 0.0 {
                                json!({ "quota": quota })
                            } else {
                                json!({ "quota": Value::Null })
                            };
                            let shdrs = [("x-identity-proof", p.as_str())];
                            api::request::<Value>(
                                &format!("/api/v1/identity/users/{}/storage", encode_uri(saved_id)),
                                Req {
                                    method: "PUT",
                                    body: Some(body.to_string()),
                                    csrf: Some(&token),
                                    headers: &shdrs,
                                    ..Default::default()
                                },
                            )
                            .await?;
                        }
                    }
                }
                let _ = editor.try_set(None);
                Ok(())
            })
        });
        mutate(work);
    };

    let session_sv = StoredValue::new(session.clone());
    let connections_count = Signal::derive(move || connections.with(Vec::len));
    let clients_count = Signal::derive(move || clients.with(Vec::len));
    let users_count = Signal::derive(move || users.with(Vec::len));
    let groups_count = Signal::derive(move || groups.with(Vec::len));

    view! {
        <div class="identity-settings">
            <WorkspaceHeader app="settings" session=session.clone() on_signed_out=on_signed_out/>
            <div class="ui-settings-layout">
                {move || navigation.get().then(|| view! {
                    <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| navigation.set(false)/>
                })}
                <aside id=navigation_id class=move || if navigation.get() { "sidebar ui-settings-nav is-open" } else { "sidebar ui-settings-nav" }>
                    <nav aria-label="Settings overview">
                        <button
                            aria-current=move || (tab.get() == "Overview").then_some("page")
                            on:click=move |_| choose_section.run("Overview")
                        >
                            <Icon name=OVERVIEW_SECTION.icon/>
                            <span>"Overview"</span>
                        </button>
                    </nav>
                    <nav aria-label="Personal settings">
                        <h2>"Personal"</h2>
                        {PERSONAL_SECTIONS.into_iter().map(|s| view! {
                            <button
                                aria-current=move || (tab.get() == s.id).then_some("page")
                                on:click=move |_| choose_section.run(s.id)
                            >
                                <Icon name=s.icon/>
                                <span>{s.label}</span>
                            </button>
                        }).collect_view()}
                    </nav>
                    {admin.then(|| view! {
                        <nav aria-label="Administration settings">
                            <h2>"Administration"</h2>
                            {ADMIN_SECTIONS.into_iter().map(|s| {
                                let aria = (s.id == "Instance appearance").then_some("Administration appearance");
                                view! {
                                    <button
                                        aria-label=aria
                                        aria-current=move || (tab.get() == s.id).then_some("page")
                                        on:click=move |_| choose_section.run(s.id)
                                    >
                                        <Icon name=s.icon/>
                                        <span>{s.label}</span>
                                    </button>
                                }
                            }).collect_view()}
                        </nav>
                    })}
                </aside>
                <main class="identity-main" aria-busy=move || (busy.get() || loading.get()).to_string()>
                    <div class="settings-content">
                        {move || {
                            let current = tab.get();
                            (current != "Overview").then(|| {
                                let ctx = if ADMIN_SECTIONS.iter().any(|s| s.id == current) {
                                    "Administration"
                                } else {
                                    "Personal settings"
                                };
                                view! { <p class="settings-context">{ctx}</p> }
                            })
                        }}
                        {move || (!error.get().is_empty()).then(|| view! {
                            <div class="identity-message">
                                <p role="alert" class="error">{error.get()}</p>
                                <button
                                    class="text-button"
                                    on:click=move |_| {
                                        loading.set(true);
                                        spawn_local(async move {
                                            busy.set(true);
                                            error.set(String::new());
                                            notice.set(String::new());
                                            match refresh().await {
                                                Ok(()) => { let _ = notice.try_set("Changes saved.".to_owned()); }
                                                Err(cause) => { let _ = error.try_set(cause.message); }
                                            }
                                            let _ = busy.try_set(false);
                                            let _ = loading.try_set(false);
                                        });
                                    }
                                >
                                    "Reload settings"
                                </button>
                            </div>
                        })}
                        {move || (!notice.get().is_empty()).then(|| view! { <p role="status" class="success-banner">{notice.get()}</p> })}
                        {move || {
                            if loading.get() {
                                return view! { <p role="status">"Loading settings\u{2026}"</p> }.into_any();
                            }
                            let current = tab.get();
                            let s = session_sv.get_value();
                            match current {
                                "Overview" => view! {
                                    <SettingsOverview
                                        session=s
                                        connections=connections_count
                                        clients=clients_count
                                        users=users_count
                                        groups=groups_count
                                        credentials=credentials
                                        status=status
                                        on_open=choose_section
                                    />
                                }.into_any(),
                                "Appearance" => view! { <AppearanceSettings session=s/> }.into_any(),
                                "Instance appearance" if admin => view! { <InstanceAppearanceSettings session=s/> }.into_any(),
                                "Regional settings" => view! { <RegionalSettings session=s/> }.into_any(),
                                "Profile" => view! { <ProfileSettings session=s/> }.into_any(),
                                "Account" => view! {
                                    <h1>"Account security"</h1>
                                    <AccountSecurity session=s on_signed_out=on_signed_out/>
                                }.into_any(),
                                "Connected applications" => {
                                    let q = query.get();
                                    let filtered: Vec<Value> = connections.get().into_iter().filter(|c| json_matches(c, &q)).collect();
                                    let empty = filtered.is_empty();
                                    view! {
                                        <section class="identity-section">
                                            <div class="identity-section-heading">
                                                <div>
                                                    <h1>"Connected applications"</h1>
                                                    <p class="muted">"Applications you have allowed to use your Dar identity."</p>
                                                </div>
                                            </div>
                                            {filtered.into_iter().map(|c| {
                                                let cid = c["id"].as_str().unwrap_or("").to_owned();
                                                let name_opt = c["name"].as_str().map(str::to_owned);
                                                let display = name_opt.clone().or_else(|| c["clientId"].as_str().map(str::to_owned)).unwrap_or_default();
                                                let symbol = first_char(name_opt.as_deref().unwrap_or("A"));
                                                let revoke_name = name_opt.unwrap_or_else(|| "This application".to_owned());
                                                let token = csrf.get_value();
                                                view! {
                                                    <div class="identity-row">
                                                        <span class="identity-app-symbol">{symbol}</span>
                                                        <div>
                                                            <strong>{display}</strong>
                                                            <small>"Account access"</small>
                                                        </div>
                                                        <button
                                                            class="text-button danger"
                                                            disabled=move || busy.get()
                                                            on:click=move |_| {
                                                                let cid = cid.clone();
                                                                let token = token.clone();
                                                                let desc = format!("{revoke_name} will need your consent again. Its own signed-in session may remain active.");
                                                                confirmation.set(Some(ConfirmationState {
                                                                    title: "Revoke access?".to_owned(),
                                                                    description: desc,
                                                                    work: Arc::new(move || {
                                                                        confirmation.set(None);
                                                                        let cid = cid.clone();
                                                                        let token = token.clone();
                                                                        run_work(Box::pin(async move {
                                                                            api::request::<Value>(
                                                                                &format!("/api/v1/identity/connections/{}", encode_uri(&cid)),
                                                                                Req { method: "DELETE", csrf: Some(&token), ..Default::default() },
                                                                            )
                                                                            .await
                                                                            .map(|_| ())
                                                                        }));
                                                                    }),
                                                                }));
                                                            }
                                                        >
                                                            "Revoke"
                                                        </button>
                                                    </div>
                                                }
                                            }).collect_view()}
                                            {empty.then(|| view! {
                                                <p class="identity-empty">{format!("No connected applications{}.", if !q.is_empty() { " match your search" } else { " yet" })}</p>
                                            })}
                                        </section>
                                    }.into_any()
                                }
                                "Applications" if admin => {
                                    let q = query.get();
                                    let filtered: Vec<Client> = clients
                                        .get()
                                        .into_iter()
                                        .filter(|c| {
                                            if q.is_empty() {
                                                return true;
                                            }
                                            let text = format!("{} {} {} {}", c.id, c.name, c.scopes, c.redirect_uris.join(" "));
                                            text.to_lowercase().contains(&q.to_lowercase())
                                        })
                                        .collect();
                                    let empty = filtered.is_empty();
                                    let issuer = status.with(|st| st.as_ref().and_then(|v| v["issuer"].as_str()).unwrap_or("").to_owned());
                                    view! {
                                        <section class="identity-section">
                                            <div class="identity-section-heading">
                                                <div>
                                                    <h1>"Registered applications"</h1>
                                                    <p class="muted">"Choose which applications can use Dar for sign-in."</p>
                                                </div>
                                                <button class="primary-button" on:click=move |_| editor.set(Some(EditorState { kind: "client", item: None }))>"Register application"</button>
                                            </div>
                                            {filtered.into_iter().map(|c| {
                                                let symbol = first_char(&c.name);
                                                let subtitle = format!(
                                                    "{} \u{00b7} {}",
                                                    if c.kind == "public" { "Public client" } else { "Confidential client" },
                                                    if c.require_pkce { "PKCE required" } else { "PKCE optional" }
                                                );
                                                let redirects = c.redirect_uris.join("\n");
                                                let is_public = c.kind == "public";
                                                let c_del = c.clone();
                                                let c_rot = c.clone();
                                                let c_edit = json!({
                                                    "id": c.id,
                                                    "revision": c.revision,
                                                    "enabled": c.enabled,
                                                    "name": c.name,
                                                    "redirectUris": c.redirect_uris,
                                                    "postLogoutRedirectUris": c.post_logout_redirect_uris,
                                                    "scopes": c.scopes,
                                                    "requirePkce": c.require_pkce,
                                                    "type": c.kind,
                                                });
                                                let token = csrf.get_value();
                                                view! {
                                                    <article class="identity-card">
                                                        <div class="identity-row">
                                                            <span class="identity-app-symbol">{symbol}</span>
                                                            <div>
                                                                <strong>{c.name.clone()}</strong>
                                                                <small>{subtitle}</small>
                                                            </div>
                                                            <span class=if c.enabled { "identity-badge enabled" } else { "identity-badge " }>
                                                                {if c.enabled { "Enabled" } else { "Disabled" }}
                                                            </span>
                                                        </div>
                                                        <dl>
                                                            <dt>"Client ID"</dt>
                                                            <dd>{c.id.clone()}</dd>
                                                            <dt>"Redirects"</dt>
                                                            <dd>{redirects}</dd>
                                                            <dt>"Permissions"</dt>
                                                            <dd>{c.scopes.clone()}</dd>
                                                        </dl>
                                                        <div class="dialog-actions">
                                                            <button
                                                                class="text-button danger"
                                                                disabled=move || busy.get()
                                                                on:click=move |_| {
                                                                    remove_item(
                                                                        format!("/api/v1/identity/clients/{}", c_del.id),
                                                                        format!("W/\"{}\"", c_del.revision),
                                                                        "Delete application?".to_owned(),
                                                                        format!("Remove {} and revoke its Dar grants?", c_del.name),
                                                                    );
                                                                }
                                                            >
                                                                "Delete"
                                                            </button>
                                                            {(!is_public).then(|| view! {
                                                                <button
                                                                    class="secondary-button"
                                                                    disabled=move || busy.get()
                                                                    on:click=move |_| {
                                                                        let cid = c_rot.id.clone();
                                                                        let ver = format!("W/\"{}\"", c_rot.revision);
                                                                        let token = token.clone();
                                                                        confirmation.set(Some(ConfirmationState {
                                                                            title: "Rotate client secret?".to_owned(),
                                                                            description: "The previous secret and existing grants stop working. Update the application with the new secret.".to_owned(),
                                                                            work: Arc::new(move || {
                                                                                confirmation.set(None);
                                                                                let cid = cid.clone();
                                                                                let ver = ver.clone();
                                                                                let token = token.clone();
                                                                                let pf: ProofWork = Arc::new(move |p: String| {
                                                                                    let cid = cid.clone();
                                                                                    let ver = ver.clone();
                                                                                    let token = token.clone();
                                                                                    Box::pin(async move {
                                                                                        let hdrs = [("x-identity-proof", p.as_str()), ("if-match", ver.as_str())];
                                                                                        let res = api::request::<SecretState>(
                                                                                            &format!("/api/v1/identity/clients/{cid}/secret"),
                                                                                            Req { method: "POST", csrf: Some(&token), headers: &hdrs, ..Default::default() },
                                                                                        )
                                                                                        .await?;
                                                                                        let _ = secret.try_set(Some(res));
                                                                                        Ok(())
                                                                                    })
                                                                                });
                                                                                mutate(pf);
                                                                            }),
                                                                        }));
                                                                    }
                                                                >
                                                                    "Rotate secret"
                                                                </button>
                                                            })}
                                                            <button
                                                                class="secondary-button"
                                                                on:click=move |_| editor.set(Some(EditorState { kind: "client", item: Some(c_edit.clone()) }))
                                                            >
                                                                "Edit application"
                                                            </button>
                                                        </div>
                                                    </article>
                                                }
                                            }).collect_view()}
                                            {empty.then(|| view! {
                                                <p class="identity-empty">{format!("No registered applications{}.", if !q.is_empty() { " match your search" } else { " yet" })}</p>
                                            })}
                                            <label>
                                                "Issuer address"
                                                <input readonly=true prop:value=issuer on:focus=select_input/>
                                            </label>
                                        </section>
                                    }.into_any()
                                }
                                "Directory" if admin => {
                                    let q = query.get();
                                    let filtered_users: Vec<Value> = users.get().into_iter().filter(|u| json_matches(u, &q)).collect();
                                    let filtered_groups: Vec<Value> = groups.get().into_iter().filter(|g| json_matches(g, &q)).collect();
                                    let users_empty = filtered_users.is_empty();
                                    let groups_empty = filtered_groups.is_empty();
                                    let storage_map = storage.get();
                                    let me_id = current_user_id.get_value();
                                    view! {
                                        <div class="identity-section-heading">
                                            <div>
                                                <h1>"Directory"</h1>
                                                <p class="muted">"Native accounts and groups. Groups do not grant administrator or app permissions."</p>
                                            </div>
                                            <div class="dialog-actions">
                                                <button class="secondary-button" on:click=move |_| editor.set(Some(EditorState { kind: "group", item: None }))>"Create group"</button>
                                                <button class="primary-button" on:click=move |_| editor.set(Some(EditorState { kind: "user", item: None }))>"Create user"</button>
                                            </div>
                                        </div>
                                        <section class="identity-section">
                                            <h2>"People"</h2>
                                            {filtered_users.into_iter().map(|u| {
                                                let uid = u["id"].as_str().unwrap_or("").to_owned();
                                                let uname = u["userName"].as_str().unwrap_or("").to_owned();
                                                let dname = u["displayName"].as_str().unwrap_or("").to_owned();
                                                let active = u["active"].as_bool().unwrap_or(false);
                                                let ver = u["meta"]["version"].as_str().unwrap_or("").to_owned();
                                                let symbol = first_char(&dname);
                                                let summary = storage_summary(storage_map.get(&uid));
                                                let sub = format!("@{uname} \u{00b7} {} \u{00b7} {summary}", if active { "Active" } else { "Suspended" });
                                                let protected = uid == me_id || uname == "admin";
                                                let u_edit = u.clone();
                                                let uname_btn = uname.clone();
                                                view! {
                                                    <div class="identity-row">
                                                        <span class="identity-app-symbol">{symbol}</span>
                                                        <div>
                                                            <strong>{dname.clone()}</strong>
                                                            <small>{sub}</small>
                                                        </div>
                                                        {if !protected {
                                                            let uid_del = uid.clone();
                                                            let ver_del = ver.clone();
                                                            let dname_del = dname.clone();
                                                            view! {
                                                                <button class="text-button" on:click=move |_| editor.set(Some(EditorState { kind: "user", item: Some(u_edit.clone()) }))>
                                                                    {format!("Edit {uname_btn}")}
                                                                </button>
                                                                <button
                                                                    class="text-button danger"
                                                                    disabled=move || busy.get()
                                                                    on:click=move |_| {
                                                                        remove_item(
                                                                            format!("/api/v1/identity/users/{uid_del}"),
                                                                            ver_del.clone(),
                                                                            "Delete directory user?".to_owned(),
                                                                            format!("Disable {dname_del} and remove their directory identity. Their files, contacts and calendars are retained. The username stays reserved."),
                                                                        );
                                                                    }
                                                                >
                                                                    "Delete"
                                                                </button>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <small>"Protected operator"</small>
                                                                <button class="text-button" on:click=move |_| editor.set(Some(EditorState { kind: "user", item: Some(u_edit.clone()) }))>
                                                                    "Edit profile"
                                                                </button>
                                                            }.into_any()
                                                        }}
                                                    </div>
                                                }
                                            }).collect_view()}
                                            {users_empty.then(|| view! { <p class="identity-empty">"No people match your search."</p> })}
                                        </section>
                                        <section class="identity-section">
                                            <h2>"Groups"</h2>
                                            {filtered_groups.into_iter().map(|g| {
                                                let gid = g["id"].as_str().unwrap_or("").to_owned();
                                                let gname = g["displayName"].as_str().unwrap_or("").to_owned();
                                                let ver = g["meta"]["version"].as_str().unwrap_or("").to_owned();
                                                let count = g["members"].as_array().map(Vec::len).unwrap_or(0);
                                                let g_edit = g.clone();
                                                let gname_btn = gname.clone();
                                                view! {
                                                    <div class="identity-row">
                                                        <div>
                                                            <strong>{gname}</strong>
                                                            <small>{format!("{count} members")}</small>
                                                        </div>
                                                        <button class="text-button" on:click=move |_| editor.set(Some(EditorState { kind: "group", item: Some(g_edit.clone()) }))>
                                                            {format!("Edit {gname_btn}")}
                                                        </button>
                                                        <button
                                                            class="text-button danger"
                                                            disabled=move || busy.get()
                                                            on:click=move |_| {
                                                                remove_item(
                                                                    format!("/api/v1/identity/groups/{gid}"),
                                                                    ver.clone(),
                                                                    "Delete group?".to_owned(),
                                                                    "Remove this directory group and its memberships?".to_owned(),
                                                                );
                                                            }
                                                        >
                                                            "Delete"
                                                        </button>
                                                    </div>
                                                }
                                            }).collect_view()}
                                            {groups_empty.then(|| view! {
                                                <p class="identity-empty">{format!("No groups{}.", if !q.is_empty() { " match your search" } else { " yet" })}</p>
                                            })}
                                        </section>
                                    }.into_any()
                                }
                                "SCIM" if admin => {
                                    let q = query.get();
                                    let filtered: Vec<Value> = credentials.get().into_iter().filter(|c| json_matches(c, &q)).collect();
                                    let empty = filtered.is_empty();
                                    let scim_url = status.with(|st| st.as_ref().and_then(|v| v["scimUrl"].as_str()).unwrap_or("").to_owned());
                                    let now = Date::now();
                                    view! {
                                        <section class="identity-section">
                                            <div class="identity-section-heading">
                                                <div>
                                                    <h1>"SCIM provisioning"</h1>
                                                    <p class="muted">"Dedicated, revocable credentials for directory provisioning."</p>
                                                </div>
                                                <button class="primary-button" on:click=move |_| editor.set(Some(EditorState { kind: "credential", item: None }))>"Create SCIM credential"</button>
                                            </div>
                                            <label>
                                                "SCIM base URL"
                                                <input prop:value=scim_url readonly=true on:focus=select_input/>
                                            </label>
                                            {filtered.into_iter().map(|c| {
                                                let cid = c["id"].as_str().unwrap_or("").to_owned();
                                                let label = c["label"].as_str().unwrap_or("").to_owned();
                                                let scope_text = if c["scope"].as_str() == Some("write") { "Read and write" } else { "Read only" };
                                                let revoked = c["revoked"].as_bool().unwrap_or(false);
                                                let exp_ts = parse_ts(&c["expiresAt"]).unwrap_or(0.0);
                                                let exp_str = local_date(exp_ts, &[]);
                                                let state_text = if revoked { "Revoked" } else if exp_ts < now { "Expired" } else { "Active" };
                                                let can_revoke = !revoked && exp_ts > now;
                                                let token = csrf.get_value();
                                                view! {
                                                    <div class="identity-row">
                                                        <div>
                                                            <strong>{label}</strong>
                                                            <small>{format!("{scope_text} \u{00b7} Expires {exp_str} \u{00b7} {state_text}")}</small>
                                                        </div>
                                                        {can_revoke.then(|| view! {
                                                            <button
                                                                class="text-button danger"
                                                                disabled=move || busy.get()
                                                                on:click=move |_| {
                                                                    let cid = cid.clone();
                                                                    let token = token.clone();
                                                                    confirmation.set(Some(ConfirmationState {
                                                                        title: "Revoke SCIM credential?".to_owned(),
                                                                        description: "Provisioning clients using this credential will lose access.".to_owned(),
                                                                        work: Arc::new(move || {
                                                                            confirmation.set(None);
                                                                            let cid = cid.clone();
                                                                            let token = token.clone();
                                                                            let pf: ProofWork = Arc::new(move |p: String| {
                                                                                let cid = cid.clone();
                                                                                let token = token.clone();
                                                                                Box::pin(async move {
                                                                                    let hdrs = [("x-identity-proof", p.as_str())];
                                                                                    api::request::<Value>(
                                                                                        &format!("/api/v1/identity/credentials/{cid}"),
                                                                                        Req { method: "DELETE", csrf: Some(&token), headers: &hdrs, ..Default::default() },
                                                                                    )
                                                                                    .await
                                                                                    .map(|_| ())
                                                                                })
                                                                            });
                                                                            mutate(pf);
                                                                        }),
                                                                    }));
                                                                }
                                                            >
                                                                "Revoke"
                                                            </button>
                                                        })}
                                                    </div>
                                                }
                                            }).collect_view()}
                                            {empty.then(|| view! {
                                                <p class="identity-empty">{format!("No SCIM credentials{}.", if !q.is_empty() { " match your search" } else { " yet" })}</p>
                                            })}
                                        </section>
                                    }.into_any()
                                }
                                "Signing keys" if admin => {
                                    let st = status.get();
                                    let active_kid = st.as_ref().and_then(|v| v["keys"]["activeKid"].as_str()).unwrap_or("").to_owned();
                                    let rotated_at = st.as_ref().and_then(|v| parse_ts(&v["keys"]["rotatedAt"])).map(|ts| local_date_time(ts, &[])).unwrap_or_default();
                                    let retained: Vec<(String, String)> = st
                                        .as_ref()
                                        .and_then(|v| v["keys"]["retained"].as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .map(|k| {
                                                    let kid = k["kid"].as_str().unwrap_or("").to_owned();
                                                    let retire = parse_ts(&k["retireAt"]).map(|ts| local_date_time(ts, &[])).unwrap_or_default();
                                                    (kid, retire)
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    let token = csrf.get_value();
                                    view! {
                                        <section class="identity-section">
                                            <h1>"OIDC signing keys"</h1>
                                            <p class="muted">"External applications verify Dar\u{2019}s ID tokens using these public keys."</p>
                                            <dl>
                                                <dt>"Active key"</dt>
                                                <dd>{active_kid}</dd>
                                                <dt>"Last rotated"</dt>
                                                <dd>{rotated_at}</dd>
                                            </dl>
                                            {retained.into_iter().map(|(kid, retire)| view! {
                                                <p class="small muted">{format!("{kid} \u{00b7} retained until {retire}")}</p>
                                            }).collect_view()}
                                            <p class="muted">"Previous keys remain published for ten minutes so unexpired ID tokens can be verified."</p>
                                            <button
                                                class="secondary-button"
                                                disabled=move || busy.get()
                                                on:click=move |_| {
                                                    let token = token.clone();
                                                    confirmation.set(Some(ConfirmationState {
                                                        title: "Rotate signing key?".to_owned(),
                                                        description: "New ID tokens will use a fresh key. Connected applications must refresh Dar\u{2019}s public key set.".to_owned(),
                                                        work: Arc::new(move || {
                                                            confirmation.set(None);
                                                            let token = token.clone();
                                                            let pf: ProofWork = Arc::new(move |p: String| {
                                                                let token = token.clone();
                                                                Box::pin(async move {
                                                                    let hdrs = [("x-identity-proof", p.as_str())];
                                                                    api::request::<Value>(
                                                                        "/api/v1/identity/keys/rotate",
                                                                        Req { method: "POST", csrf: Some(&token), headers: &hdrs, ..Default::default() },
                                                                    )
                                                                    .await
                                                                    .map(|_| ())
                                                                })
                                                            });
                                                            mutate(pf);
                                                        }),
                                                    }));
                                                }
                                            >
                                                "Rotate signing key"
                                            </button>
                                        </section>
                                    }.into_any()
                                }
                                _ => ().into_any(),
                            }
                        }}
                        {move || (admin && tab.get() == "Installed apps").then(|| view! {
                            <InstalledAppsSettings session=session_sv.get_value()/>
                        })}
                    </div>
                </main>
            </div>

            {move || editor.get().map(|ed| {
                let kind = ed.kind;
                let item = StoredValue::new(ed.item.clone());
                let title = if ed.item.is_some() {
                    format!("Edit {kind}")
                } else if kind == "client" {
                    "Register application".to_owned()
                } else if kind == "credential" {
                    "Create SCIM credential".to_owned()
                } else {
                    format!("Create {kind}")
                };
                view! {
                    <Dialog
                        title=title
                        on_close=Callback::new(move |_| if !busy.get_untracked() { editor.set(None); })
                    >
                        <form on:submit=save_editor>
                            {move || {
                                let it = item.get_value();
                                match kind {
                                    "client" => {
                                        let name = it.as_ref().and_then(|v| v["name"].as_str()).unwrap_or("").to_owned();
                                        let ctype = if it.as_ref().and_then(|v| v["type"].as_str()) == Some("public") { "public" } else { "confidential" };
                                        let type_sig = RwSignal::new(ctype.to_owned());
                                        let redirects = it.as_ref().and_then(|v| v["redirectUris"].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                        let post_logout = it.as_ref().and_then(|v| v["postLogoutRedirectUris"].as_array()).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                        let scopes = it.as_ref().and_then(|v| v["scopes"].as_str()).unwrap_or("openid profile email").to_owned();
                                        let enabled = it.as_ref().and_then(|v| v["enabled"].as_bool()).unwrap_or(true);
                                        let pkce_hint = if it.as_ref().and_then(|v| v["requirePkce"].as_bool()) == Some(false) {
                                            "PKCE is optional for this confidential client."
                                        } else {
                                            "PKCE is required."
                                        };
                                        view! {
                                            <label>
                                                "Application name"
                                                <input name="name" autofocus required maxlength="100" value=name/>
                                            </label>
                                            <label>
                                                "Client type"
                                                <Select
                                                    name="type"
                                                    value=type_sig
                                                    options=vec![
                                                        ("confidential".into(), "Confidential \u{00b7} server application".into()),
                                                        ("public".into(), "Public \u{00b7} browser or local client".into()),
                                                    ]
                                                />
                                            </label>
                                            <label>
                                                "Redirect URLs"
                                                <textarea aria-label="Redirect URLs" name="redirectUris" required rows="3" placeholder="https://app.example.com/callback">{redirects}</textarea>
                                                <small>{format!("One exact URL per line. {pkce_hint}")}</small>
                                            </label>
                                            <label>
                                                "Post-logout redirect URLs"
                                                <textarea name="postLogoutRedirectUris" rows="2">{post_logout}</textarea>
                                            </label>
                                            <label>
                                                "Allowed permissions"
                                                <input aria-label="Allowed permissions" name="scopes" required value=scopes/>
                                                <small>"openid, profile, email, groups, offline_access"</small>
                                            </label>
                                            <label class="identity-check">
                                                <input type="checkbox" name="enabled" checked=enabled/>
                                                "Enabled"
                                            </label>
                                        }.into_any()
                                    }
                                    "user" => {
                                        let is_edit = it.is_some();
                                        let is_self = it.as_ref().and_then(|v| v["id"].as_str()) == Some(current_user_id.get_value().as_str());
                                        let uname = it.as_ref().and_then(|v| v["userName"].as_str()).unwrap_or("").to_owned();
                                        let dname = it.as_ref().and_then(|v| v["displayName"].as_str()).unwrap_or("").to_owned();
                                        let gname = it.as_ref().and_then(|v| v["name"]["givenName"].as_str()).unwrap_or("").to_owned();
                                        let fname = it.as_ref().and_then(|v| v["name"]["familyName"].as_str()).unwrap_or("").to_owned();
                                        let emails = it.as_ref().and_then(|v| v["emails"].as_array()).map(|a| a.iter().filter_map(|e| e["value"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                        let org = it.as_ref().and_then(|v| v[ENTERPRISE]["organization"].as_str()).unwrap_or("").to_owned();
                                        let dept = it.as_ref().and_then(|v| v[ENTERPRISE]["department"].as_str()).unwrap_or("").to_owned();
                                        let emp = it.as_ref().and_then(|v| v[ENTERPRISE]["employeeNumber"].as_str()).unwrap_or("").to_owned();
                                        let pw_label = if is_edit { "Replacement password (optional)" } else { "Local password (optional)" };
                                        let quota_default = it
                                            .as_ref()
                                            .and_then(|v| v["id"].as_str())
                                            .and_then(|id| storage.with_untracked(|m| m.get(id).map(|s| s.quota)))
                                            .filter(|q| *q > 0.0)
                                            .map(format_gb)
                                            .unwrap_or_default();
                                        let active = it.as_ref().and_then(|v| v["active"].as_bool()).unwrap_or(true);
                                        view! {
                                            <label>
                                                "Username"
                                                <input readonly=is_self name="userName" autofocus required minlength="3" maxlength="64" pattern="[a-zA-Z0-9][a-zA-Z0-9_.\\-]{2,63}" value=uname/>
                                            </label>
                                            <label>
                                                "Display name"
                                                <input name="displayName" required maxlength="100" value=dname/>
                                            </label>
                                            <div class="identity-form-grid">
                                                <label>
                                                    "Given name"
                                                    <input name="givenName" value=gname/>
                                                </label>
                                                <label>
                                                    "Family name"
                                                    <input name="familyName" value=fname/>
                                                </label>
                                            </div>
                                            <label>
                                                "Email addresses"
                                                <textarea aria-label="Email addresses" name="emails" rows="2">{emails}</textarea>
                                                <small>"One address per line. The first is primary; addresses are not marked verified."</small>
                                            </label>
                                            <div class="identity-form-grid">
                                                <label>
                                                    "Organization"
                                                    <input name="organization" value=org/>
                                                </label>
                                                <label>
                                                    "Department"
                                                    <input name="department" value=dept/>
                                                </label>
                                            </div>
                                            <label>
                                                "Employee number"
                                                <input name="employeeNumber" value=emp/>
                                            </label>
                                            <label>
                                                {pw_label}
                                                <input aria-label=pw_label disabled=is_self name="password" type="password" autocomplete="new-password" minlength="15" maxlength="1024"/>
                                                <small>"Without a password, a new user cannot sign in locally. Password changes revoke all access."</small>
                                            </label>
                                            <label>
                                                "Storage limit (GB)"
                                                <input aria-label="Storage limit (GB)" name="storageLimit" type="number" inputmode="decimal" min="0.1" step="any" placeholder="Unlimited" value=quota_default/>
                                                <small>"Leave blank for unlimited storage. Lowering it below current use blocks new uploads but keeps existing files."</small>
                                            </label>
                                            <label class="identity-check">
                                                <input disabled=is_self type="checkbox" name="active" checked=active/>
                                                "Active account"
                                            </label>
                                        }.into_any()
                                    }
                                    "group" => {
                                        let gname = it.as_ref().and_then(|v| v["displayName"].as_str()).unwrap_or("").to_owned();
                                        let member_ids: Vec<String> = it
                                            .as_ref()
                                            .and_then(|v| v["members"].as_array())
                                            .map(|a| a.iter().filter_map(|m| m["value"].as_str().map(str::to_owned)).collect())
                                            .unwrap_or_default();
                                        let user_list = users.get();
                                        view! {
                                            <label>
                                                "Group name"
                                                <input name="displayName" autofocus required maxlength="100" value=gname/>
                                            </label>
                                            <fieldset class="identity-members">
                                                <legend>"Members"</legend>
                                                {user_list.into_iter().map(|u| {
                                                    let uid = u["id"].as_str().unwrap_or("").to_owned();
                                                    let dname = u["displayName"].as_str().unwrap_or("").to_owned();
                                                    let uname = u["userName"].as_str().unwrap_or("").to_owned();
                                                    let checked = member_ids.iter().any(|m| m == &uid);
                                                    view! {
                                                        <label class="identity-check">
                                                            <input type="checkbox" name="members" value=uid checked=checked/>
                                                            {format!("{dname} (@{uname})")}
                                                        </label>
                                                    }
                                                }).collect_view()}
                                            </fieldset>
                                        }.into_any()
                                    }
                                    _ => {
                                        let scope_sig = RwSignal::new("read".to_owned());
                                        view! {
                                            <label>
                                                "Credential name"
                                                <input name="label" autofocus required maxlength="100" placeholder="Directory sync"/>
                                            </label>
                                            <label>
                                                "Access"
                                                <Select
                                                    name="scope"
                                                    value=scope_sig
                                                    options=vec![
                                                        ("read".into(), "Read only".into()),
                                                        ("write".into(), "Read and write".into()),
                                                    ]
                                                />
                                            </label>
                                            <label>
                                                "Expires in days"
                                                <input name="days" type="number" min="1" max="365" required value="30"/>
                                            </label>
                                        }.into_any()
                                    }
                                }
                            }}
                            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                            <div class="dialog-actions">
                                <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| editor.set(None)>"Cancel"</button>
                                <button class="primary-button" disabled=move || busy.get()>"Save"</button>
                            </div>
                        </form>
                    </Dialog>
                }
            })}

            {move || confirmation.get().map(|conf| {
                let title = conf.title.clone();
                let desc = StoredValue::new(conf.description);
                let action_label = StoredValue::new(conf.title.trim_end_matches('?').to_owned());
                let work = StoredValue::new(conf.work);
                view! {
                    <Dialog title=title on_close=Callback::new(move |_| confirmation.set(None))>
                        <p>{desc.get_value()}</p>
                        <div class="dialog-actions">
                            <button class="text-button" on:click=move |_| confirmation.set(None)>"Cancel"</button>
                            <button class="danger-button" disabled=move || busy.get() on:click=move |_| (work.get_value())()>{action_label.get_value()}</button>
                        </div>
                    </Dialog>
                }
            })}

            {move || pending.get().map(|pend| {
                let pend_work = StoredValue::new(pend.work);
                view! {
                    <Dialog title="Confirm your identity" on_close=Callback::new(move |_| pending.set(None))>
                        <p>"Enter your password to authorize administration changes for five minutes."</p>
                        <form on:submit=move |e: web_sys::SubmitEvent| {
                            e.prevent_default();
                            let Some(form_el) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()) else { return };
                            let Ok(form) = web_sys::FormData::new_with_form(&form_el) else { return };
                            let password = form.get("password").as_string().unwrap_or_default();
                            let token = csrf.get_value();
                            let work = pend_work.get_value();
                            run_work(Box::pin(async move {
                                #[derive(Deserialize)]
                                #[serde(rename_all = "camelCase")]
                                struct ConfResp {
                                    proof: String,
                                    expires_at: String,
                                }
                                let value = api::request::<ConfResp>(
                                    "/api/v1/identity/confirmation",
                                    Req { method: "POST", body: Some(json!({ "password": password }).to_string()), csrf: Some(&token), ..Default::default() },
                                )
                                .await?;
                                let until = Date::parse(&value.expires_at);
                                let _ = proof.try_set(Some((value.proof.clone(), until)));
                                let _ = pending.try_set(None);
                                work(value.proof).await
                            }));
                        }>
                            <label>
                                "Current password"
                                <input name="password" type="password" autocomplete="current-password" autofocus required/>
                            </label>
                            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                            <div class="dialog-actions">
                                <button class="text-button" type="button" on:click=move |_| pending.set(None)>"Cancel"</button>
                                <button class="primary-button" disabled=move || busy.get()>"Confirm identity"</button>
                            </div>
                        </form>
                    </Dialog>
                }
            })}

            {move || secret.get().map(|sec| {
                let sec_sv = StoredValue::new(sec);
                view! {
                    <Dialog title="Save your credential" on_close=Callback::new(move |_| secret.set(None))>
                        <p>"This secret is shown once. Save it in the application or provisioning client."</p>
                        {move || sec_sv.get_value().id.map(|id| view! {
                            <label>
                                "Identifier"
                                <input readonly=true prop:value=id on:focus=select_input/>
                            </label>
                        })}
                        <label>
                            "Secret"
                            <input readonly=true prop:value=move || sec_sv.get_value().secret on:focus=select_input/>
                        </label>
                        <div class="dialog-actions">
                            <button
                                class="secondary-button"
                                on:click=move |_| {
                                    let text = sec_sv.get_value().secret;
                                    spawn_local(async move {
                                        let promise = window().navigator().clipboard().write_text(&text);
                                        match wasm_bindgen_futures::JsFuture::from(promise).await {
                                            Ok(_) => { let _ = notice.try_set("Credential copied.".to_owned()); }
                                            Err(_) => { let _ = notice.try_set("Select the secret and copy it manually.".to_owned()); }
                                        }
                                    });
                                }
                            >
                                "Copy secret"
                            </button>
                            <button class="primary-button" on:click=move |_| secret.set(None)>"I saved it"</button>
                        </div>
                    </Dialog>
                }
            })}
        </div>
    }
}

// ---- OIDC interaction consent / login screen ----------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InteractionClient {
    #[allow(dead_code)]
    id: String,
    name: String,
    redirect_uri: String,
}

#[derive(Clone, Debug, Deserialize)]
struct InteractionPermission {
    #[allow(dead_code)]
    scope: String,
    label: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interaction {
    #[allow(dead_code)]
    uid: String,
    prompt: String,
    client: InteractionClient,
    #[serde(default)]
    permissions: Vec<InteractionPermission>,
    #[serde(default)]
    requires_password: bool,
    #[serde(default)]
    provider_account_id: Option<String>,
}

#[derive(Deserialize)]
struct RedirectResp {
    redirect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppearanceLogo {
    logo_url: Option<String>,
}

/// Sign-in card matching `app.rs`'s `Auth` with `initialized = true` (since `app::Auth` is private).
#[component]
fn SignInScreen(on_login: Callback<BrowserSession>) -> impl IntoView {
    let logo: RwSignal<Option<String>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(value) = api::get::<AppearanceLogo>("/api/v1/appearance").await {
            let _ = logo.try_set(value.logo_url);
        }
    });
    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(form) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        let Ok(data) = web_sys::FormData::new_with_form(&form) else {
            return;
        };
        let mut body = Map::new();
        for name in ["username", "password"] {
            if let Some(value) = data.get(name).as_string() {
                body.insert(name.into(), value.into());
            }
        }
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            match api::request::<BrowserSession>(
                "/api/v1/auth/login",
                Req {
                    method: "POST",
                    body: Some(Value::Object(body).to_string()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(session) => {
                    on_login.try_run(session);
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                }
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <main class="auth-page">
            <section class="auth-card" aria-label="Sign in">
                <div class="signin-brand">
                    {move || match logo.get() {
                        Some(src) => view! { <img src=src alt="Dar" on:error=move |_| logo.set(None)/> }.into_any(),
                        None => view! { <Brand/> }.into_any(),
                    }}
                </div>
                <h1>"Sign in"</h1>
                <form on:submit=submit>
                    <label>
                        "Username"
                        <input name="username" autocomplete="username" pattern="[a-zA-Z0-9][a-zA-Z0-9_.\\-]{2,63}" maxlength="64" required autofocus/>
                    </label>
                    <label>
                        "Password"
                        <input name="password" type="password" autocomplete="current-password" minlength="1" maxlength="1024" required/>
                    </label>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <button class="primary-button full" type="submit" disabled=move || busy.get()>
                        {move || if busy.get() { "Signing in\u{2026}" } else { "Sign in" }}
                    </button>
                </form>
            </section>
        </main>
    }
}

fn redirect_host(uri: &str) -> String {
    let Ok(url) = web_sys::Url::new(uri) else {
        return String::new();
    };
    let host = url.host();
    if !host.is_empty() {
        host
    } else {
        url.protocol().trim_end_matches(':').to_owned()
    }
}

/// OIDC sign-in / consent interaction page (`/identity/interaction/<uid>`).
#[component]
pub fn IdentityInteraction(
    #[prop(into)] session: Signal<Option<BrowserSession>>,
    on_login: Callback<BrowserSession>,
) -> impl IntoView {
    let pathname = window().location().pathname().unwrap_or_default();
    let uid = pathname.rsplit('/').next().unwrap_or("").to_owned();
    let endpoint = StoredValue::new(format!(
        "/api/v1/identity/interactions/{}",
        encode_uri(&uid)
    ));

    let detail: RwSignal<Option<Interaction>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let switching = RwSignal::new(false);
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        let Some(_) = session.get() else { return };
        let url = endpoint.get_value();
        let active = StoredValue::new(true);
        on_cleanup(move || active.set_value(false));
        spawn_local(async move {
            match api::get::<Interaction>(&url).await {
                Ok(value) => {
                    if active.get_value() {
                        let _ = detail.try_set(Some(value));
                    }
                }
                Err(cause) => {
                    if active.get_value() {
                        let _ = error.try_set(cause.message);
                    }
                }
            }
        });
    });

    let decide = move |action: &'static str, password: Option<String>| {
        let Some(sess) = session.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        let url = endpoint.get_value();
        let mut body = Map::new();
        body.insert("action".into(), action.into());
        if let Some(pw) = password {
            body.insert("password".into(), pw.into());
        }
        spawn_local(async move {
            match api::request::<RedirectResp>(
                &url,
                Req {
                    method: "POST",
                    body: Some(Value::Object(body).to_string()),
                    csrf: Some(&sess.csrf_token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(res) => {
                    let _ = window().location().assign(&res.redirect);
                }
                Err(cause) => {
                    let _ = error.try_set(cause.message);
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    move || {
        let current_session = session.get();
        if current_session.is_none() || switching.get() {
            return view! {
                <SignInScreen on_login=Callback::new(move |s: BrowserSession| {
                    { on_login.try_run(s.clone()); };
                    switching.set(false);
                    detail.set(None);
                    let url = endpoint.get_value();
                    spawn_local(async move {
                        match api::get::<Interaction>(&url).await {
                            Ok(v) => {
                                let _ = detail.try_set(Some(v));
                                match api::request::<RedirectResp>(
                                    &url,
                                    Req { method: "POST", body: Some(json!({ "action": "login" }).to_string()), csrf: Some(&s.csrf_token), ..Default::default() },
                                )
                                .await
                                {
                                    Ok(r) => { let _ = window().location().assign(&r.redirect); }
                                    Err(cause) => { let _ = error.try_set(cause.message); }
                                }
                            }
                            Err(cause) => { let _ = error.try_set(cause.message); }
                        }
                    });
                })/>
            }
            .into_any();
        }
        let sess = current_session.expect("checked");
        let signed_out = Callback::new(move |_| {
            let _ = window().location().assign("/");
        });
        let display_name = sess.user.display_name.clone();
        let username = sess.user.username.clone();
        let initials: String = display_name
            .chars()
            .take(2)
            .collect::<String>()
            .to_uppercase();
        let user_id = sess.user.id.clone();

        view! {
            <div class="identity-consent-page">
                <WorkspaceHeader app="settings" session=sess on_signed_out=signed_out/>
                <main class="identity-consent" aria-busy=move || busy.get().to_string()>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    {move || match detail.get() {
                        None => view! {
                            <h1>"Opening sign-in request"</h1>
                            <p role="status">
                                {move || if !error.get().is_empty() {
                                    "This request may have expired. Return to the application and try again."
                                } else {
                                    "Loading\u{2026}"
                                }}
                            </p>
                            <a href="/">"Return to Dar"</a>
                        }.into_any(),
                        Some(d) => {
                            let is_login = d.prompt == "login";
                            let heading = if is_login {
                                format!("Sign in to {}", d.client.name)
                            } else {
                                "Review requested access".to_owned()
                            };
                            let symbol = first_char(&d.client.name);
                            let host = redirect_host(&d.client.redirect_uri);
                            let wrong_account = d.provider_account_id.as_deref().is_some_and(|id| id != user_id);
                            let requires_pw = d.requires_password;
                            let perms = d.permissions;
                            let dname = display_name.clone();
                            let uname = username.clone();
                            let init = initials.clone();
                            view! {
                                <div class="identity-row">
                                    <span class="identity-app-symbol">{symbol}</span>
                                    <div>
                                        <strong>{d.client.name}</strong>
                                        <small>{host}</small>
                                    </div>
                                </div>
                                <h1>{heading}</h1>
                                <div class="identity-consent-account">
                                    <span class="avatar">{init}</span>
                                    <div>
                                        <strong>{dname}</strong>
                                        <small>{format!("@{uname}")}</small>
                                    </div>
                                    <button class="text-button" disabled=move || busy.get() on:click=move |_| switching.set(true)>"Switch account"</button>
                                </div>
                                <ul class="identity-permissions">
                                    {perms.into_iter().map(|p| view! {
                                        <li><Icon name="check"/><span>{p.label}</span></li>
                                    }).collect_view()}
                                </ul>
                                <p class="muted small">"You can revoke access from account settings."</p>
                                <form on:submit=move |e: web_sys::SubmitEvent| {
                                    e.prevent_default();
                                    let pw = e
                                        .target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok())
                                        .and_then(|f| web_sys::FormData::new_with_form(&f).ok())
                                        .and_then(|fd| fd.get("password").as_string())
                                        .unwrap_or_default();
                                    decide(if is_login { "login" } else { "approve" }, Some(pw));
                                }>
                                    {requires_pw.then(|| view! {
                                        <label>
                                            "Confirm your password"
                                            <input name="password" type="password" autocomplete="current-password" autofocus required maxlength="1024"/>
                                        </label>
                                    })}
                                    {wrong_account.then(|| view! {
                                        <p role="alert">"This request belongs to another account. Switch accounts to continue."</p>
                                    })}
                                    <div class="dialog-actions">
                                        <button type="button" class="text-button" disabled=move || busy.get() on:click=move |_| decide("deny", None)>"Deny"</button>
                                        <button class="primary-button" disabled=move || busy.get() || (!is_login && wrong_account)>
                                            {if is_login { "Continue" } else { "Allow access" }}
                                        </button>
                                    </div>
                                </form>
                            }.into_any()
                        }
                    }}
                </main>
            </div>
        }
        .into_any()
    }
}
