//! Chat sidebar: create action, account tabs, WhatsApp views or Discord Direct messages
//! and collapsible server/channel tree with voice members, voice dock, compact account
//! status strip, and settings entry.

use std::collections::BTreeSet;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::model::{
    ChatAccount, ChatServer, ChatThread, ChatView, ChatVoiceCall, ChatVoiceServers, initials,
    phone_of, platform_name, quiet_until, state_text,
};
use super::voice::VoiceMembers;
use crate::icons::Icon;
use crate::pim::avatar_hue;
use crate::ui::{ActionMenu, MenuAction};

fn account_dot_class(state: &str, prefix: &str) -> String {
    let modifier = match state {
        "connected" => "is-online",
        "disconnected" | "connecting" | "linking" => "is-warn",
        _ => "is-err",
    };
    format!("{prefix} {modifier}")
}

/// Account switcher tablist rendered in the sidebar on desktop and in the list header on phones.
#[component]
pub fn AccountTabs(
    #[prop(into)] accounts: Signal<Vec<ChatAccount>>,
    #[prop(into)] account_key: Signal<String>,
    #[prop(into)] adding: Signal<bool>,
    class: &'static str,
    on_choose: Callback<String>,
    on_link: Callback<()>,
) -> impl IntoView {
    let has_accounts = Memo::new(move |_| !accounts.with(Vec::is_empty));
    view! {
        <div class=class role="tablist" aria-label="Accounts">
            {move || has_accounts.get().then(|| view! {
                <div class="chat-account-tabs">
                    <For
                        each=move || accounts.get()
                        key=|a| (a.id.clone(), a.name.clone(), a.unread, a.platform.clone(), a.state.clone())
                        let:a
                    >
                        {
                            let id = a.id.clone();
                            let id_click = id.clone();
                            let is_current = {
                                let id = id.clone();
                                move || account_key.get() == id
                            };
                            let is_current2 = is_current.clone();
                            let dot_class = account_dot_class(&a.state, "chat-account-dot");
                            let platform_icon = if a.platform == "discord" { "hash" } else { "chat" };
                            let unread = a.unread;
                            view! {
                                <button
                                    type="button"
                                    role="tab"
                                    data-keep-drawer=""
                                    aria-selected=move || is_current().to_string()
                                    class=move || if is_current2() { "chat-account-tab is-current" } else { "chat-account-tab" }
                                    title=a.name.clone()
                                    on:click=move |_| { on_choose.try_run(id_click.clone()); }
                                >
                                    <span class=format!("chat-platform is-{}", a.platform) aria-hidden="true">
                                        <Icon name=platform_icon/>
                                    </span>
                                    <span class="chat-account-name">{a.name.clone()}</span>
                                    <span class=dot_class aria-hidden="true"></span>
                                    <span class="sr-only">{format!(" ({})", platform_name(&a.platform))}</span>
                                    {(unread > 0).then(|| view! {
                                        <span class="chat-account-count">{unread}<span class="sr-only">" unread"</span></span>
                                    })}
                                </button>
                            }
                        }
                    </For>
                </div>
                <span class="chat-account-sep" aria-hidden="true"></span>
            })}
            {move || if has_accounts.get() {
                view! {
                    <button
                        type="button"
                        class=move || if adding.get() { "icon-button chat-account-add is-active" } else { "icon-button chat-account-add" }
                        aria-label="Link an account"
                        aria-pressed=move || adding.get().to_string()
                        title="Link an account"
                        on:click=move |_| { on_link.try_run(()); }
                    >
                        <Icon name="plus"/>
                    </button>
                }.into_any()
            } else {
                view! {
                    <button
                        type="button"
                        class=move || if adding.get() { "chat-account-add is-empty is-active" } else { "chat-account-add is-empty" }
                        aria-pressed=move || adding.get().to_string()
                        on:click=move |_| { on_link.try_run(()); }
                    >
                        <Icon name="plus"/>
                        <span>"Link an account"</span>
                    </button>
                }.into_any()
            }}
        </div>
    }
}

/// Chat workspace sidebar navigation, account switcher, views or Discord tree, voice dock,
/// compact status strip, and settings entry.
#[component]
pub fn ChatSidebar(
    navigation_id: String,
    navigation_ref: NodeRef<html::Aside>,
    navigation: RwSignal<bool>,
    #[prop(into)] accounts: Signal<Vec<ChatAccount>>,
    #[prop(into)] account: Signal<Option<ChatAccount>>,
    #[prop(into)] account_key: Signal<String>,
    #[prop(into)] adding: Signal<bool>,
    #[prop(into)] discord: Signal<bool>,
    #[prop(into)] has_account: Signal<bool>,
    #[prop(into)] view: Signal<ChatView>,
    #[prop(into)] selected: Signal<String>,
    #[prop(into)] channel_selected: Signal<bool>,
    #[prop(into)] servers: Signal<Option<Vec<ChatServer>>>,
    #[prop(into)] voice: Signal<ChatVoiceServers>,
    #[prop(into)] call: Signal<Option<ChatVoiceCall>>,
    #[prop(into)] speaking: Signal<BTreeSet<String>>,
    #[prop(into)] me: Signal<String>,
    render_dock: Callback<&'static str, AnyView>,
    on_new_chat: Callback<()>,
    on_choose_account: Callback<String>,
    on_link: Callback<(Option<String>, Option<String>)>,
    on_choose_view: Callback<ChatView>,
    on_direct_messages: Callback<()>,
    on_open_channel: Callback<(ChatThread, web_sys::HtmlElement)>,
    on_settings: Callback<()>,
    account_actions: Callback<ChatAccount, Vec<MenuAction>>,
) -> impl IntoView {
    let collapsed: RwSignal<BTreeSet<String>> = RwSignal::new(BTreeSet::new());
    let collapsed_categories: RwSignal<BTreeSet<String>> = RwSignal::new(BTreeSet::new());

    let toggle_server = move |id: String| {
        collapsed.update(|set| {
            if !set.remove(&id) {
                set.insert(id);
            }
        });
    };

    view! {
        <aside
            id=navigation_id
            node_ref=navigation_ref
            class=move || if navigation.get() { "sidebar chat-sidebar is-open" } else { "sidebar chat-sidebar" }
            aria-label="Chat navigation"
        >
            <button
                type="button"
                class="new-button ui-new"
                disabled=move || account.with(|a| a.as_ref().is_none_or(|a| a.state == "loggedOut" || a.state == "linking"))
                on:click=move |_| {
                    navigation.set(false);
                    on_new_chat.try_run(());
                }
            >
                <Icon name="plus"/>
                {move || if discord.get() { "New message" } else { "New chat" }}
            </button>

            <AccountTabs
                accounts=accounts
                account_key=account_key
                adding=adding
                class="chat-accounts"
                on_choose=on_choose_account
                on_link=Callback::new(move |_| { on_link.try_run((None, None)); })
            />

            <div class="chat-side-scroll">
                {move || discord.get().then(|| account.get()).flatten().map(|a| {
                    let unread = a.counts.unread;
                    view! {
                        <nav aria-label="Direct messages">
                            <button
                                type="button"
                                class=move || if channel_selected.get() { "nav-item chat-view" } else { "nav-item chat-view active" }
                                aria-current=move || (!channel_selected.get()).then_some("page")
                                on:click=move |_| {
                                    navigation.set(false);
                                    on_direct_messages.try_run(());
                                }
                            >
                                <Icon name="contacts"/>
                                <span class="chat-row-title">"Direct messages"</span>
                                {(unread > 0).then(|| view! { <span class="chat-count">{unread}</span> })}
                            </button>
                        </nav>
                    }
                })}

                {move || (discord.get() && has_account.get()).then(|| view! {
                    <ServerTree
                        servers=servers
                        selected=selected
                        collapsed=collapsed
                        collapsed_categories=collapsed_categories
                        on_toggle=Callback::new(toggle_server)
                        on_open=on_open_channel
                        voice=voice
                        call=call
                        speaking=speaking
                        me=me
                    />
                })}

                {move || (!discord.get()).then(|| account.get()).flatten().map(|a| view! {
                    <nav aria-label="Chat views">
                        {ChatView::ALL.into_iter().map(|v| {
                            let count = a.counts.for_view(v);
                            view! {
                                <button
                                    type="button"
                                    class=move || if view.get() == v { "nav-item chat-view active" } else { "nav-item chat-view" }
                                    aria-current=move || (view.get() == v).then_some("page")
                                    on:click=move |_| {
                                        navigation.set(false);
                                        on_choose_view.try_run(v);
                                    }
                                >
                                    <Icon name=v.icon()/>
                                    <span class="chat-row-title">{v.label()}</span>
                                    {(count > 0).then(|| view! { <span class="chat-count">{count}</span> })}
                                </button>
                            }
                        }).collect_view()}
                    </nav>
                })}
            </div>

            {move || render_dock.run("")}

            {move || account.get().map(|a| {
                let (id, platform) = (a.id.clone(), a.platform.clone());
                let a_menu = a.clone();
                let actions = Callback::new(move |_| account_actions.run(a_menu.clone()));
                view! {
                    <AccountStatus
                        account=a
                        on_link=Callback::new(move |_| {
                            navigation.set(false);
                            on_link.try_run((Some(id.clone()), Some(platform.clone())));
                        })
                        actions=actions
                    />
                }
            })}

            <button
                type="button"
                class="nav-item ui-app-settings-entry"
                on:click=move |_| {
                    navigation.set(false);
                    on_settings.try_run(());
                }
            >
                <Icon name="settings"/>"Chat settings"
            </button>
        </aside>
    }
}

/// Compact single-line status strip at the bottom of the sidebar (visible on desktop and phones).
#[component]
fn AccountStatus(
    account: ChatAccount,
    on_link: Callback<()>,
    actions: Callback<(), Vec<MenuAction>>,
) -> impl IntoView {
    let phone = phone_of(&account.jid);
    let has_phone = !phone.is_empty();
    let detail = state_text(&account.detail);
    let discord = account.platform == "discord";
    let tone = match account.state.as_str() {
        "connected" | "connecting" | "linking" => "",
        "disconnected" | "loggedOut" => "is-warning",
        _ => "is-error",
    };
    let (heading, sub): (String, Option<String>) = match account.state.as_str() {
        "connected" if account.importing => (
            if has_phone {
                phone
            } else {
                account.name.clone()
            },
            Some(if account.history_progress > 0 {
                format!("Importing {}%", account.history_progress)
            } else {
                "Importing\u{2026}".into()
            }),
        ),
        "connected" if discord => (
            account.name.clone(),
            Some(if account.name.eq_ignore_ascii_case("discord") {
                "Connected".into()
            } else {
                "Discord".into()
            }),
        ),
        "connected" => (
            if has_phone {
                phone
            } else {
                account.name.clone()
            },
            if account.name.is_empty() || !has_phone {
                Some("WhatsApp".into())
            } else {
                Some(account.name.clone())
            },
        ),
        "connecting" => ("Connecting\u{2026}".into(), has_phone.then_some(phone)),
        "disconnected" => (
            "Reconnecting\u{2026}".into(),
            Some("Waiting for connection".into()),
        ),
        "linking" => ("Not linked yet".into(), None),
        "loggedOut" => (
            if discord {
                "Signed out".into()
            } else {
                "Unlinked".into()
            },
            Some("Read-only".into()),
        ),
        _ => (
            if discord {
                "Discord stopped".into()
            } else {
                "WhatsApp stopped".into()
            },
            (!detail.is_empty()).then(|| detail.to_owned()),
        ),
    };
    let title = if !detail.is_empty() {
        detail.to_owned()
    } else if let Some(s) = &sub {
        format!("{heading} \u{b7} {s}")
    } else {
        heading.clone()
    };
    let relink_label = match account.state.as_str() {
        "loggedOut" | "failed" => Some("Link again"),
        "linking" => Some("Link"),
        _ => None,
    };

    view! {
        <div class=format!("chat-status {tone}") title=title>
            {if account.importing {
                view! { <span class="chat-spinner" aria-hidden="true"></span> }.into_any()
            } else {
                view! { <span class="chat-status-dot" aria-hidden="true"></span> }.into_any()
            }}
            <div class="chat-status-text">
                <b>{heading}</b>
                {sub.map(|s| view! { <span>{s}</span> })}
                {relink_label.map(|text| view! {
                    <button type="button" class="text-button" on:click=move |_| { on_link.try_run(()); }>
                        {text}
                    </button>
                })}
            </div>
            <ActionMenu label=format!("Actions for {}", account.name) actions=actions>
                <Icon name="more"/>
            </ActionMenu>
        </div>
    }
}

fn channel_row(
    c: ChatThread,
    server_id: String,
    selected: Signal<String>,
    voice: Signal<ChatVoiceServers>,
    call: Signal<Option<ChatVoiceCall>>,
    speaking: Signal<BTreeSet<String>>,
    me: Signal<String>,
    on_open: Callback<(ChatThread, web_sys::HtmlElement)>,
) -> impl IntoView {
    let cid = c.id.clone();
    let is_voice = c.kind == "voice";
    let muted = quiet_until(c.muted_until);
    let jid = c.jid.clone();
    let sid = server_id;
    let members = {
        let (jid, sid) = (jid.clone(), sid);
        Memo::new(move |_| {
            if is_voice {
                voice.with(|v| v.get(&sid).and_then(|s| s.get(&jid)).cloned().unwrap_or_default())
            } else {
                Vec::new()
            }
        })
    };
    let live = {
        let jid = jid.clone();
        Memo::new(move |_| is_voice && call.with(|c| c.as_ref().is_some_and(|c| c.channel_id == jid)))
    };
    let current = {
        let cid = cid.clone();
        Memo::new(move |_| selected.with(|s| *s == cid))
    };
    let unread = c.unread > 0 && !muted && !is_voice;
    let mentions = c.mentions;
    let name = c.name.clone();
    let label = {
        let name = name.clone();
        move || {
            let count = members.with(Vec::len);
            format!(
                "{} {name}{}{}{}",
                if is_voice { "Voice channel" } else { "Channel" },
                if live.get() {
                    ", you are in this call".to_owned()
                } else if count > 0 {
                    format!(", {count} in voice")
                } else {
                    String::new()
                },
                if mentions > 0 {
                    format!(", {mentions} mentions")
                } else if unread {
                    ", unread".into()
                } else {
                    String::new()
                },
                if muted { ", muted" } else { "" }
            )
        }
    };
    let thread = StoredValue::new(c);
    view! {
        <li>
            <button
                type="button"
                class=move || format!(
                    "nav-item chat-channel{}{}{}{}",
                    if current.get() { " active" } else { "" },
                    if unread { " is-unread" } else { "" },
                    if muted { " is-muted" } else { "" },
                    if live.get() { " is-live" } else { "" }
                )
                aria-current=move || current.get().then_some("page")
                aria-label=label
                on:click=move |event| {
                    if let Some(el) = event.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                        on_open.try_run((thread.get_value(), el));
                    }
                }
            >
                <Icon name=if is_voice { "speaker" } else { "hash" }/>
                <span class="chat-row-title">{name}</span>
                {(mentions > 0).then(|| view! { <span class="chat-mentions">{mentions}</span> })}
                {muted.then(|| view! { <Icon name="bellOff" class="chat-channel-muted"/> })}
            </button>
            {move || {
                let list = members.get();
                (!list.is_empty()).then(|| view! { <VoiceMembers members=list speaking=speaking.get() me=me.get()/> })
            }}
        </li>
    }
}

/// Discord servers in the sidebar: each opens to collapsible category sections and channels.
#[component]
fn ServerTree(
    servers: Signal<Option<Vec<ChatServer>>>,
    selected: Signal<String>,
    collapsed: RwSignal<BTreeSet<String>>,
    collapsed_categories: RwSignal<BTreeSet<String>>,
    on_toggle: Callback<String>,
    on_open: Callback<(ChatThread, web_sys::HtmlElement)>,
    voice: Signal<ChatVoiceServers>,
    call: Signal<Option<ChatVoiceCall>>,
    speaking: Signal<BTreeSet<String>>,
    me: Signal<String>,
) -> impl IntoView {
    move || {
        let Some(list) = servers.get() else {
            return view! { <p class="chat-hint chat-tree-hint" role="status">"Loading servers\u{2026}"</p> }.into_any();
        };
        if list.is_empty() {
            return view! { <p class="chat-hint chat-tree-hint">"No servers yet."</p> }.into_any();
        }
        view! {
            <nav class="chat-servers" aria-label="Servers">
                <h2 class="chat-tree-heading">"Servers"</h2>
                {list.into_iter().map(|s| {
                    let sid = s.id.clone();
                    let open = {
                        let sid = sid.clone();
                        Memo::new(move |_| !collapsed.with(|c| c.contains(&sid)))
                    };
                    let quiet = quiet_until(s.muted_until);
                    let unread = s.unread;
                    let name = s.name.clone();
                    let server_id = s.server_id.clone();
                    let channels = StoredValue::new(s.channels.clone());
                    let mentions = s.mentions;
                    view! {
                        <div class="chat-server">
                            <button
                                type="button"
                                data-keep-drawer=""
                                class=move || format!(
                                    "nav-item chat-server-row{}{}",
                                    if unread && !open.get() { " is-unread" } else { "" },
                                    if quiet { " is-muted" } else { "" }
                                )
                                aria-expanded=move || open.get().to_string()
                                on:click=move |_| { on_toggle.try_run(sid.clone()); }
                            >
                                <span class=format!("chat-server-mark contact-initials {}", avatar_hue(&server_id)) aria-hidden="true">{initials(&name)}</span>
                                <span class="chat-row-title">{name.clone()}</span>
                                {(mentions > 0).then(|| view! { <span class="chat-mentions">{mentions}<span class="sr-only">" mentions"</span></span> })}
                                {move || if open.get() { view! { <Icon name="chevron" class="chat-server-chevron is-open"/> } } else { view! { <Icon name="chevron" class="chat-server-chevron"/> } }}
                            </button>
                            {move || open.get().then(|| {
                                let server_id = server_id.clone();
                                let server_name = name.clone();
                                let list = channels.get_value();
                                if list.is_empty() {
                                    return view! {
                                        <ul class="chat-channels" aria-label=format!("Channels in {server_name}")>
                                            <li class="chat-section">"No channels you can see"</li>
                                        </ul>
                                    }.into_any();
                                }
                                let mut groups: Vec<(String, Vec<ChatThread>)> = Vec::new();
                                for ch in list {
                                    if let Some((sec, items)) = groups.last_mut()
                                        && *sec == ch.section
                                    {
                                        items.push(ch);
                                    } else {
                                        groups.push((ch.section.clone(), vec![ch]));
                                    }
                                }
                                groups.into_iter().map(|(sec, items)| {
                                    if sec.is_empty() {
                                        let sid = server_id.clone();
                                        let sname = server_name.clone();
                                        view! {
                                            <ul class="chat-channels" aria-label=format!("Channels in {sname}")>
                                                {items.into_iter().map(|c| channel_row(c, sid.clone(), selected, voice, call, speaking, me, on_open)).collect_view()}
                                            </ul>
                                        }.into_any()
                                    } else {
                                        let cat_key = format!("{server_id}/{sec}");
                                        let cat_open = {
                                            let k = cat_key.clone();
                                            Memo::new(move |_| !collapsed_categories.with(|c| c.contains(&k)))
                                        };
                                        let items_sv = StoredValue::new(items);
                                        let sid_sv = StoredValue::new(server_id.clone());
                                        let sname_sv = StoredValue::new(server_name.clone());
                                        let sec_sv = StoredValue::new(sec.clone());
                                        view! {
                                            <div class="chat-category">
                                                <button
                                                    type="button"
                                                    data-keep-drawer=""
                                                    class=move || if cat_open.get() { "chat-category-toggle is-open" } else { "chat-category-toggle" }
                                                    aria-expanded=move || cat_open.get().to_string()
                                                    on:click=move |_| {
                                                        let k = cat_key.clone();
                                                        collapsed_categories.update(|set| {
                                                            if !set.remove(&k) {
                                                                set.insert(k);
                                                            }
                                                        });
                                                    }
                                                >
                                                    <Icon name="chevron"/>
                                                    <span>{sec}</span>
                                                </button>
                                                {move || cat_open.get().then(|| {
                                                    let sid = sid_sv.get_value();
                                                    let sname = sname_sv.get_value();
                                                    let sec_name = sec_sv.get_value();
                                                    let list = items_sv.get_value();
                                                    let rows: Vec<_> = list
                                                        .into_iter()
                                                        .map(|c| channel_row(c, sid.clone(), selected, voice, call, speaking, me, on_open))
                                                        .collect();
                                                    view! {
                                                        <ul class="chat-channels" aria-label=format!("{sec_name} in {sname}")>
                                                            {rows}
                                                        </ul>
                                                    }
                                                })}
                                            </div>
                                        }.into_any()
                                    }
                                }).collect_view().into_any()
                            })}
                        </div>
                    }
                }).collect_view()}
            </nav>
        }.into_any()
    }
}
