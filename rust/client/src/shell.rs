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
use crate::shared::context;
use crate::ui::{document, listen, unique_id, window, ActionMenu, MenuAction};

#[derive(Clone, Debug, PartialEq)]
pub struct AppEntry {
    pub id: String,
    pub label: String,
    pub icon: &'static str,
    pub href: String,
}

const APPS: [(&str, &str, &str, &str); 11] = [
    ("files", "Files", "folder", "/"),
    ("photos", "Photos", "image", "/photos"),
    ("books", "Books", "book", "/books"),
    ("contacts", "Contacts", "contacts", "/contacts"),
    ("calendar", "Calendar", "calendar", "/calendar"),
    ("notes", "Notes", "file", "/notes"),
    ("tasks", "Tasks", "check", "/tasks"),
    ("news", "News", "rss", "/news"),
    ("chat", "Chat", "chat", "/chat"),
    ("agent", "Agent", "agent", "/agent"),
    ("git", "Git", "branch", "/git"),
];

/// Built-in apps the administrator has not disabled, followed by enabled installed apps.
pub fn app_list() -> Vec<AppEntry> {
    let shared = context();
    let disabled = shared.app_settings.value.with(|s| s.as_ref().map(|s| s.disabled.clone()).unwrap_or_default());
    let installed = shared.installed.value.with(|i| i.as_ref().map(|i| i.items.clone()).unwrap_or_default());
    APPS.iter()
        .filter(|(id, ..)| !disabled.iter().any(|d| d == id))
        .map(|(id, label, icon, href)| AppEntry { id: (*id).into(), label: (*label).into(), icon, href: (*href).into() })
        .chain(installed.into_iter().map(|app| AppEntry { id: format!("app:{}", app.id), label: app.name, icon: "clock", href: format!("/apps/{}", app.id) }))
        .collect()
}

pub fn app_label(id: &str, list: &[AppEntry]) -> String {
    if id == "settings" {
        return "Settings".into();
    }
    list.iter().find(|a| a.id == id).map(|a| a.label.clone()).or_else(|| APPS.iter().find(|a| a.0 == id).map(|a| a.1.to_owned())).unwrap_or_else(|| "App".into())
}

/// Phones show this many apps in the bottom bar; the rest are under More.
const PHONE_SLOTS: usize = 4;

#[component]
pub fn Avatar(#[prop(into)] name: String) -> impl IntoView {
    let initials: String = name.split_whitespace().filter_map(|w| w.chars().next()).collect::<String>().chars().take(2).collect::<String>().to_uppercase();
    view! { <span class="avatar" aria-hidden="true">{initials}</span> }
}

/// The persistent app rail; on phones a bottom bar with four apps and a More menu.
#[component]
pub fn AppRail(#[prop(into)] active: String, administrator: bool, account: ChildrenFn) -> impl IntoView {
    let active = StoredValue::new(active);
    let rail = move || {
        let list = app_list();
        let active = active.get_value();
        let active_index = list.iter().position(|a| a.id == active);
        let on_phone: Vec<bool> = (0..list.len()).map(|i| i < PHONE_SLOTS - usize::from(active_index.is_some_and(|a| a >= PHONE_SLOTS)) || Some(i) == active_index).collect();
        let overflow: Vec<AppEntry> = list.iter().zip(&on_phone).filter(|(_, p)| !**p).map(|(a, _)| a.clone()).collect();
        let more_active = active == "settings";
        let mut more: Vec<MenuAction> = overflow.iter().map(|a| MenuAction::new(a.label.clone(), a.icon).href(a.href.clone()).current(a.id == active)).collect();
        let mut settings = MenuAction::new("Settings", "settings").href("/settings").current(active == "settings");
        settings.separator = !overflow.is_empty();
        more.push(settings);
        if administrator {
            more.push(MenuAction::new("Manage apps", "grid").href("/settings?section=installedApps"));
        }
        let items = list.into_iter().zip(on_phone).map(|(app, phone)| {
            let current = (app.id == active).then_some("page");
            view! {
                <a href=app.href class=if phone { "app-rail-item" } else { "app-rail-item app-rail-overflow" } aria-current=current>
                    <span class="app-rail-icon"><Icon name=app.icon/></span><span class="app-rail-label">{app.label}</span>
                </a>
            }
        }).collect_view();
        let more = StoredValue::new(more);
        view! {
            {items}
            <ActionMenu label="More apps" class=if more_active { "app-rail-more is-current" } else { "app-rail-more" } actions=move |_| more.get_value()>
                <span class="app-rail-icon"><Icon name="grid"/></span><span class="app-rail-label">"More"</span>
            </ActionMenu>
        }
    };
    view! {
        <nav class="app-rail" aria-label="Applications">
            <a href="/" class="app-rail-brand" aria-label="Dar home"><span aria-hidden="true">"D"</span></a>
            <div class="app-rail-apps">{rail}</div>
            <div class="app-rail-end">
                <a href="/settings" class="app-rail-item app-rail-settings" aria-label="Settings" title="Settings" aria-current=move || (active.get_value() == "settings").then_some("page")><span class="app-rail-icon"><Icon name="settings"/></span></a>
                {account()}
            </div>
        </nav>
    }
}

/// The same account actions are available from the rail and the phone top bar.
#[component]
pub fn WorkspaceAccount(session: BrowserSession, on_signed_out: Callback<()>, #[prop(into)] class: String) -> impl IntoView {
    let error = RwSignal::new(String::new());
    let user = session.user.clone();
    let csrf = StoredValue::new(session.csrf_token.clone());
    let administrator = user.administrator;
    let actions = move |_| {
        let mut list = vec![MenuAction::new("Settings", "settings").href("/settings"), MenuAction::new("Account security", "lock").href("/settings?section=security")];
        if administrator {
            list.push(MenuAction::new("Directory", "shield").href("/settings?section=directory").section("Administration"));
            list.push(MenuAction::new("Manage apps", "grid").href("/settings?section=installedApps"));
        }
        list.push(MenuAction::new("Sign out", "logout").separator().run(move || {
            spawn_local(async move {
                match api::request::<serde_json::Value>("/api/v1/auth/logout", Req { method: "POST", csrf: Some(&csrf.get_value()), ..Default::default() }).await {
                    Ok(_) => { on_signed_out.try_run(()); },
                    Err(e) => error.set(e.message),
                }
            });
        }));
        list
    };
    let (name, username) = (user.display_name.clone(), user.username.clone());
    let heading: ChildrenFn = std::sync::Arc::new(move || {
        let (name, username) = (name.clone(), username.clone());
        view! { <Avatar name=name.clone()/><span><strong>{name}</strong><small>"@"{username}</small></span> }.into_any()
    });
    view! {
        <div class=format!("account-area {class}")>
            <ActionMenu label="Open account menu" class="ui-account" heading=heading actions=actions>
                <Avatar name=user.display_name.clone()/><span class="sr-only">{user.display_name.clone()}</span>
            </ActionMenu>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
        </div>
    }
}

/// The app rail and the top bar with the app title, search and app-level tools.
#[component]
pub fn WorkspaceHeader(#[prop(into)] app: String, session: BrowserSession, on_signed_out: Callback<()>, #[prop(optional, into)] on_settings: Option<Callback<()>>) -> impl IntoView {
    let label = {
        let app = app.clone();
        move || app_label(&app, &app_list())
    };
    let label = Memo::new(move |_| label());
    let (rail_session, top_session) = (session.clone(), session.clone());
    let account: ChildrenFn = std::sync::Arc::new(move || view! { <WorkspaceAccount session=rail_session.clone() on_signed_out=on_signed_out class="rail-account"/> }.into_any());
    view! {
        <AppRail active=app administrator=session.user.administrator account=account/>
        <header class=if on_settings.is_some() { "topbar has-settings" } else { "topbar" }>
            <span class="topbar-title">{move || label.get()}</span>
            <GlobalSearch account=session.user.id.clone()/>
            <div class="topbar-tools">
                {on_settings.map(|open| view! {
                    <button type="button" class="icon-button" aria-label=move || format!("{} settings", label.get()) title=move || format!("{} settings", label.get()) on:click=move |_| open.run(())><Icon name="settings"/></button>
                })}
            </div>
            <WorkspaceAccount session=top_session on_signed_out=on_signed_out class="topbar-account"/>
        </header>
    }
}

fn is_mac() -> bool {
    window().navigator().platform().map(|p| p.contains("Mac") || p.contains("iPhone") || p.contains("iPad")).unwrap_or(false)
}

/// The top-bar search trigger; Ctrl/Cmd+K or a click opens the spotlight palette.
#[component]
pub fn GlobalSearch(#[prop(into)] account: String) -> impl IntoView {
    let open = RwSignal::new(false);
    listen(&document(), "keydown", false, move |event| {
        let event: web_sys::KeyboardEvent = event.unchecked_into();
        let blocked = document().query_selector("dialog[open]:not(.spotlight),[data-editor-pane]").ok().flatten().is_some();
        if (event.ctrl_key() || event.meta_key()) && !event.alt_key() && event.key().to_lowercase() == "k" && !event.default_prevented() && !blocked {
            event.prevent_default();
            open.set(true);
            if let Some(input) = document().query_selector(".spotlight input").ok().flatten().and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()) {
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

const ORDER: [&str; 9] = ["Photos", "Files", "Contacts", "Calendar", "Notes", "Tasks", "News", "Chat", "Applications"];

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
    let mut value: Recent = storage().and_then(|s| s.get_item(key).ok().flatten()).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    value.queries.truncate(6);
    value.opened.retain(|r| r.href.starts_with('/'));
    value.opened.truncate(5);
    value
}

fn command(id: &str, title: String, detail: &str, href: String) -> Entry {
    Entry { key: format!("go:{id}"), section: "Actions".into(), title, detail: detail.into(), href, group: None, thumbnail: None, result: None }
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
        let mut list: Vec<Entry> = app_list().into_iter().map(|app| command(&app.id, format!("Go to {}", app.label), "Open app", app.href)).collect();
        list.push(command("settings", "Open Dar settings".into(), "Settings", "/settings".into()));
        list.push(command("appearance", "Appearance settings".into(), "Theme and colors", "/settings?section=appearance".into()));
        list.push(command("security", "Account security".into(), "Passwords and app passwords", "/settings?section=security".into()));
        list.push(command("regional", "Regional settings".into(), "Time zone", "/settings?section=regional".into()));
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
    let count_of = move |group: &str| counts.with(|c| c.iter().find(|(g, _)| g == group).map(|(_, n)| *n));
    let scopes = Memo::new(move |_| ORDER.iter().filter(|g| count_of(g).is_some()).map(|g| g.to_string()).collect::<Vec<_>>());
    let effective = Memo::new(move |_| {
        let s = scope.get();
        if s != "All" && count_of(&s).is_some() { s } else { "All".into() }
    });
    let entries = Memo::new(move |_| {
        let term = term.get();
        let matches = |e: &Entry| e.title.to_lowercase().contains(&term.to_lowercase());
        if command_mode.get() {
            return commands.get().into_iter().filter(matches).collect::<Vec<_>>();
        }
        if term.is_empty() {
            let mut list: Vec<Entry> = recent
                .get()
                .opened
                .into_iter()
                .map(|item| Entry { key: format!("recent:{}", item.id), section: "Jump back in".into(), title: item.title.clone(), detail: format!("{} · {}", item.group, item.detail), href: item.href.clone(), group: Some(item.group.clone()), thumbnail: item.thumbnail.clone(), result: Some(item) })
                .collect();
            list.extend(commands.get().into_iter().take(4));
            return list;
        }
        let scope = effective.get();
        let mut list: Vec<Entry> = if scope == "All" { commands.get().into_iter().filter(matches).take(3).collect() } else { Vec::new() };
        let items = result.get().items;
        for group in ORDER.iter().filter(|g| scope == "All" || **g == scope) {
            for item in items.iter().filter(|i| i.group == *group) {
                list.push(Entry { key: format!("{}:{}", item.id, item.group), section: item.group.clone(), title: item.title.clone(), detail: item.detail.clone(), href: item.href.clone(), group: Some(item.group.clone()), thumbnail: item.thumbnail.clone(), result: Some(item.clone()) });
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
    let current_index = move || entries.with(|e| if e.is_empty() { None } else { Some(active.get().min(e.len() - 1)) });
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
            let _ = window().open_with_url_and_target_and_features(&entry.href, "_blank", "noopener");
            return;
        }
        { on_close.try_run(()); };
        navigate(&entry.href);
    };
    let cycle_scope = move |step: i64| {
        let mut list = vec!["All".to_owned()];
        list.extend(scopes.get_untracked());
        if list.len() < 2 {
            return false;
        }
        let len = list.len() as i64;
        let index = list.iter().position(|s| *s == effective.get_untracked()).unwrap_or(0) as i64;
        scope.set(list[((index + step + len) % len) as usize].clone());
        true
    };
    let status = move || {
        if command_mode.get() {
            return if entries.with(Vec::is_empty) { "No matching commands." } else { "" };
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
    match (!needle.is_empty()).then(|| lower.find(&needle)).flatten().filter(|i| lower.len() == text.len() && text.is_char_boundary(*i) && text.is_char_boundary(*i + needle.len())) {
        None => view! { {text} }.into_any(),
        Some(index) => {
            let end = index + needle.len();
            view! { {text[..index].to_owned()}<mark>{text[index..end].to_owned()}</mark>{text[end..].to_owned()} }.into_any()
        }
    }
}
