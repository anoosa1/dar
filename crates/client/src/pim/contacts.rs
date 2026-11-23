//! Contacts (`pim/contacts.tsx`): address books, groups and favorites, the contact list
//! with letter headings, the details panel and the contact editor.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::presentation::contact_href;
use super::{Collection, CollectionDot, CollectionEditor, Done, Item, PimShell, Sharing, avatar_hue, etag, initials, send, use_pim};
use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::shared::context;
use crate::ui::{ActionMenu, Dialog, EditorPane, MenuAction, Select, window};

const BASE: &str = "/api/v1/contacts/books";

#[derive(Deserialize)]
struct Books {
    items: Vec<Collection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct People {
    items: Vec<Item>,
    groups: Vec<String>,
    next_cursor: String,
}

fn s(data: &Value, key: &str) -> String {
    data[key].as_str().unwrap_or_default().to_owned()
}

fn empty() -> Value {
    json!({ "displayName": "", "firstName": "", "lastName": "", "middleName": "", "prefix": "", "suffix": "", "organization": "", "title": "", "notes": "", "birthday": "", "emails": [], "phones": [], "urls": [], "addresses": [], "groups": [], "favorite": false })
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe20..=0xfe2f)
}

/// The letter heading for a row under the current name sort (`sortLetter`).
fn sort_letter(data: &Value, sort: &str) -> String {
    let preferred = s(data, if sort == "lastName" { "lastName" } else { "firstName" });
    let key = if preferred.is_empty() { s(data, "displayName") } else { preferred };
    let decomposed: String = js_sys::JsString::from(key.trim()).normalize("NFD").into();
    let first = decomposed.chars().find(|c| !is_mark(*c)).map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    if first.chars().next().is_some_and(char::is_alphabetic) { first } else { "#".into() }
}

/// vCard birthdays (YYYY-MM-DD or --MM-DD) without time zone shifts.
fn birthday(value: &str) -> String {
    let date = |y: i32, m: u32, d: u32| js_sys::Date::utc(y as f64, (m - 1) as f64) + (d as f64 - 1.0) * 86_400_000.0;
    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [y, m, d] if y.len() == 4 => match (y.parse(), m.parse(), d.parse()) {
            (Ok(y), Ok(m), Ok(d)) => crate::dates::local_date(date(y, m, d), &[("year", "numeric"), ("month", "long"), ("day", "numeric"), ("timeZone", "UTC")]),
            _ => value.into(),
        },
        ["", "", m, d] => match (m.parse(), d.parse()) {
            (Ok(m), Ok(d)) => crate::dates::local_date(date(2000, m, d), &[("month", "long"), ("day", "numeric"), ("timeZone", "UTC")]),
            _ => value.into(),
        },
        _ => value.into(),
    }
}

fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

fn search_params(pairs: &[(&str, String)]) -> String {
    let params = web_sys::UrlSearchParams::new().expect("params");
    for (k, v) in pairs {
        params.append(k, v);
    }
    params.to_string().as_string().unwrap_or_default()
}

#[component]
pub fn Contacts(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let initial = web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default()).expect("params");
    let books: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let book = RwSignal::new(initial.get("book").unwrap_or_default());
    let favorite = RwSignal::new(false);
    let group = RwSignal::new(String::new());
    let sort = RwSignal::new("firstName".to_owned());
    let people: RwSignal<Vec<Item>> = RwSignal::new(Vec::new());
    let groups: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let cursor = RwSignal::new(String::new());
    let next = RwSignal::new(String::new());
    let previous: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let selected: RwSignal<Option<(String, String)>> = RwSignal::new(match (initial.get("book"), initial.get("contact")) {
        (Some(b), Some(c)) => Some((b, c)),
        _ => None,
    });
    let detail: RwSignal<Option<Item>> = RwSignal::new(None);
    let modal: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let notice = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let delete_count: RwSignal<Option<usize>> = RwSignal::new(None);
    let action_book: RwSignal<Option<Collection>> = RwSignal::new(None);
    let book_page = RwSignal::new(0usize);
    let page_size = RwSignal::new(1usize);
    let book_list = NodeRef::<html::Div>::new();
    let group_list = NodeRef::<html::Div>::new();
    let session = StoredValue::new(session);
    let csrf = move || session.with_value(|s| s.csrf_token.clone());
    let loaded_list = StoredValue::new(String::new());
    let loaded_detail = StoredValue::new(String::new());
    let sequence = RwSignal::new(0u32);
    let action_sequence = RwSignal::new(0u32);

    let preferences = context().preferences;
    Effect::new(move |_| {
        if let Some(p) = preferences.value.get().and_then(|p| p.rest.get("contactSort").and_then(Value::as_str).map(str::to_owned)) {
            sort.set(p);
        }
    });
    // How many address books fit beside the groups; phones list them all.
    Effect::new(move |_| {
        books.track();
        groups.track();
        let Some(node) = book_list.get() else { return };
        let width = window().inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1000.0);
        let groups_height = group_list.get().map_or(0, |g| g.offset_height()) as f64;
        page_size.set(if width <= 800.0 { 1000 } else { (((node.client_height() as f64 - groups_height - 40.0) / 42.0).floor() as usize).clamp(1, 8) });
    });
    Effect::new(move |_| {
        let (list, size) = (books.get(), page_size.get());
        if let Some(index) = list.iter().position(|row| row.id == book.get()) {
            book_page.set(index / size);
        }
        book_page.update(|p| *p = (*p).min(list.len().div_ceil(size).max(1) - 1));
    });
    let collection = Memo::new(move |_| books.with(|b| b.iter().find(|x| x.id == book.get()).cloned()));
    let detail_book = Memo::new(move |_| detail.with(|d| d.as_ref().and_then(|d| books.with(|b| b.iter().find(|x| x.id == d.collection_id).cloned()))));
    let writable = Memo::new(move |_| detail_book.with(|b| b.as_ref().is_some_and(|b| b.role != "read")));

    Effect::new(move |_| {
        pim.epoch.track();
        let key = json!([book.get(), sort.get(), group.get(), favorite.get(), cursor.get()]).to_string();
        if loaded_list.get_value() != key {
            loading.set(true);
        }
        let id = sequence.get_untracked() + 1;
        sequence.set(id);
        let query = search_params(&[("book", book.get_untracked()), ("q", String::new()), ("sort", sort.get_untracked()), ("group", group.get_untracked()), ("favorite", if favorite.get_untracked() { "1".into() } else { "0".into() }), ("cursor", cursor.get_untracked())]);
        spawn_local(async move {
            let b = api::get::<Books>(BASE).await;
            let list = api::get::<People>(&format!("/api/v1/contacts/people?{query}")).await;
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            match (b, list) {
                (Ok(b), Ok(list)) => {
                    loaded_list.set_value(key);
                    books.set(b.items);
                    people.set(list.items);
                    groups.set(list.groups);
                    next.set(list.next_cursor);
                    error.set(String::new());
                }
                (Err(e), _) | (_, Err(e)) => {
                    error.set(e.message);
                    people.set(Vec::new());
                }
            }
            loading.set(false);
        });
    });
    Effect::new(move |_| {
        pim.epoch.track();
        let current = selected.get();
        let key = current.as_ref().map(|(c, h)| format!("{c}/{h}")).unwrap_or_default();
        if loaded_detail.get_value() != key {
            detail.set(None);
        }
        loaded_detail.set_value(key.clone());
        let Some((collection_id, href)) = current else { return };
        spawn_local(async move {
            let result = api::get::<Item>(&format!("{BASE}/{collection_id}/items/{href}")).await;
            if loaded_detail.try_get_value().as_deref() != Some(key.as_str()) {
                return;
            }
            match result {
                Ok(row) => detail.set(Some(row)),
                Err(e) => {
                    selected.set(None);
                    notice.set("This contact was removed or your access changed.".into());
                    error.set(e.message);
                }
            }
        });
    });
    crate::ui::listen(&window(), "popstate", false, move |_| {
        let params = web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default()).expect("params");
        book.set(params.get("book").unwrap_or_default());
        selected.set(match (params.get("book"), params.get("contact")) {
            (Some(b), Some(c)) => Some((b, c)),
            _ => None,
        });
    });
    let reset = move || {
        cursor.set(String::new());
        previous.set(Vec::new());
        selected.set(None);
    };
    let push = |url: &str| {
        let _ = window().history().expect("history").push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
    };
    let scope = move |id: String, starred: bool| {
        book.set(id.clone());
        favorite.set(starred);
        group.set(String::new());
        reset();
        push(&if id.is_empty() { "/contacts".into() } else { format!("/contacts?book={id}") });
    };
    let choose = move |row: &Item| {
        selected.set(Some((row.collection_id.clone(), row.href.clone())));
        push(&format!("/contacts?{}", search_params(&[("book", row.collection_id.clone()), ("contact", row.href.clone())])));
    };
    let run = move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            match work.await {
                Ok(()) => {
                    pim.refresh();
                    modal.set(None);
                }
                Err(e) => error.set(e),
            }
            busy.set(false);
        });
    };
    let book_action = move |target: Collection, kind: &'static str| {
        let id = action_sequence.get_untracked() + 1;
        action_sequence.set(id);
        action_book.set(Some(target.clone()));
        modal.set(Some(kind));
        error.set(String::new());
        if kind == "deleteBook" {
            delete_count.set(None);
            spawn_local(async move {
                match api::get::<Value>(&format!("{BASE}/{}", target.id)).await {
                    Ok(v) if action_sequence.try_get_untracked() == Some(id) => delete_count.set(v["count"].as_u64().map(|c| c as usize)),
                    Err(e) if action_sequence.try_get_untracked() == Some(id) => error.set(e.message),
                    _ => {}
                }
            });
        }
    };

    let sidebar: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            <button type="button" class="new-button ui-new" on:click=move |_| modal.set(Some(if books.with(|b| b.iter().any(|x| x.role != "read")) { "new" } else { "book" }))><Icon name="plus"/>"New contact"</button>
            <button class=move || if book.get().is_empty() && !favorite.get() { "nav-item active" } else { "nav-item " } on:click=move |_| scope(String::new(), false)><Icon name="contacts"/>"All contacts"</button>
            <button class=move || if favorite.get() { "nav-item active" } else { "nav-item " } on:click=move |_| scope(String::new(), true)><Icon name="star"/>"Favorites"</button>
            <div class="pim-sidebar-heading"><strong>"Address books"</strong><button class="icon-button" aria-label="New address book" on:click=move |_| modal.set(Some("book"))><Icon name="plus"/></button></div>
            <div class="contacts-collections" node_ref=book_list>
                <div class="contacts-books">
                    {move || {
                        let (p, size) = (book_page.get(), page_size.get());
                        books.get().into_iter().skip(p * size).take(size).map(|b| {
                            let active = book.get() == b.id;
                            let (id, b2, b3, b4) = (b.id.clone(), b.clone(), b.clone(), b.clone());
                            let name = b.name.clone();
                            view! {
                                <div class="contacts-book-row">
                                    <button class=if active { "nav-item active" } else { "nav-item " } aria-pressed=active.to_string() on:click=move |_| scope(id.clone(), false)>
                                        <CollectionDot color=b.color.clone()/><span>{b.name.clone()}</span>{(b.role != "owner").then(|| view! { <small>"Shared"</small> })}
                                    </button>
                                    {(b.role == "owner").then(|| view! {
                                        <ActionMenu label=format!("Actions for address book {name}") class="contacts-book-actions" actions=move |_| vec![
                                            MenuAction::new("Edit address book", "pencil").run({ let b = b2.clone(); move || book_action(b.clone(), "renameBook") }),
                                            MenuAction::new("Share address book", "share").run({ let b = b3.clone(); move || book_action(b.clone(), "share") }),
                                            MenuAction::new("Delete address book", "trash").danger().run({ let b = b4.clone(); move || book_action(b.clone(), "deleteBook") }),
                                        ]><Icon name="more"/></ActionMenu>
                                    })}
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>
                {move || (books.with(Vec::len) > page_size.get()).then(|| view! {
                    <nav class="contacts-book-pages" aria-label="Address book list pages">
                        <button class="icon-button" aria-label="Previous address books" disabled=move || book_page.get() == 0 on:click=move |_| book_page.update(|p| *p = p.saturating_sub(1))><Icon name="back"/></button>
                        <span>{move || format!("{} / {}", book_page.get() + 1, books.with(Vec::len).div_ceil(page_size.get()))}</span>
                        <button class="icon-button" aria-label="Next address books" disabled={move || (book_page.get() + 1) * page_size.get() >= books.with(Vec::len)} on:click=move |_| book_page.update(|p| *p += 1)><Icon name="chevron"/></button>
                    </nav>
                })}
                <div class="contacts-group-area" node_ref=group_list>
                    {move || {
                        let mut list = groups.get();
                        let g = group.get();
                        if !g.is_empty() && !list.contains(&g) { list.push(g.clone()); }
                        (!list.is_empty()).then(|| view! {
                            <div class="pim-sidebar-heading"><strong>"Groups"</strong></div>
                            <nav class="contacts-groups" aria-label="Groups">
                                {list.into_iter().map(|name| {
                                    let active = group.get() == name;
                                    let chosen = name.clone();
                                    view! { <button class=if active { "nav-item active" } else { "nav-item " } aria-pressed=active.to_string() on:click=move |_| { group.set(if group.get_untracked() == chosen { String::new() } else { chosen.clone() }); reset(); }><Icon name="tag"/><span>{name.clone()}</span></button> }
                                }).collect_view()}
                            </nav>
                        })
                    }}
                </div>
            </div>
        }
        .into_any()
    });

    let details: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            <div class="pim-detail-top"><span>"Contact"</span><button class="icon-button" aria-label="Close contact details" on:click=move |_| {
                selected.set(None);
                let b = book.get_untracked();
                let _ = window().history().expect("history").replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&if b.is_empty() { "/contacts".into() } else { format!("/contacts?book={b}") }));
            }><Icon name="close"/></button></div>
            {move || match detail.get() {
                None => view! { <p role="status">"Loading contact…"</p> }.into_any(),
                Some(d) => contact_details(d, detail_book.get(), writable.get(), busy, modal, run, csrf()).into_any(),
            }}
        }
        .into_any()
    });

    let title = move || if favorite.get() { "Favorites".to_owned() } else { collection.get().map(|c| c.name).unwrap_or_else(|| "All contacts".into()) };
    let list = move || {
        if loading.get() {
            return view! { <p role="status">"Loading contacts…"</p> }.into_any();
        }
        let rows = people.get();
        if rows.is_empty() {
            return view! {
                <div class="pim-empty"><h2>{if group.get().is_empty() { "No contacts" } else { "No matching contacts" }}</h2>
                    <p>{if books.with(Vec::is_empty) { "Create an address book to get started." } else { "Create a contact or import vCards into an address book." }}</p></div>
            }
            .into_any();
        }
        let sort_now = sort.get();
        let all_books = books.get();
        let current = selected.get();
        let mut last = String::new();
        view! {
            <div class="contact-list">
                {rows.into_iter().map(|row| {
                    let letter = sort_letter(&row.data, &sort_now);
                    let heading = (letter != last).then(|| view! { <h2 class="contact-letter">{letter.clone()}</h2> });
                    last = letter;
                    let b = all_books.iter().find(|b| b.id == row.collection_id).cloned();
                    let name = s(&row.data, "displayName");
                    let work = [s(&row.data, "title"), s(&row.data, "organization")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" · ");
                    let subtitle = if !work.is_empty() { work } else { row.data["emails"][0]["value"].as_str().or(row.data["phones"][0]["value"].as_str()).unwrap_or("Contact").to_owned() };
                    let is_selected = current.as_ref().is_some_and(|(c, h)| *c == row.collection_id && *h == row.href);
                    let star = row.data["favorite"] == json!(true);
                    let picked = row.clone();
                    let book_name = b.as_ref().map(|b| b.name.clone()).unwrap_or_default();
                    view! {
                        <div class="contact-entry">
                            {heading}
                            <button class=if is_selected { "contact-row selected" } else { "contact-row " } on:click=move |_| choose(&picked)>
                                <span class=format!("contact-initials {}", avatar_hue(&name))>{initials(&name)}</span>
                                <span><strong>{name.clone()}</strong><small>{subtitle}</small></span>
                                {star.then(|| view! { <Icon name="star" class="contact-favorite"/> })}
                                <span class="contact-book" title=book_name.clone()><CollectionDot color=b.map(|b| b.color).unwrap_or_default()/><span class="sr-only">{book_name.clone()}</span></span>
                            </button>
                        </div>
                    }
                }).collect_view()}
            </div>
        }
        .into_any()
    };
    let save = Callback::new(move |(data, target, done): (Value, String, Done)| {
        let editing = modal.get_untracked() == Some("edit");
        let current = detail.get_untracked();
        spawn_local(async move {
            let (url, method, if_match) = match (editing, &current) {
                (true, Some(d)) => (format!("{BASE}/{}/items/{}", d.collection_id, d.href), "PUT", Some(etag(&d.id, d.revision))),
                _ => (format!("{BASE}/{target}/items"), "POST", None),
            };
            match send::<Item>(&url, method, Some(json!({ "data": data })), &csrf(), if_match).await {
                Ok(row) => {
                    choose(&row);
                    pim.refresh();
                    modal.set(None);
                    notice.set("Contact saved.".into());
                    { done.try_run(Ok(())); };
                }
                Err(e) => { done.try_run(Err(e.message)); },
            }
        });
    });
    let shell_session = session.get_value();
    view! {
        <PimShell kind="contacts" main_class="contacts-main" session=shell_session on_signed_out=on_signed_out sidebar=sidebar has_details=Signal::from(Memo::new(move |_| selected.with(Option::is_some))) details=details pim=pim>
            <div class="contacts-heading"><h1>{title}</h1>
                {move || (!group.get().is_empty()).then(|| view! { <span class="pim-filter-chip"><Icon name="tag"/>{group.get()}<button class="icon-button" aria-label=format!("Clear group {}", group.get()) on:click=move |_| { group.set(String::new()); reset(); }><Icon name="close"/></button></span> })}
                <button class="icon-button" aria-label="Refresh" title="Refresh" on:click=move |_| pim.refresh()><Icon name="refresh"/></button>
            </div>
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || (!notice.get().is_empty()).then(|| view! { <p role="status" class="notice">{notice.get()}</p> })}
            {list}
            {move || (!previous.with(Vec::is_empty) || !next.with(String::is_empty)).then(|| view! {
                <div class="pagination">
                    <button class="text-button" disabled=move || previous.with(Vec::is_empty) on:click=move |_| { cursor.set(previous.with_untracked(|p| p.last().cloned()).unwrap_or_default()); previous.update(|p| { p.pop(); }); }>"Previous"</button>
                    <button class="text-button" disabled=move || next.with(String::is_empty) on:click=move |_| { previous.update(|p| p.push(cursor.get_untracked())); cursor.set(next.get_untracked()); }>"Next"</button>
                </div>
            })}
            {move || matches!(modal.get(), Some("new" | "edit")).then(|| {
                let editing = modal.get_untracked() == Some("edit");
                let initial = if editing { detail.get_untracked().map(|d| d.data) } else { None };
                let initial_book = if editing { detail.get_untracked().map(|d| d.collection_id).unwrap_or_default() } else { book.get_untracked() };
                view! { <ContactEditor books=books.get_untracked().into_iter().filter(|b| b.role != "read").collect() initial=initial initial_book=initial_book on_save=save on_close=Callback::new(move |_| modal.set(None))/> }
            })}
            {move || matches!(modal.get(), Some("book" | "renameBook")).then(|| {
                let rename = modal.get_untracked() == Some("renameBook");
                let target = if rename { action_book.get_untracked() } else { None };
                let t2 = target.clone();
                view! {
                    <CollectionEditor title=if rename { "Edit address book" } else { "New address book" } collection=target.unwrap_or_else(|| Collection { id: String::new(), owner_id: String::new(), kind: String::new(), name: String::new(), color: "#73396f".into(), revision: 0, role: String::new(), subscription: None }) on_close=Callback::new(move |_| modal.set(None))
                        on_save=Callback::new(move |(name, color, done): (String, String, Done)| {
                            let t = t2.clone();
                            spawn_local(async move {
                                let (url, method, m) = match &t { Some(t) => (format!("{BASE}/{}", t.id), "PUT", Some(etag(&t.id, t.revision))), None => (BASE.to_owned(), "POST", None) };
                                match send::<Collection>(&url, method, Some(json!({ "name": name, "color": color })), &csrf(), m).await {
                                    Ok(row) => { scope(row.id, false); pim.refresh(); { done.try_run(Ok(())); }; }
                                    Err(e) => { done.try_run(Err(e.message)); },
                                }
                            });
                        })/>
                }
            })}
            {move || (modal.get() == Some("share")).then(|| action_book.get_untracked().map(|b| {
                let base = format!("{BASE}/{}", b.id);
                view! { <Sharing collection=b base=base session=session.get_value() on_close=Callback::new(move |_| modal.set(None)) on_changed=Callback::new(move |_| pim.refresh())/> }
            })).flatten()}
            {move || matches!(modal.get(), Some("delete" | "deleteBook")).then(|| {
                let contact = modal.get_untracked() == Some("delete");
                view! {
                    <Dialog title=if contact { "Delete contact?" } else { "Delete address book?" } on_close=Callback::new(move |_| modal.set(None))>
                        <p>{move || if contact { format!("Permanently remove {}?", detail.with(|d| d.as_ref().map(|d| s(&d.data, "displayName")).unwrap_or_default())) } else { match delete_count.get() { None => "Loading address book details…".into(), Some(n) => format!("Remove {} and all its {n} contacts? Export it first if you need a copy.", action_book.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default())) } }}</p>
                        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                        <div class="dialog-actions"><button class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button class="danger-button" disabled=move || busy.get() || (!contact && delete_count.with(Option::is_none)) on:click=move |_| {
                                let (d, b) = (detail.get_untracked(), action_book.get_untracked());
                                run(Box::pin(async move {
                                    let (url, tag) = match (contact, &d, &b) {
                                        (true, Some(d), _) => (format!("{BASE}/{}/items/{}", d.collection_id, d.href), etag(&d.id, d.revision)),
                                        (false, _, Some(b)) => (format!("{BASE}/{}", b.id), etag(&b.id, b.revision)),
                                        _ => return Ok(()),
                                    };
                                    send::<Value>(&url, "DELETE", None, &csrf(), Some(tag)).await.map_err(|e| e.message)?;
                                    selected.set(None);
                                    if !contact { scope(String::new(), false); }
                                    notice.set("Deleted.".into());
                                    Ok(())
                                }));
                            }>"Delete permanently"</button></div>
                    </Dialog>
                }
            })}
        </PimShell>
    }
}

#[allow(clippy::too_many_arguments)]
fn contact_details(d: Item, book: Option<Collection>, writable: bool, busy: RwSignal<bool>, modal: RwSignal<Option<&'static str>>, run: impl Fn(std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>) + Copy + Send + Sync + 'static, csrf: String) -> impl IntoView {
    let data = d.data.clone();
    let name = s(&data, "displayName");
    let work = [s(&data, "title"), s(&data, "organization")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" · ");
    let first = |key: &str| data[key][0]["value"].as_str().map(str::to_owned);
    let email = first("emails");
    let phone = first("phones").filter(|p| contact_href("Phone", p).is_some());
    let favorite = data["favorite"] == json!(true);
    let item = StoredValue::new(d.clone());
    let csrf = StoredValue::new(csrf);
    let list = |key: &str| data[key].as_array().cloned().unwrap_or_default();
    let has_card = !list("emails").is_empty() || !list("phones").is_empty() || !list("urls").is_empty() || !list("addresses").is_empty() || !s(&data, "birthday").is_empty();
    let groups: Vec<String> = list("groups").iter().filter_map(|g| g.as_str().map(str::to_owned)).collect();
    let notes = s(&data, "notes");
    let values = [("Email", "mail", list("emails")), ("Phone", "phone", list("phones")), ("Website", "globe", list("urls"))]
        .into_iter()
        .flat_map(|(label, icon, values)| {
            values.into_iter().map(move |v| {
                let value = v["value"].as_str().unwrap_or_default().to_owned();
                let tag = v["label"].as_str().filter(|l| !l.is_empty()).map(capitalized).unwrap_or_else(|| label.to_owned());
                let link = contact_href(label, &value);
                let site = label == "Website";
                view! {
                    <div class="property"><Icon name=icon/><div>
                        {match link { Some(href) => view! { <a href=href target=site.then_some("_blank") rel=site.then_some("noopener noreferrer")>{value.clone()}</a> }.into_any(), None => view! { <span>{value.clone()}</span> }.into_any() }}
                        <small>{tag}</small>
                    </div></div>
                }
            })
        })
        .collect_view();
    let addresses = list("addresses")
        .into_iter()
        .map(|a| {
            let get = |k: &str| a[k].as_str().unwrap_or_default().to_owned();
            let first = [(!get("poBox").is_empty()).then(|| format!("PO Box {}", get("poBox"))), Some(get("extended")), Some(get("street"))].into_iter().flatten().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(", ");
            let second = ["city", "region", "postalCode", "country"].iter().map(|k| get(k)).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(", ");
            let label = get("label");
            view! { <div class="property"><Icon name="location"/><div><span>{first}</span><span>{second}</span><small>{if label.is_empty() { "Address".to_owned() } else { format!("{} address", capitalized(&label)) }}</small></div></div> }
        })
        .collect_view();
    let born = s(&data, "birthday");
    view! {
        <div class="pim-hero"><span class=format!("contact-portrait {}", avatar_hue(&name)) aria-hidden="true">{initials(&name)}</span><h2>{name.clone()}</h2>{(!work.is_empty()).then(|| view! { <p class="muted">{work.clone()}</p> })}</div>
        <div class="quick-actions">
            {email.map(|e| view! { <a class="quick-action" href=contact_href("Email", &e).unwrap_or_default() aria-label=format!("Email {e}")><span><Icon name="mail"/></span>"Email"</a> })}
            {phone.map(|p| view! { <a class="quick-action" href=contact_href("Phone", &p).unwrap_or_default() aria-label=format!("Call {p}")><span><Icon name="phone"/></span>"Call"</a> })}
            {writable.then(|| view! {
                <button class="quick-action" aria-pressed=favorite.to_string() aria-label=if favorite { "Remove favorite" } else { "Add favorite" } disabled=move || busy.get() on:click=move |_| {
                    let d = item.get_value();
                    run(Box::pin(async move {
                        let mut data = d.data.clone();
                        data["favorite"] = json!(!favorite);
                        send::<Value>(&format!("{BASE}/{}/items/{}", d.collection_id, d.href), "PUT", Some(json!({ "data": data })), &csrf.get_value(), Some(etag(&d.id, d.revision))).await.map(|_| ()).map_err(|e| e.message)
                    }));
                }><span><Icon name="star"/></span>"Favorite"</button>
                <button class="quick-action" aria-label="Edit contact" on:click=move |_| modal.set(Some("edit"))><span><Icon name="pencil"/></span>"Edit"</button>
            })}
        </div>
        {has_card.then(|| view! {
            <div class="property-card">{values}{addresses}
                {(!born.is_empty()).then(|| view! { <div class="property"><Icon name="cake"/><div><span>{birthday(&born)}</span><small>"Birthday"</small></div></div> })}
            </div>
        })}
        {(!groups.is_empty() || !notes.is_empty()).then(|| view! {
            <div class="property-card">
                {(!groups.is_empty()).then(|| view! { <div class="property"><Icon name="tag"/><div class="pim-chips">{groups.iter().map(|g| view! { <span>{g.clone()}</span> }).collect_view()}</div></div> })}
                {(!notes.is_empty()).then(|| view! { <div class="property"><Icon name="note"/><div><p class="pim-notes">{notes.clone()}</p></div></div> })}
            </div>
        })}
        <div class="pim-detail-footer">
            <span class="muted"><CollectionDot color=book.as_ref().map(|b| b.color.clone()).unwrap_or_default()/>{book.as_ref().map(|b| b.name.clone())}</span>
            <a class="text-button" aria-label="Export contact" title="Export contact" href=format!("{BASE}/{}/items/{}?export=1", d.collection_id, d.href)><Icon name="download"/>"Export"</a>
            {writable.then(|| view! { <button class="text-button danger" aria-label="Delete contact" title="Delete contact" on:click=move |_| modal.set(Some("delete"))><Icon name="trash"/>"Delete"</button> })}
        </div>
    }
}

const ADDRESS_LABELS: [(&str, &str); 8] = [("label", "Label"), ("street", "Street"), ("extended", "Apartment or suite"), ("poBox", "PO box"), ("city", "City"), ("region", "Region"), ("postalCode", "Postal code"), ("country", "Country")];

/// The contact form (`ContactEditor`).
#[component]
fn ContactEditor(books: Vec<Collection>, initial: Option<Value>, #[prop(into)] initial_book: String, on_save: Callback<(Value, String, Done)>, on_close: Callback<()>) -> impl IntoView {
    let editing = initial.is_some();
    let start = initial.clone().unwrap_or_else(empty);
    let data = RwSignal::new(start.clone());
    let first_book = books.first().map(|b| b.id.clone()).unwrap_or_default();
    let book = RwSignal::new(if initial_book.is_empty() { first_book } else { initial_book });
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let original = json!({ "data": start, "book": book.get_untracked() });
    let dirty = Signal::derive(move || json!({ "data": data.get(), "book": book.get() }) != original);
    let extra_names = initial.as_ref().is_some_and(|i| {
        let joined = [s(i, "firstName"), s(i, "lastName")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ");
        !s(i, "prefix").is_empty() || !s(i, "middleName").is_empty() || !s(i, "suffix").is_empty() || !s(i, "displayName").is_empty() && s(i, "displayName") != joined
    });
    let set = move |key: &'static str, value: Value| data.update(|d| d[key] = value);
    let input = move |key: &'static str, label: &'static str, autofocus: bool| {
        view! { <label>{label}<input prop:value=move || data.with(|d| s(d, key)) on:input=move |e| set(key, json!(event_target_value(&e))) maxlength="1024" autofocus=autofocus/></label> }
    };
    let display_placeholder = move || data.with(|d| [s(d, "firstName"), s(d, "lastName")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" "));
    let book_options: Vec<(String, String)> = books.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    let multi = move |key: &'static str, label: &'static str, icon: &'static str| {
        let kind = match key {
            "emails" => "email",
            "phones" => "tel",
            _ => "url",
        };
        let options: &'static [&'static str] = if key == "phones" { &["mobile", "home", "work", "other"] } else { &["home", "work", "other"] };
        view! {
            <fieldset class="form-section"><legend><Icon name=icon/>{label}</legend>
                {move || data.with(|d| d[key].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, v)| {
                    let n = i + 1;
                    view! {
                        <div class="pim-multi-row">
                            <label><span class="sr-only">{format!("{label} {n}")}</span><input aria-label=format!("{label} {n}") type=kind prop:value=v["value"].as_str().unwrap_or_default().to_owned() on:input=move |e| data.update(|d| d[key][i]["value"] = json!(event_target_value(&e)))/></label>
                            <label><span class="sr-only">{format!("Label for {label} {n}")}</span><input aria-label=format!("Label for {label} {n}") list=format!("contact-labels-{key}") prop:value=v["label"].as_str().unwrap_or_default().to_owned() on:input=move |e| data.update(|d| d[key][i]["label"] = json!(event_target_value(&e)))/></label>
                            <button type="button" class="icon-button" aria-label=format!("Remove {label} {n}") on:click=move |_| data.update(|d| { if let Some(list) = d[key].as_array_mut() { list.remove(i); } })><Icon name="close"/></button>
                        </div>
                    }
                }).collect_view()}
                <datalist id=format!("contact-labels-{key}")>{options.iter().map(|o| view! { <option value=*o></option> }).collect_view()}</datalist>
                <button type="button" class="text-button add-button" on:click=move |_| data.update(|d| { if let Some(list) = d[key].as_array_mut() { list.push(json!({ "value": "", "label": if key == "phones" { "mobile" } else { "home" } })); } })><Icon name="plus"/>"Add "{label.to_lowercase()}</button>
            </fieldset>
        }
    };
    view! {
        <EditorPane title=if editing { "Edit contact" } else { "New contact" } on_close=on_close dirty=dirty>
            <form class="pim-editor contact-editor" on:submit=move |e: web_sys::SubmitEvent| {
                e.prevent_default();
                busy.set(true);
                error.set(String::new());
                { on_save.try_run((data.get_untracked(), book.get_untracked(), Callback::new(move |result: Result<(), String>| {
                    busy.set(false);
                    if let Err(e) = result { error.set(e) }
                }))); };
            }>
                {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                {(!editing).then(|| view! { <label>"Address book"<Select options=book_options.clone() value=book/></label> })}
                <section class="form-section"><h3><Icon name="user"/>"Name"</h3>
                    <div class="pim-form-grid">{input("firstName", "First name", !editing)}{input("lastName", "Last name", false)}</div>
                    <details class="form-more" open=extra_names><summary>"Prefix, middle name, suffix and display name"</summary>
                        <div class="pim-form-grid">{input("prefix", "Prefix", false)}{input("middleName", "Middle name", false)}{input("suffix", "Suffix", false)}
                            <label>"Display name"<input prop:value=move || data.with(|d| s(d, "displayName")) on:input=move |e| set("displayName", json!(event_target_value(&e))) maxlength="1024" placeholder=move || { let p = display_placeholder(); (!p.is_empty()).then_some(p) }/></label>
                        </div>
                    </details>
                </section>
                <section class="form-section"><h3><Icon name="building"/>"Work"</h3><div class="pim-form-grid">{input("organization", "Organization", false)}{input("title", "Job title", false)}</div></section>
                {multi("emails", "Email", "mail")}{multi("phones", "Phone", "phone")}{multi("urls", "Website", "globe")}
                <fieldset class="form-section"><legend><Icon name="location"/>"Postal addresses"</legend>
                    {move || data.with(|d| d["addresses"].as_array().cloned().unwrap_or_default()).into_iter().enumerate().map(|(i, a)| view! {
                        <div class="pim-form-grid contact-address">
                            {ADDRESS_LABELS.iter().map(|&(key, label)| {
                                let value = a[key].as_str().unwrap_or_default().to_owned();
                                view! { <label class=(key == "street").then_some("wide")>{label}<input prop:value=value on:input=move |e| data.update(|d| d["addresses"][i][key] = json!(event_target_value(&e)))/></label> }
                            }).collect_view()}
                            <button type="button" class="text-button danger" on:click=move |_| data.update(|d| { if let Some(list) = d["addresses"].as_array_mut() { list.remove(i); } })><Icon name="trash"/>"Remove address"</button>
                        </div>
                    }).collect_view()}
                    <button type="button" class="text-button add-button" on:click=move |_| data.update(|d| { if let Some(list) = d["addresses"].as_array_mut() { list.push(json!({ "label": "home", "street": "", "city": "", "region": "", "postalCode": "", "country": "" })); } })><Icon name="plus"/>"Add address"</button>
                </fieldset>
                <section class="form-section"><h3><Icon name="info"/>"More"</h3>
                    <div class="pim-form-grid"><label>"Birthday"<input placeholder="YYYY-MM-DD or --MM-DD" prop:value=move || data.with(|d| s(d, "birthday")) on:input=move |e| set("birthday", json!(event_target_value(&e)))/></label></div>
                    <label>"Groups (one per line)"<textarea rows="2" prop:value=move || data.with(|d| d["groups"].as_array().map(|g| g.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default()) on:input=move |e| set("groups", json!(event_target_value(&e).split('\n').collect::<Vec<_>>()))></textarea></label>
                    <label>"Notes"<textarea rows="4" prop:value=move || data.with(|d| s(d, "notes")) on:input=move |e| set("notes", json!(event_target_value(&e)))></textarea></label>
                    <label class="pim-checkbox"><input type="checkbox" prop:checked=move || data.with(|d| d["favorite"] == json!(true)) on:change=move |e| set("favorite", json!(event_target_checked(&e)))/>"Favorite"</label>
                </section>
                <div class="dialog-actions pane-footer"><button type="button" class="text-button">"Cancel"</button><button class="primary-button" disabled=move || busy.get() || book.with(String::is_empty)>"Save contact"</button></div>
            </form>
        </EditorPane>
    }
}
