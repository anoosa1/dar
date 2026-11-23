//! Administration → Apps (`installed-app-settings.tsx`): enable or disable apps instance-wide.

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, BrowserSession, Req};
use crate::icons::Icon;
use crate::ui::window;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedApp {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub can_toggle: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    revision: u64,
    #[serde(default)]
    #[allow(dead_code)]
    disabled: Vec<String>,
    #[serde(default)]
    items: Vec<ManagedApp>,
}

fn app_icon(id: &str) -> &'static str {
    match id {
        "files" => "folder",
        "contacts" => "contacts",
        "calendar" => "calendar",
        "notes" => "file",
        "tasks" => "check",
        "agent" => "agent",
        _ => "clock",
    }
}

/// Administration → Apps section (`InstalledAppsSettings`).
#[component]
pub fn InstalledAppsSettings(session: BrowserSession) -> impl IntoView {
    let inventory: RwSignal<Option<Inventory>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token);

    let refresh = move |signal: Option<web_sys::AbortSignal>| async move {
        let value = api::request::<Inventory>(
            "/api/v1/admin/installedApps",
            Req { signal, ..Default::default() },
        )
        .await?;
        let _ = inventory.try_set(Some(value));
        Ok::<(), api::RequestError>(())
    };

    if let Ok(controller) = web_sys::AbortController::new() {
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        let signal_check = SendWrapper::new(signal.clone());
        on_cleanup(move || abort_ref.take().abort());
        spawn_local(async move {
            if let Err(e) = refresh(Some(signal)).await
                && !signal_check.aborted()
            {
                let _ = error.try_set(e.message);
            }
        });
    }

    let toggle = move |item: ManagedApp| {
        let Some(current) = inventory.get_untracked() else { return };
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        let token = csrf.get_value();
        spawn_local(async move {
            let etag = format!("\"apps:{}\"", current.revision);
            let body = json!({
                "id": item.id,
                "enabled": !item.enabled,
                "revision": current.revision,
            });
            let result = api::request::<Inventory>(
                "/api/v1/admin/installedApps",
                Req {
                    method: "PUT",
                    body: Some(body.to_string()),
                    csrf: Some(&token),
                    headers: &[("if-match", &etag)],
                    ..Default::default()
                },
            )
            .await;
            match result {
                Ok(next) => {
                    let updated = next.items.iter().find(|app| app.id == item.id).map(|a| a.enabled).unwrap_or(!item.enabled);
                    let _ = inventory.try_set(Some(next));
                    if let Ok(ev) = web_sys::Event::new("dar:apps") {
                        let _ = window().dispatch_event(&ev);
                    }
                    let _ = notice.try_set(format!(
                        "{} {}.",
                        item.name,
                        if updated { "enabled" } else { "disabled" }
                    ));
                }
                Err(e) => {
                    let _ = error.try_set(e.message);
                    let _ = refresh(None).await;
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <h1>"Apps"</h1>
        <p class="muted">"Enable or disable apps for everyone. Saved data is retained."</p>
        {move || (!error.get().is_empty()).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
        {move || (!notice.get().is_empty()).then(|| view! { <p role="status">{notice.get()}</p> })}
        {move || match inventory.get() {
            None => view! {
                <p role="status">"Loading apps\u{2026}"</p>
                {move || (!error.get().is_empty()).then(|| view! {
                    <button
                        class="secondary-button"
                        on:click=move |_| {
                            error.set(String::new());
                            spawn_local(async move {
                                if let Err(e) = refresh(None).await {
                                    let _ = error.try_set(e.message);
                                }
                            });
                        }
                    >
                        "Try again"
                    </button>
                })}
            }.into_any(),
            Some(inv) => view! {
                <div class="managed-apps">
                    {inv.items.into_iter().map(|item| {
                        let icon = app_icon(&item.id);
                        let name = item.name.clone();
                        let id = item.id.clone();
                        let version = item.version.clone();
                        let permissions_text = if item.permissions.is_empty() {
                            "No SDK data permissions".to_owned()
                        } else {
                            item.permissions.join(" \u{00b7} ")
                        };
                        let status = item.status.clone();
                        let can_toggle = item.can_toggle;
                        let enabled = item.enabled;
                        let button_label = format!("{} {}", if enabled { "Disable" } else { "Enable" }, item.name);
                        let item_for_toggle = item.clone();
                        let aria_name = name.clone();
                        view! {
                            <section class="managed-app" aria-label=aria_name>
                                <span class="managed-app-icon"><Icon name=icon/></span>
                                <div class="managed-app-name">
                                    <h2>{name}</h2>
                                    {(!version.is_empty()).then(|| view! {
                                        <p class="muted small">{format!("{id} \u{00b7} {version}")}</p>
                                        <p class="muted small">{permissions_text}</p>
                                    })}
                                    {(!status.is_empty() && status != "Ready").then(|| view! {
                                        <p class="muted small">{status}</p>
                                    })}
                                </div>
                                {can_toggle.then(|| view! {
                                    <span class="managed-app-status">{if enabled { "Enabled" } else { "Disabled" }}</span>
                                    <button
                                        type="button"
                                        class="secondary-button"
                                        disabled=move || busy.get()
                                        aria-label=button_label
                                        on:click=move |_| toggle(item_for_toggle.clone())
                                    >
                                        {if enabled { "Disable" } else { "Enable" }}
                                    </button>
                                })}
                            </section>
                        }
                    }).collect_view()}
                </div>
            }.into_any(),
        }}
    }
}
