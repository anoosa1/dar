//! The page root (`main.tsx`, `auth.tsx`, `appearance.tsx`): checks the instance and
//! session, applies the theme, and shows sign-in or the current app.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession, Req};
use crate::live::Live;
use crate::navigation::{intercept_links, set_route, Route};
use crate::shared::{self, context};
use crate::ui::{listen, window};

#[derive(Deserialize)]
struct Status {
    initialized: bool,
}

fn opening() -> impl IntoView {
    view! { <main class="startup-state" aria-busy="true"><div class="brand-mark"></div><p role="status">"Opening your space…"</p></main> }
}

#[component]
pub fn App() -> impl IntoView {
    let status: RwSignal<Option<bool>> = RwSignal::new(None);
    let session: RwSignal<Option<BrowserSession>> = RwSignal::new(None);
    let error = RwSignal::new(false);
    let route = Route { path: RwSignal::new(window().location().pathname().unwrap_or_default()), visit: RwSignal::new(0) };
    set_route(route);
    provide_context(route);
    let live = Live::new();
    crate::live::register(live);
    provide_context(live);
    listen(&window(), "popstate", false, move |_| {
        let path = window().location().pathname().unwrap_or_default();
        if path != route.path.get_untracked() {
            route.path.set(path);
            route.visit.update(|v| *v += 1);
        }
    });
    spawn_local(async move {
        let (status_res, session_res) = futures::future::join(
            api::get::<Status>("/api/v1/auth/status"),
            api::get::<BrowserSession>("/api/v1/auth/session"),
        )
        .await;
        match status_res {
            Ok(value) => {
                if value.initialized {
                    match session_res {
                        Ok(s) => session.set(Some(s)),
                        Err(e) if e.status == 401 => {}
                        Err(_) => error.set(true),
                    }
                }
                status.set(Some(value.initialized));
            }
            Err(_) => error.set(true),
        }
    });
    let signed_out = Callback::new(move |_| {
        session.set(None);
        status.set(Some(true));
    });
    // Only a change between shared links, identity interactions and the apps remounts the root.
    let page_kind = Memo::new(move |_| route.path.with(|p| if p.starts_with("/s/") { 1 } else if p.starts_with("/identity/interaction/") { 2 } else { 0 }));
    move || {
        if error.get() {
            return view! {
                <main class="startup-state"><h1>"Unable to connect"</h1><p role="alert">"Your instance could not be reached. Check the connection and try again."</p>
                    <button class="primary-button" on:click=|_| { let _ = window().location().reload(); }>"Try again"</button></main>
            }
            .into_any();
        }
        let kind = page_kind.get();
        if kind == 1 {
            return view! { <crate::photos::PublicPhotos/> }.into_any();
        }
        let Some(initialized) = status.get() else { return opening().into_any() };
        // An identity interaction signs in inline, before the sign-in screen.
        if kind == 2 {
            route.visit.track();
            // Its consent page uses the shared app data, as signed-in pages do.
            provide_context(shared::Context::new());
            return view! { <crate::identity::IdentityInteraction session=Signal::derive(move || session.get()) on_login=Callback::new(move |s| session.set(Some(s)))/> }.into_any();
        }
        match session.get() {
            None => {
                apply_theme(None);
                view! { <Auth initialized=initialized on_login=Callback::new(move |s| session.set(Some(s)))/> }.into_any()
            }
            Some(s) => view! { <SignedIn session=s on_signed_out=signed_out/> }.into_any(),
        }
    }
}

/// Everything behind sign-in: shared data, the live connection, the theme and the routes.
#[component]
fn SignedIn(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    provide_context(shared::Context::new());
    let live = expect_context::<Live>();
    live.open(&session);
    intercept_links();
    use_appearance(&session);
    let route = crate::navigation::route_now();
    // The rail and top bar stay mounted; only the routed app below them is replaced.
    let frame = crate::shell::Frame::provide();
    let frame_view = view! { <crate::shell::AppFrame frame=frame session=session.clone() on_signed_out=on_signed_out/> };
    let session = StoredValue::new(session);
    let routed = move || {
        route.visit.track();
        let path = route.path.get();
        let session = session.get_value();
        match path.as_str() {
            "/contacts" | "/calendar" | "/identity" | "/settings" => {
                let app = crate::lazy_apps::LazyApp::from_kind(if path == "/settings" { "identity" } else { &path[1..] });
                view! { <crate::lazy_apps::LoadApp app=app.expect("listed above") session=session on_signed_out=on_signed_out/> }.into_any()
            }
            "/news" | "/mail" | "/chat" | "/photos" | "/books" | "/git" | "/agent" | "/notes" | "/tasks" => {
                let kind = path[1..].to_owned();
                view! { <OptionalWorkspace kind=kind session=session on_signed_out=on_signed_out/> }.into_any()
            }
            other if other.starts_with("/apps/") => {
                let id = other.split('/').nth(2).unwrap_or_default().to_owned();
                view! { <crate::installed_apps::InstalledAppPage id=id session=session on_signed_out=on_signed_out/> }.into_any()
            }
            _ => {
                crate::lazy_apps::preload_soon();
                view! { <crate::files::Files session=session on_signed_out=on_signed_out/> }.into_any()
            }
        }
    };
    view! { {frame_view}{routed} }
}

/// The theme follows the account's appearance preference, or the system.
fn use_appearance(session: &BrowserSession) {
    let key = format!("dar.appearance.{}", session.user.id);
    let saved = window().local_storage().ok().flatten().and_then(|s| s.get_item(&key).ok().flatten());
    let value = RwSignal::new(saved.unwrap_or_else(|| "system".into()));
    let preferences = context().preferences;
    let pref_appearance = Memo::new(move |_| preferences.value.with(|p| p.as_ref().map(|p| p.appearance.clone().unwrap_or_else(|| "system".into()))));
    Effect::new(move |_| {
        if let Some(next) = pref_appearance.get() {
            if let Some(s) = window().local_storage().ok().flatten() {
                let _ = s.set_item(&key, &next);
            }
            if value.with_untracked(|cur| *cur != next) {
                value.set(next);
            }
        }
    });
    Effect::new(move |_| apply_theme(Some(value.get())));
    if let Ok(Some(media)) = window().match_media("(prefers-color-scheme: dark)") {
        listen(&media, "change", false, move |_| apply_theme(Some(value.get_untracked())));
    }
}

fn apply_theme(value: Option<String>) {
    let value = value.unwrap_or_else(|| "system".into());
    let dark = window().match_media("(prefers-color-scheme: dark)").ok().flatten().is_some_and(|m| m.matches());
    let theme = if value == "system" { if dark { "mocha".to_owned() } else { "latte".to_owned() } } else { value };
    if let Some(root) = window().document().and_then(|d| d.document_element()) {
        let _ = root.set_attribute("data-theme", &theme);
    }
}

#[component]
pub fn Brand() -> impl IntoView {
    view! { <div class="brand"><span class="brand-mark" aria-hidden="true"></span><span>"dar"</span></div> }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Appearance {
    logo_url: Option<String>,
}

/// Sign-in, or the first account's setup (`auth.tsx`).
#[component]
fn Auth(initialized: bool, on_login: Callback<BrowserSession>) -> impl IntoView {
    let logo: RwSignal<Option<String>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(value) = api::get::<Appearance>("/api/v1/appearance").await {
            logo.set(value.logo_url);
        }
    });
    let submit = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(form) = event.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()) else { return };
        let data = web_sys::FormData::new_with_form(&form).expect("form data");
        let mut body = serde_json::Map::new();
        for name in ["displayName", "username", "password", "bootstrapToken"] {
            if let Some(value) = data.get(name).as_string() {
                body.insert(name.into(), value.into());
            }
        }
        busy.set(true);
        error.set(String::new());
        spawn_local(async move {
            let url = format!("/api/v1/auth/{}", if initialized { "login" } else { "setup" });
            match api::request::<BrowserSession>(&url, Req { method: "POST", body: Some(serde_json::Value::Object(body).to_string()), ..Default::default() }).await {
                Ok(session) => { on_login.try_run(session); },
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    };
    let label = if initialized { "Sign in" } else { "Set up Dar" };
    view! {
        <main class="auth-page">
            <section class="auth-card" aria-label=label>
                <div class="signin-brand">{move || match logo.get() {
                    Some(src) => view! { <img src=src alt="Dar" on:error=move |_| logo.set(None)/> }.into_any(),
                    None => view! { <Brand/> }.into_any(),
                }}</div>
                <h1>{if initialized { "Sign in" } else { "Set up Dar" }}</h1>
                <form on:submit=submit>
                    {(!initialized).then(|| view! { <label>"Display name"<input name="displayName" autocomplete="name" maxlength="100" required autofocus/></label> })}
                    <label>"Username"<input name="username" autocomplete="username" pattern="[a-zA-Z0-9][a-zA-Z0-9_.\\-]{2,63}" maxlength="64" required autofocus=initialized/></label>
                    <label>"Password"<input name="password" type="password" autocomplete=if initialized { "current-password" } else { "new-password" } minlength=if initialized { "1" } else { "15" } maxlength="1024" required/></label>
                    {(!initialized).then(|| view! {
                        <small class="field-hint">"Use a passphrase of at least 15 characters."</small>
                        <label>"Instance setup key"<input name="bootstrapToken" type="password" autocomplete="off" required/><small class="field-hint">"Available to the operator in the instance’s bootstrap-token file."</small></label>
                    })}
                    {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <button class="primary-button full" type="submit" disabled=move || busy.get()>{move || if busy.get() { "Signing in…" } else if initialized { "Sign in" } else { "Create your account" }}</button>
                </form>
            </section>
        </main>
    }
}

/// Notes, Tasks and the optional apps open unless an administrator disabled them (`OptionalWorkspace`).
#[component]
fn OptionalWorkspace(kind: String, session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let settings = context().app_settings;
    let kind = StoredValue::new(kind);
    let session = StoredValue::new(session);
    let enabled = Memo::new(move |_| {
        let kind = kind.get_value();
        settings.value.with(|s| s.as_ref().map(|s| !s.disabled.contains(&kind)))
    });
    move || {
        let kind = kind.get_value();
        let session = session.get_value();
        let is_enabled = enabled.get();
        if is_enabled == Some(true) {
            return match crate::lazy_apps::LazyApp::from_kind(&kind) {
                Some(app) => view! { <crate::lazy_apps::LoadApp app=app session=session on_signed_out=on_signed_out/> }.into_any(),
                None => ().into_any(),
            };
        }
        let error = settings.error.get();
        let body = if !error.is_empty() {
            view! { <h1>"Unable to load apps"</h1><p role="alert">{error}</p><button class="primary-button" on:click=|_| { let _ = window().location().reload(); }>"Try again"</button> }.into_any()
        } else if is_enabled.is_some() {
            view! { <h1>"App disabled"</h1><p>"This app has been disabled by an administrator."</p> }.into_any()
        } else {
            view! { <p role="status">"Opening app\u{2026}"</p> }.into_any()
        };
        view! {
            <div class="app-shell">
                <crate::shell::WorkspaceHeader app=kind session=session on_signed_out=on_signed_out/>
                <main class="startup-state">{body}<a href="/">"Return to Files"</a></main>
            </div>
        }
        .into_any()
    }
}
