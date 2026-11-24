//! Contacts app: scrollable sidebar with smart views, live counts & upcoming birthdays;
//! List / Table / Cards directory with A–Z jump rail, quick filters, hover multi-select &
//! bulk actions; rich inspector with photos, copy buttons, inline groups & vCard QR modal;
//! Merge & fix duplicate cleanup; redesigned editor; and single-key keyboard shortcuts.

mod detail;
mod editor;
mod list;
mod merge;
mod model;
mod sidebar;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use self::detail::ContactDetails;
use self::editor::ContactEditor;
use self::list::ContactsList;
use self::merge::MergeView;
use self::model::{
    BASE, Books, ContactFilter, ContactScope, ContactToast, DuplicatePair, People, UndoAction,
    book_counts, contact_groups, display_name, find_duplicates, group_counts, matches_filter,
    merge_contact_data, s, search_params, sort_contacts, upcoming_birthdays, vcard_qr_payload,
};
use self::sidebar::ContactsSidebar;
use super::{Collection, CollectionEditor, Done, Item, PimShell, Sharing, etag, send, use_pim};
use crate::api::{self, BrowserSession};
use crate::chat::{qr_matrix, qr_path};
use crate::icons::Icon;
use crate::shared::{context, save_preferences};
use crate::ui::{Dialog, Select, document, window};

#[component]
pub fn Contacts(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let pim = use_pim(on_signed_out);
    let initial =
        web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default())
            .expect("params");
    let initial_book = initial.get("book").unwrap_or_default();
    let scope = RwSignal::new(if initial_book.is_empty() {
        ContactScope::All
    } else {
        ContactScope::Book(initial_book.clone())
    });
    let active_group = RwSignal::new(String::new());
    let filter = RwSignal::new(ContactFilter::All);
    let sort = RwSignal::new("firstName".to_owned());
    let view_mode = RwSignal::new("list".to_owned());
    let books: RwSignal<Vec<Collection>> = RwSignal::new(Vec::new());
    let all_people: RwSignal<Vec<Item>> = RwSignal::new(Vec::new());
    let people: RwSignal<Vec<Item>> = RwSignal::new(Vec::new());
    let groups: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let cursor = RwSignal::new(String::new());
    let next = RwSignal::new(String::new());
    let previous: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let selected: RwSignal<Option<(String, String)>> =
        RwSignal::new(match (initial.get("book"), initial.get("contact")) {
            (Some(b), Some(c)) => Some((b, c)),
            _ => None,
        });
    let detail: RwSignal<Option<Item>> = RwSignal::new(None);
    let checked: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let dismissed_dups: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let modal: RwSignal<Option<&'static str>> = RwSignal::new(None);
    let qr_item: RwSignal<Option<Item>> = RwSignal::new(None);
    let action_book: RwSignal<Option<Collection>> = RwSignal::new(None);
    let action_group: RwSignal<String> = RwSignal::new(String::new());
    let dialog_input = RwSignal::new(String::new());
    let bulk_target_book = RwSignal::new(String::new());
    let delete_count: RwSignal<Option<usize>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let toast: RwSignal<Option<ContactToast>> = RwSignal::new(None);
    let toast_seq = RwSignal::new(0u32);
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);

    let session = StoredValue::new(session);
    let csrf = move || session.with_value(|s| s.csrf_token.clone());
    let loaded_list = StoredValue::new(String::new());
    let loaded_detail = StoredValue::new(String::new());
    let sequence = RwSignal::new(0u32);
    let action_sequence = RwSignal::new(0u32);

    let show_toast = move |message: String, undo: Option<UndoAction>| {
        let id = toast_seq.get_untracked() + 1;
        toast_seq.set(id);
        toast.set(Some(ContactToast { message, undo }));
        spawn_local(async move {
            crate::sleep(4500).await;
            if toast_seq.try_get_untracked() == Some(id) {
                toast.try_set(None);
            }
        });
    };

    let preferences = context().preferences;
    Effect::new(move |_| {
        if let Some(p) = preferences.value.get() {
            if let Some(s_val) = p.rest.get("contactSort").and_then(Value::as_str) {
                sort.set(s_val.to_owned());
            }
            if let Some(v_val) = p.rest.get("contactView").and_then(Value::as_str) {
                view_mode.set(v_val.to_owned());
            }
        }
    });

    let name_order = Signal::derive(move || {
        preferences
            .value
            .with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get("contactNameOrder"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "firstLast".to_owned())
    });
    let show_birthdays = Signal::derive(move || {
        preferences.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("contactShowBirthdays"))
                .and_then(Value::as_bool)
                .unwrap_or(true)
        })
    });
    let default_book_pref = Signal::derive(move || {
        preferences
            .value
            .with(|p| {
                p.as_ref()
                    .and_then(|p| p.rest.get("contactDefaultBook"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    });

    let persist_pref = move |key: &'static str, value: Value| {
        let sess = session.get_value();
        spawn_local(async move {
            let rev = preferences
                .value
                .get_untracked()
                .map(|p| p.revision)
                .unwrap_or(0);
            if let Ok(next_prefs) =
                save_preferences(json!({ "revision": rev, key: value }), rev, &sess).await
            {
                preferences.value.try_set(Some(next_prefs));
            }
        });
    };

    let active_book_id = Memo::new(move |_| match scope.get() {
        ContactScope::Book(id) => id,
        _ => String::new(),
    });
    let collection = Memo::new(move |_| {
        let id = active_book_id.get();
        books.with(|b| b.iter().find(|x| x.id == id).cloned())
    });
    let detail_book = Memo::new(move |_| {
        detail.with(|d| {
            d.as_ref()
                .and_then(|d| books.with(|b| b.iter().find(|x| x.id == d.collection_id).cloned()))
        })
    });
    let writable =
        Memo::new(move |_| detail_book.with(|b| b.as_ref().is_some_and(|b| b.role != "read")));

    // Load books, full overview for counts/birthdays/duplicates, and scoped page.
    Effect::new(move |_| {
        pim.epoch.track();
        let sc = scope.get();
        let book_param = match &sc {
            ContactScope::Book(id) => id.clone(),
            _ => String::new(),
        };
        let fav_param = sc == ContactScope::Favorites;
        let sort_now = sort.get();
        let server_sort = if sort_now == "lastName" {
            "lastName".to_owned()
        } else {
            "firstName".to_owned()
        };
        let grp = active_group.get();
        let cur = cursor.get();
        let key = json!([book_param, server_sort, grp, fav_param, cur]).to_string();
        if loaded_list.get_value() != key {
            loading.set(true);
        }
        let id = sequence.get_untracked() + 1;
        sequence.set(id);

        let query = search_params(&[
            ("book", book_param.clone()),
            ("q", String::new()),
            ("sort", server_sort.clone()),
            ("group", grp.clone()),
            ("favorite", if fav_param { "1".into() } else { "0".into() }),
            ("cursor", cur.clone()),
        ]);
        let overview_query = search_params(&[
            ("book", String::new()),
            ("q", String::new()),
            ("sort", "firstName".into()),
            ("group", String::new()),
            ("favorite", "0".into()),
            ("cursor", String::new()),
        ]);
        let is_unscoped_first =
            book_param.is_empty() && grp.is_empty() && !fav_param && cur.is_empty();

        spawn_local(async move {
            let b = api::get::<Books>(BASE).await;
            let list = api::get::<People>(&format!("/api/v1/contacts/people?{query}")).await;
            let overview = if is_unscoped_first {
                None
            } else {
                api::get::<People>(&format!("/api/v1/contacts/people?{overview_query}"))
                    .await
                    .ok()
            };
            if sequence.try_get_untracked() != Some(id) {
                return;
            }
            match (b, list) {
                (Ok(b), Ok(list)) => {
                    loaded_list.set_value(key);
                    books.set(b.items);
                    if let Some(ov) = overview {
                        all_people.set(ov.items);
                        groups.set(ov.groups);
                    } else {
                        all_people.set(list.items.clone());
                        groups.set(list.groups.clone());
                    }
                    people.set(list.items);
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

    // Load selected contact details.
    Effect::new(move |_| {
        pim.epoch.track();
        let current = selected.get();
        let key = current
            .as_ref()
            .map(|(c, h)| format!("{c}/{h}"))
            .unwrap_or_default();
        if loaded_detail.get_value() != key {
            detail.set(None);
        }
        loaded_detail.set_value(key.clone());
        let Some((collection_id, href)) = current else {
            return;
        };
        spawn_local(async move {
            let result = api::get::<Item>(&format!("{BASE}/{collection_id}/items/{href}")).await;
            if loaded_detail.try_get_value().as_deref() != Some(key.as_str()) {
                return;
            }
            match result {
                Ok(row) => detail.set(Some(row)),
                Err(e) => {
                    selected.set(None);
                    show_toast(
                        "This contact was removed or your access changed.".into(),
                        None,
                    );
                    error.set(e.message);
                }
            }
        });
    });

    crate::ui::listen(&window(), "popstate", false, move |_| {
        let params = web_sys::UrlSearchParams::new_with_str(
            &window().location().search().unwrap_or_default(),
        )
        .expect("params");
        let b = params.get("book").unwrap_or_default();
        scope.set(if b.is_empty() {
            ContactScope::All
        } else {
            ContactScope::Book(b)
        });
        selected.set(match (params.get("book"), params.get("contact")) {
            (Some(b), Some(c)) => Some((b, c)),
            _ => None,
        });
    });

    let push_url = |url: &str| {
        let _ = window().history().expect("history").push_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(url),
        );
    };
    let reset_nav = move || {
        cursor.set(String::new());
        previous.set(Vec::new());
        selected.set(None);
        checked.set(HashSet::new());
    };
    let select_scope = move |next_scope: ContactScope| {
        scope.set(next_scope.clone());
        active_group.set(String::new());
        reset_nav();
        let url = match next_scope {
            ContactScope::Book(id) => format!("/contacts?book={id}"),
            _ => "/contacts".to_owned(),
        };
        push_url(&url);
    };
    let choose = move |row: &Item| {
        selected.set(Some((row.collection_id.clone(), row.href.clone())));
        push_url(&format!(
            "/contacts?{}",
            search_params(&[
                ("book", row.collection_id.clone()),
                ("contact", row.href.clone()),
            ])
        ));
    };
    let close_details = move || {
        selected.set(None);
        let b = active_book_id.get_untracked();
        let _ = window().history().expect("history").replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&if b.is_empty() {
                "/contacts".into()
            } else {
                format!("/contacts?book={b}")
            }),
        );
    };

    // Derived counts, filtered/sorted visible rows, duplicates & birthdays.
    let total_count = Memo::new(move |_| all_people.with(Vec::len));
    let fav_count = Memo::new(move |_| {
        all_people.with(|list| {
            list.iter()
                .filter(|r| r.data["favorite"] == json!(true))
                .count()
        })
    });
    let ungrouped_count = Memo::new(move |_| {
        all_people.with(|list| {
            list.iter()
                .filter(|r| contact_groups(&r.data).is_empty())
                .count()
        })
    });
    let b_counts = Memo::new(move |_| all_people.with(|l| book_counts(l)));
    let g_counts = Memo::new(move |_| all_people.with(|l| group_counts(l)));
    let duplicates = Memo::new(move |_| {
        let dismissed = dismissed_dups.get();
        all_people.with(|l| find_duplicates(l, &dismissed))
    });
    let birthdays = Memo::new(move |_| all_people.with(|l| upcoming_birthdays(l)));

    let visible_rows = Memo::new(move |_| {
        let sc = scope.get();
        let flt = filter.get();
        let sort_now = sort.get();
        let mut rows = if sc == ContactScope::Ungrouped {
            all_people.with(|list| {
                list.iter()
                    .filter(|r| contact_groups(&r.data).is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
            })
        } else {
            people.get()
        };
        rows.retain(|r| matches_filter(&r.data, flt));
        sort_contacts(&mut rows, &sort_now);
        rows
    });

    let run =
        move |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
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

    // Single-key keyboard shortcuts (outside inputs/editors/dialogs).
    crate::ui::listen(&window(), "keydown", false, move |ev| {
        let Ok(e) = ev.dyn_into::<web_sys::KeyboardEvent>() else {
            return;
        };
        if e.ctrl_key() || e.meta_key() || e.alt_key() || e.default_prevented() {
            return;
        }
        if modal.try_get_untracked().flatten().is_some()
            || qr_item.try_get_untracked().flatten().is_some()
        {
            return;
        }
        let in_field = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|el| {
                el.closest("input,textarea,select,[contenteditable],[role=\"dialog\"]")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if in_field {
            return;
        }
        match e.key().as_str() {
            "c" | "C" => {
                e.prevent_default();
                modal.set(Some(
                    if books.with_untracked(|b| b.iter().any(|x| x.role != "read")) {
                        "new"
                    } else {
                        "book"
                    },
                ));
            }
            "j" | "J" | "k" | "K" => {
                let rows = visible_rows.get_untracked();
                if rows.is_empty() {
                    return;
                }
                e.prevent_default();
                let cur = selected.get_untracked();
                let idx = cur.as_ref().and_then(|(c, h)| {
                    rows.iter()
                        .position(|r| r.collection_id == *c && r.href == *h)
                });
                let next_idx = match (idx, e.key().as_str()) {
                    (Some(i), "j" | "J") => (i + 1).min(rows.len() - 1),
                    (Some(i), _) => i.saturating_sub(1),
                    (None, "j" | "J") => 0,
                    (None, _) => rows.len() - 1,
                };
                choose(&rows[next_idx]);
            }
            "e" | "E" => {
                if detail.with_untracked(Option::is_some) && writable.get_untracked() {
                    e.prevent_default();
                    modal.set(Some("edit"));
                }
            }
            "f" | "F" => {
                if let Some(d) = detail.get_untracked()
                    && writable.get_untracked()
                {
                    e.prevent_default();
                    let prev = d.data.clone();
                    let next_fav = prev["favorite"] != json!(true);
                    let cid = d.collection_id.clone();
                    let href = d.href.clone();
                    let tag = etag(&d.id, d.revision);
                    let token = csrf();
                    spawn_local(async move {
                        let mut data = prev.clone();
                        data["favorite"] = json!(next_fav);
                        if let Ok(updated) = send::<Item>(
                            &format!("{BASE}/{cid}/items/{href}"),
                            "PUT",
                            Some(json!({ "data": data })),
                            &token,
                            Some(tag),
                        )
                        .await
                        {
                            pim.refresh();
                            show_toast(
                                if next_fav {
                                    "Added to Favorites".into()
                                } else {
                                    "Removed from Favorites".into()
                                },
                                Some(UndoAction::ToggleFavorite {
                                    collection_id: cid,
                                    href,
                                    revision: updated.revision,
                                    previous_data: prev,
                                }),
                            );
                        }
                    });
                }
            }
            "x" | "X" => {
                if let Some(d) = detail.get_untracked() {
                    e.prevent_default();
                    checked.update(|set| {
                        if !set.remove(&d.id) {
                            set.insert(d.id);
                        }
                    });
                }
            }
            "Escape" => {
                if !checked.with_untracked(HashSet::is_empty) {
                    checked.set(HashSet::new());
                } else if selected.with_untracked(Option::is_some) {
                    close_details();
                }
            }
            _ => {}
        }
    });

    let toggle_favorite_cb = Callback::new(move |row: Item| {
        let prev = row.data.clone();
        let next_fav = prev["favorite"] != json!(true);
        let cid = row.collection_id.clone();
        let href = row.href.clone();
        let tag = etag(&row.id, row.revision);
        let token = csrf();
        spawn_local(async move {
            busy.set(true);
            let mut data = prev.clone();
            data["favorite"] = json!(next_fav);
            match send::<Item>(
                &format!("{BASE}/{cid}/items/{href}"),
                "PUT",
                Some(json!({ "data": data })),
                &token,
                Some(tag),
            )
            .await
            {
                Ok(updated) => {
                    pim.refresh();
                    show_toast(
                        if next_fav {
                            "Added to Favorites".into()
                        } else {
                            "Removed from Favorites".into()
                        },
                        Some(UndoAction::ToggleFavorite {
                            collection_id: cid,
                            href,
                            revision: updated.revision,
                            previous_data: prev,
                        }),
                    );
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    });

    let clone_cb = Callback::new(move |row: Item| {
        let mut data = row.data.clone();
        let dn = s(&data, "displayName");
        if !dn.is_empty() {
            data["displayName"] = json!(format!("{dn} (copy)"));
        }
        let cid = row.collection_id.clone();
        let token = csrf();
        spawn_local(async move {
            busy.set(true);
            match send::<Item>(
                &format!("{BASE}/{cid}/items"),
                "POST",
                Some(json!({ "data": data })),
                &token,
                None,
            )
            .await
            {
                Ok(created) => {
                    choose(&created);
                    pim.refresh();
                    show_toast("Contact cloned.".into(), None);
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    });

    let update_groups_cb = Callback::new(move |(row, next_groups): (Item, Vec<String>)| {
        let mut data = row.data.clone();
        data["groups"] = json!(next_groups);
        let cid = row.collection_id.clone();
        let href = row.href.clone();
        let tag = etag(&row.id, row.revision);
        let token = csrf();
        spawn_local(async move {
            busy.set(true);
            match send::<Item>(
                &format!("{BASE}/{cid}/items/{href}"),
                "PUT",
                Some(json!({ "data": data })),
                &token,
                Some(tag),
            )
            .await
            {
                Ok(updated) => {
                    detail.set(Some(updated));
                    pim.refresh();
                    show_toast("Groups updated.".into(), None);
                }
                Err(e) => error.set(e.message),
            }
            busy.set(false);
        });
    });

    let merge_pair_cb = Callback::new(move |pair: DuplicatePair| {
        let merged = merge_contact_data(&pair.primary.data, &pair.secondary.data);
        let p_url = format!(
            "{BASE}/{}/items/{}",
            pair.primary.collection_id, pair.primary.href
        );
        let p_tag = etag(&pair.primary.id, pair.primary.revision);
        let s_url = format!(
            "{BASE}/{}/items/{}",
            pair.secondary.collection_id, pair.secondary.href
        );
        let s_tag = etag(&pair.secondary.id, pair.secondary.revision);
        let token = csrf();
        spawn_local(async move {
            busy.set(true);
            error.set(String::new());
            let res: Result<(), String> = async {
                send::<Item>(
                    &p_url,
                    "PUT",
                    Some(json!({ "data": merged })),
                    &token,
                    Some(p_tag),
                )
                .await
                .map_err(|e| e.message)?;
                send::<Value>(&s_url, "DELETE", None, &token, Some(s_tag))
                    .await
                    .map_err(|e| e.message)?;
                Ok(())
            }
            .await;
            match res {
                Ok(()) => {
                    pim.refresh();
                    show_toast("Merged duplicate contacts.".into(), None);
                }
                Err(e) => error.set(e),
            }
            busy.set(false);
        });
    });

    let merge_all_cb = Callback::new(move |()| {
        let list = duplicates.get_untracked();
        let writable_ids: HashSet<String> = books.with_untracked(|bs| {
            bs.iter()
                .filter(|b| b.role != "read")
                .map(|b| b.id.clone())
                .collect()
        });
        let token = csrf();
        spawn_local(async move {
            busy.set(true);
            let mut touched = HashSet::new();
            let mut merged_count = 0usize;
            for pair in list {
                if !writable_ids.contains(&pair.primary.collection_id)
                    || !writable_ids.contains(&pair.secondary.collection_id)
                    || touched.contains(&pair.primary.id)
                    || touched.contains(&pair.secondary.id)
                {
                    continue;
                }
                let merged = merge_contact_data(&pair.primary.data, &pair.secondary.data);
                let p_url = format!(
                    "{BASE}/{}/items/{}",
                    pair.primary.collection_id, pair.primary.href
                );
                let p_tag = etag(&pair.primary.id, pair.primary.revision);
                let s_url = format!(
                    "{BASE}/{}/items/{}",
                    pair.secondary.collection_id, pair.secondary.href
                );
                let s_tag = etag(&pair.secondary.id, pair.secondary.revision);
                if send::<Item>(
                    &p_url,
                    "PUT",
                    Some(json!({ "data": merged })),
                    &token,
                    Some(p_tag),
                )
                .await
                .is_ok()
                    && send::<Value>(&s_url, "DELETE", None, &token, Some(s_tag))
                        .await
                        .is_ok()
                {
                    touched.insert(pair.primary.id);
                    touched.insert(pair.secondary.id);
                    merged_count += 1;
                }
            }
            pim.refresh();
            show_toast(format!("Merged {merged_count} duplicate pair(s)."), None);
            busy.set(false);
        });
    });

    let bulk_export_cb = Callback::new(move |()| {
        let ids = checked.get_untracked();
        let items: Vec<Item> = visible_rows
            .get_untracked()
            .into_iter()
            .filter(|r| ids.contains(&r.id))
            .collect();
        if items.is_empty() {
            return;
        }
        spawn_local(async move {
            busy.set(true);
            let mut combined = String::new();
            for item in &items {
                if let Ok(resp) = wasm_bindgen_futures::JsFuture::from(window().fetch_with_str(
                    &format!("{BASE}/{}/items/{}?export=1", item.collection_id, item.href),
                ))
                .await
                    && let Ok(r) = resp.dyn_into::<web_sys::Response>()
                    && r.ok()
                    && let Ok(p) = r.text()
                    && let Ok(txt) = wasm_bindgen_futures::JsFuture::from(p).await
                    && let Some(s) = txt.as_string()
                {
                    combined.push_str(&s);
                }
            }
            if !combined.is_empty() {
                let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(&combined));
                let bag = web_sys::BlobPropertyBag::new();
                bag.set_type("text/vcard;charset=utf-8");
                if let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &bag)
                    && let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob)
                    && let Ok(el) = document().create_element("a")
                    && let Ok(a) = el.dyn_into::<web_sys::HtmlAnchorElement>()
                {
                    a.set_href(&url);
                    a.set_download("contacts-selected.vcf");
                    a.click();
                    let _ = web_sys::Url::revoke_object_url(&url);
                    show_toast(format!("Exported {} contacts.", items.len()), None);
                }
            }
            busy.set(false);
        });
    });

    let book_action = Callback::new(move |(target, kind): (Collection, &'static str)| {
        let id = action_sequence.get_untracked() + 1;
        action_sequence.set(id);
        action_book.set(Some(target.clone()));
        modal.set(Some(kind));
        error.set(String::new());
        if kind == "deleteBook" {
            delete_count.set(None);
            spawn_local(async move {
                match api::get::<Value>(&format!("{BASE}/{}", target.id)).await {
                    Ok(v) if action_sequence.try_get_untracked() == Some(id) => {
                        delete_count.set(v["count"].as_u64().map(|c| c as usize))
                    }
                    Err(e) if action_sequence.try_get_untracked() == Some(id) => {
                        error.set(e.message)
                    }
                    _ => {}
                }
            });
        }
    });

    let group_action = Callback::new(move |(gname, kind): (String, &'static str)| {
        action_group.set(gname.clone());
        dialog_input.set(gname);
        error.set(String::new());
        modal.set(Some(kind));
    });

    let save = Callback::new(move |(data, target_book, done): (Value, String, Done)| {
        let editing = modal.get_untracked() == Some("edit");
        let current = detail.get_untracked();
        let token = csrf();
        spawn_local(async move {
            let result = match (editing, &current) {
                (true, Some(d)) if target_book == d.collection_id || target_book.is_empty() => {
                    send::<Item>(
                        &format!("{BASE}/{}/items/{}", d.collection_id, d.href),
                        "PUT",
                        Some(json!({ "data": data })),
                        &token,
                        Some(etag(&d.id, d.revision)),
                    )
                    .await
                }
                (true, Some(d)) => {
                    // Move to another address book: create in target, then remove original.
                    match send::<Item>(
                        &format!("{BASE}/{target_book}/items"),
                        "POST",
                        Some(json!({ "data": data })),
                        &token,
                        None,
                    )
                    .await
                    {
                        Ok(created) => {
                            let _ = send::<Value>(
                                &format!("{BASE}/{}/items/{}", d.collection_id, d.href),
                                "DELETE",
                                None,
                                &token,
                                Some(etag(&d.id, d.revision)),
                            )
                            .await;
                            Ok(created)
                        }
                        Err(e) => Err(e),
                    }
                }
                _ => {
                    send::<Item>(
                        &format!("{BASE}/{target_book}/items"),
                        "POST",
                        Some(json!({ "data": data })),
                        &token,
                        None,
                    )
                    .await
                }
            };
            match result {
                Ok(row) => {
                    choose(&row);
                    pim.refresh();
                    modal.set(None);
                    show_toast("Contact saved.".into(), None);
                    done.try_run(Ok(()));
                }
                Err(e) => {
                    done.try_run(Err(e.message));
                }
            }
        });
    });

    let title = Signal::derive(move || match scope.get() {
        ContactScope::Favorites => "Favorites".to_owned(),
        ContactScope::Ungrouped => "Ungrouped".to_owned(),
        ContactScope::Merge => "Merge & fix".to_owned(),
        ContactScope::Book(_) => collection
            .get()
            .map(|c| c.name)
            .unwrap_or_else(|| "All contacts".into()),
        ContactScope::All => "All contacts".to_owned(),
    });

    let sidebar_fn: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            <ContactsSidebar
                books=books
                groups=groups
                scope=scope
                active_group=active_group
                total_count=total_count
                fav_count=fav_count
                ungrouped_count=ungrouped_count
                book_counts=b_counts
                group_counts=g_counts
                duplicates=duplicates
                birthdays=birthdays
                show_birthdays=show_birthdays
                on_new_contact=Callback::new(move |()| {
                    modal.set(Some(if books.with(|b| b.iter().any(|x| x.role != "read")) {
                        "new"
                    } else {
                        "book"
                    }));
                })
                on_new_book=Callback::new(move |()| modal.set(Some("book")))
                on_new_group=Callback::new(move |()| {
                    dialog_input.set(String::new());
                    error.set(String::new());
                    modal.set(Some("newGroup"));
                })
                on_select_scope=Callback::new(move |sc| select_scope(sc))
                on_toggle_group=Callback::new(move |chosen: String| {
                    if scope.get_untracked() == ContactScope::Merge
                        || scope.get_untracked() == ContactScope::Ungrouped
                    {
                        scope.set(ContactScope::All);
                    }
                    active_group.set(if active_group.get_untracked() == chosen {
                        String::new()
                    } else {
                        chosen
                    });
                    reset_nav();
                })
                on_book_action=book_action
                on_group_action=group_action
                on_open_birthday=Callback::new(move |(cid, href): (String, String)| {
                    if scope.get_untracked() == ContactScope::Merge {
                        scope.set(ContactScope::All);
                    }
                    selected.set(Some((cid.clone(), href.clone())));
                    push_url(&format!(
                        "/contacts?{}",
                        search_params(&[("book", cid), ("contact", href)])
                    ));
                })
            />
        }
        .into_any()
    });

    let has_details_sig = Signal::from(Memo::new(move |_| {
        scope.get() != ContactScope::Merge && selected.with(Option::is_some)
    }));

    let details_fn: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            {move || match detail.get() {
                None => view! {
                    <div class="pim-detail-top">
                        <span>"Contact"</span>
                        <button type="button" class="icon-button" aria-label="Close contact details" on:click=move |_| close_details()><Icon name="close"/></button>
                    </div>
                    <p role="status">"Loading contact\u{2026}"</p>
                }.into_any(),
                Some(d) => view! {
                    <ContactDetails
                        item=d
                        book=detail_book.get()
                        all_groups=Signal::from(groups)
                        name_order=name_order
                        writable=writable.get()
                        busy=busy
                        on_close=Callback::new(move |()| close_details())
                        on_edit=Callback::new(move |()| modal.set(Some("edit")))
                        on_toggle_favorite=toggle_favorite_cb
                        on_clone=clone_cb
                        on_show_qr=Callback::new(move |item| qr_item.set(Some(item)))
                        on_delete=Callback::new(move |()| modal.set(Some("delete")))
                        on_filter_group=Callback::new(move |g| {
                            active_group.set(g);
                            cursor.set(String::new());
                            previous.set(Vec::new());
                        })
                        on_update_groups=update_groups_cb
                    />
                }.into_any(),
            }}
        }
        .into_any()
    });

    let shell_session = session.get_value();
    view! {
        <PimShell
            kind="contacts"
            main_class="contacts-main"
            session=shell_session
            on_signed_out=on_signed_out
            sidebar=sidebar_fn
            has_details=has_details_sig
            details=details_fn
            pim=pim
        >
            {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            {move || if scope.get() == ContactScope::Merge {
                view! {
                    <MergeView
                        pairs=duplicates
                        books=books
                        busy=busy
                        on_merge=merge_pair_cb
                        on_dismiss=Callback::new(move |key: String| {
                            dismissed_dups.update(|set| { set.insert(key); });
                            show_toast("Suggestion dismissed.".into(), None);
                        })
                        on_merge_all=merge_all_cb
                    />
                }.into_any()
            } else {
                view! {
                    <ContactsList
                        title=title
                        rows=visible_rows
                        books=books
                        loading=loading
                        sort=sort
                        name_order=name_order
                        view_mode=view_mode
                        filter=filter
                        active_group=active_group
                        selected=selected
                        checked=checked
                        has_details=has_details_sig
                        on_choose=Callback::new(move |row: Item| choose(&row))
                        on_toggle_favorite=toggle_favorite_cb
                        on_toggle_check=Callback::new(move |id: String| {
                            checked.update(|set| {
                                if !set.remove(&id) {
                                    set.insert(id);
                                }
                            });
                        })
                        on_select_all=Callback::new(move |()| {
                            let all_ids: HashSet<String> = visible_rows
                                .get_untracked()
                                .into_iter()
                                .map(|r| r.id)
                                .collect();
                            if checked.with_untracked(HashSet::len) >= all_ids.len() {
                                checked.set(HashSet::new());
                            } else {
                                checked.set(all_ids);
                            }
                        })
                        on_clear_checked=Callback::new(move |()| checked.set(HashSet::new()))
                        on_bulk_group=Callback::new(move |()| {
                            dialog_input.set(String::new());
                            error.set(String::new());
                            modal.set(Some("bulkGroup"));
                        })
                        on_bulk_move=Callback::new(move |()| {
                            let first = books.with_untracked(|b| {
                                b.iter().find(|x| x.role != "read").map(|x| x.id.clone()).unwrap_or_default()
                            });
                            bulk_target_book.set(first);
                            error.set(String::new());
                            modal.set(Some("bulkMove"));
                        })
                        on_bulk_export=bulk_export_cb
                        on_bulk_delete=Callback::new(move |()| {
                            error.set(String::new());
                            modal.set(Some("bulkDelete"));
                        })
                        on_change_sort=Callback::new(move |next_sort: String| {
                            sort.set(next_sort.clone());
                            persist_pref("contactSort", json!(next_sort));
                        })
                        on_change_view=Callback::new(move |next_view: String| {
                            view_mode.set(next_view.clone());
                            if next_view == "table" {
                                selected.set(None);
                            }
                            persist_pref("contactView", json!(next_view));
                        })
                        on_clear_group=Callback::new(move |()| {
                            active_group.set(String::new());
                            reset_nav();
                        })
                        on_refresh=Callback::new(move |()| pim.refresh())
                    />
                    {move || (!previous.with(Vec::is_empty) || !next.with(String::is_empty)).then(|| view! {
                        <div class="pagination">
                            <button type="button" class="text-button" disabled=move || previous.with(Vec::is_empty) on:click=move |_| {
                                cursor.set(previous.with_untracked(|p| p.last().cloned()).unwrap_or_default());
                                previous.update(|p| { p.pop(); });
                            }>"Previous"</button>
                            <button type="button" class="text-button" disabled=move || next.with(String::is_empty) on:click=move |_| {
                                previous.update(|p| p.push(cursor.get_untracked()));
                                cursor.set(next.get_untracked());
                            }>"Next"</button>
                        </div>
                    })}
                }.into_any()
            }}

            {move || toast.get().map(|t| {
                let undo = t.undo.clone();
                view! {
                    <div class="contacts-toast" role="status" aria-live="polite">
                        <span>{t.message}</span>
                        {undo.map(|act| view! {
                            <button type="button" class="text-button" on:click=move |_| {
                                toast.set(None);
                                match act.clone() {
                                    UndoAction::ToggleFavorite { collection_id, href, revision, previous_data } => {
                                        let token = csrf();
                                        let id_opt = detail.get_untracked().map(|d| d.id).or_else(|| {
                                            all_people.with_untracked(|l| {
                                                l.iter().find(|r| r.collection_id == collection_id && r.href == href).map(|r| r.id.clone())
                                            })
                                        });
                                        if let Some(id) = id_opt {
                                            spawn_local(async move {
                                                let _ = send::<Item>(
                                                    &format!("{BASE}/{collection_id}/items/{href}"),
                                                    "PUT",
                                                    Some(json!({ "data": previous_data })),
                                                    &token,
                                                    Some(etag(&id, revision)),
                                                ).await;
                                                pim.refresh();
                                            });
                                        }
                                    }
                                }
                            }>"Undo"</button>
                        })}
                    </div>
                }
            })}

            {move || matches!(modal.get(), Some("new" | "edit")).then(|| {
                let editing = modal.get_untracked() == Some("edit");
                let cur_detail = if editing { detail.get_untracked() } else { None };
                let initial_data = cur_detail.as_ref().map(|d| d.data.clone());
                let initial_href = cur_detail.as_ref().map(|d| d.href.clone()).unwrap_or_default();
                let initial_rev = cur_detail.as_ref().map(|d| d.revision).unwrap_or(0);
                let writable_books: Vec<Collection> = books
                    .get_untracked()
                    .into_iter()
                    .filter(|b| b.role != "read")
                    .collect();
                let init_book = if editing {
                    cur_detail.map(|d| d.collection_id).unwrap_or_default()
                } else {
                    let active = active_book_id.get_untracked();
                    let pref = default_book_pref.get_untracked();
                    if !active.is_empty() && writable_books.iter().any(|b| b.id == active) {
                        active
                    } else if !pref.is_empty() && writable_books.iter().any(|b| b.id == pref) {
                        pref
                    } else {
                        writable_books.first().map(|b| b.id.clone()).unwrap_or_default()
                    }
                };
                view! {
                    <ContactEditor
                        books=writable_books
                        all_groups=groups.get_untracked()
                        initial=initial_data
                        initial_href=initial_href
                        initial_revision=initial_rev
                        initial_book=init_book
                        on_save=save
                        on_close=Callback::new(move |_| modal.set(None))
                    />
                }
            })}

            {move || matches!(modal.get(), Some("book" | "renameBook")).then(|| {
                let rename = modal.get_untracked() == Some("renameBook");
                let target = if rename { action_book.get_untracked() } else { None };
                let t2 = target.clone();
                view! {
                    <CollectionEditor
                        title=if rename { "Edit address book" } else { "New address book" }
                        collection=target.unwrap_or_else(|| Collection {
                            id: String::new(),
                            owner_id: String::new(),
                            kind: String::new(),
                            name: String::new(),
                            color: "#73396f".into(),
                            revision: 0,
                            role: String::new(),
                            subscription: None,
                        })
                        on_close=Callback::new(move |_| modal.set(None))
                        on_save=Callback::new(move |(name, color, done): (String, String, Done)| {
                            let t = t2.clone();
                            spawn_local(async move {
                                let (url, method, m) = match &t {
                                    Some(t) => (format!("{BASE}/{}", t.id), "PUT", Some(etag(&t.id, t.revision))),
                                    None => (BASE.to_owned(), "POST", None),
                                };
                                match send::<Collection>(&url, method, Some(json!({ "name": name, "color": color })), &csrf(), m).await {
                                    Ok(row) => {
                                        select_scope(ContactScope::Book(row.id));
                                        pim.refresh();
                                        modal.set(None);
                                        done.try_run(Ok(()));
                                    }
                                    Err(e) => {
                                        done.try_run(Err(e.message));
                                    }
                                }
                            });
                        })
                    />
                }
            })}

            {move || (modal.get() == Some("share")).then(|| action_book.get_untracked().map(|b| {
                let base = format!("{BASE}/{}", b.id);
                view! {
                    <Sharing
                        collection=b
                        base=base
                        session=session.get_value()
                        on_close=Callback::new(move |_| modal.set(None))
                        on_changed=Callback::new(move |_| pim.refresh())
                    />
                }
            })).flatten()}

            {move || qr_item.get().map(|item| {
                let name = display_name(&item.data, "firstLast");
                let payload = vcard_qr_payload(&item.data);
                let qr = qr_matrix(&payload).ok().map(|m| qr_path(&m));
                let export_href = format!("{BASE}/{}/items/{}?export=1", item.collection_id, item.href);
                view! {
                    <Dialog title=name on_close=Callback::new(move |_| qr_item.set(None))>
                        <div class="contacts-qr-body">
                            {match qr.clone() {
                                Some((d_path, size)) => view! {
                                    <div class="contacts-qr-frame">
                                        <svg viewBox=format!("0 0 {size} {size}") role="img" aria-label="Contact vCard QR code">
                                            <path d=d_path></path>
                                        </svg>
                                    </div>
                                }.into_any(),
                                None => view! { <p class="muted">"This contact has too much data to fit in a QR code. Use Download .vcf instead."</p> }.into_any(),
                            }}
                            <p class="muted contacts-qr-hint">"Scan with a phone camera to save this contact."</p>
                        </div>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| qr_item.set(None)>"Close"</button>
                            <a class="primary-button" href=export_href.clone()>"Download .vcf"</a>
                        </div>
                    </Dialog>
                }
            })}

            {move || matches!(modal.get(), Some("newGroup" | "renameGroup" | "bulkGroup")).then(|| {
                let kind = modal.get_untracked().unwrap_or("newGroup");
                let dlg_title = match kind {
                    "renameGroup" => format!("Rename group {}", action_group.get_untracked()),
                    "bulkGroup" => format!("Add {} contact(s) to group", checked.with_untracked(HashSet::len)),
                    _ => "New group".into(),
                };
                let btn_label = match kind {
                    "renameGroup" => "Rename group",
                    "bulkGroup" => "Add to group",
                    _ => "Create group",
                };
                view! {
                    <Dialog title=dlg_title on_close=Callback::new(move |_| modal.set(None))>
                        <form on:submit=move |e: web_sys::SubmitEvent| {
                            e.prevent_default();
                            let name = dialog_input.get_untracked().trim().to_owned();
                            if name.is_empty() {
                                return;
                            }
                            if kind == "newGroup" {
                                groups.update(|g| {
                                    if !g.contains(&name) {
                                        g.push(name.clone());
                                        g.sort();
                                    }
                                });
                                active_group.set(name.clone());
                                modal.set(None);
                                show_toast(format!("Group \"{name}\" ready. Assign contacts to keep it."), None);
                                return;
                            }
                            let writable_ids: HashSet<String> = books.with_untracked(|bs| {
                                bs.iter().filter(|b| b.role != "read").map(|b| b.id.clone()).collect()
                            });
                            let targets: Vec<Item> = if kind == "bulkGroup" {
                                let ids = checked.get_untracked();
                                visible_rows.get_untracked().into_iter().filter(|r| ids.contains(&r.id) && writable_ids.contains(&r.collection_id)).collect()
                            } else {
                                let old = action_group.get_untracked();
                                all_people.get_untracked().into_iter().filter(|r| writable_ids.contains(&r.collection_id) && contact_groups(&r.data).contains(&old)).collect()
                            };
                            let old_group = action_group.get_untracked();
                            let token = csrf();
                            run(Box::pin(async move {
                                for item in targets {
                                    let mut grps = contact_groups(&item.data);
                                    if kind == "renameGroup" {
                                        grps.retain(|g| g != &old_group);
                                    }
                                    if !grps.contains(&name) {
                                        grps.push(name.clone());
                                    }
                                    let mut data = item.data.clone();
                                    data["groups"] = json!(grps);
                                    send::<Item>(
                                        &format!("{BASE}/{}/items/{}", item.collection_id, item.href),
                                        "PUT",
                                        Some(json!({ "data": data })),
                                        &token,
                                        Some(etag(&item.id, item.revision)),
                                    ).await.map_err(|e| e.message)?;
                                }
                                if kind == "renameGroup" && active_group.get_untracked() == old_group {
                                    active_group.set(name.clone());
                                }
                                if kind == "bulkGroup" {
                                    checked.set(HashSet::new());
                                }
                                show_toast("Groups updated.".into(), None);
                                Ok(())
                            }));
                        }>
                            <label>
                                "Group name"
                                <input
                                    autofocus=true
                                    required=true
                                    maxlength="100"
                                    prop:value=move || dialog_input.get()
                                    on:input=move |e| dialog_input.set(event_target_value(&e))
                                />
                            </label>
                            {(kind == "bulkGroup").then(|| {
                                let existing = groups.get_untracked();
                                (!existing.is_empty()).then(|| view! {
                                    <div class="contacts-group-suggestions">
                                        {existing.into_iter().map(|g| {
                                            let c = g.clone();
                                            view! {
                                                <button type="button" class="contacts-suggest-pill" on:click=move |_| dialog_input.set(c.clone())>{g}</button>
                                            }
                                        }).collect_view()}
                                    </div>
                                })
                            })}
                            <div class="dialog-actions">
                                <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                                <button class="primary-button" disabled=move || busy.get()>{btn_label}</button>
                            </div>
                        </form>
                    </Dialog>
                }
            })}

            {move || (modal.get() == Some("deleteGroup")).then(|| {
                let g = action_group.get_untracked();
                let g_msg = g.clone();
                view! {
                    <Dialog title=format!("Remove group {g}?") on_close=Callback::new(move |_| modal.set(None))>
                        <p>{format!("Remove \"{g_msg}\" from all contacts in your writable address books? The contacts themselves are kept.")}</p>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button type="button" class="danger-button" disabled=move || busy.get() on:click=move |_| {
                                let target_g = action_group.get_untracked();
                                let writable_ids: HashSet<String> = books.with_untracked(|bs| {
                                    bs.iter().filter(|b| b.role != "read").map(|b| b.id.clone()).collect()
                                });
                                let targets: Vec<Item> = all_people.get_untracked().into_iter().filter(|r| writable_ids.contains(&r.collection_id) && contact_groups(&r.data).contains(&target_g)).collect();
                                let token = csrf();
                                run(Box::pin(async move {
                                    for item in targets {
                                        let next_g: Vec<String> = contact_groups(&item.data).into_iter().filter(|x| x != &target_g).collect();
                                        let mut data = item.data.clone();
                                        data["groups"] = json!(next_g);
                                        send::<Item>(&format!("{BASE}/{}/items/{}", item.collection_id, item.href), "PUT", Some(json!({ "data": data })), &token, Some(etag(&item.id, item.revision))).await.map_err(|e| e.message)?;
                                    }
                                    if active_group.get_untracked() == target_g {
                                        active_group.set(String::new());
                                    }
                                    show_toast("Group removed.".into(), None);
                                    Ok(())
                                }));
                            }>"Remove group"</button>
                        </div>
                    </Dialog>
                }
            })}

            {move || (modal.get() == Some("bulkMove")).then(|| {
                let opts: Vec<(String, String)> = books
                    .get_untracked()
                    .into_iter()
                    .filter(|b| b.role != "read")
                    .map(|b| (b.id, b.name))
                    .collect();
                let count = checked.with_untracked(HashSet::len);
                view! {
                    <Dialog title=format!("Move {count} contact(s)") on_close=Callback::new(move |_| modal.set(None))>
                        <label>
                            "Destination address book"
                            <Select options=opts.clone() value=bulk_target_book/>
                        </label>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button type="button" class="primary-button" disabled=move || busy.get() || bulk_target_book.with(String::is_empty) on:click=move |_| {
                                let target = bulk_target_book.get_untracked();
                                let ids = checked.get_untracked();
                                let writable_ids: HashSet<String> = books.with_untracked(|bs| {
                                    bs.iter().filter(|b| b.role != "read").map(|b| b.id.clone()).collect()
                                });
                                let targets: Vec<Item> = visible_rows.get_untracked().into_iter().filter(|r| ids.contains(&r.id) && r.collection_id != target && writable_ids.contains(&r.collection_id)).collect();
                                let token = csrf();
                                run(Box::pin(async move {
                                    for item in targets {
                                        send::<Item>(&format!("{BASE}/{target}/items"), "POST", Some(json!({ "data": item.data })), &token, None).await.map_err(|e| e.message)?;
                                        send::<Value>(&format!("{BASE}/{}/items/{}", item.collection_id, item.href), "DELETE", None, &token, Some(etag(&item.id, item.revision))).await.map_err(|e| e.message)?;
                                    }
                                    checked.set(HashSet::new());
                                    selected.set(None);
                                    show_toast("Contacts moved.".into(), None);
                                    Ok(())
                                }));
                            }>"Move"</button>
                        </div>
                    </Dialog>
                }
            })}

            {move || (modal.get() == Some("bulkDelete")).then(|| {
                let count = checked.with_untracked(HashSet::len);
                view! {
                    <Dialog title=format!("Delete {count} contact(s)?") on_close=Callback::new(move |_| modal.set(None))>
                        <p>{format!("Permanently remove {count} selected contact(s)?")}</p>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button type="button" class="danger-button" disabled=move || busy.get() on:click=move |_| {
                                let ids = checked.get_untracked();
                                let writable_ids: HashSet<String> = books.with_untracked(|bs| {
                                    bs.iter().filter(|b| b.role != "read").map(|b| b.id.clone()).collect()
                                });
                                let targets: Vec<Item> = visible_rows.get_untracked().into_iter().filter(|r| ids.contains(&r.id) && writable_ids.contains(&r.collection_id)).collect();
                                let token = csrf();
                                run(Box::pin(async move {
                                    for item in targets {
                                        send::<Value>(&format!("{BASE}/{}/items/{}", item.collection_id, item.href), "DELETE", None, &token, Some(etag(&item.id, item.revision))).await.map_err(|e| e.message)?;
                                    }
                                    checked.set(HashSet::new());
                                    selected.set(None);
                                    show_toast("Selected contacts deleted.".into(), None);
                                    Ok(())
                                }));
                            }>"Delete permanently"</button>
                        </div>
                    </Dialog>
                }
            })}

            {move || matches!(modal.get(), Some("delete" | "deleteBook")).then(|| {
                let contact = modal.get_untracked() == Some("delete");
                view! {
                    <Dialog title=if contact { "Delete contact?" } else { "Delete address book?" } on_close=Callback::new(move |_| modal.set(None))>
                        <p>{move || if contact {
                            format!("Permanently remove {}?", detail.with(|d| d.as_ref().map(|d| s(&d.data, "displayName")).unwrap_or_default()))
                        } else {
                            match delete_count.get() {
                                None => "Loading address book details\u{2026}".into(),
                                Some(n) => format!("Remove {} and all its {n} contacts? Export it first if you need a copy.", action_book.with(|b| b.as_ref().map(|b| b.name.clone()).unwrap_or_default())),
                            }
                        }}</p>
                        {move || (!error.get().is_empty()).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| modal.set(None)>"Cancel"</button>
                            <button type="button" class="danger-button" disabled=move || busy.get() || (!contact && delete_count.with(Option::is_none)) on:click=move |_| {
                                let (d, b) = (detail.get_untracked(), action_book.get_untracked());
                                run(Box::pin(async move {
                                    let (url, tag) = match (contact, &d, &b) {
                                        (true, Some(d), _) => (format!("{BASE}/{}/items/{}", d.collection_id, d.href), etag(&d.id, d.revision)),
                                        (false, _, Some(b)) => (format!("{BASE}/{}", b.id), etag(&b.id, b.revision)),
                                        _ => return Ok(()),
                                    };
                                    send::<Value>(&url, "DELETE", None, &csrf(), Some(tag)).await.map_err(|e| e.message)?;
                                    selected.set(None);
                                    if !contact {
                                        select_scope(ContactScope::All);
                                    }
                                    show_toast("Deleted.".into(), None);
                                    Ok(())
                                }));
                            }>"Delete permanently"</button>
                        </div>
                    </Dialog>
                }
            })}
        </PimShell>
    }
}
