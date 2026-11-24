//! Contacts middle sheet: header with view switcher & inline sort, filter chips, tonal
//! bulk-selection bar, List / Table / Cards views, and an A–Z quick-jump rail.

use std::collections::HashSet;

use leptos::html;
use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen::JsCast;

use super::model::{
    ContactFilter, SORT_OPTIONS, VIEW_MODES, display_name, photo_url, sort_letter, subtitle,
};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionDot, Item, avatar_hue, initials};
use crate::ui::{ActionMenu, MenuAction};

const LETTERS: [char; 27] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '#',
];

#[component]
pub fn ContactsList(
    title: Signal<String>,
    rows: Memo<Vec<Item>>,
    books: RwSignal<Vec<Collection>>,
    loading: RwSignal<bool>,
    sort: RwSignal<String>,
    name_order: Signal<String>,
    view_mode: RwSignal<String>,
    filter: RwSignal<ContactFilter>,
    active_group: RwSignal<String>,
    selected: RwSignal<Option<(String, String)>>,
    checked: RwSignal<HashSet<String>>,
    has_details: Signal<bool>,
    on_choose: Callback<Item>,
    on_toggle_favorite: Callback<Item>,
    on_toggle_check: Callback<String>,
    on_select_all: Callback<()>,
    on_clear_checked: Callback<()>,
    on_bulk_group: Callback<()>,
    on_bulk_move: Callback<()>,
    on_bulk_export: Callback<()>,
    on_bulk_delete: Callback<()>,
    on_change_sort: Callback<String>,
    on_change_view: Callback<String>,
    on_clear_group: Callback<()>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let list_ref = NodeRef::<html::Div>::new();
    let has_selection = Memo::new(move |_| !checked.with(HashSet::is_empty));
    let any_writable_book = Memo::new(move |_| books.with(|b| b.iter().any(|x| x.role != "read")));
    let multiple_writable =
        Memo::new(move |_| books.with(|b| b.iter().filter(|x| x.role != "read").count() >= 2));

    let jump_to = move |letter: String| {
        if let Some(el) = list_ref.get_untracked()
            && let Ok(Some(target)) = el.query_selector(&format!("[data-letter=\"{letter}\"]"))
            && let Ok(html_el) = target.dyn_into::<web_sys::HtmlElement>()
        {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Start);
            html_el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    };

    let present_letters = Memo::new(move |_| {
        let sort_now = sort.get();
        rows.with(|list| {
            list.iter()
                .map(|r| sort_letter(&r.data, &sort_now))
                .collect::<HashSet<String>>()
        })
    });

    view! {
        <div class="contacts-heading">
            <h1>{move || title.get()}</h1>
            <span class="contacts-count-pill" aria-hidden="true">{move || rows.with(Vec::len)}</span>
            {move || (!active_group.with(String::is_empty)).then(|| {
                let g = active_group.get();
                view! {
                    <span class="pim-filter-chip">
                        <Icon name="tag"/>
                        {g.clone()}
                        <button
                            type="button"
                            class="icon-button"
                            aria-label=format!("Clear group {g}")
                            on:click=move |_| on_clear_group.run(())
                        >
                            <Icon name="close"/>
                        </button>
                    </span>
                }
            })}
            <span class="contacts-heading-spacer"></span>
            <div class="contacts-heading-controls">
                <div class="segmented contacts-view-switch" role="radiogroup" aria-label="Contact view">
                    {VIEW_MODES.into_iter().map(|(id, label, icon)| {
                        view! {
                            <button
                                type="button"
                                role="radio"
                                title=label
                                aria-label=format!("{label} view")
                                aria-checked=move || (view_mode.get() == id).to_string()
                                on:click=move |_| on_change_view.run(id.to_owned())
                            >
                                <Icon name=icon/>
                            </button>
                        }
                    }).collect_view()}
                </div>
                <ActionMenu
                    label="Sort contacts"
                    class="contacts-sort-menu"
                    actions=move |_| {
                        let current = sort.get();
                        SORT_OPTIONS
                            .into_iter()
                            .map(|(key, label)| {
                                MenuAction::plain(label)
                                    .current(current == key)
                                    .run(move || on_change_sort.run(key.to_owned()))
                            })
                            .collect()
                    }
                >
                    <Icon name="sort"/>
                    <span class="contacts-sort-label">
                        {move || {
                            let cur = sort.get();
                            SORT_OPTIONS
                                .iter()
                                .find(|(k, _)| *k == cur)
                                .map(|(_, l)| *l)
                                .unwrap_or("First name")
                        }}
                    </span>
                </ActionMenu>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Refresh"
                    title="Refresh"
                    on:click=move |_| on_refresh.run(())
                >
                    <Icon name="refresh"/>
                </button>
            </div>
        </div>

        {move || if has_selection.get() {
            let count = checked.with(HashSet::len);
            let total = rows.with(Vec::len);
            view! {
                <div class="contacts-selection-bar" role="region" aria-label="Bulk contact actions">
                    <button
                        type="button"
                        class="contacts-sel-btn"
                        on:click=move |_| on_select_all.run(())
                    >
                        {if count >= total && total > 0 { "All selected" } else { "Select all" }}
                    </button>
                    <strong class="contacts-sel-count">{format!("{count} selected")}</strong>
                    {move || any_writable_book.get().then(|| view! {
                        <button type="button" class="contacts-sel-btn" on:click=move |_| on_bulk_group.run(())>
                            <Icon name="tag"/><span>"Group"</span>
                        </button>
                    })}
                    {move || multiple_writable.get().then(|| view! {
                        <button type="button" class="contacts-sel-btn" on:click=move |_| on_bulk_move.run(())>
                            <Icon name="move"/><span>"Move"</span>
                        </button>
                    })}
                    <button type="button" class="contacts-sel-btn" on:click=move |_| on_bulk_export.run(())>
                        <Icon name="download"/><span>"Export"</span>
                    </button>
                    {move || any_writable_book.get().then(|| view! {
                        <button type="button" class="contacts-sel-btn danger" aria-label="Delete selected contacts" on:click=move |_| on_bulk_delete.run(())>
                            <Icon name="trash"/><span>"Delete"</span>
                        </button>
                    })}
                    <button type="button" class="icon-button contacts-sel-close" aria-label="Clear selection" on:click=move |_| on_clear_checked.run(())>
                        <Icon name="close"/>
                    </button>
                </div>
            }.into_any()
        } else {
            view! {
                <div class="contacts-filter-strip" role="group" aria-label="Quick filters">
                    {[
                        (ContactFilter::All, "All"),
                        (ContactFilter::Email, "Has email"),
                        (ContactFilter::Phone, "Has phone"),
                        (ContactFilter::Birthday, "Birthdays"),
                    ].into_iter().map(|(kind, label)| {
                        view! {
                            <button
                                type="button"
                                class=move || if filter.get() == kind { "contacts-filter-chip is-active" } else { "contacts-filter-chip" }
                                aria-pressed=move || (filter.get() == kind).to_string()
                                on:click=move |_| filter.set(kind)
                            >
                                {label}
                            </button>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}

        {move || {
            if loading.get() {
                return view! { <p role="status" class="contacts-status">"Loading contacts\u{2026}"</p> }.into_any();
            }
            let list = rows.get();
            if list.is_empty() {
                return view! {
                    <div class="pim-empty">
                        <h2>{if active_group.with(String::is_empty) && filter.get() == ContactFilter::All { "No contacts" } else { "No matching contacts" }}</h2>
                        <p>{if books.with(Vec::is_empty) { "Create an address book to get started." } else { "Create a contact or import vCards into an address book." }}</p>
                    </div>
                }.into_any();
            }
            let sort_now = sort.get();
            let order_now = name_order.get();
            let mode_now = view_mode.get();
            let all_books = books.get();
            let current = selected.get();
            let checked_set = checked.get();
            let selecting = !checked_set.is_empty();
            let show_table_cols = mode_now == "table" && !has_details.get();

            if mode_now == "cards" {
                return view! {
                    <div class="contacts-list-wrap">
                        <div node_ref=list_ref class=format!("contact-list contacts-cards-scroll {}", if selecting { "is-selecting" } else { "" })>
                            <div class="contacts-cards-grid">
                                {list.into_iter().map(|row| {
                                    let b = all_books.iter().find(|b| b.id == row.collection_id).cloned();
                                    let name = display_name(&row.data, &order_now);
                                    let sub = subtitle(&row.data);
                                    let is_selected = current.as_ref().is_some_and(|(c, h)| *c == row.collection_id && *h == row.href);
                                    let star = row.data["favorite"] == json!(true);
                                    let has_photo = row.data["hasPhoto"] == json!(true);
                                    let p_url = photo_url(&row.collection_id, &row.href, row.revision);
                                    let picked = row.clone();
                                    let fav_row = row.clone();
                                    let writable = b.as_ref().is_some_and(|b| b.role != "read");
                                    let book_color = b.as_ref().map(|b| b.color.clone()).unwrap_or_default();
                                    let book_name = b.as_ref().map(|b| b.name.clone()).unwrap_or_default();
                                    view! {
                                        <div class=if is_selected { "contacts-card is-selected" } else { "contacts-card" }>
                                            <button type="button" class="contacts-card-main" on:click=move |_| on_choose.run(picked.clone())>
                                                <span class=format!("contact-initials contacts-card-avatar {}", avatar_hue(&name))>
                                                    {if has_photo {
                                                        view! { <img src=p_url alt="" class="contact-avatar-img"/> }.into_any()
                                                    } else {
                                                        view! { {initials(&name)} }.into_any()
                                                    }}
                                                </span>
                                                <strong>{name.clone()}</strong>
                                                <small>{sub}</small>
                                                <span class="contacts-card-meta">
                                                    <CollectionDot color=book_color/>
                                                    <span>{book_name}</span>
                                                </span>
                                            </button>
                                            {writable.then(|| view! {
                                                <button
                                                    type="button"
                                                    class=if star { "contacts-card-star is-fav" } else { "contacts-card-star" }
                                                    aria-label=if star { "Remove from favorites" } else { "Add to favorites" }
                                                    aria-pressed=star.to_string()
                                                    on:click=move |_| on_toggle_favorite.run(fav_row.clone())
                                                >
                                                    <Icon name="star"/>
                                                </button>
                                            })}
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    </div>
                }.into_any();
            }

            let mut last = String::new();
            view! {
                {show_table_cols.then(|| view! {
                    <div class="contacts-table-header" aria-hidden="true">
                        <span>"Name"</span>
                        <span>"Email"</span>
                        <span>"Phone"</span>
                        <span class="contacts-table-col-end">"Book"</span>
                    </div>
                })}
                <div class="contacts-list-wrap">
                    <div node_ref=list_ref class=format!("contact-list {} {}", if selecting { "is-selecting" } else { "" }, if show_table_cols { "is-table-mode" } else { "" })>
                        {list.into_iter().map(|row| {
                            let letter = sort_letter(&row.data, &sort_now);
                            let heading = (letter != last).then(|| view! {
                                <h2 class="contact-letter" data-letter=letter.clone()>{letter.clone()}</h2>
                            });
                            last = letter;
                            let b = all_books.iter().find(|b| b.id == row.collection_id).cloned();
                            let name = display_name(&row.data, &order_now);
                            let sub = subtitle(&row.data);
                            let primary_email = row.data["emails"][0]["value"].as_str().unwrap_or("—").to_owned();
                            let primary_phone = row.data["phones"][0]["value"].as_str().unwrap_or("—").to_owned();
                            let is_selected = current.as_ref().is_some_and(|(c, h)| *c == row.collection_id && *h == row.href);
                            let is_checked = checked_set.contains(&row.id);
                            let star = row.data["favorite"] == json!(true);
                            let has_photo = row.data["hasPhoto"] == json!(true);
                            let p_url = photo_url(&row.collection_id, &row.href, row.revision);
                            let writable = b.as_ref().is_some_and(|b| b.role != "read");
                            let picked = row.clone();
                            let fav_row = row.clone();
                            let row_id = row.id.clone();
                            let book_name = b.as_ref().map(|b| b.name.clone()).unwrap_or_default();
                            let book_color = b.as_ref().map(|b| b.color.clone()).unwrap_or_default();
                            view! {
                                <div class="contact-entry">
                                    {heading}
                                    <div class=format!(
                                        "contact-row-wrap {} {}",
                                        if is_selected { "is-selected" } else { "" },
                                        if is_checked { "is-checked" } else { "" }
                                    )>
                                        <button
                                            type="button"
                                            class=if is_selected { "contact-row selected" } else { "contact-row" }
                                            on:click=move |_| on_choose.run(picked.clone())
                                        >
                                            <span class=format!("contact-initials {}", avatar_hue(&name))>
                                                {if has_photo {
                                                    view! { <img src=p_url alt="" class="contact-avatar-img"/> }.into_any()
                                                } else {
                                                    view! { {initials(&name)} }.into_any()
                                                }}
                                            </span>
                                            <span class="contact-row-text">
                                                <strong>{name.clone()}</strong>
                                                <small>{sub}</small>
                                            </span>
                                            {show_table_cols.then(|| view! {
                                                <span class="contact-table-cell">{primary_email}</span>
                                                <span class="contact-table-cell num">{primary_phone}</span>
                                            })}
                                            <span class="contact-book" title=book_name.clone()>
                                                <CollectionDot color=book_color/>
                                                <span class="sr-only">{book_name.clone()}</span>
                                            </span>
                                        </button>
                                        <button
                                            type="button"
                                            role="checkbox"
                                            class=if is_checked { "contact-row-check is-checked" } else { "contact-row-check" }
                                            aria-label=format!("Select {name}")
                                            aria-checked=is_checked.to_string()
                                            on:click=move |_| on_toggle_check.run(row_id.clone())
                                        >
                                            {is_checked.then(|| view! { <Icon name="check"/> })}
                                        </button>
                                        {if writable {
                                            view! {
                                                <button
                                                    type="button"
                                                    class=if star { "contact-row-star is-fav" } else { "contact-row-star" }
                                                    aria-label=if star { "Remove from favorites" } else { "Add to favorites" }
                                                    aria-pressed=star.to_string()
                                                    on:click=move |_| on_toggle_favorite.run(fav_row.clone())
                                                >
                                                    <Icon name="star"/>
                                                </button>
                                            }.into_any()
                                        } else {
                                            star.then(|| view! {
                                                <span class="contact-row-star is-fav is-static" aria-hidden="true"><Icon name="star"/></span>
                                            }).into_any()
                                        }}
                                    </div>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                    <nav class="contacts-az-rail" aria-label="Jump to letter">
                        {move || {
                            let present = present_letters.get();
                            LETTERS.into_iter().map(|ch| {
                                let s_ch = ch.to_string();
                                let has = present.contains(&s_ch);
                                let target = s_ch.clone();
                                view! {
                                    <button
                                        type="button"
                                        class=if has { "contacts-az-btn has-items" } else { "contacts-az-btn" }
                                        disabled=!has
                                        on:click=move |_| jump_to(target.clone())
                                    >
                                        {s_ch}
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </nav>
                </div>
            }.into_any()
        }}
    }
}
