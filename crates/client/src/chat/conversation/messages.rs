//! Message bubbles, flat Discord rows, inline attachments, hover quick actions, and reactions.

use leptos::prelude::*;

use super::super::{
    CHAT_QUICK_REACTIONS, ChatAvatar, ChatMessage, ChatPrefs, LightboxItem, Ticks, clock,
    emoji_label, fit, iso, kind_icon, kind_label, local_name, media_url, now, phone_of,
    platform_name,
};
use super::format::render_message_text;
use super::voice_note::VoiceNotePlayer;
use crate::api::format_bytes;
use crate::dates::{local_date_time, local_time};
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::pim::avatar_hue;
use crate::ui::{ActionMenu, MenuAction};

/// WhatsApp lets senders edit a message for a short while after sending it.
const EDIT_WINDOW: f64 = 15.0 * 60_000.0;

#[component]
pub(super) fn Bubble(
    thread_id: String,
    message: ChatMessage,
    tail: bool,
    show_name: bool,
    can_change: bool,
    platform: String,
    prefs: Signal<ChatPrefs>,
    found_id: RwSignal<String>,
    flash_id: RwSignal<String>,
    on_save: Callback<()>,
    on_reply: Callback<()>,
    on_copy: Callback<()>,
    on_react: Callback<(String, bool)>,
    on_edit: Callback<()>,
    on_revoke: Callback<()>,
    on_delete_for_me: Callback<()>,
    on_retry: Callback<()>,
    on_discard: Callback<()>,
    on_jump_quote: Callback<String>,
    on_open_lightbox: Callback<LightboxItem>,
) -> impl IntoView {
    let m = message;
    let label = if m.kind != "text" {
        kind_label(&m.kind)
    } else {
        ""
    };
    let icon = kind_icon(&m.kind);
    let discord = platform == "discord";
    let file = if m
        .media
        .as_ref()
        .is_some_and(|media| media.state != "expired")
    {
        media_url(&thread_id, &m.message_id)
    } else {
        String::new()
    };
    let mine = m
        .reactions
        .iter()
        .find(|r| r.mine)
        .map(|r| r.emoji.clone())
        .unwrap_or_default();
    let live = m.status != "pending" && m.status != "failed";
    // WhatsApp keeps one reaction per person; Discord allows several, each toggled on its own.
    let reactions = StoredValue::new(m.reactions.clone());
    let mine_sv = StoredValue::new(mine.clone());
    let actions = {
        let (file, mine, deleted, text, from_me, kind, at) = (
            file.clone(),
            mine,
            m.deleted,
            m.text.clone(),
            m.from_me,
            m.kind.clone(),
            m.at,
        );
        move || -> Vec<MenuAction> {
            if deleted {
                return vec![
                    MenuAction::new("Delete from Dar\u{2026}", "trash")
                        .danger()
                        .run(move || {
                            on_delete_for_me.try_run(());
                        }),
                ];
            }
            let mut actions = Vec::new();
            if can_change && live {
                let react_items: Vec<MenuAction> = if discord {
                    CHAT_QUICK_REACTIONS
                        .iter()
                        .map(|emoji| {
                            let emoji = (*emoji).to_owned();
                            let on = reactions
                                .with_value(|r| r.iter().any(|r| r.emoji == emoji && r.mine));
                            MenuAction::plain(emoji.clone()).current(on).run(move || {
                                on_react.try_run((emoji.clone(), on));
                            })
                        })
                        .collect()
                } else {
                    let mut items: Vec<MenuAction> = CHAT_QUICK_REACTIONS
                        .iter()
                        .map(|emoji| {
                            let emoji = (*emoji).to_owned();
                            let is_mine = emoji == mine;
                            MenuAction::plain(emoji.clone())
                                .current(is_mine)
                                .run(move || {
                                    on_react.try_run((
                                        if is_mine {
                                            String::new()
                                        } else {
                                            emoji.clone()
                                        },
                                        false,
                                    ));
                                })
                        })
                        .collect();
                    if !mine.is_empty() {
                        items.push(MenuAction::new("Remove reaction", "close").separator().run(
                            move || {
                                on_react.try_run((String::new(), false));
                            },
                        ));
                    }
                    items
                };
                actions.push(MenuAction::new("React", "star").items(react_items));
                actions.push(MenuAction::new("Reply", "reply").run(move || {
                    on_reply.try_run(());
                }));
            }
            if !text.is_empty() {
                actions.push(MenuAction::new("Copy text", "copy").run(move || {
                    on_copy.try_run(());
                }));
            }
            if can_change
                && live
                && from_me
                && kind == "text"
                && (discord || now() - at < EDIT_WINDOW)
            {
                actions.push(MenuAction::new("Edit", "pencil").run(move || {
                    on_edit.try_run(());
                }));
            }
            if !file.is_empty() {
                actions.push(
                    MenuAction::new("Download", "download")
                        .href(format!("{file}&download=1"))
                        .download()
                        .separator(),
                );
                actions.push(
                    MenuAction::new("Save to Files\u{2026}", "folder").run(move || {
                        on_save.try_run(());
                    }),
                );
            }
            let revoke = can_change && live && from_me;
            if revoke {
                actions.push(
                    MenuAction::new("Delete for everyone\u{2026}", "trash")
                        .danger()
                        .separator()
                        .run(move || {
                            on_revoke.try_run(());
                        }),
                );
            }
            let mut delete = MenuAction::new("Delete from Dar\u{2026}", "trash").danger();
            if !revoke {
                delete = delete.separator();
            }
            actions.push(delete.run(move || {
                on_delete_for_me.try_run(());
            }));
            actions
        }
    };
    let photo = !m.deleted
        && m.media.as_ref().is_some_and(|media| {
            (m.kind == "image" || m.kind == "sticker")
                && media.state != "expired"
                && media.state != "retrying"
        });
    let who = if m.from_me && discord {
        "You".to_owned()
    } else if !m.sender_name.is_empty() {
        m.sender_name.clone()
    } else {
        Some(phone_of(&m.sender))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "Unknown".into())
    };
    let time_text = local_time(m.at, &[("hour", "numeric"), ("minute", "2-digit")]);
    let time_title = local_date_time(m.at, &[("dateStyle", "medium"), ("timeStyle", "short")]);
    let at_iso = iso(m.at);
    let msg_id_for_class = m.message_id.clone();
    let failed = m.status == "failed";
    let from_me = m.from_me;
    let class = move || {
        let found = found_id.with(|f| !f.is_empty() && f == &msg_id_for_class);
        let flashed = flash_id.with(|f| !f.is_empty() && f == &msg_id_for_class);
        format!(
            "chat-message {}{}{}{}{}{}",
            if from_me { "is-out" } else { "is-in" },
            if tail { " is-tail" } else { "" },
            if show_name { " is-first" } else { "" },
            if failed { " is-failed" } else { "" },
            if found { " is-found" } else { "" },
            if flashed { " is-flash" } else { "" },
        )
    };
    let reactions_view = (!m.reactions.is_empty()).then(|| {
        view! {
            <div class="chat-reactions">
                {m.reactions.iter().map(|r| {
                    let who = if discord {
                        if r.mine {
                            if r.count > 1 {
                                format!("You and {} other{}", r.count - 1, if r.count > 2 { "s" } else { "" })
                            } else {
                                "You".into()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        let names = if r.mine {
                            std::iter::once("You".to_owned())
                                .chain(r.names.iter().cloned())
                                .collect::<Vec<_>>()
                                .join(", ")
                        } else {
                            r.names.join(", ")
                        };
                        let others = r.count as usize > r.names.len() + usize::from(r.mine);
                        format!("{names}{}", if others { " and others" } else { "" })
                    };
                    let shown = emoji_label(&r.emoji);
                    let count = r.count;
                    let class = if r.mine { "is-mine" } else { "" };
                    if can_change && live {
                        let (emoji, is_mine) = (r.emoji.clone(), r.mine);
                        let aria = format!(
                            "{shown} {count}{}. {}",
                            if who.is_empty() { String::new() } else { format!(", {who}") },
                            if is_mine {
                                "Remove your reaction".to_owned()
                            } else {
                                format!("React with {shown}")
                            }
                        );
                        view! {
                            <button
                                type="button"
                                class=class
                                aria-pressed=is_mine.to_string()
                                title=who
                                aria-label=aria
                                on:click=move |_| {
                                    if discord {
                                        on_react.try_run((emoji.clone(), is_mine));
                                    } else {
                                        on_react.try_run((if is_mine { String::new() } else { emoji.clone() }, false));
                                    }
                                }
                            >
                                {shown.clone()}{(count > 1).then(|| view! { <small>{count}</small> })}
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <span class=class title=who>
                                {shown.clone()}{(count > 1).then(|| view! { <small>{count}</small> })}
                            </span>
                        }.into_any()
                    }
                }).collect_view()}
            </div>
        }
    });
    let text_for_body = m.text.clone();
    let body = if m.deleted {
        view! {
            <span class="chat-text is-deleted"><Icon name="close"/>"This message was deleted"</span>
        }
        .into_any()
    } else {
        let media = if m.media.is_some() {
            Some(
                view! {
                    <MediaView
                        thread_id=thread_id.clone()
                        message=m.clone()
                        platform=platform.clone()
                        sender=who.clone()
                        time_title=time_title.clone()
                        on_open_lightbox=on_open_lightbox
                    />
                }
                .into_any(),
            )
        } else if !label.is_empty() {
            let text = if m.kind == "document" && !m.file_name.is_empty() {
                m.file_name.clone()
            } else {
                label.to_owned()
            };
            Some(
                view! { <span class="chat-media"><Icon name=icon.unwrap_or("file")/>{text}</span> }
                    .into_any(),
            )
        } else {
            None
        };
        let text_view = (!text_for_body.is_empty()).then(move || {
            view! {
                <span class="chat-text">
                    {move || render_message_text(&text_for_body, prefs.get().format_markdown, discord)}
                </span>
            }
        });
        view! { {media}{text_view} }.into_any()
    };

    let quick_react = move |emoji: &'static str| {
        if discord {
            let already = reactions.with_value(|r| r.iter().any(|r| r.emoji == emoji && r.mine));
            on_react.try_run((emoji.to_owned(), already));
        } else if mine_sv.with_value(|m| m == emoji) {
            on_react.try_run((String::new(), false));
        } else {
            on_react.try_run((emoji.to_owned(), false));
        }
    };
    let quick_actions = (!m.deleted && can_change && live).then(|| {
        view! {
            <div class="chat-quick-actions">
                <button
                    type="button"
                    class="chat-quick-btn"
                    aria-label="Quick react thumbs up"
                    title="React \u{1f44d}"
                    on:click=move |_| quick_react("\u{1f44d}")
                >
                    "\u{1f44d}"
                </button>
                <button
                    type="button"
                    class="chat-quick-btn"
                    aria-label="Quick react heart"
                    title="React \u{2764}\u{fe0f}"
                    on:click=move |_| quick_react("\u{2764}\u{fe0f}")
                >
                    "\u{2764}\u{fe0f}"
                </button>
                <button
                    type="button"
                    class="chat-quick-btn"
                    aria-label="Quick reply"
                    title="Reply"
                    on:click=move |_| { on_reply.try_run(()); }
                >
                    <Icon name="reply"/>
                </button>
            </div>
        }
    });

    view! {
        <div data-message=m.message_id.clone() class=class>
            {(discord && show_name).then(|| view! {
                <span class="chat-message-avatar"><ChatAvatar name=who.clone() group=false size="is-small"/></span>
            })}
            <div class="chat-line">
                <div class=if photo { "chat-bubble has-photo" } else { "chat-bubble" }>
                    {show_name.then(|| view! {
                        <span class=format!("chat-sender {}", avatar_hue(&m.sender))>
                            {who.clone()}
                            {discord.then(|| view! {
                                <time class="chat-sender-time" datetime=at_iso.clone()>{time_text.clone()}</time>
                            })}
                        </span>
                    })}
                    {(!m.quoted_id.is_empty()).then(|| {
                        let qid = m.quoted_id.clone();
                        view! {
                            <button type="button" class="chat-quote" on:click=move |_| { on_jump_quote.try_run(qid.clone()); }>
                                <b>{if m.quoted_sender.is_empty() { "Message".to_owned() } else { m.quoted_sender.clone() }}</b>
                                <span>{if m.quoted_text.is_empty() { "Message".to_owned() } else { m.quoted_text.clone() }}</span>
                            </button>
                        }
                    })}
                    {body}
                    {(discord && m.edited && !m.deleted).then(|| view! {
                        <small class="chat-edited">"(edited)"</small>
                    })}
                    <span class="chat-meta">
                        {(m.edited && !m.deleted && !discord).then_some("edited \u{b7} ")}
                        <time datetime=at_iso.clone() title=time_title>{time_text.clone()}</time>
                        {m.from_me.then(|| view! { <Ticks status=m.status.clone()/> })}
                    </span>
                </div>
                {quick_actions}
                <ActionMenu label="Message actions" class="chat-message-menu" actions=Callback::new(move |_| actions())>
                    <Icon name="more"/>
                </ActionMenu>
            </div>
            {reactions_view}
            {(m.status == "failed").then(|| view! {
                <span class="chat-failed">
                    <Icon name="alert"/>"Not sent \u{b7} "
                    <button type="button" class="text-button" on:click=move |_| { on_retry.try_run(()); }>"Retry"</button>
                    " \u{b7} "
                    <button type="button" class="text-button" on:click=move |_| { on_discard.try_run(()); }>"Delete"</button>
                </span>
            })}
        </div>
    }
}

/// An attachment in a bubble. Opening it downloads it from the bridge first if Dar does not have it yet.
#[component]
pub(super) fn MediaView(
    thread_id: String,
    message: ChatMessage,
    platform: String,
    sender: String,
    time_title: String,
    on_open_lightbox: Callback<LightboxItem>,
) -> impl IntoView {
    let m = message;
    let media = m.media.clone().unwrap_or_default();
    let attempt = RwSignal::new(0u32);
    let failed = RwSignal::new(false);
    let play = RwSignal::new(false);
    let base_url = media_url(&thread_id, &m.message_id);
    let url = Memo::new(move |_| {
        let a = attempt.get();
        if a > 0 {
            format!("{base_url}&attempt={a}")
        } else {
            base_url.clone()
        }
    });
    let label = kind_label(&m.kind);
    let name = local_name(&m.kind, &media.mime, &m.file_name, m.at, &platform);
    if media.state == "expired" {
        return view! {
            <span class="chat-media is-missing">
                <Icon name="alert"/>{format!("{label} is no longer on {}", platform_name(&platform))}
            </span>
        }
        .into_any();
    }
    if media.state == "retrying" {
        return view! {
            <span class="chat-media is-missing">
                <span class="chat-spinner" aria-hidden="true"></span>
                {format!("{label}: waiting for your phone to send it again")}
            </span>
        }
        .into_any();
    }
    let kind = m.kind.clone();
    let text = m.text.clone();
    let msg_sv = StoredValue::new(m);
    (move || {
        if failed.get() {
            return view! {
                <span class="chat-media is-missing">
                    <Icon name="alert"/>{format!("{label} could not be loaded")}
                    <button type="button" class="text-button" on:click=move |_| { failed.set(false); attempt.update(|n| *n += 1); }>"Retry"</button>
                </span>
            }
            .into_any();
        }
        match kind.as_str() {
            "image" | "sticker" => {
                let sticker = kind == "sticker";
                let (w, h) = fit(
                    media.width,
                    media.height,
                    if sticker { 160.0 } else { 280.0 },
                    if sticker { 160.0 } else { 320.0 },
                );
                let current_url = url.get();
                let open_url = current_url.clone();
                let open_name = name.clone();
                let open_sender = sender.clone();
                let open_time = time_title.clone();
                let open_caption = text.clone();
                view! {
                    <button
                        type="button"
                        class=if sticker { "chat-photo is-sticker" } else { "chat-photo" }
                        aria-label=format!("Open {}", label.to_lowercase())
                        on:click=move |_| {
                            on_open_lightbox.try_run(LightboxItem {
                                url: open_url.clone(),
                                download_url: format!("{open_url}&download=1"),
                                file_name: open_name.clone(),
                                sender: open_sender.clone(),
                                time: open_time.clone(),
                                caption: open_caption.clone(),
                                message: Some(msg_sv.get_value()),
                            });
                        }
                    >
                        <img
                            src=current_url
                            alt=if text.is_empty() { label.to_owned() } else { text.clone() }
                            width=w
                            height=h
                            loading="lazy"
                            decoding="async"
                            on:error=move |_| failed.set(true)
                        />
                    </button>
                }
                .into_any()
            }
            "video" => {
                if play.get() {
                    return view! {
                        <video
                            class="chat-video"
                            src=url.get()
                            controls
                            autoplay
                            playsinline
                            preload="metadata"
                            on:error=move |_| failed.set(true)
                        ></video>
                    }
                    .into_any();
                }
                let detail = [
                    if media.seconds > 0 {
                        clock(media.seconds)
                    } else {
                        String::new()
                    },
                    if media.size > 0.0 {
                        format_bytes(media.size)
                    } else {
                        String::new()
                    },
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
                view! {
                    <button type="button" class="chat-file" on:click=move |_| play.set(true)>
                        <span class="chat-file-icon"><Icon name="media"/></span>
                        <span class="chat-file-name"><b>"Play video"</b><small>{detail}</small></span>
                    </button>
                }
                .into_any()
            }
            "audio" | "voice" => view! {
                <VoiceNotePlayer
                    url=url.get()
                    seconds=media.seconds
                    label=label
                    on_error=Callback::new(move |_| failed.set(true))
                />
            }
            .into_any(),
            _ => view! {
                <a class="chat-file" href=format!("{}&download=1", url.get()) download=name.clone()>
                    <FileTile kind="file" name=name.clone() media_type=media.mime.clone()/>
                    <span class="chat-file-name"><b>{name.clone()}</b><small>{format_bytes(media.size)}</small></span>
                    <Icon name="download"/>
                </a>
            }
            .into_any(),
        }
    })
    .into_any()
}
