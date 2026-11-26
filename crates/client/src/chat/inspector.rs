//! Chat info sheet: Overview, Media, and Members tabs beside the open conversation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::{
    BASE, ChatAvatar, ChatInfo, ChatSetting, ChatThread, LightboxItem, local_name, media_url,
    mute_menu_items, muted, phone_of, platform_name, search_params,
};
use crate::api::{self, BrowserSession, format_bytes};
use crate::dates::local_date_time;
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::ui::ActionMenu;

/// Tabbed inspector sheet showing contact/group/channel overview, shared media, and members.
#[component]
pub fn ChatInfoSheet(
    thread_id: String,
    session: BrowserSession,
    epoch: RwSignal<u32>,
    on_close: Callback<()>,
    on_setting: Callback<ChatSetting>,
    on_add_contact: Callback<ChatThread>,
    on_lightbox: Callback<LightboxItem>,
) -> impl IntoView {
    let _ = session;
    let info: RwSignal<Option<ChatInfo>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let tab = RwSignal::new("overview");
    let thread_key = StoredValue::new(thread_id);

    Effect::new(move |_| {
        epoch.track();
        let gone = Arc::new(AtomicBool::new(false));
        {
            let gone = gone.clone();
            on_cleanup(move || gone.store(true, Ordering::Relaxed));
        }
        let id = thread_key.get_value();
        spawn_local(async move {
            let result = api::get::<ChatInfo>(&format!(
                "{BASE}/info?{}",
                search_params(&[("threadId", &id)])
            ))
            .await;
            if gone.load(Ordering::Relaxed) {
                return;
            }
            match result {
                Ok(value) => {
                    if info.try_with_untracked(|i| i.as_ref() != Some(&value)) == Some(true) {
                        info.try_set(Some(value));
                    }
                    error.try_set(String::new());
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
        });
    });

    let title = move || {
        info.with(|i| match i {
            Some(i) if !i.thread.server_id.is_empty() => "Channel info",
            Some(i) if i.thread.kind == "group" => "Group info",
            Some(i) if i.account.platform == "discord" => "Chat info",
            Some(_) | None => "Contact info",
        })
    };

    view! {
        <section class="chat-info" aria-label="Chat info">
            <header class="chat-conversation-head">
                <button
                    type="button"
                    class="icon-button chat-back"
                    aria-label="Back to chat"
                    on:click=move |_| { on_close.try_run(()); }
                >
                    <Icon name="back"/>
                </button>
                <h2 class="chat-info-title">{title}</h2>
                <button
                    type="button"
                    class="icon-button chat-close"
                    aria-label="Close chat info"
                    on:click=move |_| { on_close.try_run(()); }
                >
                    <Icon name="close"/>
                </button>
            </header>
            <div class="chat-info-body">
                {move || (!error.with(String::is_empty)).then(|| view! {
                    <p class="error" role="alert">{error.get()}</p>
                })}
                {move || (info.with(Option::is_none) && error.with(String::is_empty)).then(|| view! {
                    <p class="chat-empty" role="status">"Loading\u{2026}"</p>
                })}
                {move || info.get().map(|info| {
                    let thread = info.thread.clone();
                    let name = if !thread.name.is_empty() {
                        thread.name.clone()
                    } else {
                        Some(phone_of(&thread.jid))
                            .filter(|p| !p.is_empty())
                            .unwrap_or_else(|| "Chat".into())
                    };
                    let quiet = muted(&thread);
                    let group = thread.kind == "group";
                    let pictures: Vec<_> = info
                        .media
                        .iter()
                        .filter(|m| (m.kind == "image" || m.kind == "sticker") && m.state == "stored")
                        .take(9)
                        .cloned()
                        .collect();
                    let files: Vec<_> = info
                        .media
                        .iter()
                        .filter(|m| m.kind == "document")
                        .take(6)
                        .cloned()
                        .collect();
                    let linked = matches!(
                        info.account.state.as_str(),
                        "connected" | "connecting" | "disconnected"
                    );
                    let discord = info.account.platform == "discord";
                    let channel = !thread.server_id.is_empty();
                    let platform = info.account.platform.clone();
                    let has_members = group && !discord;
                    let subtitle = if channel {
                        format!(
                            "{}{}",
                            info.server.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "Server".into()),
                            if thread.section.is_empty() {
                                String::new()
                            } else {
                                format!(" \u{b7} {}", thread.section)
                            }
                        )
                    } else if group {
                        format!(
                            "Group{}",
                            info.group
                                .as_ref()
                                .map(|g| format!(" \u{b7} {} members", g.members.len()))
                                .unwrap_or_default()
                        )
                    } else if discord {
                        "Direct message".into()
                    } else {
                        phone_of(&thread.jid)
                    };
                    let topic = info
                        .group
                        .as_ref()
                        .map(|g| g.topic.clone())
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| thread.topic.clone());
                    let pinned = thread.pinned;
                    let archived = thread.archived;
                    let tid = thread_key.get_value();
                    let thread_sv = StoredValue::new(thread.clone());
                    let shown_sender = if channel { format!("#{name}") } else { name.clone() };
                    let account_icon = if discord { "hash" } else { "chat" };
                    let active_tab = move || {
                        let t = tab.get();
                        if t == "members" && !has_members {
                            "overview"
                        } else {
                            t
                        }
                    };

                    view! {
                        <div class="chat-info-hero">
                            {if channel {
                                view! {
                                    <span class="chat-channel-mark is-large" aria-hidden="true">
                                        <Icon name=if thread.kind == "voice" { "speaker" } else { "hash" }/>
                                    </span>
                                }.into_any()
                            } else {
                                view! { <ChatAvatar name=name.clone() group=group size="is-large"/> }.into_any()
                            }}
                            <h3>{if channel { format!("#{name}") } else { name.clone() }}</h3>
                            <p>{format!("{subtitle} \u{b7} {}", platform_name(&platform))}</p>
                        </div>
                        {linked.then(|| view! {
                            <div class="chat-info-actions">
                                {if quiet {
                                    view! {
                                        <button
                                            type="button"
                                            class="is-on"
                                            on:click=move |_| {
                                                on_setting.try_run(ChatSetting {
                                                    muted_until: Some(0),
                                                    ..Default::default()
                                                });
                                            }
                                        >
                                            <span><Icon name="bell"/></span>"Unmute"
                                        </button>
                                    }.into_any()
                                } else {
                                    view! {
                                        <ActionMenu
                                            label="Mute"
                                            class="chat-info-mute"
                                            actions=Callback::new(move |_| mute_menu_items(on_setting))
                                        >
                                            <span><Icon name="bellOff"/></span>"Mute"
                                        </ActionMenu>
                                    }.into_any()
                                }}
                                {(!discord).then(|| view! {
                                    <button
                                        type="button"
                                        class=if pinned { "is-on" } else { "" }
                                        disabled=archived
                                        on:click=move |_| {
                                            on_setting.try_run(ChatSetting {
                                                pinned: Some(!pinned),
                                                ..Default::default()
                                            });
                                        }
                                    >
                                        <span><Icon name="pin"/></span>{if pinned { "Unpin" } else { "Pin" }}
                                    </button>
                                    <button
                                        type="button"
                                        class=if archived { "is-on" } else { "" }
                                        on:click=move |_| {
                                            on_setting.try_run(ChatSetting {
                                                archived: Some(!archived),
                                                ..Default::default()
                                            });
                                        }
                                    >
                                        <span><Icon name="archive"/></span>{if archived { "Unarchive" } else { "Archive" }}
                                    </button>
                                })}
                            </div>
                        })}
                        <div class="chat-info-tabs" role="tablist" aria-label="Chat info sections">
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (active_tab() == "overview").to_string()
                                class=move || if active_tab() == "overview" { "is-active" } else { "" }
                                on:click=move |_| tab.set("overview")
                            >
                                "Overview"
                            </button>
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (active_tab() == "media").to_string()
                                class=move || if active_tab() == "media" { "is-active" } else { "" }
                                on:click=move |_| tab.set("media")
                            >
                                "Media"
                            </button>
                            {has_members.then(|| view! {
                                <button
                                    type="button"
                                    role="tab"
                                    aria-selected=move || (active_tab() == "members").to_string()
                                    class=move || if active_tab() == "members" { "is-active" } else { "" }
                                    on:click=move |_| tab.set("members")
                                >
                                    "Members"
                                </button>
                            })}
                        </div>
                        {move || match active_tab() {
                            "media" => {
                                let empty = pictures.is_empty() && files.is_empty();
                                view! {
                                    <div class="chat-info-panel" role="tabpanel">
                                        {(!pictures.is_empty()).then(|| {
                                            let tid = tid.clone();
                                            let platform = platform.clone();
                                            let sender = shown_sender.clone();
                                            view! {
                                                <h4 class="chat-info-heading">"Photos"</h4>
                                                <div class="chat-info-grid">
                                                    {pictures.iter().map(|m| {
                                                        let url = media_url(&tid, &m.message_id);
                                                        let file_name = local_name(&m.kind, &m.mime, &m.file_name, m.at, &platform);
                                                        let time = local_date_time(m.at, &[("dateStyle", "medium"), ("timeStyle", "short")]);
                                                        let caption = m.file_name.clone();
                                                        let item = LightboxItem {
                                                            url: url.clone(),
                                                            download_url: format!("{url}&download=1"),
                                                            file_name: file_name.clone(),
                                                            sender: sender.clone(),
                                                            time,
                                                            caption,
                                                            message: None,
                                                        };
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class="chat-info-thumb"
                                                                aria-label=format!("Open {file_name}")
                                                                on:click=move |_| { on_lightbox.try_run(item.clone()); }
                                                            >
                                                                <img src=url alt="" loading="lazy"/>
                                                            </button>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            }
                                        })}
                                        {(!files.is_empty()).then(|| {
                                            let tid = tid.clone();
                                            let platform = platform.clone();
                                            view! {
                                                <h4 class="chat-info-heading">"Documents"</h4>
                                                <div class="chat-info-files">
                                                    {files.iter().map(|m| {
                                                        let file = local_name(&m.kind, &m.mime, &m.file_name, m.at, &platform);
                                                        view! {
                                                            <a class="chat-file" href=format!("{}&download=1", media_url(&tid, &m.message_id)) download=file.clone()>
                                                                <FileTile kind="file" name=file.clone() media_type=m.mime.clone()/>
                                                                <span class="chat-file-name">
                                                                    <b>{file.clone()}</b>
                                                                    <small>{format_bytes(m.size)}</small>
                                                                </span>
                                                            </a>
                                                        }
                                                    }).collect_view()}
                                                </div>
                                            }
                                        })}
                                        {empty.then(|| view! {
                                            <p class="chat-hint">"No shared media yet."</p>
                                        })}
                                    </div>
                                }.into_any()
                            }
                            "members" => {
                                let members_heading = format!(
                                    "Members{}",
                                    info.group
                                        .as_ref()
                                        .map(|g| format!(" \u{b7} {}", g.members.len()))
                                        .unwrap_or_default()
                                );
                                view! {
                                    <div class="chat-info-panel" role="tabpanel">
                                        <h4 class="chat-info-heading">{members_heading}</h4>
                                        {(!info.group_error.is_empty()).then(|| view! {
                                            <p class="chat-hint">{info.group_error.clone()}</p>
                                        })}
                                        {info.group.clone().map(|g| view! {
                                            <ul class="chat-info-members">
                                                {g.members.into_iter().map(|member| {
                                                    let shown = if !member.name.is_empty() {
                                                        member.name.clone()
                                                    } else {
                                                        Some(phone_of(&member.jid))
                                                            .filter(|p| !p.is_empty())
                                                            .unwrap_or_else(|| "Unknown".into())
                                                    };
                                                    let avatar_name = if member.name.is_empty() {
                                                        member.jid.clone()
                                                    } else {
                                                        member.name.clone()
                                                    };
                                                    view! {
                                                        <li>
                                                            <ChatAvatar name=avatar_name group=false size="is-tiny"/>
                                                            <span class="chat-member-name">{shown}</span>
                                                            {member.admin.then(|| view! { <small>"Admin"</small> })}
                                                        </li>
                                                    }
                                                }).collect_view()}
                                            </ul>
                                        })}
                                    </div>
                                }.into_any()
                            }
                            _ => {
                                let phone = phone_of(&thread.jid);
                                view! {
                                    <div class="chat-info-panel" role="tabpanel">
                                        {(!topic.is_empty()).then(|| view! {
                                            <p class="chat-info-topic">{topic.clone()}</p>
                                        })}
                                        {(!group && !discord).then(|| view! {
                                            <div class="chat-contact">
                                                {match thread.contact.clone() {
                                                    Some(contact) => {
                                                        let on_whatsapp = if !thread.whatsapp_name.is_empty()
                                                            && thread.whatsapp_name != contact.name
                                                            && thread.whatsapp_name != phone
                                                        {
                                                            format!(" \u{b7} on WhatsApp \u{201c}{}\u{201d}", thread.whatsapp_name)
                                                        } else {
                                                            String::new()
                                                        };
                                                        let href = format!(
                                                            "/contacts?{}",
                                                            search_params(&[("book", &contact.book), ("contact", &contact.href)])
                                                        );
                                                        view! {
                                                            <ChatAvatar name=contact.name.clone() group=false size="is-small"/>
                                                            <div>
                                                                <b>{contact.name.clone()}</b>
                                                                <small>{format!("Dar contact{on_whatsapp}")}</small>
                                                            </div>
                                                            <a class="text-button" href=href>"Open"</a>
                                                        }.into_any()
                                                    }
                                                    None => view! {
                                                        <span class="chat-empty-icon is-small"><Icon name="contacts"/></span>
                                                        <div>
                                                            <b>"Not in your contacts"</b>
                                                            <small>"Save this number in Dar Contacts."</small>
                                                        </div>
                                                        <button
                                                            type="button"
                                                            class="text-button"
                                                            on:click=move |_| { on_add_contact.try_run(thread_sv.get_value()); }
                                                        >
                                                            "Add"
                                                        </button>
                                                    }.into_any(),
                                                }}
                                            </div>
                                        })}
                                        <dl class="chat-info-card">
                                            <div>
                                                <dt><Icon name=account_icon/>"Account"</dt>
                                                <dd>{info.account.name.clone()}</dd>
                                            </div>
                                            <div>
                                                <dt><Icon name="clock"/>"Messages in Dar"</dt>
                                                <dd>{format!("{}{}", locale_number(info.messages), if info.complete { "" } else { "+" })}</dd>
                                            </div>
                                            {quiet.then(|| view! {
                                                <div>
                                                    <dt><Icon name="bellOff"/>"Muted"</dt>
                                                    <dd>{if thread.muted_until == -1.0 {
                                                        "Always".to_owned()
                                                    } else {
                                                        format!("Until {}", local_date_time(thread.muted_until, &[("dateStyle", "medium"), ("timeStyle", "short")]))
                                                    }}</dd>
                                                </div>
                                            })}
                                        </dl>
                                    </div>
                                }.into_any()
                            }
                        }}
                    }
                })}
            </div>
        </section>
    }
}

/// `number.toLocaleString()`.
fn locale_number(value: u64) -> String {
    js_sys::Number::from(value as f64)
        .to_locale_string("default")
        .as_string()
        .unwrap_or_else(|| value.to_string())
}
