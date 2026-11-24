//! Contacts sidebar: + New contact, scrollable smart views (All, Favorites, Ungrouped,
//! Merge & fix), Address books with live counts and actions, manageable Groups, and an
//! Upcoming birthdays footer card.

use std::collections::BTreeMap;

use leptos::prelude::*;

use super::model::{ContactScope, DuplicatePair, UpcomingBirthday};
use crate::icons::Icon;
use crate::pim::{Collection, CollectionDot};
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub fn ContactsSidebar(
    books: RwSignal<Vec<Collection>>,
    groups: RwSignal<Vec<String>>,
    scope: RwSignal<ContactScope>,
    active_group: RwSignal<String>,
    total_count: Memo<usize>,
    fav_count: Memo<usize>,
    ungrouped_count: Memo<usize>,
    book_counts: Memo<BTreeMap<String, usize>>,
    group_counts: Memo<BTreeMap<String, usize>>,
    duplicates: Memo<Vec<DuplicatePair>>,
    birthdays: Memo<Vec<UpcomingBirthday>>,
    show_birthdays: Signal<bool>,
    on_new_contact: Callback<()>,
    on_new_book: Callback<()>,
    on_new_group: Callback<()>,
    on_select_scope: Callback<ContactScope>,
    on_toggle_group: Callback<String>,
    on_book_action: Callback<(Collection, &'static str)>,
    on_group_action: Callback<(String, &'static str)>,
    on_open_birthday: Callback<(String, String)>,
) -> impl IntoView {
    let any_writable = Memo::new(move |_| books.with(|b| b.iter().any(|x| x.role != "read")));

    view! {
        <button
            type="button"
            class="new-button ui-new"
            on:click=move |_| on_new_contact.run(())
        >
            <Icon name="plus"/>"New contact"
        </button>

        <div class="contacts-sidebar-scroll">
            <nav class="contacts-nav-views" aria-label="Views">
                <button
                    type="button"
                    class=move || if scope.get() == ContactScope::All && active_group.with(String::is_empty) { "nav-item active" } else { "nav-item" }
                    aria-label="All contacts"
                    on:click=move |_| on_select_scope.run(ContactScope::All)
                >
                    <Icon name="contacts"/>
                    <span class="contacts-nav-label">"All contacts"</span>
                    <span class="contacts-nav-count" aria-hidden="true">{move || total_count.get()}</span>
                </button>
                <button
                    type="button"
                    class=move || if scope.get() == ContactScope::Favorites { "nav-item active" } else { "nav-item" }
                    aria-label="Favorites"
                    on:click=move |_| on_select_scope.run(ContactScope::Favorites)
                >
                    <Icon name="star"/>
                    <span class="contacts-nav-label">"Favorites"</span>
                    <span class="contacts-nav-count" aria-hidden="true">{move || fav_count.get()}</span>
                </button>
                <button
                    type="button"
                    class=move || if scope.get() == ContactScope::Ungrouped { "nav-item active" } else { "nav-item" }
                    aria-label="Ungrouped"
                    on:click=move |_| on_select_scope.run(ContactScope::Ungrouped)
                >
                    <Icon name="user"/>
                    <span class="contacts-nav-label">"Ungrouped"</span>
                    <span class="contacts-nav-count" aria-hidden="true">{move || ungrouped_count.get()}</span>
                </button>
                <button
                    type="button"
                    class=move || if scope.get() == ContactScope::Merge { "nav-item active" } else { "nav-item" }
                    aria-label="Merge & fix"
                    on:click=move |_| on_select_scope.run(ContactScope::Merge)
                >
                    <Icon name="merge"/>
                    <span class="contacts-nav-label">"Merge & fix"</span>
                    {move || {
                        let n = duplicates.with(Vec::len);
                        (n > 0).then(|| view! { <span class="contacts-nav-badge" aria-hidden="true">{n}</span> })
                    }}
                </button>
            </nav>

            <div class="pim-sidebar-heading">
                <strong>"Address books"</strong>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="New address book"
                    title="New address book"
                    on:click=move |_| on_new_book.run(())
                >
                    <Icon name="plus"/>
                </button>
            </div>
            <div class="contacts-books-list">
                {move || {
                    let counts = book_counts.get();
                    books.get().into_iter().map(|b| {
                        let id = b.id.clone();
                        let active = scope.get() == ContactScope::Book(id.clone());
                        let count = counts.get(&id).copied().unwrap_or(0);
                        let name = b.name.clone();
                        let aria_name = b.name.clone();
                        let (b2, b3, b4) = (b.clone(), b.clone(), b.clone());
                        view! {
                            <div class="contacts-book-row">
                                <button
                                    type="button"
                                    class=if active { "nav-item active" } else { "nav-item" }
                                    aria-label=aria_name
                                    aria-pressed=active.to_string()
                                    on:click=move |_| on_select_scope.run(ContactScope::Book(id.clone()))
                                >
                                    <CollectionDot color=b.color.clone()/>
                                    <span class="contacts-nav-label">{b.name.clone()}</span>
                                    {(b.role != "owner").then(|| view! { <small class="contacts-shared-tag" aria-hidden="true">"Shared"</small> })}
                                    <span class="contacts-nav-count" aria-hidden="true">{count}</span>
                                </button>
                                {(b.role == "owner").then(|| view! {
                                    <ActionMenu
                                        label=format!("Actions for address book {name}")
                                        class="contacts-book-actions"
                                        actions=move |_| vec![
                                            MenuAction::new("Edit address book", "pencil").run({
                                                let b = b2.clone();
                                                move || on_book_action.run((b.clone(), "renameBook"))
                                            }),
                                            MenuAction::new("Share address book", "share").run({
                                                let b = b3.clone();
                                                move || on_book_action.run((b.clone(), "share"))
                                            }),
                                            MenuAction::new("Delete address book", "trash").danger().run({
                                                let b = b4.clone();
                                                move || on_book_action.run((b.clone(), "deleteBook"))
                                            }),
                                        ]
                                    >
                                        <Icon name="more"/>
                                    </ActionMenu>
                                })}
                            </div>
                        }
                    }).collect_view()
                }}
            </div>

            <div class="pim-sidebar-heading">
                <strong>"Groups"</strong>
                {move || any_writable.get().then(|| view! {
                    <button
                        type="button"
                        class="icon-button"
                        aria-label="New group"
                        title="New group"
                        on:click=move |_| on_new_group.run(())
                    >
                        <Icon name="plus"/>
                    </button>
                })}
            </div>
            <nav class="contacts-groups" aria-label="Groups">
                {move || {
                    let mut list = groups.get();
                    let g = active_group.get();
                    if !g.is_empty() && !list.contains(&g) {
                        list.push(g);
                    }
                    let counts = group_counts.get();
                    let writable = any_writable.get();
                    list.into_iter().map(|name| {
                        let active = active_group.get() == name;
                        let count = counts.get(&name).copied().unwrap_or(0);
                        let chosen = name.clone();
                        let aria_name = name.clone();
                        let n_rename = name.clone();
                        let n_delete = name.clone();
                        view! {
                            <div class="contacts-book-row contacts-group-row">
                                <button
                                    type="button"
                                    class=if active { "nav-item active" } else { "nav-item" }
                                    aria-label=aria_name
                                    aria-pressed=active.to_string()
                                    on:click=move |_| on_toggle_group.run(chosen.clone())
                                >
                                    <Icon name="tag"/>
                                    <span class="contacts-nav-label">{name.clone()}</span>
                                    <span class="contacts-nav-count" aria-hidden="true">{count}</span>
                                </button>
                                {writable.then(|| view! {
                                    <ActionMenu
                                        label=format!("Actions for group {name}")
                                        class="contacts-book-actions"
                                        actions=move |_| vec![
                                            MenuAction::new("Rename group", "pencil").run({
                                                let g = n_rename.clone();
                                                move || on_group_action.run((g.clone(), "renameGroup"))
                                            }),
                                            MenuAction::new("Remove group from contacts", "trash").danger().run({
                                                let g = n_delete.clone();
                                                move || on_group_action.run((g.clone(), "deleteGroup"))
                                            }),
                                        ]
                                    >
                                        <Icon name="more"/>
                                    </ActionMenu>
                                })}
                            </div>
                        }
                    }).collect_view()
                }}
            </nav>
        </div>

        {move || {
            if !show_birthdays.get() {
                return None;
            }
            let upcoming = birthdays.get();
            (!upcoming.is_empty()).then(|| view! {
                <div class="contacts-sidebar-footer">
                    <div class="contacts-bday-card">
                        <div class="contacts-bday-head">
                            <Icon name="cake"/>
                            <span>"Upcoming birthdays"</span>
                        </div>
                        {upcoming.into_iter().map(|b| {
                            let (cid, href) = (b.collection_id.clone(), b.href.clone());
                            view! {
                                <button
                                    type="button"
                                    class="contacts-bday-row"
                                    on:click=move |_| on_open_birthday.run((cid.clone(), href.clone()))
                                >
                                    <span class="contacts-bday-name">{b.name}</span>
                                    <span class="contacts-bday-when">{b.label}</span>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>
            })
        }}
    }
}
