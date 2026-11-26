//! Message composer with per-thread draft preservation, emoji picker, reply/edit bar, and shortcuts.

use std::collections::BTreeMap;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::super::{
    CHAT_EMOJI_PALETTE, CHAT_LIMITS_DISCORD_TEXT, CHAT_LIMITS_TEXT, ChatMessage, ChatPrefs,
    kind_label,
};
use crate::icons::Icon;
use crate::markdown::{byte_index, utf16_len};
use crate::ui::{document, listen};

/// Conversation composer with per-thread drafts, emoji popover, reply/edit bar, and keyboard shortcuts.
#[component]
pub(super) fn Composer(
    thread_id: String,
    text: RwSignal<String>,
    reply: RwSignal<Option<ChatMessage>>,
    editing: RwSignal<Option<ChatMessage>>,
    sending: RwSignal<bool>,
    #[prop(into)] name: Signal<String>,
    #[prop(into)] shown: Signal<String>,
    #[prop(into)] discord: Signal<bool>,
    #[prop(into)] channel: Signal<bool>,
    #[prop(into)] can_send: Signal<bool>,
    #[prop(into)] prefs: Signal<ChatPrefs>,
    drafts: RwSignal<BTreeMap<String, String>>,
    input: NodeRef<html::Textarea>,
    on_send: Callback<()>,
) -> impl IntoView {
    let emoji_open = RwSignal::new(false);
    let compose_ref = NodeRef::<html::Form>::new();
    let thread_sv = StoredValue::new(thread_id);

    if editing.with_untracked(Option::is_none) {
        let initial = thread_sv.with_value(|id| {
            drafts.with_untracked(|d| d.get(id).cloned().unwrap_or_default())
        });
        text.set(initial);
    }

    Effect::new(move |_| {
        if !emoji_open.get() {
            return;
        }
        listen(&document(), "pointerdown", false, move |ev| {
            let target = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok());
            if let Some(root) = compose_ref.get_untracked()
                && !root.contains(target.as_ref())
            {
                emoji_open.set(false);
            }
        });
    });

    let restore_draft = move || {
        let draft = thread_sv.with_value(|id| {
            drafts.with_untracked(|d| d.get(id).cloned().unwrap_or_default())
        });
        text.set(draft);
    };

    let insert_emoji = move |emoji: &'static str| {
        let current = text.get_untracked();
        let (next, caret) = if let Some(el) = input.get_untracked() {
            let start = byte_index(&current, el.selection_start().ok().flatten().unwrap_or(0));
            let end = byte_index(&current, el.selection_end().ok().flatten().unwrap_or(0));
            let (s, e) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            let next = format!("{}{emoji}{}", &current[..s], &current[e..]);
            let caret = utf16_len(&current[..s]) + utf16_len(emoji);
            (next, Some(caret))
        } else {
            (format!("{current}{emoji}"), None)
        };
        text.set(next.clone());
        if editing.get_untracked().is_none() {
            let id = thread_sv.get_value();
            drafts.update(|d| {
                d.insert(id, next);
            });
        }
        emoji_open.set(false);
        if let Some(el) = input.get_untracked() {
            let _ = el.focus();
            if let Some(c) = caret {
                let _ = el.set_selection_range(c, c);
            }
        }
    };

    let keys = move |event: web_sys::KeyboardEvent| {
        if event.key() == "Enter" && !event.is_composing() {
            let should_send = if prefs.get_untracked().send_on_mod() {
                event.ctrl_key() || event.meta_key()
            } else {
                !event.shift_key()
            };
            if should_send {
                event.prevent_default();
                on_send.try_run(());
                return;
            }
        }
        if event.key() == "Escape" {
            if emoji_open.get_untracked() {
                event.prevent_default();
                event.stop_propagation();
                emoji_open.set(false);
            } else if reply.with_untracked(Option::is_some) || editing.with_untracked(Option::is_some) {
                event.prevent_default();
                reply.set(None);
                if editing.with_untracked(Option::is_some) {
                    editing.set(None);
                    restore_draft();
                }
            }
        }
    };

    view! {
        <form
            node_ref=compose_ref
            class="chat-composer"
            on:submit=move |event: web_sys::SubmitEvent| {
                event.prevent_default();
                on_send.try_run(());
            }
        >
            {move || reply.get().map(|r| {
                let who = if r.from_me {
                    "yourself".to_owned()
                } else if !r.sender_name.is_empty() {
                    r.sender_name.clone()
                } else {
                    name.get_untracked()
                };
                let body = if r.text.is_empty() {
                    kind_label(&r.kind).to_owned()
                } else {
                    r.text.clone()
                };
                view! {
                    <div class="chat-reply-bar">
                        <div><b>{format!("Replying to {who}")}</b><span>{body}</span></div>
                        <button type="button" class="icon-button" aria-label="Cancel reply" on:click=move |_| reply.set(None)>
                            <Icon name="close"/>
                        </button>
                    </div>
                }
            })}
            {move || editing.get().map(|e| {
                view! {
                    <div class="chat-reply-bar">
                        <div><b>"Editing message"</b><span>{e.text.clone()}</span></div>
                        <button
                            type="button"
                            class="icon-button"
                            aria-label="Cancel editing"
                            on:click=move |_| {
                                editing.set(None);
                                restore_draft();
                            }
                        >
                            <Icon name="close"/>
                        </button>
                    </div>
                }
            })}
            <Show when=move || emoji_open.get() && prefs.get().show_emoji && can_send.get()>
                <div class="chat-emoji-pop" role="group" aria-label="Emoji picker">
                    {CHAT_EMOJI_PALETTE.into_iter().map(|emoji| {
                        view! {
                            <button type="button" on:click=move |_| insert_emoji(emoji)>{emoji}</button>
                        }
                    }).collect_view()}
                </div>
            </Show>
            <div class="chat-compose-box">
                {move || prefs.get().show_emoji.then(|| view! {
                    <button
                        type="button"
                        class="icon-button chat-emoji-trigger"
                        aria-label="Insert emoji"
                        title="Insert emoji"
                        aria-expanded=move || emoji_open.get().to_string()
                        disabled=move || !can_send.get()
                        on:click=move |_| emoji_open.update(|o| *o = !*o)
                    >
                        <Icon name="smile"/>
                    </button>
                })}
                <textarea
                    node_ref=input
                    rows="1"
                    prop:value=move || text.get()
                    maxlength=move || if discord.get() { CHAT_LIMITS_DISCORD_TEXT } else { CHAT_LIMITS_TEXT }
                    disabled=move || !can_send.get()
                    aria-label=move || format!("Message {}", shown.get())
                    placeholder=move || {
                        if !can_send.get() {
                            "Link again to send messages".to_owned()
                        } else if discord.get() {
                            format!(
                                "Message {}",
                                if channel.get() {
                                    shown.get()
                                } else {
                                    format!("@{}", name.get())
                                }
                            )
                        } else {
                            "Message".into()
                        }
                    }
                    on:input=move |event| {
                        let value = event_target_value(&event);
                        text.set(value.clone());
                        if editing.get_untracked().is_none() {
                            let id = thread_sv.get_value();
                            drafts.update(|d| {
                                if value.is_empty() {
                                    d.remove(&id);
                                } else {
                                    d.insert(id, value);
                                }
                            });
                        }
                    }
                    on:keydown=keys
                ></textarea>
                {move || discord.get().then(|| {
                    let count = text.with(|t| t.chars().count());
                    let class = if count >= CHAT_LIMITS_DISCORD_TEXT {
                        "chat-char-count is-over"
                    } else if count >= CHAT_LIMITS_DISCORD_TEXT * 9 / 10 {
                        "chat-char-count is-near"
                    } else {
                        "chat-char-count"
                    };
                    view! {
                        <span class=class>
                            {format!("{count} / {CHAT_LIMITS_DISCORD_TEXT}")}
                        </span>
                    }
                })}
                <button
                    type="submit"
                    class="chat-send"
                    aria-label=move || if editing.with(Option::is_some) { "Save edit" } else { "Send" }
                    disabled=move || !can_send.get() || text.with(|t| t.trim().is_empty()) || sending.get()
                >
                    <Icon name=Signal::derive(move || if editing.with(Option::is_some) { "check" } else { "send" })/>
                </button>
            </div>
        </form>
    }
}
