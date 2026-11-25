//! Mail message list: header (Select-all menu, count pill, Split/List & Comfortable/Compact
//! controls, Sort, Refresh, Shortcuts), quick filter + account scope chips, tonal bulk selection
//! bar, Trash/Junk empty banner, and conversation-grouped rows with hover triage actions.

use std::collections::{HashMap, HashSet};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::model::{
    Account, BASE, Detail, MailDensity, MailFilter, MailLayout, MailSort, MailState, Pane, View,
    avatar_tone, build_rows, day_label, encode, initials, time_label, tone,
};
use crate::api;
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

/// Shared action menu for both the bulk selection bar and the reader More menu.
pub(crate) fn message_actions(
    state: MailState,
    ids: Vec<String>,
    roles: Vec<String>,
    account: Option<Account>,
    single: Option<Detail>,
    on_view_source: Option<Callback<()>>,
) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    let has = |role: &str| {
        account
            .as_ref()
            .is_some_and(|a| a.mailboxes.iter().any(|m| m.role == role))
    };
    if let Some(d) = &single {
        let id = d.summary.id.clone();
        actions.push(
            MenuAction::new("Reply", "reply")
                .shortcut("r")
                .section("Reply")
                .run(move || state.reply("reply")),
        );
        actions.push(
            MenuAction::new("Reply all", "replyAll")
                .shortcut("a")
                .run(move || state.reply("all")),
        );
        actions.push(
            MenuAction::new("Forward", "forward")
                .shortcut("f")
                .run(move || state.reply("forward")),
        );
        let seen = d.summary.seen;
        let ids2 = vec![id.clone()];
        actions.push(
            MenuAction::new(
                if seen {
                    "Mark as unread"
                } else {
                    "Mark as read"
                },
                "mailUnread",
            )
            .shortcut("u")
            .section("Organize")
            .run(move || state.set_flags(ids2.clone(), Some(!seen), None, true)),
        );
    }
    if let Some(a) = account.clone() {
        let mut targets: Vec<MenuAction> = Vec::new();
        for m in a
            .mailboxes
            .iter()
            .filter(|m| m.role != "folder" && !roles.iter().all(|r| *r == m.id))
        {
            let (ids, target, name) = (ids.clone(), m.id.clone(), m.name.clone());
            targets.push(MenuAction::plain(m.name.clone()).run(move || {
                state.move_to(
                    ids.clone(),
                    None,
                    Some(target.clone()),
                    format!("moved to {name}"),
                )
            }));
        }
        if !targets.is_empty() {
            actions.push(MenuAction::new("Move to", "move").items(targets));
        }
    }
    if has("junk") {
        let ids = ids.clone();
        actions.push(
            MenuAction::new("Report spam", "spam").run(move || {
                state.move_to(ids.clone(), Some("junk"), None, "moved to Junk".into())
            }),
        );
    }
    if let Some(d) = single {
        let id = d.summary.id.clone();
        let file = d.summary.file_id.clone();
        actions.push(
            MenuAction::new("Show in Files", "folder")
                .section("File")
                .run(move || {
                    let (id, file) = (id.clone(), file.clone());
                    spawn_local(async move {
                        match api::get::<Value>(&format!(
                            "{BASE}/messages/{}/location",
                            encode(&id)
                        ))
                        .await
                        {
                            Ok(v) => crate::navigation::navigate(&format!(
                                "/?folder={}&file={}",
                                encode(v["parentId"].as_str().unwrap_or_default()),
                                encode(v["fileId"].as_str().unwrap_or(&file))
                            )),
                            Err(e) => state.fail(e.message),
                        }
                    });
                }),
        );
        if let Some(cb) = on_view_source {
            actions.push(MenuAction::new("View raw source", "code").run(move || {
                cb.try_run(());
            }));
        }
        actions.push(
            MenuAction::new("Download .eml", "download")
                .href(format!("{BASE}/messages/{}/raw", encode(&d.summary.id)))
                .download(),
        );
    }
    actions
}

#[component]
pub(crate) fn MailList(state: MailState, title: Memo<String>) -> impl IntoView {
    let list_ref = NodeRef::<html::Div>::new();

    let rows = Memo::new(move |_| {
        let items = state.items.get();
        let view = state.view.get();
        let group = state.group_threads.get();
        let filter = state.filter.get();
        let af = state.account_filter.get();
        let sort = state.sort.get();
        let accounts = state
            .overview
            .with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
        build_rows(&items, &view, group, filter, af.as_deref(), sort, &accounts)
    });

    let current_mailbox_role = Memo::new(move |_| match state.view.get() {
        View::Mailbox(id) => state.mailbox(&id).map(|(_, m)| m.role).unwrap_or_default(),
        _ => String::new(),
    });

    let is_selecting = Memo::new(move |_| !state.checked.with(HashSet::is_empty));

    view! {
        <main class="pim-main mail-main" tabindex="-1" aria-label=move || title.get()>
            {move || if !is_selecting.get() {
                view! {
                    <div class="mail-list-heading">
                        <button
                            type="button"
                            class="icon-button mail-menu-button"
                            aria-label="Mail navigation"
                            on:click=move |_| state.navigation.set(true)
                        >
                            <Icon name="menu"/>
                        </button>
                        <ActionMenu
                            label="Select messages"
                            class="mail-select-all"
                            actions=move |_| {
                                let cur = rows.get_untracked();
                                let all_ids: HashSet<String> = cur.iter().flat_map(|r| r.ids.clone()).collect();
                                let unread_ids: HashSet<String> = cur.iter().filter(|r| r.unread).flat_map(|r| r.ids.clone()).collect();
                                let read_ids: HashSet<String> = cur.iter().filter(|r| !r.unread).flat_map(|r| r.ids.clone()).collect();
                                let flagged_ids: HashSet<String> = cur.iter().filter(|r| r.flagged).flat_map(|r| r.ids.clone()).collect();
                                vec![
                                    MenuAction::plain("All").run(move || state.checked.set(all_ids.clone())),
                                    MenuAction::plain("None").run(move || state.checked.set(HashSet::new())),
                                    MenuAction::plain("Unread").run(move || state.checked.set(unread_ids.clone())),
                                    MenuAction::plain("Read").run(move || state.checked.set(read_ids.clone())),
                                    MenuAction::plain("Flagged").run(move || state.checked.set(flagged_ids.clone())),
                                ]
                            }
                        >
                            <span class="mail-select-box" aria-hidden="true"></span>
                            <Icon name="chevron"/>
                        </ActionMenu>
                        <h1>{move || title.get()}</h1>
                        {move || {
                            let n = rows.with(Vec::len);
                            (n > 0).then(|| view! { <span class="mail-count-pill" aria-label="Conversation count">{n}</span> })
                        }}
                        <div class="mail-tools">
                            <div class="mail-layout-switch" role="radiogroup" aria-label="Reading layout">
                                <button
                                    type="button"
                                    role="radio"
                                    aria-label="Split view"
                                    title="Split view"
                                    aria-checked=move || (state.layout.get() == MailLayout::Split).to_string()
                                    class=move || if state.layout.get() == MailLayout::Split { "is-active" } else { "" }
                                    on:click=move |_| {
                                        state.layout.set(MailLayout::Split);
                                        state.save_pref("mailLayout", json!("split"));
                                    }
                                >
                                    <Icon name="splitPane"/>
                                </button>
                                <button
                                    type="button"
                                    role="radio"
                                    aria-label="List view"
                                    title="List view"
                                    aria-checked=move || (state.layout.get() == MailLayout::List).to_string()
                                    class=move || if state.layout.get() == MailLayout::List { "is-active" } else { "" }
                                    on:click=move |_| {
                                        state.layout.set(MailLayout::List);
                                        state.save_pref("mailLayout", json!("list"));
                                    }
                                >
                                    <Icon name="list"/>
                                </button>
                            </div>
                            <ActionMenu
                                label="Sort and density"
                                class="mail-sort-menu"
                                actions=move |_| {
                                    let cur_sort = state.sort.get_untracked();
                                    let cur_density = state.density.get_untracked();
                                    vec![
                                        MenuAction::plain("Newest first").section("Sort").current(cur_sort == MailSort::Newest).run(move || state.sort.set(MailSort::Newest)),
                                        MenuAction::plain("Oldest first").current(cur_sort == MailSort::Oldest).run(move || state.sort.set(MailSort::Oldest)),
                                        MenuAction::plain("Sender (A\u{2013}Z)").current(cur_sort == MailSort::Sender).run(move || state.sort.set(MailSort::Sender)),
                                        MenuAction::plain("Largest first").current(cur_sort == MailSort::Size).run(move || state.sort.set(MailSort::Size)),
                                        MenuAction::plain("Comfortable rows").section("Density").current(cur_density == MailDensity::Comfortable).run(move || {
                                            state.density.set(MailDensity::Comfortable);
                                            state.save_pref("mailDensity", json!("comfortable"));
                                        }),
                                        MenuAction::plain("Compact 1-line rows").current(cur_density == MailDensity::Compact).run(move || {
                                            state.density.set(MailDensity::Compact);
                                            state.save_pref("mailDensity", json!("compact"));
                                        }),
                                    ]
                                }
                            >
                                <Icon name="sort"/>
                                <span class="mail-sort-label">{move || state.sort.get().label()}</span>
                            </ActionMenu>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Check for mail"
                                title="Check for mail"
                                disabled=move || state.syncing.get()
                                on:click=move |_| state.sync(None)
                            >
                                <Icon name="refresh"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button"
                                aria-label="Mark all shown as read"
                                title="Mark all shown as read"
                                disabled=move || state.items.with(|i| !i.iter().any(|i| !i.seen))
                                on:click=move |_| {
                                    let ids: Vec<String> = state.items.with_untracked(|i| i.iter().filter(|i| !i.seen).map(|i| i.id.clone()).collect());
                                    state.set_flags(ids, Some(true), None, true);
                                }
                            >
                                <Icon name="checks"/>
                            </button>
                            <button
                                type="button"
                                class="icon-button mail-kbd-btn"
                                aria-label="Keyboard shortcuts"
                                title="Keyboard shortcuts (?)"
                                on:click=move |_| state.shortcuts_open.set(true)
                            >
                                <Icon name="keyboard"/>
                            </button>
                        </div>
                    </div>
                }.into_any()
            } else {
                let count = Memo::new(move |_| state.checked.with(HashSet::len));
                let ids = move || state.checked.get_untracked().into_iter().collect::<Vec<_>>();
                view! {
                    <div class="mail-list-heading mail-selection" role="toolbar" aria-label="Selected messages">
                        <button type="button" class="icon-button" aria-label="Clear selection" on:click=move |_| state.checked.set(HashSet::new())><Icon name="close"/></button>
                        <b>{move || format!("{} selected", count.get())}</b>
                        <button type="button" class="icon-button" aria-label="Archive" title="Archive (e)" on:click=move |_| state.move_to(ids(), Some("archive"), None, "archived".into())><Icon name="archive"/></button>
                        <button type="button" class="icon-button" aria-label="Delete" title="Delete (#)" on:click=move |_| state.move_to(ids(), Some("trash"), None, "moved to Trash".into())><Icon name="trash"/></button>
                        <button type="button" class="icon-button" aria-label="Mark as read" title="Mark as read" on:click=move |_| state.set_flags(ids(), Some(true), None, true)><Icon name="checks"/></button>
                        <button type="button" class="icon-button" aria-label="Mark as unread" title="Mark as unread" on:click=move |_| state.set_flags(ids(), Some(false), None, true)><Icon name="mailUnread"/></button>
                        <button type="button" class="icon-button" aria-label="Flag" title="Flag" on:click=move |_| state.set_flags(ids(), None, Some(true), true)><Icon name="flag"/></button>
                        <ActionMenu label="More actions for the selection" actions=move |_| {
                            let ids = ids();
                            let first = state.items.with_untracked(|i| i.iter().find(|i| ids.contains(&i.id)).map(|i| i.account_id.clone()));
                            let same = state.items.with_untracked(|i| i.iter().filter(|i| ids.contains(&i.id)).all(|i| Some(&i.account_id) == first.as_ref()));
                            let account = first.filter(|_| same).and_then(|id| state.account(&id));
                            let roles = state.items.with_untracked(|i| i.iter().filter(|i| ids.contains(&i.id)).map(|i| i.mailbox_id.clone()).collect());
                            message_actions(state, ids, roles, account, None, None)
                        }><Icon name="more"/></ActionMenu>
                    </div>
                }.into_any()
            }}

            // Quick Filter + Account Scope sub-bar (shown once accounts exist and not bulk-selecting)
            {move || {
                let has_accounts = state.overview.with(|o| o.as_ref().is_some_and(|o| !o.accounts.is_empty()));
                (has_accounts && !is_selecting.get()).then(|| {
                    let unified = matches!(state.view.get(), View::Unified(_));
                    let accounts = state.overview.with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
                    let cur_filter = state.filter.get();
                    let cur_acct = state.account_filter.get();
                    view! {
                        <div class="mail-filter-bar" role="toolbar" aria-label="Quick filters">
                            {[(MailFilter::All, "All"), (MailFilter::Unread, "Unread"), (MailFilter::Flagged, "Flagged"), (MailFilter::Attachments, "Has files")]
                                .into_iter()
                                .map(|(f, label)| {
                                    let active = cur_filter == f;
                                    view! {
                                        <button
                                            type="button"
                                            class=if active { "mail-filter-chip is-active" } else { "mail-filter-chip" }
                                            aria-pressed=active.to_string()
                                            on:click=move |_| state.filter.set(f)
                                        >
                                            {(f == MailFilter::Attachments).then(|| view! { <Icon name="paperclip"/> })}
                                            <span>{label}</span>
                                        </button>
                                    }
                                })
                                .collect_view()}
                            {(unified && accounts.len() > 1).then(|| view! {
                                <span class="mail-filter-sep" aria-hidden="true"></span>
                                {accounts.into_iter().map(|a| {
                                    let id = a.id.clone();
                                    let active = cur_acct.as_deref() == Some(id.as_str());
                                    view! {
                                        <button
                                            type="button"
                                            class=if active { "mail-filter-chip is-active" } else { "mail-filter-chip" }
                                            aria-pressed=active.to_string()
                                            on:click=move |_| {
                                                state.account_filter.update(|cur| {
                                                    if cur.as_deref() == Some(id.as_str()) {
                                                        *cur = None;
                                                    } else {
                                                        *cur = Some(id.clone());
                                                    }
                                                });
                                            }
                                        >
                                            <span class=format!("mail-acct is-dot {}", tone(a.color)) aria-hidden="true"></span>
                                            <span>{a.name}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            })}
                        </div>
                    }
                })
            }}

            // Empty Trash / Empty Junk banner
            {move || {
                let role = current_mailbox_role.get();
                let count = state.items.with(Vec::len);
                ((role == "trash" || role == "junk") && count > 0).then(|| {
                    let is_trash = role == "trash";
                    view! {
                        <div class="mail-trash-banner" role="region" aria-label=if is_trash { "Trash actions" } else { "Junk actions" }>
                            <Icon name=if is_trash { "trash" } else { "spam" }/>
                            <span>{if is_trash { "Messages in Trash can be removed permanently." } else { "Messages marked as spam sit here." }}</span>
                            <button
                                type="button"
                                class="secondary-button"
                                on:click=move |_| {
                                    let ids: Vec<String> = state.items.with_untracked(|items| items.iter().map(|i| i.id.clone()).collect());
                                    state.move_to(ids, Some("trash"), None, "moved to Trash".into());
                                }
                            >
                                {if is_trash { "Empty Trash" } else { "Move all to Trash" }}
                            </button>
                        </div>
                    }
                })
            }}

            {move || (!state.load_error.with(String::is_empty)).then(|| view! {
                <div class="mail-banner is-error" role="alert">
                    <Icon name="alert"/>
                    <span><b>"Mail could not be loaded"</b>{state.load_error.get()}</span>
                    <button class="secondary-button" on:click=move |_| state.reload()>"Retry"</button>
                </div>
            })}

            {move || state.overview.with(|o| o.as_ref().map(|o| o.accounts.iter().filter(|a| a.status == "signIn" || a.status == "error" || !a.provider_available).cloned().collect::<Vec<_>>()).unwrap_or_default()).into_iter().map(|a| {
                let id = a.id.clone();
                let sign_in = a.status == "signIn";
                let heading = if !a.provider_available {
                    format!("{}: the administrator turned off its provider", a.name)
                } else if sign_in {
                    format!("{} needs you to sign in again", a.name)
                } else {
                    format!("{} is not syncing", a.name)
                };
                view! {
                    <div class=if sign_in { "mail-banner is-error" } else { "mail-banner" } role="status">
                        <Icon name="alert"/>
                        <span><b>{heading}</b>{a.last_error.clone()}" Mirrored messages stay readable."</span>
                        {if sign_in {
                            view! { <button class="secondary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(Some(id.clone()))))>"Sign in again"</button> }.into_any()
                        } else {
                            view! { <button class="secondary-button" on:click=move |_| state.sync(Some(id.clone()))>"Retry now"</button> }.into_any()
                        }}
                    </div>
                }
            }).collect_view()}

            <div
                class=move || format!(
                    "mail-list{}{}",
                    if state.density.get() == MailDensity::Compact { " is-compact" } else { "" },
                    if is_selecting.get() { " is-selecting" } else { "" }
                )
                node_ref=list_ref
                role="list"
                aria-label=move || format!("{} messages", title.get())
            >
                {move || (state.list_state.get() == "loading" && state.items.with(Vec::is_empty)).then(|| view! {
                    <p class="mail-empty" role="status">"Loading messages\u{2026}"</p>
                })}
                {move || (state.list_state.get() == "error").then(|| view! {
                    <div class="mail-empty" role="alert">
                        <h2>"Messages could not be loaded"</h2>
                        <p>{state.list_error.get()}</p>
                        <button class="secondary-button" on:click=move |_| state.load_items()>"Retry"</button>
                    </div>
                })}
                {move || (state.list_state.get() == "ready" && rows.with(Vec::is_empty)).then(|| {
                    let none = state.overview.with(|o| o.as_ref().is_some_and(|o| o.accounts.is_empty()));
                    let administrator = state.overview.with(|o| o.as_ref().is_some_and(|o| o.administrator));
                    let providers = state.overview.with(|o| o.as_ref().map(|o| o.providers.len()).unwrap_or(0));
                    let filtered = state.filter.get() != MailFilter::All || state.account_filter.with(Option::is_some);
                    if none {
                        view! {
                            <div class="mail-empty">
                                <Icon name="mail"/>
                                <h2>"Add your first mail account"</h2>
                                {if providers == 0 && administrator {
                                    view! {
                                        <p>"Set up a mail provider first: its servers and, for Google or Microsoft, its OAuth client."</p>
                                        <button class="primary-button" on:click=move |_| state.pane.set(Some(Pane::Settings("Providers")))>"Set up a provider"</button>
                                    }.into_any()
                                } else if providers == 0 {
                                    view! { <p>"Your administrator has not set up a mail provider yet."</p> }.into_any()
                                } else {
                                    view! {
                                        <p>"Its messages are mirrored as .eml files in Files, so they stay readable even when the server is not."</p>
                                        <button class="primary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(None)))><Icon name="plus"/>"Add account"</button>
                                    }.into_any()
                                }}
                            </div>
                        }.into_any()
                    } else if filtered {
                        view! {
                            <div class="mail-empty">
                                <Icon name="inbox"/>
                                <h2>"No matching conversations"</h2>
                                <p>"Try clearing the active filter chips to see all messages in this view."</p>
                                <button type="button" class="secondary-button" on:click=move |_| { state.filter.set(MailFilter::All); state.account_filter.set(None); }>"Show all"</button>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="mail-empty">
                                <Icon name="inbox"/>
                                <h2>"No messages here"</h2>
                                <p>"Messages you receive or move here will appear in this list."</p>
                            </div>
                        }.into_any()
                    }
                })}

                {move || {
                    let list = rows.get();
                    let unified = matches!(state.view.get(), View::Unified(_));
                    let show_box_badge = matches!(state.view.get(), View::Unified("all" | "flagged" | "unread"));
                    let colors: HashMap<String, u32> = state.overview.with(|o| {
                        o.as_ref()
                            .map(|o| o.accounts.iter().map(|a| (a.id.clone(), a.color)).collect())
                            .unwrap_or_default()
                    });
                    let mut last_day = String::new();
                    let by_date = matches!(state.sort.get(), MailSort::Newest | MailSort::Oldest);

                    list.into_iter().map(|row| {
                        let item = row.head;
                        let heading = by_date.then(|| {
                            let day = day_label(item.date);
                            (day != last_day).then(|| {
                                last_day = day.clone();
                                view! { <div class="mail-day" role="presentation">{day}</div> }
                            })
                        }).flatten();

                        let id = item.id.clone();
                        let row_ids_sel = row.ids.clone();
                        let row_ids_chk = row.ids.clone();
                        let row_ids_arch = row.ids.clone();
                        let row_ids_del = row.ids.clone();
                        let row_ids_read = row.ids.clone();
                        let row_ids_flag = row.ids.clone();
                        let id_open = id.clone();
                        let id_check = id.clone();

                        let is_sel = Memo::new(move |_| {
                            state.selected.get().is_some_and(|s| row_ids_sel.contains(&s))
                        });
                        let is_checked = Memo::new(move |_| {
                            state.checked.with(|c| row_ids_chk.iter().any(|x| c.contains(x)))
                        });
                        let account_dot = unified.then(|| {
                            colors.get(&item.account_id).map(|c| {
                                view! { <span class=format!("mail-acct is-dot {}", tone(*c)) aria-hidden="true"></span> }
                            })
                        });
                        let box_label = show_box_badge.then(|| match item.mailbox_role.as_str() {
                            "inbox" => Some("Inbox"),
                            "sent" => Some("Sent"),
                            "archive" => Some("Archive"),
                            "drafts" => Some("Draft"),
                            "trash" => Some("Trash"),
                            "junk" => Some("Junk"),
                            _ => None,
                        }).flatten();
                        let unread = row.unread;
                        let flagged = row.flagged;
                        let check_label = format!("Select message from {}", row.participants);
                        let avatar_seed = if item.mailbox_role == "sent" || item.mailbox_role == "drafts" || item.draft {
                            item.recipients.to.first().map(|a| a.address.as_str()).unwrap_or(&item.from_address)
                        } else {
                            &item.from_address
                        };
                        let avatar_name = if item.mailbox_role == "sent" || item.mailbox_role == "drafts" || item.draft {
                            item.recipients.to.first().map(|a| a.label()).unwrap_or_else(|| item.sender())
                        } else {
                            item.sender()
                        };

                        view! {
                            {heading}
                            <div
                                role="listitem"
                                data-message=id.clone()
                                class=move || format!(
                                    "mail-message{}{}{}",
                                    if unread { "" } else { " is-read" },
                                    if is_sel.get() { " is-selected" } else { "" },
                                    if is_checked.get() { " is-checked" } else { "" }
                                )
                            >
                                <input
                                    type="checkbox"
                                    class="mail-check"
                                    aria-label=check_label
                                    prop:checked=move || is_checked.get()
                                    on:click=move |e| {
                                        e.stop_propagation();
                                        state.toggle_check(id_check.clone(), e.shift_key());
                                    }
                                />
                                <button
                                    type="button"
                                    class="mail-row-open"
                                    aria-current=move || is_sel.get().then_some("true")
                                    on:click=move |_| state.open(id_open.clone())
                                >
                                    <span class="mail-dot" aria-hidden="true"></span>
                                    <span class=avatar_tone(avatar_seed) aria-hidden="true">{initials(&avatar_name)}</span>
                                    <span class="mail-from">
                                        <span class="mail-from-name">{row.participants}</span>
                                        {(row.count > 1).then(|| view! { <span class="mail-thread-count">{row.count}</span> })}
                                        {unread.then(|| view! { <span class="sr-only">", unread"</span> })}
                                    </span>
                                    <span class="mail-time">
                                        {row.attachments.then(|| view! { <span class="mail-clip" title="Has attachments"><Icon name="paperclip"/><span class="sr-only">"Has attachments"</span></span> })}
                                        {flagged.then(|| view! { <span class="mail-flag" title="Flagged"><Icon name="flag"/><span class="sr-only">"Flagged"</span></span> })}
                                        {row.answered.then(|| view! { <span class="mail-answered" title="Answered"><Icon name="reply"/><span class="sr-only">"Answered"</span></span> })}
                                        <time>{time_label(item.date)}</time>
                                    </span>
                                    <span class="mail-subject">
                                        <span class="mail-subject-text">{item.subject_or()}</span>
                                        <span class="mail-compact-sep" aria-hidden="true">" \u{2014} "</span>
                                        <span class="mail-compact-snip">{item.snippet.clone()}</span>
                                    </span>
                                    <span class="mail-snippet">
                                        {account_dot}
                                        {item.draft.then(|| view! { <span class="mail-label is-draft">"Draft"</span> })}
                                        {box_label.map(|l| view! { <span class="mail-label">{l}</span> })}
                                        <span class="mail-snippet-text">{item.snippet.clone()}</span>
                                    </span>
                                </button>
                                <div class="mail-hover-actions" aria-label="Quick actions">
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label="Archive"
                                        title="Archive (e)"
                                        on:click=move |e| {
                                            e.stop_propagation();
                                            state.move_to(row_ids_arch.clone(), Some("archive"), None, "archived".into());
                                        }
                                    >
                                        <Icon name="archive"/>
                                    </button>
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label="Delete"
                                        title="Delete (#)"
                                        on:click=move |e| {
                                            e.stop_propagation();
                                            state.move_to(row_ids_del.clone(), Some("trash"), None, "moved to Trash".into());
                                        }
                                    >
                                        <Icon name="trash"/>
                                    </button>
                                    <button
                                        type="button"
                                        class="icon-button"
                                        aria-label=if unread { "Mark as read" } else { "Mark as unread" }
                                        title=if unread { "Mark as read" } else { "Mark as unread (u)" }
                                        on:click=move |e| {
                                            e.stop_propagation();
                                            state.set_flags(row_ids_read.clone(), Some(unread), None, true);
                                        }
                                    >
                                        <Icon name=if unread { "checks" } else { "mailUnread" }/>
                                    </button>
                                    <button
                                        type="button"
                                        class=if flagged { "icon-button is-flagged" } else { "icon-button" }
                                        aria-label=if flagged { "Remove flag" } else { "Flag" }
                                        title="Flag (s)"
                                        on:click=move |e| {
                                            e.stop_propagation();
                                            state.set_flags(row_ids_flag.clone(), None, Some(!flagged), true);
                                        }
                                    >
                                        <Icon name="flag"/>
                                    </button>
                                </div>
                            </div>
                        }
                    }).collect_view()
                }}

                {move || (state.cursor.with(Option::is_some) && state.list_state.get() == "ready").then(|| view! {
                    <div class="mail-more">
                        <button class="text-button" on:click=move |_| { spawn_local(state.fetch_items(false)); }>"Load more"</button>
                    </div>
                })}
            </div>
        </main>
    }
}
