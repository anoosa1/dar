//! Mail sidebar: Compose CTA, unified views with crisp icons & counts, reactive collapsible
//! accounts tree, inline "+ New folder" creation, and sync status footer.

use leptos::prelude::*;

use super::model::{MailState, Mailbox, Pane, UNIFIED, View, encode, initials, relative, tone};
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction};

#[component]
pub(crate) fn MailSidebar(state: MailState, navigation_id: String) -> impl IntoView {
    let creating_for = RwSignal::new(None::<String>);
    let new_folder_name = RwSignal::new(String::new());

    let view_row = move |target: View,
                         label: String,
                         icon: &'static str,
                         count: Signal<u32>,
                         extra_class: &'static str| {
        let t2 = target.clone();
        let active = Memo::new(move |_| state.view.get() == t2);
        let t3 = target.clone();
        view! {
            <button
                type="button"
                class=move || format!("nav-item mail-view {extra_class}{}", if active.get() { " active" } else { "" })
                aria-current=move || active.get().then_some("page")
                on:click=move |_| state.choose(t3.clone())
            >
                <Icon name=icon/>
                <span class="mail-row-title">{label}</span>
                {move || (count.get() > 0).then(|| view! { <span class="mail-count">{count.get()}</span> })}
            </button>
        }
    };

    view! {
        <aside
            id=navigation_id
            class=move || if state.navigation.get() { "sidebar mail-sidebar is-open" } else { "sidebar mail-sidebar" }
            aria-label="Mail navigation"
        >
            <button type="button" class="new-button ui-new" on:click=move |_| state.new_message()>
                <Icon name="pencil"/>"Compose"
            </button>

            <div class="mail-sidebar-scroll">
                <nav aria-label="Mail views">
                    {UNIFIED.into_iter().map(|(id, label, icon)| {
                        let count = Signal::derive(move || {
                            state.overview.with(|o| {
                                o.as_ref().map(|o| match id {
                                    "inbox" => o.counts.inbox,
                                    "unread" => o.counts.unread,
                                    "flagged" => o.counts.flagged,
                                    "drafts" => o.counts.drafts,
                                    _ => 0,
                                }).unwrap_or(0)
                            })
                        });
                        view_row(View::Unified(id), label.to_owned(), icon, count, "")
                    }).collect_view()}
                </nav>

                <div class="sidebar-heading mail-heading">
                    <h2>"Accounts"</h2>
                    <button
                        type="button"
                        class="icon-button"
                        aria-label="Add account"
                        title="Add account"
                        on:click=move |_| state.pane.set(Some(Pane::AddAccount(None)))
                    >
                        <Icon name="plus"/>
                    </button>
                </div>

                <nav aria-label="Accounts and mailboxes" class="mail-accounts">
                    {move || {
                        let accounts = state
                            .overview
                            .with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
                        accounts.into_iter().map(|a| {
                            let account_id = a.id.clone();
                            let open = Memo::new({
                                let id = account_id.clone();
                                move |_| !state.collapsed.with(|c| c.contains(&id))
                            });
                            let protocol = if a.protocol == "jmap" {
                                "JMAP".to_owned()
                            } else if a.auth == "oauth" {
                                a.provider_name.clone()
                            } else {
                                "IMAP".to_owned()
                            };
                            let problem = a.status == "error" || a.status == "signIn" || !a.provider_available;
                            let unread: u32 = a.mailboxes.iter().filter(|m| m.role == "inbox").map(|m| m.unread).sum();
                            let mut boxes = a.mailboxes.clone();
                            let mut ordered: Vec<(Mailbox, usize)> = Vec::new();
                            fn place(all: &[Mailbox], parent: &str, depth: usize, out: &mut Vec<(Mailbox, usize)>) {
                                for m in all.iter().filter(|m| m.parent_id == parent) {
                                    out.push((m.clone(), depth));
                                    if depth < 6 {
                                        place(all, &m.id, depth + 1, out);
                                    }
                                }
                            }
                            place(&boxes, "", 0, &mut ordered);
                            boxes.retain(|m| !ordered.iter().any(|(o, _)| o.id == m.id));
                            ordered.extend(boxes.into_iter().map(|m| (m, 0)));

                            let id_toggle = account_id.clone();
                            let id_menu = account_id.clone();
                            let id_menu2 = account_id.clone();
                            let id_menu3 = account_id.clone();
                            let id_menu4 = account_id.clone();
                            let id_new_folder = account_id.clone();
                            let id_new_form = account_id.clone();
                            let id_new_submit = account_id.clone();
                            let name = a.name.clone();
                            let name_for_toggle = a.name.clone();

                            view! {
                                <div class="mail-row mail-account-row">
                                    <button
                                        type="button"
                                        class="icon-button mail-toggle"
                                        aria-label=move || format!("{} {}", if open.get() { "Collapse" } else { "Expand" }, name_for_toggle)
                                        aria-expanded=move || open.get().to_string()
                                        on:click=move |_| {
                                            state.collapsed.update(|c| {
                                                if !c.remove(&id_toggle) {
                                                    c.insert(id_toggle.clone());
                                                }
                                            });
                                        }
                                    >
                                        <Icon name="chevron"/>
                                    </button>
                                    <span class=format!("mail-acct {}", tone(a.color)) aria-hidden="true">
                                        {initials(&a.name).chars().next().unwrap_or('?').to_string()}
                                    </span>
                                    <span class="mail-row-title mail-account-name" title=a.address.clone()>{a.name.clone()}</span>
                                    {problem.then(|| view! {
                                        <span class="mail-warning" title=a.last_error.clone()>
                                            <Icon name="alert"/>
                                            <span class="sr-only">"Needs attention"</span>
                                        </span>
                                    })}
                                    {move || (unread > 0 && !open.get()).then(|| view! {
                                        <span class="mail-count">{unread}</span>
                                    })}
                                    <span class="mail-proto">{protocol}</span>
                                    <ActionMenu
                                        label=format!("Actions for {name}")
                                        class="mail-row-menu"
                                        actions=move |_| vec![
                                            MenuAction::new("Sync now", "refresh").run({ let id = id_menu.clone(); move || state.sync(Some(id.clone())) }),
                                            MenuAction::new("New folder", "plus").run({
                                                let id = id_menu4.clone();
                                                move || {
                                                    state.collapsed.update(|c| { c.remove(&id); });
                                                    new_folder_name.set(String::new());
                                                    creating_for.set(Some(id.clone()));
                                                }
                                            }),
                                            MenuAction::new("Account settings", "settings").run(move || state.pane.set(Some(Pane::Settings("Accounts")))),
                                            MenuAction::new("Sign in again", "key").run({ let id = id_menu2.clone(); move || state.pane.set(Some(Pane::AddAccount(Some(id.clone())))) }),
                                            MenuAction::new("Open in Files", "folder").href(format!("/?folder={}", encode(&state.account(&id_menu3).map(|a| a.folder_id).unwrap_or_default()))),
                                        ]
                                    >
                                        <Icon name="more"/>
                                    </ActionMenu>
                                </div>
                                {move || open.get().then(|| {
                                    let is_creating = creating_for.get().as_deref() == Some(id_new_form.as_str());
                                    let id_btn = id_new_folder.clone();
                                    let id_sub = id_new_submit.clone();
                                    view! {
                                        {ordered.iter().map(|(m, depth)| {
                                            let target = View::Mailbox(m.id.clone());
                                            let icon = match m.role.as_str() {
                                                "inbox" => "inbox",
                                                "sent" => "paperPlane",
                                                "drafts" => "pencil",
                                                "trash" => "trash",
                                                "junk" => "spam",
                                                "archive" => "archive",
                                                _ => "folder",
                                            };
                                            let count = m.unread;
                                            let class = if *depth > 0 { "is-nested is-deep" } else { "is-nested" };
                                            if m.role == "folder" {
                                                return view! {
                                                    <div class=format!("nav-item mail-view mail-container {class}") data-depth=depth.min(&6).to_string()>
                                                        <Icon name="folder"/>
                                                        <span class="mail-row-title">{m.name.clone()}</span>
                                                    </div>
                                                }.into_any();
                                            }
                                            let count = Signal::derive(move || if count > 0 { count } else { 0 });
                                            view! {
                                                <div class="mail-mailbox" data-depth=depth.min(&6).to_string()>
                                                    {view_row(target, m.name.clone(), icon, count, class)}
                                                </div>
                                            }.into_any()
                                        }).collect_view()}
                                        {if is_creating {
                                            view! {
                                                <form
                                                    class="mail-new-folder-form"
                                                    on:submit=move |e| {
                                                        e.prevent_default();
                                                        let n = new_folder_name.get_untracked();
                                                        if !n.trim().is_empty() {
                                                            state.create_mailbox(id_sub.clone(), n);
                                                            creating_for.set(None);
                                                            new_folder_name.set(String::new());
                                                        }
                                                    }
                                                >
                                                    <input
                                                        type="text"
                                                        maxlength="100"
                                                        placeholder="Folder name"
                                                        aria-label="New folder name"
                                                        autofocus
                                                        prop:value=move || new_folder_name.get()
                                                        on:input=move |e| new_folder_name.set(event_target_value(&e))
                                                        on:keydown=move |e| if e.key() == "Escape" { creating_for.set(None); }
                                                    />
                                                    <button type="submit" class="icon-button" aria-label="Create folder" title="Create folder"><Icon name="check"/></button>
                                                    <button type="button" class="icon-button" aria-label="Cancel" title="Cancel" on:click=move |_| creating_for.set(None)><Icon name="close"/></button>
                                                </form>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="mail-mailbox mail-new-folder-row">
                                                    <button
                                                        type="button"
                                                        class="nav-item mail-view is-nested is-subtle"
                                                        on:click=move |_| {
                                                            new_folder_name.set(String::new());
                                                            creating_for.set(Some(id_btn.clone()));
                                                        }
                                                    >
                                                        <Icon name="plus"/>
                                                        <span class="mail-row-title">"New folder\u{2026}"</span>
                                                    </button>
                                                </div>
                                            }.into_any()
                                        }}
                                    }
                                })}
                            }
                        }).collect_view()
                    }}
                </nav>
            </div>

            <div class="sidebar-footer mail-footer">
                <button
                    type="button"
                    class="text-button mail-sync-status"
                    disabled=move || state.syncing.get()
                    on:click=move |_| state.sync(None)
                >
                    <span class=move || {
                        let bad = state.overview.with(|o| o.as_ref().is_some_and(|o| o.accounts.iter().any(|a| a.status == "error" || a.status == "signIn")));
                        let none = state.overview.with(|o| o.as_ref().is_none_or(|o| o.accounts.is_empty()));
                        if state.syncing.get() {
                            "mail-sync-dot is-busy"
                        } else if bad {
                            "mail-sync-dot is-bad"
                        } else if none {
                            "mail-sync-dot is-idle"
                        } else {
                            "mail-sync-dot is-ok"
                        }
                    } aria-hidden="true"></span>
                    <span>
                        {move || {
                            if state.syncing.get() {
                                return "Checking for mail\u{2026}".to_owned();
                            }
                            state.overview.with(|o| {
                                let accounts = o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default();
                                if accounts.is_empty() {
                                    "No accounts yet".to_owned()
                                } else if accounts.iter().any(|a| a.status == "syncing" && a.synced_at == 0.0) {
                                    "Mirroring mail\u{2026}".to_owned()
                                } else {
                                    let last = accounts.iter().map(|a| a.synced_at).fold(f64::MAX, f64::min);
                                    format!("Synced {}", relative(last))
                                }
                            })
                        }}
                    </span>
                    <Icon name="refresh"/>
                </button>
            </div>
            <button
                type="button"
                class="nav-item ui-app-settings-entry"
                on:click=move |_| state.pane.set(Some(Pane::Settings("Accounts")))
            >
                <Icon name="settings"/>"Mail settings"
            </button>
        </aside>
    }
}
