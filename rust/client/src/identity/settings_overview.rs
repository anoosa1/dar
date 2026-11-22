//! Settings home (`settings-overview.tsx`): current values at a glance, theme changes in place.

use js_sys::{Array, Date, Intl, Object, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::api::{self, BrowserSession, Req};
use crate::dates::local_date;
use crate::icons::Icon;
use crate::settings::{SaveProblems, SaveState, Saver};
use crate::shell::Avatar;
use crate::ui::window;

use super::installed_app_settings::ManagedApp;
use super::instance_appearance::InstanceAppearance;

const THEMES: [(&str, &str); 3] = [
    ("system", "Follow system"),
    ("latte", "Light"),
    ("mocha", "Dark"),
];

fn plural(count: usize, one: &str, many: Option<&str>) -> String {
    if count == 1 {
        format!("{count} {one}")
    } else if let Some(many) = many {
        format!("{count} {many}")
    } else {
        format!("{count} {one}s")
    }
}

fn format_zone_clock(zone: &str, now: f64) -> String {
    let options = Object::new();
    let _ = Reflect::set(&options, &"timeZone".into(), &JsValue::from_str(zone));
    let _ = Reflect::set(&options, &"hour".into(), &"numeric".into());
    let _ = Reflect::set(&options, &"minute".into(), &"2-digit".into());
    let _ = Reflect::set(&options, &"timeZoneName".into(), &"short".into());
    let ctor = Reflect::get(&js_sys::global(), &"Intl".into())
        .ok()
        .and_then(|intl| Reflect::get(&intl, &"DateTimeFormat".into()).ok())
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    let Some(ctor) = ctor else { return String::new() };
    let args = Array::of2(&JsValue::UNDEFINED, &options);
    let Ok(formatter) = Reflect::construct(&ctor, &args) else { return String::new() };
    let formatter: Intl::DateTimeFormat = formatter.unchecked_into();
    let date = Date::new(&JsValue::from_f64(now));
    formatter
        .format()
        .call1(&JsValue::NULL, &date)
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct AppPasswordsResp {
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ManagedAppsResp {
    #[serde(default)]
    items: Vec<ManagedApp>,
}

#[component]
fn Card(
    icon: &'static str,
    title: &'static str,
    #[prop(optional)] label: Option<&'static str>,
    on_open: Callback<()>,
    #[prop(optional)] current: bool,
    children: Children,
) -> impl IntoView {
    let aria = format!("Open {}", label.unwrap_or(title));
    view! {
        <article class=if current { "settings-overview-card is-current" } else { "settings-overview-card" }>
            <button type="button" class="settings-overview-open" aria-label=aria on:click=move |_| { on_open.try_run(()); }>
                <Icon name=icon/>
                <span>{title}</span>
                <Icon name="chevron"/>
            </button>
            {children()}
        </article>
    }
}

/// Settings home (`SettingsOverview`).
#[component]
pub fn SettingsOverview(
    session: BrowserSession,
    #[prop(into)] connections: Signal<usize>,
    #[prop(into)] clients: Signal<usize>,
    #[prop(into)] users: Signal<usize>,
    #[prop(into)] groups: Signal<usize>,
    #[prop(into)] credentials: Signal<Vec<serde_json::Value>>,
    #[prop(into)] status: Signal<Option<serde_json::Value>>,
    on_open: Callback<&'static str>,
) -> impl IntoView {
    let admin = session.user.administrator;
    let display_name = session.user.display_name.clone();
    let username = session.user.username.clone();
    let saver = Saver::new(session);

    let passwords: RwSignal<Option<usize>> = RwSignal::new(None);
    let apps: RwSignal<Option<Vec<ManagedApp>>> = RwSignal::new(None);
    let logo: RwSignal<Option<InstanceAppearance>> = RwSignal::new(None);
    let now = RwSignal::new(Date::now());

    if let Ok(controller) = web_sys::AbortController::new() {
        let signal = controller.signal();
        let abort_ref = SendWrapper::new(controller);
        on_cleanup(move || abort_ref.take().abort());

        let sig1 = signal.clone();
        spawn_local(async move {
            if let Ok(v) = api::request::<AppPasswordsResp>("/api/v1/auth/app-passwords", Req { signal: Some(sig1), ..Default::default() }).await {
                let _ = passwords.try_set(Some(v.items.len()));
            }
        });
        if admin {
            let sig2 = signal.clone();
            spawn_local(async move {
                if let Ok(v) = api::request::<ManagedAppsResp>("/api/v1/admin/installedApps", Req { signal: Some(sig2), ..Default::default() }).await {
                    let _ = apps.try_set(Some(v.items));
                }
            });
            let sig3 = signal;
            spawn_local(async move {
                if let Ok(v) = api::request::<InstanceAppearance>("/api/v1/appearance", Req { signal: Some(sig3), ..Default::default() }).await {
                    let _ = logo.try_set(Some(v));
                }
            });
        }
    }

    let tick = Closure::<dyn FnMut()>::new(move || {
        let _ = now.try_set(Date::now());
    });
    if let Ok(interval_id) = window().set_interval_with_callback_and_timeout_and_arguments_0(tick.as_ref().unchecked_ref(), 30_000) {
        let kept = SendWrapper::new((interval_id, tick));
        on_cleanup(move || {
            let (id, _cb) = kept.take();
            window().clear_interval_with_handle(id);
        });
    }

    let appearance_state = saver.state("appearance");
    let theme = Signal::derive(move || saver.value("appearance").as_str().unwrap_or("system").to_owned());
    let zone = Signal::derive(move || saver.value("timezone").as_str().unwrap_or("UTC").to_owned());
    let has_prefs = Memo::new(move |_| saver.preferences.with(Option::is_some));

    view! {
        <section class="settings-overview" aria-labelledby="settings-overview-heading">
            <h1 id="settings-overview-heading" class="sr-only">"Overview"</h1>
            <div class="settings-overview-me">
                <Avatar name=display_name.clone()/>
                <div>
                    <h2>{display_name}</h2>
                    <p class="muted">{format!("@{username} \u{00b7} {}", if admin { "Administrator" } else { "Member" })}</p>
                </div>
                <button type="button" class="secondary-button" on:click=move |_| { on_open.try_run("Profile"); }>"Edit profile"</button>
            </div>
            <SaveProblems saver=saver/>
            <h2 class="settings-overview-heading">"Personal"</h2>
            <div class="settings-overview-grid">
                <Card icon="palette" title="Appearance" on_open=Callback::new(move |_| { on_open.try_run("Appearance"); })>
                    <p>
                        {move || {
                            let t = theme.get();
                            let name = THEMES.iter().find(|(id, _)| *id == t).map(|(_, l)| *l).unwrap_or("");
                            let suffix = match appearance_state.get() {
                                Some(SaveState::Saving) => " \u{00b7} Saving\u{2026}",
                                Some(SaveState::Saved) => " \u{00b7} Saved",
                                _ => "",
                            };
                            format!("{name}{suffix}")
                        }}
                    </p>
                    <div class="settings-overview-themes" role="radiogroup" aria-label="Color theme">
                        {THEMES.into_iter().map(|(id, label)| {
                            view! {
                                <button
                                    type="button"
                                    role="radio"
                                    aria-checked=move || (theme.get() == id).to_string()
                                    aria-label=label
                                    disabled=move || !has_prefs.get()
                                    class=format!("theme-swatch swatch-{id}")
                                    on:click=move |_| saver.change("appearance", serde_json::json!(id), "Appearance saved".into(), false)
                                />
                            }
                        }).collect_view()}
                    </div>
                </Card>
                <Card icon="lock" title="Security" on_open=Callback::new(move |_| { on_open.try_run("Account"); })>
                    <p>
                        "Password and sessions"
                        <br/>
                        {move || match passwords.get() {
                            None => "Checking app passwords\u{2026}".to_owned(),
                            Some(n) => plural(n, "app password", None),
                        }}
                    </p>
                </Card>
                <Card icon="globe" title="Regional settings" on_open=Callback::new(move |_| { on_open.try_run("Regional settings"); })>
                    <p>
                        {move || zone.get().replace('_', " ")}
                        {move || {
                            let clock = format_zone_clock(&zone.get(), now.get());
                            (!clock.is_empty()).then(|| view! { <br/>{format!("{clock} now")} })
                        }}
                    </p>
                </Card>
                <Card icon="plug" title="Connected applications" on_open=Callback::new(move |_| { on_open.try_run("Connected applications"); })>
                    <p>
                        {move || {
                            let n = connections.get();
                            if n > 0 {
                                format!("{} can use your Dar identity", plural(n, "application", None))
                            } else {
                                "No applications connected".to_owned()
                            }
                        }}
                    </p>
                </Card>
            </div>
            {admin.then(|| view! {
                <h2 class="settings-overview-heading">"Administration"</h2>
                <div class="settings-overview-grid">
                    <Card icon="grid" title="Apps" on_open=Callback::new(move |_| { on_open.try_run("Installed apps"); })>
                        <p>
                            {move || match apps.get() {
                                Some(list) => {
                                    let enabled = list.iter().filter(|a| a.enabled).count();
                                    format!("{enabled} of {} enabled", plural(list.len(), "app", None))
                                }
                                None => "Checking apps\u{2026}".to_owned(),
                            }}
                        </p>
                    </Card>
                    <Card icon="contacts" title="Directory" on_open=Callback::new(move |_| { on_open.try_run("Directory"); })>
                        <p>{move || format!("{} \u{00b7} {}", plural(users.get(), "person", Some("people")), plural(groups.get(), "group", None))}</p>
                    </Card>
                    <Card icon="plug" title="Applications" on_open=Callback::new(move |_| { on_open.try_run("Applications"); })>
                        <p>
                            {move || {
                                let n = clients.get();
                                if n > 0 {
                                    plural(n, "registered application", None)
                                } else {
                                    "No registered applications".to_owned()
                                }
                            }}
                        </p>
                    </Card>
                    <Card icon="refresh" title="SCIM" on_open=Callback::new(move |_| { on_open.try_run("SCIM"); })>
                        <p>
                            {move || {
                                let current_now = now.get();
                                let active = credentials.with(|list| {
                                    list.iter().filter(|c| {
                                        !c["revoked"].as_bool().unwrap_or(false)
                                            && c["expiresAt"].as_str().map(Date::parse).is_some_and(|t| t > current_now)
                                    }).count()
                                });
                                if active > 0 {
                                    plural(active, "active credential", None)
                                } else {
                                    "No active credentials".to_owned()
                                }
                            }}
                        </p>
                    </Card>
                    <Card icon="key" title="Signing keys" on_open=Callback::new(move |_| { on_open.try_run("Signing keys"); })>
                        <p>
                            {move || {
                                let rotated = status.with(|s| {
                                    s.as_ref()
                                        .and_then(|v| v.get("keys"))
                                        .and_then(|k| k.get("rotatedAt"))
                                        .and_then(|r| r.as_str().map(Date::parse).or_else(|| r.as_f64()))
                                });
                                match rotated {
                                    Some(ts) if !ts.is_nan() => format!("Rotated {}", local_date(ts, &[])),
                                    _ => "Active key".to_owned(),
                                }
                            }}
                        </p>
                    </Card>
                    <Card icon="palette" title="Sign-in appearance" label="Administration appearance" on_open=Callback::new(move |_| { on_open.try_run("Instance appearance"); })>
                        <p>
                            {move || match logo.get() {
                                Some(value) if value.logo_url.is_some() => "Instance logo",
                                Some(_) => "Dar logo",
                                None => "Checking logo\u{2026}",
                            }}
                        </p>
                    </Card>
                </div>
            })}
        </section>
    }
}
