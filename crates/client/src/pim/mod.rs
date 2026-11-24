//! Shared Contacts and Calendar parts (`pim/common.tsx`): live change tracking, the PIM
//! frame, collection editing and sharing, collection dots and avatars.

pub mod calendar;
pub mod contacts;
pub mod presentation;
pub mod schedule;
pub mod transfers;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table};
use wasm_bindgen::JsCast;

use crate::api::{self, BrowserSession, Req};
use crate::icons::Icon;
use crate::live::Live;
use crate::module_bindings::*;
use crate::settings::{AppSettings, AppSettingsButton};
use crate::shell::WorkspaceHeader;
use crate::ui::{
    ActionMenu, ConnectionState, ConnectionStatus, Dialog, EditorPane, MenuAction, Select,
};

/// Reports how a save went to the form that started it (`Ok` closes it, `Err` shows the message).
pub type Done = Callback<Result<(), String>>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub owner_id: String,
    pub kind: String,
    pub name: String,
    pub color: String,
    pub revision: u64,
    pub role: String,
    #[serde(default)]
    pub subscription: Option<Value>,
}

/// A resource with its parsed data (`Item<T>`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub collection_id: String,
    pub href: String,
    pub revision: u64,
    pub data: Value,
}

pub fn etag(id: &str, revision: u64) -> String {
    format!("\"{id}:{revision}\"")
}

/// A JSON request with the session's CSRF token and optional `If-Match`.
pub async fn send<T: serde::de::DeserializeOwned>(
    url: &str,
    method: &str,
    body: Option<Value>,
    csrf: &str,
    if_match: Option<String>,
) -> Result<T, api::RequestError> {
    let headers: Vec<(&str, &str)> = if_match
        .as_deref()
        .map(|m| vec![("if-match", m)])
        .unwrap_or_default();
    api::request(
        url,
        Req {
            method,
            body: body.map(|b| b.to_string()),
            csrf: Some(csrf),
            headers: &headers,
            ..Default::default()
        },
    )
    .await
}

/// Live change tracking (`usePim`): `epoch` moves when the account's PIM data changes.
#[derive(Clone, Copy)]
pub struct PimLive {
    pub epoch: RwSignal<u32>,
    pub state: RwSignal<ConnectionState>,
}

impl PimLive {
    pub fn refresh(self) {
        self.epoch.update(|e| *e += 1);
    }
}

pub fn use_pim(on_signed_out: Callback<()>) -> PimLive {
    let live = expect_context::<Live>();
    let pim = PimLive {
        epoch: RwSignal::new(0),
        state: RwSignal::new(ConnectionState::Connecting),
    };
    Effect::new(move |_| match live.status.get() {
        crate::live::LiveStatus::Reconnecting => pim.state.set(ConnectionState::Reconnecting),
        crate::live::LiveStatus::Error => pim.state.set(ConnectionState::Error),
        _ => {}
    });
    Effect::new(move |_| {
        let Some(c) = live.connection.get() else {
            return;
        };
        let disposed = Arc::new(AtomicBool::new(false));
        let current = |ctx: &EventContext| {
            ctx.db
                .my_pim_changes()
                .iter()
                .next()
                .map_or(0.0, |r| r.revision as f64)
        };
        // Initial and replayed data only reload what is newer than the page loaded.
        let compare = {
            let disposed = disposed.clone();
            move |ctx: &EventContext| {
                if !disposed.load(Ordering::Relaxed) && current(ctx) > api::loaded_pim_revision() {
                    pim.epoch.try_update(|e| *e += 1);
                }
            }
        };
        let inserted = c.db.my_pim_changes().on_insert({
            let compare = compare.clone();
            move |ctx, _| compare(ctx)
        });
        let deleted = c.db.me().on_delete({
            let disposed = disposed.clone();
            // Another app's unsubscribing also removes the row from the cache; only a real deletion signs out.
            move |ctx: &EventContext, _| {
                if !disposed.load(Ordering::Relaxed)
                    && !matches!(ctx.event, spacetimedb_sdk::Event::UnsubscribeApplied)
                {
                    {
                        on_signed_out.try_run(());
                    };
                }
            }
        });
        let handle = c
            .subscription_builder()
            .on_applied({
                let disposed = disposed.clone();
                move |ctx| {
                    if disposed.load(Ordering::Relaxed) {
                        return;
                    }
                    pim.state.try_set(ConnectionState::Ready);
                    if ctx
                        .db
                        .my_pim_changes()
                        .iter()
                        .next()
                        .is_some_and(|r| r.revision as f64 > api::loaded_pim_revision())
                    {
                        pim.epoch.try_update(|e| *e += 1);
                    }
                }
            })
            .on_error({
                let disposed = disposed.clone();
                move |_, _| {
                    if !disposed.load(Ordering::Relaxed) {
                        pim.state.try_set(ConnectionState::Error);
                    }
                }
            })
            .subscribe(["SELECT * FROM me", "SELECT * FROM my_pim_changes"]);
        let kept = send_wrapper::SendWrapper::new((c.clone(), handle, inserted, deleted));
        on_cleanup(move || {
            disposed.store(true, Ordering::Relaxed);
            let (c, handle, inserted, deleted) = kept.take();
            let _ = handle.unsubscribe();
            c.db.my_pim_changes().remove_on_insert(inserted);
            c.db.me().remove_on_delete(deleted);
        });
    });
    pim
}

/// A collection's color, drawn with SVG fill so no inline styles are needed.
#[component]
pub fn CollectionDot(
    #[prop(into)] color: Signal<String>,
    #[prop(optional, into)] class: Option<&'static str>,
) -> impl IntoView {
    view! {
        <svg class=class.unwrap_or("collection-dot") viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="8" cy="8" r="5" fill=move || { let c = color.get(); if c.is_empty() { "currentColor".to_owned() } else { c } }></circle>
        </svg>
    }
}

pub fn initials(name: &str) -> String {
    let value: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .collect::<String>()
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if value.is_empty() { "?".into() } else { value }
}

/// One of six palette classes, stable for a given name (`avatarHue`).
pub fn avatar_hue(name: &str) -> String {
    let hash = name
        .chars()
        .fold(0u32, |hash, c| hash.wrapping_mul(31).wrapping_add(c as u32));
    format!("avatar-hue-{}", hash % 6)
}

/// The frame every PIM app shares (`PimShell`).
#[component]
pub fn PimShell(
    #[prop(into)] kind: &'static str,
    session: BrowserSession,
    on_signed_out: Callback<()>,
    sidebar: ChildrenFn,
    #[prop(into)] has_details: Signal<bool>,
    details: ChildrenFn,
    pim: PimLive,
    #[prop(optional, into)] main_class: &'static str,
    children: ChildrenFn,
) -> impl IntoView {
    let navigation = RwSignal::new(false);
    let settings = RwSignal::new(false);
    let connected = RwSignal::new(false);
    Effect::new(move |_| {
        if pim.state.get() == ConnectionState::Ready {
            connected.set(true);
        }
    });
    let layout = match kind {
        "contacts" => "has-contacts-main",
        "calendar" => "has-calendar-main",
        _ => "has-workspace-list",
    };
    let detail_label = match kind {
        "contacts" => "Contact details",
        "calendar" => "Event details",
        "notes" => "Note details",
        _ => "Task details",
    };
    let panel = NodeRef::<html::Aside>::new();
    let main = NodeRef::<html::Main>::new();
    let origin = StoredValue::new_local(None::<web_sys::HtmlElement>);
    // On phones the details sheet takes focus when it opens.
    Effect::new(move |previous: Option<bool>| {
        let open = has_details.get();
        // The panel renders (or leaves) in the same pass, so focus moves on the next frame.
        if open
            && previous == Some(false)
            && crate::ui::window()
                .match_media("(max-width:800px)")
                .ok()
                .flatten()
                .is_some_and(|m| m.matches())
        {
            request_animation_frame(move || {
                if let Some(p) = panel.try_get_untracked().flatten() {
                    let target = p
                        .query_selector("[aria-label^=\"Close\"]")
                        .ok()
                        .flatten()
                        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
                    let _ = target.unwrap_or_else(|| p.clone().into()).focus();
                }
            });
        } else if !open && previous == Some(true) {
            // Focus returns to the button or link that opened the details, or else to the list.
            request_animation_frame(move || {
                let document = crate::ui::document();
                let on_body = document.active_element().is_none_or(|a| {
                    document
                        .body()
                        .is_some_and(|b| a == *AsRef::<web_sys::Element>::as_ref(&b))
                });
                if !on_body {
                    return;
                }
                let shown = origin
                    .try_with_value(|o| {
                        o.clone().filter(|t| {
                            t.is_connected() && {
                                let b = t.get_bounding_client_rect();
                                b.width() > 0.0 && b.height() > 0.0
                            }
                        })
                    })
                    .flatten();
                match shown {
                    Some(t) => {
                        let _ = t.focus();
                    }
                    None => {
                        if let Some(m) = main.try_get_untracked().flatten() {
                            let _ = m.focus();
                        }
                    }
                }
            });
        }
        open
    });
    Effect::new(move |_| {
        if let Some(m) = main.get() {
            crate::ui::listen(&m, "click", true, move |event| {
                let target = event
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|t| t.closest("button,a").ok().flatten())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
                origin.set_value(target);
            });
        }
    });
    let (session_header, session_settings) = (session.clone(), StoredValue::new(session));
    let sidebar = StoredValue::new(sidebar);
    let details = StoredValue::new(details);
    let children = StoredValue::new(children);
    view! {
        <div class=move || format!("app-shell pim-shell {} {layout}", if has_details.get() { "has-pim-details" } else { "" })>
            <WorkspaceHeader app=kind session=session_header on_signed_out=on_signed_out on_settings=Callback::new(move |_| settings.set(true))/>
            <div class=format!("pim-workspace {layout}")>
                {move || navigation.get().then(|| view! { <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| navigation.set(false)></button> })}
                <aside class=move || if navigation.get() { "sidebar is-open" } else { "sidebar " }
                    on:click=move |e| {
                        let target = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
                        let Some(t) = target else { return };
                        let in_button = t.closest("button").ok().flatten().is_some();
                        let excluded = t.closest(".ui-select,.contacts-book-pages,.calendar-list-pages").ok().flatten().is_some();
                        let menu = t.closest(".ui-menu").ok().flatten().is_some() && t.closest("[role=\"menuitem\"]").ok().flatten().is_none();
                        if in_button && !excluded && !menu { navigation.set(false); }
                    }>
                    {sidebar.with_value(|s| s())}
                    <AppSettingsButton app=kind on_click=Callback::new(move |_| settings.set(true))/>
                </aside>
                <main node_ref=main tabindex="-1" class=format!("pim-main {main_class}")>
                    <ConnectionStatus state=pim.state/>
                    {move || connected.get().then(|| children.with_value(|c| c()))}
                </main>
                {move || has_details.get().then(|| view! { <aside node_ref=panel tabindex="-1" class="pim-details" aria-label=detail_label>{details.with_value(|d| d())}</aside> })}
            </div>
            {move || settings.get().then(|| view! { <AppSettings app=kind session=session_settings.get_value() on_close=Callback::new(move |_| settings.set(false))/> })}
        </div>
    }
}

/// Name and color of an address book or calendar (`CollectionEditor`).
#[component]
pub fn CollectionEditor(
    #[prop(into)] title: String,
    #[prop(optional)] collection: Option<Collection>,
    on_save: Callback<(String, String, Done)>,
    on_close: Callback<()>,
) -> impl IntoView {
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let (name, color) = collection
        .map(|c| (c.name, c.color))
        .unwrap_or_else(|| (String::new(), "#73396f".into()));
    let (name, color) = (StoredValue::new(name), StoredValue::new(color));
    view! {
        <EditorPane title=title on_close=on_close>
            <form on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let data = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()).and_then(|f| web_sys::FormData::new_with_form(&f).ok());
                let get = |k: &str| data.as_ref().and_then(|d| d.get(k).as_string()).unwrap_or_default();
                busy.set(true);
                error.set(String::new());
                { on_save.try_run((get("name"), get("color"), Callback::new(move |result: Result<(), String>| {
                    busy.set(false);
                    match result { Ok(()) => { on_close.try_run(()); }, Err(e) => error.set(e) }
                }))); };
            }>
                <label>"Name"<input name="name" autofocus required maxlength="255" value=name.get_value()/></label>
                <label>"Color"<input name="color" type="color" value=color.get_value()/></label>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions"><button type="button" class="text-button">"Cancel"</button><button class="primary-button" disabled=move || busy.get()>"Save"</button></div>
            </form>
        </EditorPane>
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Grant {
    id: String,
    username: Option<String>,
    role: String,
}

#[derive(Deserialize)]
struct Grants {
    items: Vec<Grant>,
}

/// Sharing an address book or calendar with another account (`Sharing`).
#[component]
pub fn Sharing(
    collection: Collection,
    #[prop(into)] base: String,
    session: BrowserSession,
    on_close: Callback<()>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let grants: RwSignal<Vec<Grant>> = RwSignal::new(Vec::new());
    let current = RwSignal::new(collection.clone());
    let error = RwSignal::new(String::new());
    let base = StoredValue::new(base);
    let csrf = StoredValue::new(session.csrf_token.clone());
    let refresh = move || {
        spawn_local(async move {
            let b = base.get_value();
            let row = api::get::<Collection>(&b).await;
            let list = api::get::<Grants>(&format!("{b}/grants")).await;
            match (row, list) {
                (Ok(row), Ok(list)) => {
                    current.set(row);
                    grants.set(list.items);
                    {
                        on_changed.try_run(());
                    };
                }
                (Err(e), _) | (_, Err(e)) => error.set(e.message),
            }
        });
    };
    refresh();
    let title = format!("Share {}", collection.name);
    view! {
        <Dialog title=title on_close=on_close>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <form on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                let data = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlFormElement>().ok()).and_then(|f| web_sys::FormData::new_with_form(&f).ok());
                let get = |k: &str| data.as_ref().and_then(|d| d.get(k).as_string()).unwrap_or_default();
                let body = json!({ "username": get("username"), "role": get("role") });
                let c = current.get_untracked();
                spawn_local(async move {
                    match send::<Value>(&format!("{}/grants", base.get_value()), "POST", Some(body), &csrf.get_value(), Some(etag(&c.id, c.revision))).await {
                        Ok(_) => refresh(),
                        Err(e) => error.set(e.message),
                    }
                });
            }>
                <label>"Username"<input name="username" required/></label>
                <label>"Permission"<Select name="role" options=vec![("read".to_owned(), "Can view".to_owned()), ("write".to_owned(), "Can edit".to_owned())]/></label>
                <button class="primary-button">"Share"</button>
            </form>
            {move || grants.get().into_iter().map(|g| {
                let id = g.id.clone();
                view! {
                    <div class="grant"><span>"@"{g.username.clone().unwrap_or_default()}<small>{if g.role == "write" { "Can edit" } else { "Can view" }}</small></span>
                        <button class="text-button danger" on:click=move |_| {
                            let (id, c) = (id.clone(), current.get_untracked());
                            spawn_local(async move {
                                match send::<Value>(&format!("{}/grants", base.get_value()), "DELETE", Some(json!({ "id": id })), &csrf.get_value(), Some(etag(&c.id, c.revision))).await {
                                    Ok(_) => refresh(),
                                    Err(e) => error.set(e.message),
                                }
                            });
                        }>"Revoke"</button>
                    </div>
                }
            }).collect_view()}
        </Dialog>
    }
}

/// The collection-row pattern with paging sized to the sidebar (`CollectionNavigation`).
#[component]
pub fn CollectionNavigation(
    #[prop(into)] rows: Signal<Vec<Collection>>,
    #[prop(into)] selected: Signal<String>,
    noun: &'static str,
    on_select: Callback<Collection>,
    on_edit: Callback<Collection>,
    on_share: Callback<Collection>,
    on_delete: Callback<Collection>,
) -> impl IntoView {
    let list = NodeRef::<html::Div>::new();
    let page = RwSignal::new(0usize);
    let size = RwSignal::new(1usize);
    // As in React: the page follows the selection only when the selection, size or row set
    // changes, and the size follows the list's height through a ResizeObserver.
    let count = Memo::new(move |_| rows.with(Vec::len));
    let ids = Memo::new(move |_| {
        rows.with(|r| {
            r.iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>()
                .join(",")
        })
    });
    Effect::new(move |_| {
        let Some(node) = list.get() else { return };
        let count = count.get();
        let measure = {
            let node = node.clone();
            move || {
                let width = crate::ui::window()
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1000.0);
                size.try_set(if width <= 800.0 {
                    count.max(1)
                } else {
                    ((node.client_height() as f64 / 42.0).floor() as usize).max(1)
                });
            }
        };
        let callback = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(measure.clone());
        if let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) {
            observer.observe(&node);
            let kept = send_wrapper::SendWrapper::new((observer, callback));
            on_cleanup(move || kept.take().0.disconnect());
        }
        measure();
    });
    Effect::new(move |_| {
        ids.track();
        let s = size.get();
        let chosen = selected.get();
        if let Some(index) = rows.with_untracked(|r| r.iter().position(|row| row.id == chosen)) {
            page.set(index / s);
        }
    });
    Effect::new(move |_| {
        let pages = count.get().div_ceil(size.get()).max(1);
        page.update(|p| *p = (*p).min(pages - 1));
    });
    view! {
        <div node_ref=list class="workspace-collections">
            {move || {
                let (s, p) = (size.get(), page.get());
                rows.get().into_iter().skip(p * s).take(s).map(|row| {
                    let active = selected.get() == row.id;
                    let (r1, r2, r3, r4) = (row.clone(), row.clone(), row.clone(), row.clone());
                    let actions = StoredValue::new(row.clone());
                    view! {
                        <div class="contacts-book-row">
                            <button class=if active { "nav-item active" } else { "nav-item " } aria-current=active.then_some("page") on:click=move |_| { on_select.try_run(r1.clone()); }>
                                <CollectionDot color=row.color.clone()/><span>{row.name.clone()}</span>{(row.role == "read").then(|| view! { <small>"Read only"</small> })}
                            </button>
                            {(row.role == "owner").then(|| view! {
                                <ActionMenu label=format!("Actions for {noun} {}", actions.with_value(|r| r.name.clone())) class="contacts-book-actions" actions=move |_| vec![
                                    MenuAction::new(format!("Edit {noun}"), "pencil").run({ let r = r2.clone(); move || { on_edit.try_run(r.clone()); } }),
                                    MenuAction::new(format!("Share {noun}"), "share").run({ let r = r3.clone(); move || { on_share.try_run(r.clone()); } }),
                                    MenuAction::new(format!("Delete {noun}"), "trash").danger().run({ let r = r4.clone(); move || { on_delete.try_run(r.clone()); } }),
                                ]><Icon name="more"/></ActionMenu>
                            })}
                        </div>
                    }
                }).collect_view()
            }}
        </div>
        {move || (rows.with(Vec::len) > size.get()).then(|| {
            let pages = rows.with(Vec::len).div_ceil(size.get());
            view! {
                <nav class="contacts-book-pages" aria-label=format!("{noun} pages")>
                    <button class="icon-button" aria-label=format!("Previous {noun}s") disabled=move || page.get() == 0 on:click=move |_| page.update(|p| *p = p.saturating_sub(1))><Icon name="back"/></button>
                    <span>{move || format!("{} / {pages}", page.get() + 1)}</span>
                    <button class="icon-button" aria-label=format!("Next {noun}s") disabled={move || (page.get() + 1) * size.get() >= rows.with(Vec::len)} on:click=move |_| page.update(|p| *p += 1)><Icon name="chevron"/></button>
                </nav>
            }
        })}
    }
}
