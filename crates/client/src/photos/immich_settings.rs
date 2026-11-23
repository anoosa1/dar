//! Photos settings › Immich (`photos/immich-settings.tsx`): the Immich mobile app,
//! Dar's connection to an Immich server, and the live import progress bar.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use super::{PhotoImmichProgress, error_message, format_number};
use crate::api::{self, BrowserSession, Req};
use crate::dates::local_date_time;
use crate::icons::Icon;
use crate::settings::SettingRow;
use crate::ui::{Select, unique_id};

const IMMICH_BASE: &str = "/api/v1/photos/immich";

const INTERVALS: [(&str, &str); 4] = [
    ("15", "Every 15 minutes"),
    ("60", "Every hour"),
    ("360", "Every 6 hours"),
    ("1440", "Daily"),
];

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    #[serde(default)]
    device_type: String,
    #[serde(default, rename = "deviceOS")]
    device_os: String,
    #[serde(default)]
    created_at: f64,
    #[serde(default)]
    used_at: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImmichSummary {
    #[serde(default)]
    remote: Option<u64>,
    #[serde(default)]
    imported: Option<u64>,
    #[serde(default)]
    albums: Option<u64>,
    #[serde(default)]
    skipped: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImmichConnection {
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    url: String,
    #[serde(default)]
    interval_minutes: u64,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    last_run_at: f64,
    #[serde(default)]
    last_error: String,
    #[serde(default)]
    summary: ImmichSummary,
    #[serde(default)]
    revision: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImmichState {
    #[serde(default)]
    server_url: String,
    #[serde(default)]
    devices: Vec<Device>,
    #[serde(default)]
    connection: ImmichConnection,
    #[serde(default)]
    progress: Option<PhotoImmichProgress>,
}

fn when(ms: f64) -> String {
    local_date_time(ms, &[("dateStyle", "medium"), ("timeStyle", "short")])
}

fn phase_label(phase: &str) -> &'static str {
    match phase {
        "connecting" => "Connecting to Immich\u{2026}",
        "comparing" => "Comparing libraries\u{2026}",
        "importing" => "Importing from Immich",
        "albums" => "Matching albums",
        "waiting" => "More to import \u{2014} continuing shortly",
        _ => "",
    }
}

/// Photos settings › Immich: the Immich mobile app, and Dar's connection to an Immich server.
#[component]
pub fn ImmichSettings(session: BrowserSession) -> impl IntoView {
    let state: RwSignal<Option<ImmichState>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let url = RwSignal::new(String::new());
    let api_key = RwSignal::new(String::new());
    let interval_str = RwSignal::new("60".to_owned());
    let enabled = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let watch_until = RwSignal::new(0.0f64);
    let poll_id = RwSignal::new(0u32);
    let session = StoredValue::new(session);

    let load = move || {
        spawn_local(async move {
            match api::get::<ImmichState>(IMMICH_BASE).await {
                Ok(value) => {
                    if value.connection.connected {
                        url.try_set(value.connection.url.clone());
                        interval_str.try_set(value.connection.interval_minutes.to_string());
                        enabled.try_set(value.connection.enabled);
                    }
                    state.try_set(Some(value));
                    error.try_set(String::new());
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
        });
    };
    load();

    // While an import runs, its progress refreshes every two seconds.
    // Just after Connect or Sync now the import has not reported yet: keep looking for ten seconds.
    Effect::new(move |_| {
        let has_progress = state.with(|s| s.as_ref().and_then(|s| s.progress.as_ref()).is_some());
        let until = watch_until.get();
        if !has_progress && js_sys::Date::now() > until {
            return;
        }
        let id = poll_id.get_untracked() + 1;
        poll_id.set(id);
        let delay = if has_progress { 2000 } else { 1000 };
        spawn_local(async move {
            crate::sleep(delay).await;
            if poll_id.try_get_untracked() != Some(id) {
                return;
            }
            if let Ok(value) = api::get::<ImmichState>(IMMICH_BASE).await {
                state.try_update(|current| {
                    if let Some(current) = current {
                        current.progress = value.progress;
                        current.connection = value.connection;
                    }
                });
            }
        });
    });

    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        let (was_connected, revision) = state.with_untracked(|s| {
            s.as_ref()
                .map(|s| (s.connection.connected, if s.connection.connected { s.connection.revision } else { 0 }))
                .unwrap_or((false, 0))
        });
        let mut body = serde_json::json!({
            "url": url.get_untracked(),
            "intervalMinutes": interval_str.get_untracked().parse::<u64>().unwrap_or(60),
            "enabled": enabled.get_untracked(),
            "revision": revision,
        });
        let key = api_key.get_untracked();
        if !key.is_empty() {
            body["apiKey"] = key.into();
        }
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let result = api::send::<serde_json::Value>(&format!("{IMMICH_BASE}/connection"), "PUT", Some(body), &csrf).await;
            match result {
                Ok(_) => {
                    api_key.try_set(String::new());
                    notice.try_set(if was_connected {
                        "Connection saved".to_owned()
                    } else {
                        "Connected to Immich. Importing has started.".to_owned()
                    });
                    watch_until.try_set(js_sys::Date::now() + 10_000.0);
                    load();
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
            busy.try_set(false);
        });
    };

    let act = move |path: String, method: &'static str, done: &'static str| {
        busy.set(true);
        error.set(String::new());
        notice.set(String::new());
        let csrf = session.with_value(|s| s.csrf_token.clone());
        spawn_local(async move {
            let result = api::request::<serde_json::Value>(
                &format!("{IMMICH_BASE}{path}"),
                Req { method, csrf: Some(&csrf), ..Default::default() },
            )
            .await;
            match result {
                Ok(_) => {
                    notice.try_set(done.to_owned());
                    load();
                }
                Err(cause) => {
                    error.try_set(error_message(&cause.message));
                }
            }
            busy.try_set(false);
        });
    };

    let loaded = Memo::new(move |_| state.with(Option::is_some));
    let none_state = Signal::derive(|| None);
    let interval_options: Vec<(String, String)> = INTERVALS.iter().map(|(v, l)| ((*v).to_owned(), (*l).to_owned())).collect();
    let switch_id = unique_id();

    move || {
        if !loaded.get() {
            return if !error.get().is_empty() {
                view! { <p role="alert" class="error">{move || error.get()}</p> }.into_any()
            } else {
                view! { <p role="status">"Loading\u{2026}"</p> }.into_any()
            };
        }
        let username = session.with_value(|s| s.user.username.clone());
        let interval_options = interval_options.clone();
        let switch_id = switch_id.clone();
        let connected = Memo::new(move |_| state.with(|s| s.as_ref().is_some_and(|s| s.connection.connected)));
        let has_progress = Memo::new(move |_| state.with(|s| s.as_ref().and_then(|s| s.progress.as_ref()).is_some()));
        let progress_signal = Signal::derive(move || {
            state.with(|s| {
                s.as_ref()
                    .and_then(|s| s.progress.clone())
                    .unwrap_or(PhotoImmichProgress {
                        phase: String::new(),
                        done: 0,
                        total: 0,
                        imported: 0,
                        file: String::new(),
                        started_at: 0.0,
                    })
            })
        });
        view! {
            {move || (!notice.get().is_empty()).then(|| view! { <p role="status" class="photos-saved">{notice.get()}</p> })}
            <section class="settings-group" aria-label="Immich mobile app">
                <h3>"Immich mobile app"</h3>
                <SettingRow
                    label="Server URL"
                    hint="In the Immich app, enter this address. It finds Dar\u{2019}s Immich-compatible API by itself."
                    state=none_state
                    stacked=true
                >
                    <input
                        readonly
                        class="photos-immich-url"
                        value=move || state.with(|s| s.as_ref().map(|s| s.server_url.clone()).unwrap_or_default())
                        prop:value=move || state.with(|s| s.as_ref().map(|s| s.server_url.clone()).unwrap_or_default())
                        aria-label="Server URL for the Immich app"
                        on:focus=move |e| {
                            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                input.select();
                            }
                        }
                    />
                </SettingRow>
                <div class="settings-row is-stacked">
                    <div class="settings-row-text">
                        <span class="settings-row-label">"Email and password"</span>
                        <small>"Email: "<strong>{username}"@dar"</strong>" (your username, @ and anything). Password: an app password with Photos read and write."</small>
                    </div>
                    <div class="settings-row-control">
                        <a class="secondary-button" href="/settings">"Create an app password"</a>
                    </div>
                </div>
                <SettingRow
                    label="Backup and sync"
                    hint="Backups go to your Photos folder; photos already here (same file) are skipped. The app shows your library, albums shared with you, favorites, archive and trash. Locked photos stay on Dar."
                    state=none_state
                    stacked=true
                >
                    <span class="photos-hint">"Immich 3.4 protocol"</span>
                </SettingRow>
                <div class="photos-devices" role="group" aria-label="Signed-in devices">
                    <strong>"Signed-in devices"</strong>
                    {move || {
                        let devices = state.with(|s| s.as_ref().map(|s| s.devices.clone()).unwrap_or_default());
                        if devices.is_empty() {
                            view! { <p class="photos-hint">"No devices yet."</p> }.into_any()
                        } else {
                            view! {
                                <ul>
                                    {devices.into_iter().map(|d| {
                                        let label = if !d.device_os.is_empty() {
                                            d.device_os
                                        } else if !d.device_type.is_empty() {
                                            d.device_type
                                        } else {
                                            "Immich app".to_owned()
                                        };
                                        let used = when(d.used_at);
                                        let path = format!("/devices/{}", d.id);
                                        view! {
                                            <li>
                                                <Icon name="image"/>
                                                <span><strong>{label}</strong><small>"Last used "{used}</small></span>
                                                <button type="button" class="text-button danger" disabled=move || busy.get()
                                                    on:click=move |_| act(path.clone(), "DELETE", "Device signed out")>"Sign out"</button>
                                            </li>
                                        }
                                    }).collect_view()}
                                </ul>
                            }
                            .into_any()
                        }
                    }}
                </div>
            </section>
            <section class="settings-group" aria-label="Immich server">
                <h3>"Immich server"</h3>
                <p class="photos-immich-intro">"Import photos from an Immich server into Dar. Albums come along; files Dar already has are matched, not copied. Nothing is sent to Immich."</p>
                <Show when=move || connected.get()>
                    <div class="photos-immich-status" role="status">
                        {move || {
                            let c = state.with(|s| s.as_ref().map(|s| s.connection.clone()).unwrap_or_default());
                            let head = if c.last_run_at > 0.0 {
                                format!("Last sync {}", when(c.last_run_at))
                            } else {
                                "Not synced yet".to_owned()
                            };
                            let counts = if c.last_run_at > 0.0 {
                                format!(
                                    " \u{00b7} {} imported \u{00b7} {} albums \u{00b7} {} on Immich",
                                    c.summary.imported.unwrap_or(0),
                                    c.summary.albums.unwrap_or(0),
                                    c.summary.remote.unwrap_or(0)
                                )
                            } else {
                                String::new()
                            };
                            let skipped = match c.summary.skipped {
                                Some(n) if n > 0 => format!(" \u{00b7} {n} still to import"),
                                _ => String::new(),
                            };
                            view! {
                                <p><strong>{head}</strong>{counts}{skipped}</p>
                            }
                        }}
                        <Show when=move || has_progress.get()>
                            <ImmichProgressBar progress=progress_signal/>
                        </Show>
                        {move || {
                            let last_error = state.with(|s| s.as_ref().map(|s| s.connection.last_error.clone()).unwrap_or_default());
                            (!last_error.is_empty()).then(|| view! { <p class="error">{last_error}</p> })
                        }}
                        <div class="photos-immich-actions">
                            <button
                                type="button"
                                class="secondary-button"
                                disabled=move || {
                                    busy.get()
                                        || state.with(|s| {
                                            s.as_ref()
                                                .and_then(|s| s.progress.as_ref())
                                                .is_some_and(|p| p.phase != "waiting")
                                        })
                                }
                                on:click=move |_| {
                                    watch_until.set(js_sys::Date::now() + 10_000.0);
                                    act("/connection/sync".to_owned(), "POST", "Sync started");
                                }
                            >
                                <Icon name="refresh"/>"Sync now"
                            </button>
                            <button
                                type="button"
                                class="text-button danger"
                                disabled=move || busy.get()
                                on:click=move |_| act("/connection".to_owned(), "DELETE", "Disconnected. Imported photos stay in Dar.")
                            >
                                "Disconnect"
                            </button>
                        </div>
                    </div>
                </Show>
                <form class="photos-immich-form" on:submit=save>
                    <label>"Server address"
                        <input
                            prop:value=move || url.get()
                            on:input=move |e| url.set(event_target_value(&e))
                            placeholder="https://photos.example.com"
                            required
                            inputmode="url"
                            autocomplete="off"
                        />
                    </label>
                    <label>"API key"
                        <input
                            type="password"
                            prop:value=move || api_key.get()
                            on:input=move |e| api_key.set(event_target_value(&e))
                            autocomplete="off"
                            placeholder=move || if connected.get() { "Saved \u{2014} enter a new key to replace it" } else { "From Immich \u{203a} Account settings \u{203a} API keys" }
                            required=move || state.with(|s| s.as_ref().map(|s| !s.connection.connected || url.get() != s.connection.url).unwrap_or(true))
                        />
                    </label>
                    <label>"Sync"
                        <Select options=interval_options value=interval_str/>
                    </label>
                    <label class="settings-row settings-switch">
                        <span class="settings-row-text">
                            <span id=format!("{switch_id}label") class="settings-row-label">"Sync automatically"</span>
                        </span>
                        <span class="ui-switch">
                            <input
                                type="checkbox"
                                role="switch"
                                aria-labelledby=format!("{switch_id}label")
                                prop:checked=move || enabled.get()
                                on:change=move |e| enabled.set(event_target_checked(&e))
                            />
                        </span>
                    </label>
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="photos-immich-submit">
                        <button class="primary-button" disabled=move || busy.get()>
                            {move || if busy.get() { "Checking\u{2026}" } else if connected.get() { "Save" } else { "Connect" }}
                        </button>
                    </div>
                </form>
            </section>
        }
        .into_any()
    }
}

/// The live status of an Immich import: phase, a bar (indeterminate until the total is known) and counts.
#[component]
pub fn ImmichProgressBar(
    #[prop(into)] progress: Signal<PhotoImmichProgress>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let class = if compact { "photos-sync-progress is-compact" } else { "photos-sync-progress" };
    let known = move || progress.with(|p| p.total > 0);
    let percent = move || {
        progress.with(|p| {
            if p.total > 0 {
                ((p.done as f64 / p.total as f64 * 100.0).round() as u64).min(100)
            } else {
                0
            }
        })
    };
    let detail = move || {
        progress.with(|p| {
            if p.phase == "importing" && p.total > 0 {
                format!(
                    "{} of {} checked \u{00b7} {} imported",
                    format_number(p.done),
                    format_number(p.total),
                    format_number(p.imported)
                )
            } else if p.phase == "albums" && p.total > 0 {
                format!("{} of {} albums", p.done, p.total)
            } else if p.imported > 0 {
                format!("{} imported", format_number(p.imported))
            } else {
                String::new()
            }
        })
    };
    let show_bar = Memo::new(move |_| progress.with(|p| p.phase != "waiting"));
    view! {
        <div class=class role="status" aria-live="polite">
            <span class="photos-sync-label">
                <Icon name="refresh"/>
                {move || {
                    let label = progress.with(|p| phase_label(&p.phase));
                    if known() && !compact {
                        format!("{label} \u{00b7} {}%", percent())
                    } else {
                        label.to_owned()
                    }
                }}
            </span>
            <Show when=move || show_bar.get()>
                <progress
                    max=move || known().then(|| progress.with(|p| p.total.to_string()))
                    value=move || known().then(|| progress.with(|p| p.done.to_string()))
                    aria-label=move || progress.with(|p| phase_label(&p.phase))
                    aria-valuetext=move || known().then(|| format!("{}%", percent()))
                />
            </Show>
            {move || {
                let d = detail();
                (!d.is_empty()).then(|| view! { <small>{d}</small> })
            }}
            {move || {
                if compact {
                    None
                } else {
                    let file = progress.with(|p| p.file.clone());
                    (!file.is_empty()).then(|| view! { <small class="photos-sync-file">{file}</small> })
                }
            }}
        </div>
    }
}
