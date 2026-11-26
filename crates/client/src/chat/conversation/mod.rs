//! Open conversation sheet: message stream, in-chat find, unread divider, quote jump, and composer.

mod composer;
mod find;
mod format;
mod lightbox;
mod messages;
mod voice_note;

pub use format::render_message_text;
pub use lightbox::MediaLightbox;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use composer::Composer;
use find::FindBar;
use messages::Bubble;

use super::{
    BASE, ChatAccount, ChatAvatar, ChatMessage, ChatMessagePage, ChatPrefs, ChatSetting, Confirm,
    ConfirmAction, LightboxItem, RunFuture, chat_time_key, day_label, message_of,
    mute_menu_items, muted, narrow, phone_of, platform_name, search_params,
    window_clipboard_write,
};
use crate::api::{self, BrowserSession};
use crate::icons::Icon;
use crate::ui::{ActionMenu, MenuAction, document, listen, window};

#[derive(Clone, Default, PartialEq)]
struct Older {
    messages: Vec<ChatMessage>,
    before: Option<String>,
}

/// One message as the list shows it, with what decides its look.
#[derive(Clone, PartialEq, Debug)]
struct Row {
    message: ChatMessage,
    day: Option<String>,
    new_here: bool,
    tail: bool,
    show_name: bool,
    can_change: bool,
}

fn query_message_el(scroller: &web_sys::Element, message_id: &str) -> Option<web_sys::Element> {
    let escaped = js_sys::Reflect::get(&js_sys::global(), &"CSS".into())
        .ok()
        .and_then(|css| {
            js_sys::Reflect::get(&css, &"escape".into())
                .ok()
                .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
                .and_then(|f| f.call1(&css, &message_id.into()).ok())
        })
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| message_id.to_owned());
    scroller
        .query_selector(&format!("[data-message=\"{escaped}\"]"))
        .ok()
        .flatten()
}

fn scroll_to_message(el: &web_sys::Element, smooth: bool) {
    let options = web_sys::ScrollIntoViewOptions::new();
    options.set_block(web_sys::ScrollLogicalPosition::Center);
    if smooth {
        options.set_behavior(web_sys::ScrollBehavior::Smooth);
    }
    el.scroll_into_view_with_scroll_into_view_options(&options);
}

#[component]
pub fn Conversation(
    thread_id: String,
    focus_message: String,
    #[prop(into)] account: Signal<ChatAccount>,
    session: BrowserSession,
    epoch: RwSignal<u32>,
    info: RwSignal<bool>,
    #[prop(into)] prefs: Signal<ChatPrefs>,
    drafts: RwSignal<BTreeMap<String, String>>,
    on_close: Callback<()>,
    on_notice: Callback<String>,
    on_save: Callback<ChatMessage>,
    on_info: Callback<()>,
    on_setting: Callback<ChatSetting>,
    on_lightbox: Callback<LightboxItem>,
) -> impl IntoView {
    let thread_id = StoredValue::new(thread_id);
    let csrf = StoredValue::new(session.csrf_token.clone());
    let page: RwSignal<Option<ChatMessagePage>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let older = RwSignal::new(Older::default());
    let loading_older = RwSignal::new(false);
    let text = RwSignal::new(String::new());
    let reply: RwSignal<Option<ChatMessage>> = RwSignal::new(None);
    let editing: RwSignal<Option<ChatMessage>> = RwSignal::new(None);
    let sending = RwSignal::new(false);
    let asked = RwSignal::new(false);
    let confirm: RwSignal<Option<Confirm>> = RwSignal::new(None);
    let start = RwSignal::new(false);
    let new_cutoff: RwSignal<Option<String>> = RwSignal::new(None);
    let show_latest = RwSignal::new(false);
    let find_open = RwSignal::new(false);
    let find_query = RwSignal::new(String::new());
    let find_idx = RwSignal::new(0usize);
    let flashed = RwSignal::new(String::new());
    let found = RwSignal::new(String::new());
    let scroller = NodeRef::<html::Div>::new();
    let input = NodeRef::<html::Textarea>::new();
    let heading = NodeRef::<html::H2>::new();
    let stick = StoredValue::new(true);
    let first = StoredValue::new(true);
    let cutoff_checked = StoredValue::new(false);
    let found_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let flash_timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || {
            alive.store(false, Ordering::Relaxed);
            if let Some(Some(handle)) = found_timer.try_get_value() {
                handle.clear();
            }
            if let Some(Some(handle)) = flash_timer.try_get_value() {
                handle.clear();
            }
        });
    }
    let alive = StoredValue::new(alive);
    let is_alive = move || {
        alive
            .try_with_value(|a| a.load(Ordering::Relaxed))
            .unwrap_or(false)
    };

    let post = move |path: &'static str, mut body: Value| {
        let csrf = csrf.get_value();
        body["threadId"] = Value::String(thread_id.get_value());
        async move {
            api::send::<Value>(&format!("{BASE}/{path}"), "POST", Some(body), &csrf)
                .await
                .map(|_| ())
        }
    };
    let load = move || {
        let id = thread_id.get_value();
        let csrf = csrf.get_value();
        async move {
            match api::get::<ChatMessagePage>(&format!(
                "{BASE}/messages?{}",
                search_params(&[("threadId", &id)])
            ))
            .await
            {
                Ok(result) => {
                    if !is_alive() {
                        return;
                    }
                    if first.try_get_value() == Some(true) {
                        first.try_set_value(false);
                        older.try_set(Older {
                            messages: Vec::new(),
                            before: result.before.clone(),
                        });
                    }
                    if cutoff_checked.try_get_value() == Some(false) {
                        cutoff_checked.try_set_value(true);
                        let unread_count = result.thread.unread as usize;
                        if unread_count > 0 && !result.messages.is_empty() {
                            let idx = result.messages.len().saturating_sub(unread_count);
                            if let Some(m) = result.messages.get(idx) {
                                new_cutoff.try_set(Some(m.message_id.clone()));
                            }
                        }
                    }
                    let unread = result.thread.unread > 0;
                    if page.try_with_untracked(|p| p.as_ref() != Some(&result)) == Some(true) {
                        page.try_set(Some(result));
                    }
                    error.try_set(String::new());
                    if unread {
                        let _ = api::send::<Value>(
                            &format!("{BASE}/read"),
                            "POST",
                            Some(json!({ "threadId": id })),
                            &csrf,
                        )
                        .await;
                    }
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
        }
    };

    // Earlier messages from the phone arrive as history; start the pages again so they show.
    Effect::new(move |_| {
        epoch.track();
        if asked.get_untracked() {
            first.set_value(true);
        }
        spawn_local(load());
    });
    Effect::new(move |_| {
        if narrow() {
            if let Some(h) = heading.get() {
                let _ = h.focus();
            }
        } else if let Some(i) = input.get() {
            let _ = i.focus();
        }
    });

    listen(&window(), "keydown", false, move |ev| {
        let e: web_sys::KeyboardEvent = ev.unchecked_into();
        if (e.ctrl_key() || e.meta_key())
            && !e.alt_key()
            && !e.shift_key()
            && e.key().eq_ignore_ascii_case("f")
        {
            let modal_open = document()
                .query_selector("dialog[open], [data-editor-pane], .chat-lightbox")
                .ok()
                .flatten()
                .is_some();
            if !modal_open {
                e.prevent_default();
                find_open.try_set(true);
            }
        }
    });

    let keep_offset = move |height: i32| {
        request_animation_frame(move || {
            if let Some(el) = scroller.try_get_untracked().flatten() {
                el.set_scroll_top(el.scroll_top() + el.scroll_height() - height);
            }
        });
    };
    let load_older = move || {
        let Some(before) = older.with_untracked(|o| o.before.clone()) else {
            return;
        };
        if loading_older.get_untracked() {
            return;
        }
        loading_older.set(true);
        let height = scroller.get_untracked().map_or(0, |el| el.scroll_height());
        let id = thread_id.get_value();
        spawn_local(async move {
            match api::get::<ChatMessagePage>(&format!(
                "{BASE}/messages?{}",
                search_params(&[("threadId", &id), ("before", &before)])
            ))
            .await
            {
                Ok(result) => {
                    older.try_update(|o| {
                        let mut messages = result.messages;
                        messages.append(&mut o.messages);
                        *o = Older {
                            messages,
                            before: result.before,
                        };
                    });
                    keep_offset(height);
                }
                Err(cause) => {
                    on_notice.try_run(message_of(&cause));
                }
            }
            loading_older.try_set(false);
        });
    };
    let messages = Memo::new(move |_| {
        let mut seen = HashSet::new();
        let mut out: Vec<ChatMessage> = Vec::new();
        older.with(|o| {
            page.with(|p| {
                for m in o
                    .messages
                    .iter()
                    .chain(p.iter().flat_map(|p| p.messages.iter()))
                {
                    if seen.insert(m.id.clone()) {
                        out.push(m.clone());
                    }
                }
            })
        });
        out
    });

    let flash_message = move |target_id: String, smooth: bool| -> bool {
        let Some(s) = scroller.try_get_untracked().flatten() else {
            return false;
        };
        let Some(el) = query_message_el(&s, &target_id) else {
            return false;
        };
        stick.try_set_value(false);
        let gap = s.scroll_height() - s.scroll_top() - s.client_height();
        show_latest.try_set(gap > 120);
        scroll_to_message(&el, smooth);
        if let Some(Some(handle)) = flash_timer.try_get_value() {
            handle.clear();
        }
        flashed.try_set(target_id.clone());
        let clear_id = target_id;
        if let Ok(handle) = set_timeout_with_handle(
            move || {
                flashed.try_update(|cur| {
                    if *cur == clear_id {
                        cur.clear();
                    }
                });
            },
            Duration::from_millis(1200),
        ) {
            flash_timer.try_set_value(Some(handle));
        }
        true
    };

    // A search result opens the chat at its message: earlier pages load until it is found, then it is shown and outlined.
    let seek = RwSignal::new(focus_message);
    let tries = StoredValue::new(0u32);
    Effect::new(move |_| {
        let target = seek.get();
        messages.track();
        let has_before = older.with(|o| o.before.is_some());
        let busy = loading_older.get();
        if target.is_empty() || page.with(Option::is_none) {
            return;
        }
        request_animation_frame(move || {
            if seek.try_get_untracked().as_deref() != Some(target.as_str()) {
                return;
            }
            if let Some(s) = scroller.try_get_untracked().flatten()
                && let Some(el) = query_message_el(&s, &target)
            {
                stick.try_set_value(false);
                scroll_to_message(&el, false);
                found.try_set(target);
                seek.try_set(String::new());
                if let Some(Some(handle)) = found_timer.try_get_value() {
                    handle.clear();
                }
                if let Ok(handle) = set_timeout_with_handle(
                    move || {
                        found.try_set(String::new());
                    },
                    Duration::from_millis(4000),
                ) {
                    found_timer.try_set_value(Some(handle));
                }
                return;
            }
            let tried = tries.try_get_value().unwrap_or(40);
            if has_before && !busy && tried < 40 {
                tries.try_set_value(tried + 1);
                load_older();
            } else if !has_before {
                seek.try_set(String::new());
            }
        });
    });

    Effect::new(move |_| {
        messages.with(Vec::len);
        page.track();
        if stick.get_value() {
            request_animation_frame(move || {
                if let Some(el) = scroller.try_get_untracked().flatten() {
                    el.set_scroll_top(el.scroll_height());
                    show_latest.try_set(false);
                }
            });
        }
    });

    let find_matches = Memo::new(move |_| {
        let q = find_query.get().trim().to_lowercase();
        if q.is_empty() {
            return Vec::<String>::new();
        }
        messages.with(|list| {
            list.iter()
                .filter(|m| {
                    !m.deleted
                        && (m.text.to_lowercase().contains(&q)
                            || m.file_name.to_lowercase().contains(&q))
                })
                .map(|m| m.message_id.clone())
                .collect()
        })
    });
    Effect::new(move |_| {
        find_query.track();
        find_idx.set(0);
    });
    Effect::new(move |_| {
        let open = find_open.get();
        let list = find_matches.get();
        if !open || list.is_empty() {
            return;
        }
        let idx = find_idx.get_untracked().min(list.len() - 1);
        if find_idx.get_untracked() != idx {
            find_idx.set(idx);
        }
        let target = list[idx].clone();
        request_animation_frame(move || {
            let _ = flash_message(target, true);
        });
    });

    let step_find = Callback::new(move |delta: isize| {
        let list = find_matches.get_untracked();
        if list.is_empty() {
            return;
        }
        let len = list.len() as isize;
        let cur = find_idx.get_untracked() as isize;
        let next = (cur + delta).rem_euclid(len) as usize;
        find_idx.set(next);
        let target = list[next].clone();
        let _ = flash_message(target, true);
    });
    let focus_input = move || {
        if let Some(i) = input.get_untracked() {
            let _ = i.focus();
        }
    };
    let close_find = Callback::new(move |_: ()| {
        find_open.set(false);
        flashed.set(String::new());
        focus_input();
    });
    let jump_quote = Callback::new(move |qid: String| {
        if !flash_message(qid.clone(), true) {
            seek.set(qid);
        }
    });

    let thread = Memo::new(move |_| page.with(|p| p.as_ref().map(|p| p.thread.clone())));
    let name = Memo::new(move |_| {
        thread.with(|t| {
            let t = t.as_ref();
            let n = t.map(|t| t.name.clone()).unwrap_or_default();
            if !n.is_empty() {
                return n;
            }
            let p = phone_of(t.map(|t| t.jid.as_str()).unwrap_or_default());
            if p.is_empty() { "Chat".into() } else { p }
        })
    });
    let has_thread = Memo::new(move |_| thread.with(Option::is_some));
    let group = Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(|t| t.kind == "group")));
    let discord = Memo::new(move |_| account.with(|a| a.platform == "discord"));
    let channel =
        Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(|t| !t.server_id.is_empty())));
    let service = Memo::new(move |_| platform_name(&account.with(|a| a.platform.clone())));
    let shown = Memo::new(move |_| {
        if channel.get() {
            format!("#{}", name.get())
        } else {
            name.get()
        }
    });
    let can_send = Memo::new(move |_| {
        account.with(|a| {
            matches!(
                a.state.as_str(),
                "connected" | "connecting" | "disconnected"
            )
        })
    });
    // Discord: fetch what was said while Dar was away; the change feed shows it.
    if account.with_untracked(|a| a.platform == "discord" && a.state == "connected") {
        spawn_local(async move {
            let _ = post("catchUp", json!({})).await;
        });
    }
    // Discord: earlier messages come straight from Discord, then show above the ones loaded.
    let earlier = move || {
        if loading_older.get_untracked() {
            return;
        }
        loading_older.set(true);
        let height = scroller.get_untracked().map_or(0, |el| el.scroll_height());
        let oldest = messages.with_untracked(|m| m.first().cloned());
        let id = thread_id.get_value();
        let csrf = csrf.get_value();
        spawn_local(async move {
            #[derive(serde::Deserialize)]
            struct History {
                more: bool,
            }
            let result: Result<(), api::RequestError> = async {
                let result = api::send::<History>(
                    &format!("{BASE}/history"),
                    "POST",
                    Some(json!({ "threadId": id })),
                    &csrf,
                )
                .await?;
                if !result.more {
                    start.try_set(true);
                }
                if let Some(oldest) = oldest {
                    let before = format!("{id}/{}/{}", chat_time_key(oldest.at), oldest.message_id);
                    let next = api::get::<ChatMessagePage>(&format!(
                        "{BASE}/messages?{}",
                        search_params(&[("threadId", &id), ("before", &before)])
                    ))
                    .await?;
                    older.try_update(|o| {
                        let mut messages = next.messages;
                        messages.append(&mut o.messages);
                        *o = Older {
                            messages,
                            before: next.before,
                        };
                    });
                    keep_offset(height);
                } else {
                    load().await;
                }
                Ok(())
            }
            .await;
            if let Err(cause) = result {
                on_notice.try_run(message_of(&cause));
            }
            loading_older.try_set(false);
        });
    };
    let send = move || {
        let body = text.get_untracked().trim().to_owned();
        if body.is_empty() || sending.get_untracked() || !can_send.get_untracked() {
            return;
        }
        if let Some(target) = editing.get_untracked() {
            sending.set(true);
            spawn_local(async move {
                match post(
                    "edit",
                    json!({ "messageId": target.message_id, "text": body }),
                )
                .await
                {
                    Ok(()) => {
                        editing.try_set(None);
                        let id = thread_id.get_value();
                        let draft = drafts
                            .try_with_untracked(|d| d.get(&id).cloned().unwrap_or_default())
                            .unwrap_or_default();
                        text.try_set(draft);
                        load().await;
                    }
                    Err(cause) => {
                        on_notice.try_run(message_of(&cause));
                    }
                }
                sending.try_set(false);
                focus_input();
            });
            return;
        }
        // The composer clears at once and drops the thread's draft synchronously; both restore on failure.
        let quoted = reply.get_untracked();
        let id = thread_id.get_value();
        sending.set(true);
        stick.set_value(true);
        show_latest.set(false);
        text.set(String::new());
        reply.set(None);
        drafts.update(|d| {
            d.remove(&id);
        });
        spawn_local(async move {
            let mut payload = json!({ "text": body });
            if let Some(q) = &quoted {
                payload["quotedId"] = Value::String(q.message_id.clone());
            }
            match post("send", payload).await {
                Ok(()) => load().await,
                Err(cause) => {
                    text.try_update(|t| {
                        if t.is_empty() {
                            *t = body.clone();
                        }
                    });
                    drafts.try_update(|d| {
                        d.entry(id).or_insert_with(|| body.clone());
                    });
                    reply.try_update(|r| {
                        if r.is_none() {
                            *r = quoted.clone();
                        }
                    });
                    on_notice.try_run(message_of(&cause));
                }
            }
            sending.try_set(false);
            focus_input();
        });
    };
    let act = move |path: &'static str, m: ChatMessage, mut body: Value| {
        body["messageId"] = Value::String(m.message_id);
        async move {
            match post(path, body).await {
                Ok(()) => {
                    load().await;
                    Ok(())
                }
                Err(cause) => Err(message_of(&cause)),
            }
        }
    };
    let act_now = move |path: &'static str, m: ChatMessage, body: Value| {
        spawn_local(async move {
            if let Err(message) = act(path, m, body).await {
                on_notice.try_run(message);
            }
        });
    };
    let ask_phone = move || {
        spawn_local(async move {
            match post("history", json!({})).await {
                Ok(()) => {
                    asked.try_set(true);
                }
                Err(cause) => {
                    on_notice.try_run(message_of(&cause));
                }
            }
        });
    };
    let quiet = Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(muted)));
    let chat_actions = move || {
        let Some(t) = thread.get_untracked() else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if quiet.get_untracked() {
            actions.push(MenuAction::new("Unmute", "bell").run(move || {
                on_setting.try_run(ChatSetting {
                    muted_until: Some(0),
                    ..Default::default()
                });
            }));
        } else {
            actions.push(MenuAction::new("Mute", "bellOff").items(mute_menu_items(on_setting)));
        }
        if !discord.get_untracked() {
            let pinned = t.pinned;
            let archived = t.archived;
            actions.push(
                MenuAction::new(if pinned { "Unpin" } else { "Pin" }, "pin")
                    .disabled(archived)
                    .run(move || {
                        on_setting.try_run(ChatSetting {
                            pinned: Some(!pinned),
                            ..Default::default()
                        });
                    }),
            );
            actions.push(
                MenuAction::new(if archived { "Unarchive" } else { "Archive" }, "archive").run(
                    move || {
                        on_setting.try_run(ChatSetting {
                            archived: Some(!archived),
                            ..Default::default()
                        });
                    },
                ),
            );
        }
        actions
    };

    let rows = Memo::new(move |_| {
        let list = messages.get();
        let d = discord.get();
        let g = group.get();
        let cutoff = new_cutoff.get();
        let can_change = can_send.get();
        let mut last_day = String::new();
        let days: Vec<String> = list.iter().map(|m| day_label(m.at)).collect();
        list.iter()
            .enumerate()
            .map(|(i, m)| {
                let day = &days[i];
                let next = list.get(i + 1);
                let tail = next.is_none_or(|n| {
                    n.from_me != m.from_me || n.sender != m.sender || days[i + 1] != *day
                });
                let prev = i.checked_sub(1).map(|p| &list[p]);
                // Discord rows name the sender at the start of each run, as Discord does (a new run after seven minutes).
                let show_name = if d {
                    prev.is_none_or(|p| {
                        p.sender != m.sender
                            || p.from_me != m.from_me
                            || days[i - 1] != *day
                            || m.at - p.at > 7.0 * 60_000.0
                            || !m.quoted_id.is_empty()
                    })
                } else {
                    g && !m.from_me
                        && prev.is_none_or(|p| {
                            p.sender != m.sender || p.from_me || days[i - 1] != *day
                        })
                };
                let heading = (*day != last_day).then(|| {
                    last_day = day.clone();
                    day.clone()
                });
                Row {
                    message: m.clone(),
                    day: heading,
                    new_here: cutoff.as_deref() == Some(m.message_id.as_str()),
                    tail,
                    show_name,
                    can_change,
                }
            })
            .collect::<Vec<_>>()
    });

    let platform = Memo::new(move |_| account.with(|a| a.platform.clone()));
    let account_name = Memo::new(move |_| account.with(|a| a.name.clone()));
    let account_state = Memo::new(move |_| account.with(|a| a.state.clone()));
    let has_older = Memo::new(move |_| older.with(|o| o.before.is_some()));
    let show_ask_phone = Memo::new(move |_| {
        !discord.get()
            && page.with(Option::is_some)
            && older.with(|o| o.before.is_none())
            && !messages.with(Vec::is_empty)
            && can_send.get()
    });
    let show_discord_earlier = Memo::new(move |_| {
        discord.get()
            && page.with(Option::is_some)
            && older.with(|o| o.before.is_none())
            && !messages.with(Vec::is_empty)
    });
    let has_error = Memo::new(move |_| !error.with(String::is_empty));
    let show_opening =
        Memo::new(move |_| page.with(Option::is_none) && error.with(String::is_empty));
    let show_empty = Memo::new(move |_| page.with(Option::is_some) && messages.with(Vec::is_empty));
    let conv_class = move || {
        let mut c = String::from("chat-conversation");
        if discord.get() {
            c.push_str(" is-flat");
        }
        if prefs.get().is_compact() {
            c.push_str(" is-compact");
        }
        c
    };

    view! {
        <section class=conv_class>
            <header class="chat-conversation-head">
                <button type="button" class="icon-button chat-back" aria-label="Back to chats" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="back"/>
                </button>
                {move || if channel.get() {
                    let voice = thread.with(|t| t.as_ref().is_some_and(|t| t.kind == "voice"));
                    view! {
                        <span class="chat-channel-mark" aria-hidden="true">
                            <Icon name=if voice { "speaker" } else { "hash" }/>
                        </span>
                    }.into_any()
                } else {
                    view! { <ChatAvatar name=name.get() group=group.get() size="is-small"/> }.into_any()
                }}
                <div class="chat-who">
                    <h2 node_ref=heading tabindex="-1">{move || shown.get()}</h2>
                    <small>
                        {move || {
                            let base = if channel.get() {
                                thread.with(|t| t.as_ref().map(|t| t.topic.clone()).filter(|t| !t.is_empty())).unwrap_or_else(|| "Channel".into())
                            } else if discord.get() {
                                if group.get() { "Group".into() } else { "Direct message".into() }
                            } else if group.get() {
                                "Group".into()
                            } else {
                                phone_of(&thread.with(|t| t.as_ref().map(|t| t.jid.clone()).unwrap_or_default()))
                            };
                            let sep = if base.is_empty() { "" } else { " \u{b7} " };
                            view! {
                                <span>{base}</span>
                                {quiet.get().then(|| view! {
                                    <span class="chat-who-muted">{sep}<Icon name="bellOff"/><span>"Muted"</span></span>
                                })}
                            }
                        }}
                    </small>
                </div>
                <button
                    type="button"
                    class="icon-button"
                    aria-label="Find in chat"
                    title="Find in chat"
                    aria-pressed=move || find_open.get().to_string()
                    on:click=move |_| find_open.update(|o| *o = !*o)
                >
                    <Icon name="search"/>
                </button>
                <button
                    type="button"
                    class="icon-button"
                    aria-label=move || if channel.get() { "Channel info" } else { "Chat info" }
                    aria-pressed=move || info.get().to_string()
                    title=move || if channel.get() { "Channel info" } else { "Chat info" }
                    on:click=move |_| { on_info.try_run(()); }
                >
                    <Icon name="info"/>
                </button>
                {move || (can_send.get() && has_thread.get()).then(|| view! {
                    <ActionMenu label=format!("Actions for {}", shown.get()) actions=Callback::new(move |_| chat_actions())>
                        <Icon name="more"/>
                    </ActionMenu>
                })}
                <button type="button" class="icon-button chat-close" aria-label="Close chat" on:click=move |_| { on_close.try_run(()); }>
                    <Icon name="close"/>
                </button>
            </header>
            {move || find_open.get().then(|| view! {
                <FindBar
                    query=find_query
                    match_count=Signal::derive(move || find_matches.with(Vec::len))
                    current_idx=find_idx
                    on_step=step_find
                    on_close=close_find
                />
            })}
            {move || (account_state.get() == "disconnected").then(|| view! {
                <div class="chat-banner slim" role="status">
                    <Icon name="alert"/>
                    <span><b>"Reconnecting\u{2026}"</b>{format!("Messages you send wait until {} answers.", service.get())}</span>
                </div>
            })}
            {move || {
                let state = account_state.get();
                (state == "loggedOut" || state == "failed").then(|| {
                    let heading = if state == "loggedOut" {
                        if discord.get() { "Discord signed Dar out".to_owned() } else { "WhatsApp unlinked Dar".to_owned() }
                    } else {
                        format!("{} stopped", service.get())
                    };
                    view! {
                        <div class="chat-banner slim is-error" role="status">
                            <Icon name="alert"/>
                            <span><b>{heading}</b>"Your history stays here, read-only. Link again to send and receive."</span>
                        </div>
                    }
                })
            }}
            <div class="chat-messages-wrap">
            <div
                class="chat-messages"
                node_ref=scroller
                role="log"
                aria-label=move || format!("Messages in {}", shown.get())
                aria-live="polite"
                on:scroll=move |_| {
                    if let Some(el) = scroller.get_untracked() {
                        let gap = el.scroll_height() - el.scroll_top() - el.client_height();
                        stick.set_value(gap < 80);
                        show_latest.set(gap > 120);
                        if el.scroll_top() < 200 {
                            load_older();
                        }
                    }
                }
            >
                {move || has_older.get().then(|| view! {
                    <div class="chat-more">
                        <button class="text-button" disabled=move || loading_older.get() on:click=move |_| load_older()>
                            {move || if loading_older.get() { "Loading\u{2026}" } else { "Load earlier messages" }}
                        </button>
                    </div>
                })}
                {move || show_ask_phone.get().then(|| view! {
                    <div class="chat-more">
                        {move || if asked.get() {
                            view! { <span class="chat-hint">"Asked your phone; earlier messages appear here as they arrive."</span> }.into_any()
                        } else {
                            view! { <button class="text-button" on:click=move |_| ask_phone()>"Get earlier messages from your phone"</button> }.into_any()
                        }}
                    </div>
                })}
                {move || show_discord_earlier.get().then(|| view! {
                    <div class="chat-more">
                        {move || if start.get() {
                            view! { <span class="chat-hint">{format!("This is the start of {}.", shown.get())}</span> }.into_any()
                        } else if can_send.get() && account_state.get() == "connected" {
                            view! {
                                <button class="text-button" disabled=move || loading_older.get() on:click=move |_| earlier()>
                                    {move || if loading_older.get() { "Loading\u{2026}" } else { "Load earlier messages from Discord" }}
                                </button>
                            }.into_any()
                        } else {
                            ().into_any()
                        }}
                    </div>
                })}
                {move || has_error.get().then(|| view! {
                    <div class="chat-empty" role="alert">
                        <h2>"This chat could not be opened"</h2>
                        <p>{error.get()}</p>
                        <button class="secondary-button" on:click=move |_| spawn_local(load())>"Retry"</button>
                    </div>
                })}
                {move || show_opening.get().then(|| view! { <p class="chat-empty" role="status">"Opening chat\u{2026}"</p> })}
                {move || show_empty.get().then(|| view! {
                    <div class="chat-empty">
                        <h2>"No messages yet"</h2>
                        <p>{if can_send.get() { format!("Say hello. Messages go out from {}.", account_name.get()) } else { "Messages from this chat appear here.".into() }}</p>
                    </div>
                })}
                <For
                    each=move || rows.get()
                    key=|row| {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        let m = &row.message;
                        m.id.hash(&mut h);
                        m.sender.hash(&mut h);
                        m.sender_name.hash(&mut h);
                        m.from_me.hash(&mut h);
                        m.at.to_bits().hash(&mut h);
                        m.kind.hash(&mut h);
                        m.text.hash(&mut h);
                        m.file_name.hash(&mut h);
                        m.quoted_id.hash(&mut h);
                        m.quoted_sender.hash(&mut h);
                        m.quoted_text.hash(&mut h);
                        m.status.hash(&mut h);
                        m.edited.hash(&mut h);
                        m.deleted.hash(&mut h);
                        if let Some(ref media) = m.media {
                            media.kind.hash(&mut h);
                            media.mime.hash(&mut h);
                            media.size.to_bits().hash(&mut h);
                            media.seconds.hash(&mut h);
                            media.width.hash(&mut h);
                            media.height.hash(&mut h);
                            media.state.hash(&mut h);
                        }
                        m.reactions.len().hash(&mut h);
                        for r in &m.reactions {
                            r.emoji.hash(&mut h);
                            r.count.hash(&mut h);
                            r.mine.hash(&mut h);
                            r.names.hash(&mut h);
                        }
                        row.day.hash(&mut h);
                        row.new_here.hash(&mut h);
                        row.tail.hash(&mut h);
                        row.show_name.hash(&mut h);
                        row.can_change.hash(&mut h);
                        (m.message_id.clone(), h.finish())
                    }
                    let:row
                >
                    {
                        let m = row.message.clone();
                        let m_save = StoredValue::new(m.clone());
                        view! {
                            {row.day.clone().map(|day| view! { <div class="chat-day"><span>{day}</span></div> })}
                            {row.new_here.then(|| view! { <div class="chat-new-divider"><span>"New messages"</span></div> })}
                            <Bubble
                                thread_id=thread_id.get_value()
                                message=m
                                tail=row.tail
                                show_name=row.show_name
                                can_change=row.can_change
                                platform=platform.get_untracked()
                                prefs=prefs
                                found_id=found
                                flash_id=flashed
                                on_save=Callback::new(move |_| { on_save.try_run(m_save.get_value()); })
                                on_reply=Callback::new(move |_| {
                                    editing.set(None);
                                    reply.set(Some(m_save.get_value()));
                                    focus_input();
                                })
                                on_copy=Callback::new(move |_| {
                                    window_clipboard_write(&m_save.with_value(|m| m.text.clone()));
                                    on_notice.try_run("Copied".into());
                                })
                                on_react=Callback::new(move |(emoji, remove): (String, bool)| {
                                    let mut body = json!({ "emoji": emoji });
                                    if remove {
                                        body["remove"] = Value::Bool(true);
                                    }
                                    act_now("react", m_save.get_value(), body);
                                })
                                on_edit=Callback::new(move |_| {
                                    reply.set(None);
                                    let m = m_save.get_value();
                                    text.set(m.text.clone());
                                    editing.set(Some(m));
                                    focus_input();
                                })
                                on_revoke=Callback::new(move |_| {
                                    let run: Arc<dyn Fn() -> RunFuture + Send + Sync> =
                                        Arc::new(move || Box::pin(act("revoke", m_save.get_value(), json!({}))));
                                    confirm.set(Some(Confirm {
                                        title: "Delete for everyone?".into(),
                                        text: "This message is removed for everyone in the chat. People may already have seen it.".into(),
                                        action: "Delete for everyone".into(),
                                        run,
                                    }));
                                })
                                on_delete_for_me=Callback::new(move |_| {
                                    let run: Arc<dyn Fn() -> RunFuture + Send + Sync> =
                                        Arc::new(move || Box::pin(act("deleteForMe", m_save.get_value(), json!({}))));
                                    let text = if discord.get_untracked() {
                                        "This removes the message from Dar only. Discord keeps it."
                                    } else {
                                        "This removes the message from Dar only. WhatsApp and your phone keep it."
                                    };
                                    confirm.set(Some(Confirm {
                                        title: "Delete from Dar?".into(),
                                        text: text.into(),
                                        action: "Delete".into(),
                                        run,
                                    }));
                                })
                                on_retry=Callback::new(move |_| act_now("retry", m_save.get_value(), json!({})))
                                on_discard=Callback::new(move |_| act_now("discard", m_save.get_value(), json!({})))
                                on_jump_quote=jump_quote
                                on_open_lightbox=on_lightbox
                            />
                        }
                    }
                </For>
            </div>
            {move || show_latest.get().then(|| view! {
                <button
                    type="button"
                    class="chat-jump-latest"
                    aria-label="Jump to latest"
                    on:click=move |_| {
                        stick.set_value(true);
                        show_latest.set(false);
                        if let Some(el) = scroller.get_untracked() {
                            let opts = web_sys::ScrollToOptions::new();
                            opts.set_top(el.scroll_height() as f64);
                            opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                            el.scroll_to_with_scroll_to_options(&opts);
                        }
                    }
                >
                    <Icon name="arrowDown"/>
                    <span>"Latest"</span>
                </button>
            })}
            </div>
            <Composer
                thread_id=thread_id.get_value()
                text=text
                reply=reply
                editing=editing
                sending=sending
                name=name
                shown=shown
                discord=discord
                channel=channel
                can_send=can_send
                prefs=prefs
                drafts=drafts
                input=input
                on_send=Callback::new(move |_| send())
            />
            {move || confirm.get().map(|c| view! {
                <ConfirmAction confirm=c on_close=Callback::new(move |_| confirm.set(None))/>
            })}
        </section>
    }
}
