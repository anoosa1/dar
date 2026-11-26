//! Shared app chrome (`apps.tsx`, `shell.tsx`, `global-search.tsx`): the app rail, the top
//! bar with the centered search trigger, the account menu and the spotlight palette.

use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession, Req};
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::navigation::navigate;
use crate::shared::{AppLauncherPrefs, context, save_preferences};
use crate::ui::{document, listen, unique_id, use_floating, window};

#[derive(Clone, Debug, PartialEq)]
pub struct AppEntry {
    pub id: String,
    pub label: String,
    pub icon: &'static str,
    pub href: String,
}

const APPS: [(&str, &str, &str, &str); 12] = [
    ("files", "Files", "folder", "/"),
    ("photos", "Photos", "image", "/photos"),
    ("books", "Books", "book", "/books"),
    ("contacts", "Contacts", "contacts", "/contacts"),
    ("calendar", "Calendar", "calendar", "/calendar"),
    ("notes", "Notes", "file", "/notes"),
    ("tasks", "Tasks", "check", "/tasks"),
    ("mail", "Mail", "mail", "/mail"),
    ("news", "News", "rss", "/news"),
    ("chat", "Chat", "chat", "/chat"),
    ("agent", "Agent", "agent", "/agent"),
    ("git", "Git", "branch", "/git"),
];

pub fn app_tint_class(id: &str) -> &'static str {
    match id {
        "files" => "app-tint-files",
        "photos" => "app-tint-photos",
        "books" => "app-tint-books",
        "contacts" => "app-tint-contacts",
        "calendar" => "app-tint-calendar",
        "notes" => "app-tint-notes",
        "tasks" => "app-tint-tasks",
        "mail" => "app-tint-mail",
        "news" => "app-tint-news",
        "chat" => "app-tint-chat",
        "agent" => "app-tint-agent",
        "git" => "app-tint-git",
        _ => "app-tint-other",
    }
}

/// Built-in apps the administrator has not disabled, followed by enabled installed apps.
pub fn app_list() -> Vec<AppEntry> {
    let shared = context();
    let disabled = shared
        .app_settings
        .value
        .with(|s| s.as_ref().map(|s| s.disabled.clone()).unwrap_or_default());
    let installed = shared
        .installed
        .value
        .with(|i| i.as_ref().map(|i| i.items.clone()).unwrap_or_default());
    APPS.iter()
        .filter(|(id, ..)| !disabled.iter().any(|d| d == id))
        .map(|(id, label, icon, href)| AppEntry {
            id: (*id).into(),
            label: (*label).into(),
            icon,
            href: (*href).into(),
        })
        .chain(installed.into_iter().map(|app| AppEntry {
            id: format!("app:{}", app.id),
            label: app.name,
            icon: "clock",
            href: format!("/apps/{}", app.id),
        }))
        .collect()
}

/// Orders `list` according to `prefs.order` and groups into user categories,
/// or a single `"Apps"` group when no custom categories are defined.
pub fn grouped_apps(
    list: &[AppEntry],
    prefs: Option<&AppLauncherPrefs>,
) -> Vec<(String, Vec<AppEntry>)> {
    let mut ordered: Vec<AppEntry> = Vec::with_capacity(list.len());
    if let Some(p) = prefs {
        for id in &p.order {
            if let Some(item) = list.iter().find(|a| &a.id == id)
                && !ordered.iter().any(|o| o.id == item.id)
            {
                ordered.push(item.clone());
            }
        }
    }
    for item in list {
        if !ordered.iter().any(|o| o.id == item.id) {
            ordered.push(item.clone());
        }
    }
    let Some(p) = prefs.filter(|p| !p.categories.is_empty()) else {
        return vec![("Apps".into(), ordered)];
    };
    let mut groups: Vec<(String, Vec<AppEntry>)> = Vec::new();
    let mut placed: Vec<String> = Vec::new();
    for cat in &p.categories {
        let mut items: Vec<AppEntry> = Vec::new();
        for app in &ordered {
            if cat.apps.iter().any(|id| id == &app.id) && !placed.iter().any(|id| id == &app.id) {
                placed.push(app.id.clone());
                items.push(app.clone());
            }
        }
        if !items.is_empty() {
            groups.push((cat.name.clone(), items));
        }
    }
    let remaining: Vec<AppEntry> = ordered
        .into_iter()
        .filter(|app| !placed.iter().any(|id| id == &app.id))
        .collect();
    if !remaining.is_empty() {
        let heading = if groups.is_empty() { "Apps" } else { "Other" };
        groups.push((heading.into(), remaining));
    }
    if groups.is_empty() {
        groups.push(("Apps".into(), Vec::new()));
    }
    groups
}

pub fn app_label(id: &str, list: &[AppEntry]) -> String {
    if id == "settings" {
        return "Settings".into();
    }
    list.iter()
        .find(|a| a.id == id)
        .map(|a| a.label.clone())
        .or_else(|| APPS.iter().find(|a| a.0 == id).map(|a| a.1.to_owned()))
        .unwrap_or_else(|| "App".into())
}

fn is_mobile_viewport() -> bool {
    window()
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .is_some_and(|w| w <= 800.0)
}

#[component]
pub fn Avatar(#[prop(into)] name: String) -> impl IntoView {
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .collect::<String>()
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase();
    view! { <span class="avatar" aria-hidden="true">{initials}</span> }
}

/// Renders the Direction-A 2-column compact app rows (with optional user categories or `"Apps"`).
#[component]
fn AppGroupsList(
    #[prop(into)] active: Signal<String>,
    on_pick: Callback<()>,
) -> impl IntoView {
    let groups = Memo::new(move |_| {
        let list = app_list();
        let prefs = context()
            .preferences
            .value
            .with(|p| p.as_ref().and_then(|p| p.app_launcher.clone()));
        grouped_apps(&list, prefs.as_ref())
    });
    view! {
        <div class="apps-groups">
            {move || {
                let current = active.get();
                groups
                    .get()
                    .into_iter()
                    .map(|(heading, items)| {
                        let heading_aria = heading.clone();
                        view! {
                            <section class="apps-group" aria-label=heading_aria>
                                <h3 class="apps-group-heading">{heading}</h3>
                                <div class="apps-group-grid">
                                    {items
                                        .into_iter()
                                        .map(|app| {
                                            let is_current = app.id == current;
                                            let tint = app_tint_class(&app.id);
                                            let href = app.href.clone();
                                            view! {
                                                <a
                                                    href=href
                                                    class=if is_current { "apps-menu-row is-current" } else { "apps-menu-row" }
                                                    aria-current=is_current.then_some("page")
                                                    on:click=move |_| { on_pick.try_run(()); }
                                                >
                                                    <span class=format!("apps-menu-badge {tint}") aria-hidden="true">
                                                        <Icon name=app.icon/>
                                                    </span>
                                                    <span class="apps-menu-label">{app.label}</span>
                                                    {is_current.then(|| view! { <span class="apps-menu-dot" aria-hidden="true"></span> })}
                                                </a>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                            </section>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// Desktop All Apps switcher trigger (between the Dar logo and app title) and its compact 2-column popover.
#[component]
fn AllAppsSwitcher(
    #[prop(into)] active: Signal<String>,
    administrator: bool,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let trigger = NodeRef::<html::Button>::new();
    let panel = NodeRef::<html::Div>::new();
    let id = unique_id();
    use_floating(open.into(), trigger, panel, false, 6.0);

    listen(&document(), "pointerdown", true, move |event| {
        if !open.get_untracked() {
            return;
        }
        let target = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
        let inside_trigger = trigger
            .get_untracked()
            .is_some_and(|el| target.as_ref().is_some_and(|n| el.contains(Some(n))));
        let inside_panel = panel
            .get_untracked()
            .is_some_and(|el| target.as_ref().is_some_and(|n| el.contains(Some(n))));
        if !inside_trigger && !inside_panel {
            open.set(false);
        }
    });
    listen(&document(), "keydown", false, move |event| {
        if !open.get_untracked() {
            return;
        }
        let e: web_sys::KeyboardEvent = event.unchecked_into();
        if e.key() == "Escape" {
            e.prevent_default();
            open.set(false);
            if let Some(btn) = trigger.get_untracked() {
                let _ = btn.focus();
            }
        }
    });

    let close = Callback::new(move |_| open.set(false));
    let id_sv = StoredValue::new(id);

    view! {
        <div class="topbar-apps-wrap">
            <button
                type="button"
                node_ref=trigger
                class="icon-button topbar-apps-trigger"
                aria-label="All apps"
                title="All apps"
                aria-haspopup="dialog"
                aria-controls=move || id_sv.get_value()
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|v| *v = !*v)
            >
                <Icon name="apps"/>
            </button>
            <Show when=move || open.get()>
                <Portal>
                    <div
                        node_ref=panel
                        id=id_sv.get_value()
                        role="dialog"
                        aria-label="All apps"
                        class="apps-menu-popover dar-popover-surface"
                    >
                        <AppGroupsList active=active on_pick=close/>
                        <div class="apps-menu-footer">
                            <span class="apps-menu-hint">"Jump with Ctrl K"</span>
                            <div class="apps-menu-footer-links">
                                <a href="/settings?section=appearance" on:click=move |_| open.set(false)>"Customize"</a>
                                {administrator.then(|| view! {
                                    <a href="/settings?section=installedApps" on:click=move |_| open.set(false)>"Manage apps"</a>
                                })}
                            </div>
                        </div>
                    </div>
                </Portal>
            </Show>
        </div>
    }
}

/// Inline Afterglow / Nightfall / System switcher used inside both the desktop User Menu and mobile User Hub.
#[component]
fn InlineThemeSwitch(session: BrowserSession) -> impl IntoView {
    let session = StoredValue::new(session);
    let busy = RwSignal::new(false);
    let current = Memo::new(move |_| {
        context()
            .preferences
            .value
            .with(|p| {
                p.as_ref()
                    .and_then(|p| p.appearance.clone())
                    .unwrap_or_else(|| "system".into())
            })
    });
    let pick = move |theme: &'static str| {
        if busy.get_untracked() {
            return;
        }
        let session = session.get_value();
        busy.set(true);
        spawn_local(async move {
            let shared = context().preferences;
            let rev = shared
                .value
                .with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
            let res = save_preferences(serde_json::json!({ "appearance": theme }), rev, &session).await;
            if let Ok(updated) = res {
                shared.value.set(Some(updated));
            } else if let Ok(fresh) = api::get::<crate::shared::Preferences>(crate::shared::PREFERENCES).await
                && let Ok(updated) = save_preferences(serde_json::json!({ "appearance": theme }), fresh.revision, &session).await
            {
                shared.value.set(Some(updated));
            }
            let _ = busy.try_set(false);
        });
    };
    view! {
        <div class="user-theme-switch" role="radiogroup" aria-label="Appearance">
            {[("latte", "Afterglow"), ("mocha", "Nightfall"), ("system", "System")]
                .into_iter()
                .map(|(val, label)| {
                    view! {
                        <button
                            type="button"
                            role="radio"
                            aria-checked=move || (current.get() == val).to_string()
                            class=move || if current.get() == val { "is-active" } else { "" }
                            disabled=move || busy.get()
                            on:click=move |_| pick(val)
                        >
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// Account trigger: opens the upgraded User Menu popover on desktop, and the full-screen
/// User & Apps Hub (apps grid on top, account options on bottom) on phones.
#[component]
pub fn WorkspaceAccount(
    session: BrowserSession,
    #[prop(into)] active: Signal<String>,
    on_signed_out: Callback<()>,
    #[prop(into)] class: String,
) -> impl IntoView {
    let open_desktop = RwSignal::new(false);
    let open_mobile = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let trigger = NodeRef::<html::Button>::new();
    let panel = NodeRef::<html::Div>::new();
    let menu_id = unique_id();
    use_floating(open_desktop.into(), trigger, panel, true, 6.0);

    let user = session.user.clone();
    let csrf = StoredValue::new(session.csrf_token.clone());
    let session_sv = StoredValue::new(session.clone());
    let administrator = user.administrator;
    let display_name = StoredValue::new(user.display_name.clone());
    let username = StoredValue::new(user.username.clone());

    listen(&document(), "pointerdown", true, move |event| {
        if !open_desktop.get_untracked() {
            return;
        }
        let target = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
        let inside_trigger = trigger
            .get_untracked()
            .is_some_and(|el| target.as_ref().is_some_and(|n| el.contains(Some(n))));
        let inside_panel = panel
            .get_untracked()
            .is_some_and(|el| target.as_ref().is_some_and(|n| el.contains(Some(n))));
        if !inside_trigger && !inside_panel {
            open_desktop.set(false);
        }
    });
    listen(&document(), "keydown", false, move |event| {
        let e: web_sys::KeyboardEvent = event.unchecked_into();
        if e.key() == "Escape" {
            if open_desktop.get_untracked() {
                e.prevent_default();
                open_desktop.set(false);
                if let Some(btn) = trigger.get_untracked() {
                    let _ = btn.focus();
                }
            } else if open_mobile.get_untracked() {
                e.prevent_default();
                open_mobile.set(false);
                if let Some(btn) = trigger.get_untracked() {
                    let _ = btn.focus();
                }
            }
        }
    });

    let sign_out = move || {
        open_desktop.set(false);
        open_mobile.set(false);
        let token = csrf.get_value();
        spawn_local(async move {
            match api::request::<serde_json::Value>(
                "/api/v1/auth/logout",
                Req {
                    method: "POST",
                    csrf: Some(&token),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    on_signed_out.try_run(());
                }
                Err(e) => error.set(e.message),
            }
        });
    };

    let close_mobile = Callback::new(move |_| open_mobile.set(false));
    let menu_id_sv = StoredValue::new(menu_id);

    view! {
        <div class=format!("account-area {class}")>
            <button
                type="button"
                node_ref=trigger
                class="user-avatar-trigger"
                aria-label="Open account menu"
                aria-haspopup="menu"
                aria-controls=move || menu_id_sv.get_value()
                aria-expanded=move || (open_desktop.get() || open_mobile.get()).to_string()
                on:click=move |_| {
                    if is_mobile_viewport() {
                        if let Some(body) = document().body() {
                            let _ = body.class_list().remove_1("mobile-nav-open");
                        }
                        open_desktop.set(false);
                        open_mobile.update(|v| *v = !*v);
                    } else {
                        open_mobile.set(false);
                        open_desktop.update(|v| *v = !*v);
                    }
                }
            >
                <Avatar name=display_name.get_value()/>
                <span class="sr-only">{display_name.get_value()}</span>
            </button>

            <Show when=move || open_desktop.get()>
                <Portal>
                    <div
                        node_ref=panel
                        id=menu_id_sv.get_value()
                        role="menu"
                        aria-label="Open account menu"
                        class="user-menu-popover dar-popover-surface"
                    >
                        <div class="user-menu-head">
                            <Avatar name=display_name.get_value()/>
                            <div class="user-menu-meta">
                                <strong>{display_name.get_value()}</strong>
                                <small>"@"{username.get_value()}</small>
                            </div>
                            {administrator.then(|| view! { <span class="user-role-pill">"Admin"</span> })}
                        </div>
                        <div class="user-menu-theme">
                            <span class="user-menu-section-label">"Appearance"</span>
                            <InlineThemeSwitch session=session_sv.get_value()/>
                        </div>
                        <div class="user-menu-list">
                            <a href="/settings" role="menuitem" class="ui-menu-item" on:click=move |_| open_desktop.set(false)>
                                <Icon name="settings"/><span>"Settings"</span>
                            </a>
                            <a href="/settings?section=appearance" role="menuitem" class="ui-menu-item" on:click=move |_| open_desktop.set(false)>
                                <Icon name="palette"/><span>"Personalization"</span>
                            </a>
                            <a href="/settings?section=security" role="menuitem" class="ui-menu-item" on:click=move |_| open_desktop.set(false)>
                                <Icon name="lock"/><span>"Account security"</span>
                            </a>
                            {administrator.then(|| view! {
                                <hr/>
                                <span class="ui-menu-section">"Administration"</span>
                                <a href="/settings?section=directory" role="menuitem" class="ui-menu-item" on:click=move |_| open_desktop.set(false)>
                                    <Icon name="shield"/><span>"Directory"</span>
                                </a>
                                <a href="/settings?section=installedApps" role="menuitem" class="ui-menu-item" on:click=move |_| open_desktop.set(false)>
                                    <Icon name="grid"/><span>"Manage apps"</span>
                                </a>
                            })}
                            <hr/>
                            <button type="button" role="menuitem" class="ui-menu-item danger" on:click=move |_| sign_out()>
                                <Icon name="logout"/><span>"Sign out"</span>
                            </button>
                        </div>
                    </div>
                </Portal>
            </Show>

            <Show when=move || open_mobile.get()>
                <Portal>
                    <div class="mobile-user-hub" role="dialog" aria-modal="true" aria-label="Apps and account">
                        <header class="mobile-user-hub-head">
                            <a href="/" class="mobile-user-hub-brand" on:click=move |_| open_mobile.set(false)>
                                <span class="topbar-brand-mark" aria-hidden="true">"D"</span>
                                <span>"Dar"</span>
                            </a>
                            <button type="button" class="icon-button mobile-user-hub-close" aria-label="Close account menu" on:click=move |_| open_mobile.set(false)>
                                <Icon name="close"/>
                            </button>
                        </header>
                        <div class="mobile-user-hub-apps">
                            <AppGroupsList active=active on_pick=close_mobile/>
                        </div>
                        <div class="mobile-user-hub-account">
                            <div class="user-menu-head">
                                <Avatar name=display_name.get_value()/>
                                <div class="user-menu-meta">
                                    <strong>{display_name.get_value()}</strong>
                                    <small>"@"{username.get_value()}</small>
                                </div>
                                {administrator.then(|| view! { <span class="user-role-pill">"Admin"</span> })}
                            </div>
                            <InlineThemeSwitch session=session_sv.get_value()/>
                            <div class="mobile-user-hub-actions">
                                <a href="/settings" class="mobile-hub-row" on:click=move |_| open_mobile.set(false)>
                                    <Icon name="settings"/><span>"Settings"</span><Icon name="chevron"/>
                                </a>
                                <a href="/settings?section=appearance" class="mobile-hub-row" on:click=move |_| open_mobile.set(false)>
                                    <Icon name="palette"/><span>"Personalization"</span><Icon name="chevron"/>
                                </a>
                                {administrator.then(|| view! {
                                    <a href="/settings?section=installedApps" class="mobile-hub-row" on:click=move |_| open_mobile.set(false)>
                                        <Icon name="grid"/><span>"Manage apps"</span><Icon name="chevron"/>
                                    </a>
                                })}
                                <button type="button" class="mobile-hub-row danger" on:click=move |_| sign_out()>
                                    <Icon name="logout"/><span>"Sign out"</span>
                                </button>
                            </div>
                        </div>
                    </div>
                </Portal>
            </Show>

            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        </div>
    }
}

/// The signed-in frame's state: which app is showing, its settings button, and mobile drawer state.
#[derive(Clone, Copy)]
pub struct Frame {
    app: RwSignal<String>,
    /// The settings button of the app that registered last, with its registration number.
    settings: RwSignal<(u64, Option<Callback<()>>)>,
    sequence: StoredValue<u64>,
    /// An installed app built with the SDK draws its own chrome.
    own_chrome: RwSignal<bool>,
    mobile_nav_open: RwSignal<bool>,
}

impl Frame {
    pub fn provide() -> Self {
        let frame = Frame {
            app: RwSignal::new(String::new()),
            settings: RwSignal::new((0, None)),
            sequence: StoredValue::new(0),
            own_chrome: RwSignal::new(false),
            mobile_nav_open: RwSignal::new(false),
        };
        provide_context(frame);
        frame
    }

    /// Closes the shared mobile slide-over hamburger drawer if open.
    pub fn close_mobile_nav() {
        if let Some(frame) = use_context::<Frame>() {
            let _ = frame.mobile_nav_open.try_set(false);
        }
        if let Some(body) = document().body() {
            let _ = body.class_list().remove_1("mobile-nav-open");
        }
    }

    /// While `showing` is true the page draws its own chrome, and the frame steps aside.
    pub fn own_chrome(showing: Signal<bool>) {
        let Some(frame) = use_context::<Frame>() else {
            return;
        };
        Effect::new(move |_| frame.own_chrome.set(showing.get()));
        on_cleanup(move || {
            let _ = frame.own_chrome.try_set(false);
        });
    }
}

/// Shared top bar rendered once by `AppFrame` (or standalone by `WorkspaceHeader` outside `AppFrame`).
#[component]
fn TopBar(
    #[prop(into)] active: Signal<String>,
    #[prop(into)] label: Signal<String>,
    #[prop(into)] has_settings: Signal<bool>,
    on_settings: Callback<()>,
    session: BrowserSession,
    on_signed_out: Callback<()>,
    mobile_nav_open: RwSignal<bool>,
) -> impl IntoView {
    Effect::new(move |_| {
        if let Some(body) = document().body() {
            let _ = body
                .class_list()
                .toggle_with_force("mobile-nav-open", mobile_nav_open.get());
        }
    });
    on_cleanup(|| {
        if let Some(body) = document().body() {
            let _ = body.class_list().remove_1("mobile-nav-open");
        }
    });
    // Close mobile drawer when a sidebar nav item is clicked or Escape is pressed.
    listen(&document(), "click", false, move |event| {
        if !mobile_nav_open.get_untracked() {
            return;
        }
        if let Some(el) = event
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            && el
                .closest(".sidebar .nav-item, .sidebar nav button:not(.ui-menu-trigger), .sidebar .new-button, .sidebar .ui-new, .sidebar .pim-sidebar-heading button, .sidebar .ui-app-settings-entry, .ui-settings-nav button, .agent-sidebar .agent-thread-select, .agent-sidebar .agent-new-thread, .agent-sidebar .agent-util-btn, [role='menuitem']")
                .ok()
                .flatten()
                .is_some()
        {
            mobile_nav_open.set(false);
        }
    });
    listen(&document(), "keydown", false, move |event| {
        if !mobile_nav_open.get_untracked() {
            return;
        }
        let e: web_sys::KeyboardEvent = event.unchecked_into();
        if e.key() == "Escape" {
            mobile_nav_open.set(false);
        }
    });
    listen(&window(), "popstate", false, move |_| {
        mobile_nav_open.set(false);
    });

    let administrator = session.user.administrator;
    view! {
        <header class=move || if has_settings.get() { "topbar has-settings" } else { "topbar" }>
            <button
                type="button"
                class="icon-button mobile-nav-trigger"
                aria-label="Open navigation"
                aria-expanded=move || mobile_nav_open.get().to_string()
                on:click=move |_| mobile_nav_open.update(|v| *v = !*v)
            >
                <Icon name="menu"/>
            </button>

            <div class="topbar-start">
                <a href="/" class="topbar-brand" aria-label="Dar home">
                    <span class="topbar-brand-mark" aria-hidden="true">"D"</span>
                </a>
                <AllAppsSwitcher active=active administrator=administrator/>
                <span class="topbar-title">{move || label.get()}</span>
            </div>

            <GlobalSearch account=session.user.id.clone()/>

            <div class="topbar-end">
                <div class="topbar-tools">
                    {move || has_settings.get().then(|| view! {
                        <button
                            type="button"
                            class="icon-button"
                            aria-label=move || format!("{} settings", label.get())
                            title=move || format!("{} settings", label.get())
                            on:click=move |_| { on_settings.try_run(()); }
                        >
                            <Icon name="settings"/>
                        </button>
                    })}
                </div>
                <WorkspaceAccount session=session.clone() active=active on_signed_out=on_signed_out class="topbar-account"/>
            </div>
        </header>
        <Show when=move || mobile_nav_open.get()>
            <Portal>
                <button
                    type="button"
                    class="mobile-nav-scrim"
                    aria-label="Close navigation"
                    on:click=move |_| mobile_nav_open.set(false)
                />
            </Portal>
        </Show>
    }
}

/// The persistent top bar for signed-in pages. Every app fills the full viewport width beneath it.
#[component]
pub fn AppFrame(
    frame: Frame,
    session: BrowserSession,
    on_signed_out: Callback<()>,
) -> impl IntoView {
    let label = Memo::new(move |_| {
        let app = frame.app.get();
        if app.is_empty() {
            String::new()
        } else {
            app_label(&app, &app_list())
        }
    });
    let has_settings = Memo::new(move |_| frame.settings.with(|(_, s)| s.is_some()));
    let shown = Memo::new(move |_| !frame.own_chrome.get());
    Effect::new(move |_| {
        if let Some(body) = document().body() {
            let _ = body
                .class_list()
                .toggle_with_force("has-app-frame", shown.get());
            let _ = body
                .class_list()
                .toggle_with_force("has-app-settings", shown.get() && has_settings.get());
        }
    });
    on_cleanup(|| {
        if let Some(body) = document().body() {
            let _ = body
                .class_list()
                .remove_3("has-app-frame", "has-app-settings", "mobile-nav-open");
        }
    });
    let session = StoredValue::new(session);
    move || {
        shown.get().then(|| {
            let session = session.get_value();
            let on_settings = Callback::new(move |()| {
                if let Some(open) = frame.settings.get_untracked().1 {
                    open.try_run(());
                }
            });
            view! {
                <div class="app-frame">
                    <TopBar
                        active=Signal::derive(move || frame.app.get())
                        label=Signal::derive(move || label.get())
                        has_settings=Signal::derive(move || has_settings.get())
                        on_settings=on_settings
                        session=session
                        on_signed_out=on_signed_out
                        mobile_nav_open=frame.mobile_nav_open
                    />
                </div>
            }
        })
    }
}

/// Registers the current app and its settings callback with `AppFrame` (or renders a standalone
/// `TopBar` when used outside `AppFrame`).
#[component]
pub fn WorkspaceHeader(
    #[prop(into)] app: String,
    session: BrowserSession,
    on_signed_out: Callback<()>,
    #[prop(optional, into)] on_settings: Option<Callback<()>>,
) -> impl IntoView {
    if let Some(frame) = use_context::<Frame>() {
        let id = frame.sequence.get_value() + 1;
        frame.sequence.set_value(id);
        frame.app.set(app);
        frame.settings.set((id, on_settings));
        frame.mobile_nav_open.set(false);
        on_cleanup(move || {
            if frame
                .settings
                .try_with_untracked(|(current, _)| *current == id)
                == Some(true)
            {
                let _ = frame.settings.try_set((id, None));
            }
        });
        return ().into_any();
    }
    let app_sig = Signal::stored(app.clone());
    let label = Memo::new(move |_| app_label(&app_sig.get(), &app_list()));
    let has_settings = Signal::stored(on_settings.is_some());
    let mobile_nav_open = RwSignal::new(false);
    let settings_cb = Callback::new(move |()| {
        if let Some(open) = on_settings {
            open.try_run(());
        }
    });
    view! {
        <TopBar
            active=app_sig
            label=Signal::derive(move || label.get())
            has_settings=has_settings
            on_settings=settings_cb
            session=session
            on_signed_out=on_signed_out
            mobile_nav_open=mobile_nav_open
        />
    }
    .into_any()
}

fn is_mac() -> bool {
    window()
        .navigator()
        .platform()
        .map(|p| p.contains("Mac") || p.contains("iPhone") || p.contains("iPad"))
        .unwrap_or(false)
}

/// The top-bar search trigger; Ctrl/Cmd+K or a click opens the spotlight palette.
#[component]
pub fn GlobalSearch(#[prop(into)] account: String) -> impl IntoView {
    let open = RwSignal::new(false);
    listen(&document(), "keydown", false, move |event| {
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        let blocked = document()
            .query_selector("dialog[open]:not(.spotlight),[data-editor-pane]")
            .ok()
            .flatten()
            .is_some();
        if (event.ctrl_key() || event.meta_key())
            && !event.alt_key()
            && event.key().to_lowercase() == "k"
            && !event.default_prevented()
            && !blocked
        {
            event.prevent_default();
            open.set(true);
            if let Some(input) = document()
                .query_selector(".spotlight input")
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                input.select();
            }
        }
    });
    let mac = is_mac();
    let account = StoredValue::new(account);
    view! {
        <div class="global-search-wrap">
            <button type="button" class="global-search spotlight-trigger" aria-label="Open search" aria-haspopup="dialog" aria-keyshortcuts=if mac { "Meta+K" } else { "Control+K" } on:click=move |_| open.set(true)>
                <Icon name="search"/><span>"Search all apps…"</span><kbd class="search-shortcut" aria-hidden="true">{if mac { "⌘ K" } else { "Ctrl K" }}</kbd>
            </button>
            <Show when=move || open.get()>
                <Spotlight account=account.get_value() on_close=Callback::new(move |_| open.set(false))/>
            </Show>
        </div>
    }
}

const ORDER: [&str; 9] = [
    "Photos",
    "Files",
    "Contacts",
    "Calendar",
    "Notes",
    "Tasks",
    "News",
    "Chat",
    "Applications",
];

fn group_icon(group: &str) -> &'static str {
    match group {
        "Photos" => "image",
        "Files" => "file",
        "Contacts" => "contacts",
        "Calendar" => "calendar",
        "Notes" => "file",
        "Tasks" => "check",
        "News" => "rss",
        "Chat" => "chat",
        "Applications" => "grid",
        "Actions" => "chevron",
        "Recent" => "undo",
        _ => "grid",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub group: String,
    pub title: String,
    pub detail: String,
    pub href: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct SearchResults {
    items: Vec<SearchResult>,
    truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Recent {
    queries: Vec<String>,
    opened: Vec<SearchResult>,
}

/// One selectable palette row: a search result, a recently opened result or a command.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    key: String,
    section: String,
    title: String,
    detail: String,
    href: String,
    group: Option<String>,
    thumbnail: Option<String>,
    result: Option<SearchResult>,
}

fn storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

fn read_recent(key: &str) -> Recent {
    let mut value: Recent = storage()
        .and_then(|s| s.get_item(key).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    value.queries.truncate(6);
    value.opened.retain(|r| r.href.starts_with('/'));
    value.opened.truncate(5);
    value
}

fn command(id: &str, title: String, detail: &str, href: String) -> Entry {
    Entry {
        key: format!("go:{id}"),
        section: "Actions".into(),
        title,
        detail: detail.into(),
        href,
        group: None,
        thumbnail: None,
        result: None,
    }
}

#[component]
fn Spotlight(account: String, on_close: Callback<()>) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let scope = RwSignal::new("All".to_owned());
    let result = RwSignal::new(SearchResults::default());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let retry = RwSignal::new(0u32);
    let active = RwSignal::new(0usize);
    let key = format!("dar:search:{account}");
    let recent = RwSignal::new(read_recent(&key));
    let key = StoredValue::new(key);
    let dialog = NodeRef::<html::Dialog>::new();
    let input = NodeRef::<html::Input>::new();
    let id = StoredValue::new(unique_id());
    let mac = is_mac();

    let previous = send_wrapper::SendWrapper::new(crate::ui::active_element());
    on_cleanup(move || {
        if let Some(previous) = previous.take().filter(|p| p.is_connected()) {
            let _ = previous.focus();
        }
    });
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            if !dialog.open() {
                let _ = dialog.show_modal();
            }
            if let Some(input) = input.get_untracked() {
                let _ = input.focus();
            }
        }
    });
    let command_mode = Memo::new(move |_| query.get().starts_with('>'));
    let term = Memo::new(move |_| {
        let q = query.get();
        q.strip_prefix('>').unwrap_or(&q).trim().to_owned()
    });
    // Searches 160 ms after typing stops; a newer query supersedes an older one.
    let sequence = RwSignal::new(0u32);
    Effect::new(move |_| {
        retry.track();
        let (term, command_mode) = (term.get(), command_mode.get());
        let id = sequence.get_untracked() + 1;
        sequence.set(id);
        result.set(SearchResults::default());
        error.set(String::new());
        if command_mode || term.is_empty() {
            loading.set(false);
            return;
        }
        loading.set(true);
        spawn_local(async move {
            crate::sleep(160).await;
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            let url = format!("/api/v1/search?q={}", js_sys::encode_uri_component(&term));
            let value = api::get::<SearchResults>(&url).await;
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            match value {
                Ok(value) => result.set(value),
                Err(e) => error.set(e.message),
            }
            loading.set(false);
        });
    });
    let commands = Memo::new(move |_| {
        let mut list: Vec<Entry> = app_list()
            .into_iter()
            .map(|app| {
                command(
                    &app.id,
                    format!("Go to {}", app.label),
                    "Open app",
                    app.href,
                )
            })
            .collect();
        list.push(command(
            "settings",
            "Open Dar settings".into(),
            "Settings",
            "/settings".into(),
        ));
        list.push(command(
            "appearance",
            "Appearance settings".into(),
            "Theme and colors",
            "/settings?section=appearance".into(),
        ));
        list.push(command(
            "security",
            "Account security".into(),
            "Passwords and app passwords",
            "/settings?section=security".into(),
        ));
        list.push(command(
            "regional",
            "Regional settings".into(),
            "Time zone",
            "/settings?section=regional".into(),
        ));
        list
    });
    let counts = Memo::new(move |_| {
        let mut counts: Vec<(String, usize)> = Vec::new();
        result.with(|r| {
            for item in &r.items {
                match counts.iter_mut().find(|(g, _)| *g == item.group) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((item.group.clone(), 1)),
                }
            }
        });
        counts
    });
    let count_of =
        move |group: &str| counts.with(|c| c.iter().find(|(g, _)| g == group).map(|(_, n)| *n));
    let scopes = Memo::new(move |_| {
        ORDER
            .iter()
            .filter(|g| count_of(g).is_some())
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
    });
    let effective = Memo::new(move |_| {
        let s = scope.get();
        if s != "All" && count_of(&s).is_some() {
            s
        } else {
            "All".into()
        }
    });
    let entries = Memo::new(move |_| {
        let term = term.get();
        let matches = |e: &Entry| e.title.to_lowercase().contains(&term.to_lowercase());
        if command_mode.get() {
            return commands
                .get()
                .into_iter()
                .filter(matches)
                .collect::<Vec<_>>();
        }
        if term.is_empty() {
            let mut list: Vec<Entry> = recent
                .get()
                .opened
                .into_iter()
                .map(|item| Entry {
                    key: format!("recent:{}", item.id),
                    section: "Jump back in".into(),
                    title: item.title.clone(),
                    detail: format!("{} · {}", item.group, item.detail),
                    href: item.href.clone(),
                    group: Some(item.group.clone()),
                    thumbnail: item.thumbnail.clone(),
                    result: Some(item),
                })
                .collect();
            list.extend(commands.get().into_iter().take(4));
            return list;
        }
        let scope = effective.get();
        let mut list: Vec<Entry> = if scope == "All" {
            commands.get().into_iter().filter(matches).take(3).collect()
        } else {
            Vec::new()
        };
        let items = result.get().items;
        for group in ORDER.iter().filter(|g| scope == "All" || **g == scope) {
            for item in items.iter().filter(|i| i.group == *group) {
                list.push(Entry {
                    key: format!("{}:{}", item.id, item.group),
                    section: item.group.clone(),
                    title: item.title.clone(),
                    detail: item.detail.clone(),
                    href: item.href.clone(),
                    group: Some(item.group.clone()),
                    thumbnail: item.thumbnail.clone(),
                    result: Some(item.clone()),
                });
            }
        }
        list
    });
    Effect::new(move |_| {
        query.track();
        effective.track();
        result.track();
        active.set(0);
    });
    let current_index = move || {
        entries.with(|e| {
            if e.is_empty() {
                None
            } else {
                Some(active.get().min(e.len() - 1))
            }
        })
    };
    let current = move || current_index().and_then(|i| entries.with(|e| e.get(i).cloned()));
    let row_id = move |index: usize| format!("{}-{index}", id.get_value());
    {
        Effect::new(move |_| {
            let index = active.get();
            if let Some(el) = document().get_element_by_id(&row_id(index)) {
                let options = web_sys::ScrollIntoViewOptions::new();
                options.set_block(web_sys::ScrollLogicalPosition::Nearest);
                el.scroll_into_view_with_scroll_into_view_options(&options);
            }
        });
    }
    let remember = move |entry: &Entry| {
        let term = term.get_untracked();
        let mut value = recent.get_untracked();
        if !term.is_empty() && !command_mode.get_untracked() {
            value.queries.retain(|q| *q != term);
            value.queries.insert(0, term);
            value.queries.truncate(6);
        }
        if let Some(result) = &entry.result {
            value.opened.retain(|r| r.href != entry.href);
            value.opened.insert(0, result.clone());
            value.opened.truncate(5);
        }
        if let (Some(s), Ok(text)) = (storage(), serde_json::to_string(&value)) {
            let _ = s.set_item(&key.get_value(), &text);
        }
    };
    let choose = move |entry: Option<Entry>, new_tab: bool| {
        let Some(entry) = entry else { return };
        remember(&entry);
        if new_tab {
            let _ =
                window().open_with_url_and_target_and_features(&entry.href, "_blank", "noopener");
            return;
        }
        {
            on_close.try_run(());
        };
        navigate(&entry.href);
    };
    let cycle_scope = move |step: i64| {
        let mut list = vec!["All".to_owned()];
        list.extend(scopes.get_untracked());
        if list.len() < 2 {
            return false;
        }
        let len = list.len() as i64;
        let index = list
            .iter()
            .position(|s| *s == effective.get_untracked())
            .unwrap_or(0) as i64;
        scope.set(list[((index + step + len) % len) as usize].clone());
        true
    };
    let status = move || {
        if command_mode.get() {
            return if entries.with(Vec::is_empty) {
                "No matching commands."
            } else {
                ""
            };
        }
        if term.with(String::is_empty) {
            ""
        } else if loading.get() {
            "Searching all apps…"
        } else if !error.with(String::is_empty) {
            "Search could not be completed."
        } else if result.with(|r| r.items.is_empty()) {
            "No matches across your apps."
        } else if result.with(|r| r.truncated) {
            "Showing the first matches. Refine your search for more."
        } else {
            ""
        }
    };
    let focus_input = move || {
        if let Some(i) = input.get_untracked() {
            let _ = i.focus();
        }
    };
    let list_id = move || id.get_value();
    let row_id_for_input = row_id;
    view! {
        <Portal>
            <dialog node_ref=dialog class="spotlight" aria-label="Search"
                on:cancel=move |e: web_sys::Event| { e.prevent_default(); if !query.get_untracked().is_empty() { query.set(String::new()); scope.set("All".into()); } else { { on_close.try_run(()); } } }
                on:click=move |e| { if let (Some(t), Some(d)) = (e.target(), dialog.get_untracked()) && &t == AsRef::<web_sys::EventTarget>::as_ref(&*d) { { on_close.try_run(()); } } }>
                <div class="spotlight-panel">
                    <div class="spotlight-input"><Icon name="search"/>
                        <input node_ref=input type="search" role="combobox" aria-label="Search all apps" aria-autocomplete="list" aria-controls=list_id()
                            aria-expanded=move || (!entries.with(Vec::is_empty)).to_string()
                            aria-activedescendant=move || current_index().map(&row_id_for_input)
                            placeholder="Search or type > for commands" maxlength="200" prop:value=move || query.get() autocomplete="off" spellcheck="false"
                            on:input=move |e| query.set(event_target_value(&e))
                            on:keydown=move |e| {
                                let key = e.key();
                                let len = entries.with(Vec::len);
                                if key == "ArrowDown" || key == "ArrowUp" {
                                    e.prevent_default();
                                    if len > 0 { active.update(|a| { let a2 = (*a).min(len - 1) as i64; *a = ((a2 + if key == "ArrowDown" { 1 } else { -1 } + len as i64) % len as i64) as usize; }); }
                                } else if key == "Enter" && current().is_some() { e.prevent_default(); choose(current(), e.ctrl_key() || e.meta_key()); }
                                else if key == "Tab" && !term.get_untracked().is_empty() && !command_mode.get_untracked() { if cycle_scope(if e.shift_key() { -1 } else { 1 }) { e.prevent_default(); } }
                                else if key == "Backspace" && query.get_untracked() == ">" { e.prevent_default(); query.set(String::new()); }
                            }/>
                        {move || if query.with(String::is_empty) {
                            view! { <button type="button" class="spotlight-esc" aria-label="Close search" on:click=move |_| { on_close.try_run(()); }>"Esc"</button> }.into_any()
                        } else {
                            view! { <button type="button" class="icon-button" aria-label="Clear search" on:click=move |_| { query.set(String::new()); scope.set("All".into()); focus_input(); }><Icon name="close"/></button> }.into_any()
                        }}
                    </div>
                    {move || (!term.with(String::is_empty) && !command_mode.get() && !scopes.with(Vec::is_empty)).then(|| {
                        let mut list = vec!["All".to_owned()];
                        list.extend(scopes.get());
                        view! {
                            <div class="spotlight-scopes" role="toolbar" aria-label="Filter by app">
                                {list.into_iter().map(|group| {
                                    let count = if group == "All" { result.with(|r| r.items.len()) } else { count_of(&group).unwrap_or(0) };
                                    let pressed = effective.get() == group;
                                    let chosen = group.clone();
                                    view! {
                                        <button type="button" class="spotlight-scope" aria-pressed=pressed.to_string() on:click=move |_| { scope.set(chosen.clone()); focus_input(); }>
                                            {(group != "All").then(|| view! { <Icon name=group_icon(&group)/> })}{group.clone()}<b>{count}</b>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        }
                    })}
                    <div class="spotlight-body">
                        <div class="spotlight-results">
                            {move || (term.with(String::is_empty) && !command_mode.get() && !recent.with(|r| r.queries.is_empty())).then(|| view! {
                                <div class="spotlight-recent">
                                    <h2>"Recent searches"<button type="button" class="text-button" on:click=move |_| { if let Some(s) = storage() { let _ = s.remove_item(&key.get_value()); } recent.set(Recent::default()); focus_input(); }>"Clear"</button></h2>
                                    <div>{recent.get().queries.into_iter().map(|q| { let chosen = q.clone(); view! { <button type="button" class="spotlight-chip" on:click=move |_| { query.set(chosen.clone()); focus_input(); }><Icon name="undo"/>{q}</button> } }).collect_view()}</div>
                                </div>
                            })}
                            <div role="status" aria-live="polite" class="spotlight-status">{status}</div>
                            {move || (!error.with(String::is_empty)).then(|| view! { <div class="error"><p role="alert">{error.get()}</p><button class="text-button" on:click=move |_| retry.update(|n| *n += 1)>"Retry search"</button></div> })}
                            <div role="listbox" id=list_id() aria-label="Search results">
                                {move || {
                                    let entries = entries.get();
                                    let mut sections: Vec<String> = Vec::new();
                                    for e in &entries { if !sections.contains(&e.section) { sections.push(e.section.clone()); } }
                                    let term = term.get();
                                    let current = current_index();
                                    sections.into_iter().map(|section| {
                                        let count = (section != "Actions" && section != "Jump back in").then(|| view! { <span>{count_of(&section)}</span> });
                                        let rows = entries.iter().enumerate().filter(|(_, e)| e.section == section).map(|(index, entry)| {
                                            let chosen = entry.clone();
                                            view! {
                                                <a href=entry.href.clone() role="option" aria-selected=(current == Some(index)).to_string() id=row_id(index) tabindex="-1" class="global-search-result"
                                                    on:mousemove=move |_| active.set(index)
                                                    on:click=move |e| { e.prevent_default(); choose(Some(chosen.clone()), e.ctrl_key() || e.meta_key()); }>
                                                    <Glyph entry=entry.clone()/>
                                                    <span><strong><Highlight text=entry.title.clone() query=term.clone()/></strong><small><Highlight text=entry.detail.clone() query=term.clone()/></small></span>
                                                    <Icon name="chevron" class="spotlight-enter"/>
                                                </a>
                                            }
                                        }).collect_view();
                                        view! { <div role="group" aria-label=section.clone()><h2>{section.clone()}{count}</h2>{rows}</div> }
                                    }).collect_view()
                                }}
                            </div>
                        </div>
                        {move || current().map(|entry| {
                            let has_image = entry.thumbnail.is_some();
                            let (open_entry, tab_entry) = (entry.clone(), entry.clone());
                            view! {
                                <aside class="spotlight-preview" aria-label="Preview">
                                    <div class=if has_image { "spotlight-preview-art has-image" } else { "spotlight-preview-art" }>
                                        {match entry.thumbnail.clone() { Some(src) => view! { <img src=src alt=""/> }.into_any(), None => view! { <Glyph entry=entry.clone()/> }.into_any() }}
                                    </div>
                                    <h3>{entry.title.clone()}</h3><p>{entry.detail.clone()}</p><p class="spotlight-preview-app">{entry.group.clone().unwrap_or_else(|| "Command".into())}</p>
                                    <div class="spotlight-preview-actions">
                                        <button type="button" class="primary-button" on:click=move |_| choose(Some(open_entry.clone()), false)>"Open"</button>
                                        {entry.group.is_some().then(|| view! { <button type="button" class="secondary-button" on:click=move |_| choose(Some(tab_entry.clone()), true)>"New tab"</button> })}
                                    </div>
                                </aside>
                            }
                        })}
                    </div>
                    <div class="spotlight-footer" aria-hidden="true">
                        <span><kbd>"↑↓"</kbd>"Move"</span><span><kbd>"↵"</kbd>"Open"</span><span><kbd>"Tab"</kbd>"Filter app"</span>
                        <span><kbd>{if mac { "⌘ ↵" } else { "Ctrl ↵" }}</kbd>"New tab"</span><span class="spotlight-footer-end">"Type "<kbd>">"</kbd>" for commands"</span>
                    </div>
                </div>
            </dialog>
        </Portal>
    }
}

#[component]
fn Glyph(entry: Entry) -> impl IntoView {
    if let Some(src) = entry.thumbnail {
        return view! { <span class="global-search-glyph glyph-photos"><img src=src alt=""/></span> }.into_any();
    }
    let group = entry.group.clone().unwrap_or_else(|| "Actions".into());
    let inner = match group.as_str() {
        "Contacts" => {
            let initials: String = entry.title.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
            view! { {if initials.is_empty() { "?".to_owned() } else { initials }} }.into_any()
        }
        "Files" => view! { <FileTile kind=if entry.detail == "Folder" { "folder" } else { "file" } name=entry.title.clone() media_type=""/> }.into_any(),
        _ => view! { <Icon name=group_icon(&group)/> }.into_any(),
    };
    view! { <span class=format!("global-search-glyph glyph-{}", group.to_lowercase())>{inner}</span> }.into_any()
}

/// Marks the first case-insensitive occurrence of the query.
#[component]
fn Highlight(text: String, query: String) -> impl IntoView {
    let needle = query.trim().to_lowercase();
    let lower = text.to_lowercase();
    match (!needle.is_empty())
        .then(|| lower.find(&needle))
        .flatten()
        .filter(|i| {
            lower.len() == text.len()
                && text.is_char_boundary(*i)
                && text.is_char_boundary(*i + needle.len())
        }) {
        None => view! { {text} }.into_any(),
        Some(index) => {
            let end = index + needle.len();
            view! { {text[..index].to_owned()}<mark>{text[index..end].to_owned()}</mark>{text[end..].to_owned()} }.into_any()
        }
    }
}
