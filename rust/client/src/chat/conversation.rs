//! The open conversation, its message bubbles and attachments, and the chat info sheet
//! (`Conversation`, `Bubble`, `MediaView`, `ChatInfoSheet` in `chat/workspace.tsx`).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::JsCast;

use super::workspace::{ChatAvatar, Confirm, ConfirmAction, RunFuture, Ticks, day_label, emoji_label, iso, kind_icon, message_of, muted, narrow, now, phone_of};
use super::{BASE, CHAT_LIMITS_DISCORD_TEXT, CHAT_LIMITS_TEXT, CHAT_MUTE_CHOICES, CHAT_QUICK_REACTIONS, ChatAccount, ChatInfo, ChatMessage, ChatMessagePage, ChatSetting, ChatThread, chat_time_key, kind_label, local_name, platform_name, search_params};
use crate::api::{self, BrowserSession, format_bytes};
use crate::dates::{local_date_time, local_time};
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::pim::avatar_hue;
use crate::ui::{ActionMenu, MenuAction};

/// WhatsApp lets senders edit a message for a short while after sending it.
const EDIT_WINDOW: f64 = 15.0 * 60_000.0;

pub(super) fn media_url(thread_id: &str, message_id: &str) -> String {
    format!("{BASE}/media?{}", search_params(&[("threadId", thread_id), ("messageId", message_id)]))
}

fn clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Display size within a box, keeping the photo's shape; sent as width/height attributes so the space is kept while it loads.
fn fit(width: u32, height: u32, max_width: f64, max_height: f64) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_width as u32, (max_width * 0.75).round() as u32);
    }
    let (w, h) = (width as f64, height as f64);
    let scale = 1f64.min(max_width / w).min(max_height / h);
    ((w * scale).round().max(80.0) as u32, (h * scale).round().max(60.0) as u32)
}

fn mute_items(on_setting: Callback<ChatSetting>) -> Vec<MenuAction> {
    CHAT_MUTE_CHOICES
        .iter()
        .map(|(label, ms)| {
            let ms = *ms;
            MenuAction::plain(*label).run(move || { on_setting.try_run(ChatSetting { muted_until: Some(if ms < 0 { -1 } else { now() as i64 + ms }), ..Default::default() }); })
        })
        .collect()
}

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
    tail: bool,
    show_name: bool,
    found: bool,
    can_change: bool,
}

#[component]
pub(super) fn Conversation(
    thread_id: String,
    focus_message: String,
    #[prop(into)] account: Signal<ChatAccount>,
    session: BrowserSession,
    epoch: RwSignal<u32>,
    info: RwSignal<bool>,
    on_close: Callback<()>,
    on_notice: Callback<String>,
    on_save: Callback<ChatMessage>,
    on_info: Callback<()>,
    on_setting: Callback<ChatSetting>,
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
    let scroller = NodeRef::<html::Div>::new();
    let input = NodeRef::<html::Textarea>::new();
    let heading = NodeRef::<html::H2>::new();
    let stick = StoredValue::new(true);
    let first = StoredValue::new(true);
    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    let alive = StoredValue::new(alive);
    let is_alive = move || alive.try_with_value(|a| a.load(Ordering::Relaxed)).unwrap_or(false);

    let post = move |path: &'static str, mut body: Value| {
        let csrf = csrf.get_value();
        body["threadId"] = Value::String(thread_id.get_value());
        async move { api::send::<Value>(&format!("{BASE}/{path}"), "POST", Some(body), &csrf).await.map(|_| ()) }
    };
    let load = move || {
        let id = thread_id.get_value();
        let csrf = csrf.get_value();
        async move {
            match api::get::<ChatMessagePage>(&format!("{BASE}/messages?{}", search_params(&[("threadId", &id)]))).await {
                Ok(result) => {
                    if !is_alive() {
                        return;
                    }
                    if first.get_value() {
                        first.set_value(false);
                        older.set(Older { messages: Vec::new(), before: result.before.clone() });
                    }
                    let unread = result.thread.unread > 0;
                    if page.with_untracked(|p| p.as_ref() != Some(&result)) {
                        page.set(Some(result));
                    }
                    error.set(String::new());
                    if unread {
                        let _ = api::send::<Value>(&format!("{BASE}/read"), "POST", Some(json!({ "threadId": id })), &csrf).await;
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
    let keep_offset = move |height: i32| {
        request_animation_frame(move || {
            if let Some(el) = scroller.try_get_untracked().flatten() {
                el.set_scroll_top(el.scroll_top() + el.scroll_height() - height);
            }
        });
    };
    let load_older = move || {
        let Some(before) = older.with_untracked(|o| o.before.clone()) else { return };
        if loading_older.get_untracked() {
            return;
        }
        loading_older.set(true);
        let height = scroller.get_untracked().map_or(0, |el| el.scroll_height());
        let id = thread_id.get_value();
        spawn_local(async move {
            match api::get::<ChatMessagePage>(&format!("{BASE}/messages?{}", search_params(&[("threadId", &id), ("before", &before)]))).await {
                Ok(result) => {
                    older.try_update(|o| {
                        let mut messages = result.messages;
                        messages.append(&mut o.messages);
                        *o = Older { messages, before: result.before };
                    });
                    keep_offset(height);
                }
                Err(cause) => { on_notice.try_run(message_of(&cause)); },
            }
            loading_older.try_set(false);
        });
    };
    let messages = Memo::new(move |_| {
        let mut seen = HashSet::new();
        let mut out: Vec<ChatMessage> = Vec::new();
        older.with(|o| page.with(|p| {
            for m in o.messages.iter().chain(p.iter().flat_map(|p| p.messages.iter())) {
                if seen.insert(m.id.clone()) {
                    out.push(m.clone());
                }
            }
        }));
        out
    });
    // A search result opens the chat at its message: earlier pages load until it is found, then it is shown and outlined.
    let seek = RwSignal::new(focus_message);
    let found = RwSignal::new(String::new());
    let tries = StoredValue::new(0u32);
    Effect::new(move |_| {
        let target = seek.get();
        messages.track();
        let has_before = older.with(|o| o.before.is_some());
        let busy = loading_older.get();
        if target.is_empty() || page.with(Option::is_none) {
            return;
        }
        // The list renders in the same pass; look for the message once it has.
        request_animation_frame(move || {
        if seek.try_get_untracked().as_deref() != Some(target.as_str()) {
            return;
        }
        let el = scroller.try_get_untracked().flatten().and_then(|s| {
            let escaped = js_sys::Reflect::get(&js_sys::global(), &"CSS".into()).ok()
                .and_then(|css| js_sys::Reflect::get(&css, &"escape".into()).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok()).and_then(|f| f.call1(&css, &target.clone().into()).ok()))
                .and_then(|v| v.as_string()).unwrap_or_else(|| target.clone());
            s.query_selector(&format!("[data-message=\"{escaped}\"]")).ok().flatten()
        });
        if let Some(el) = el {
            stick.set_value(false);
            let options = web_sys::ScrollIntoViewOptions::new();
            options.set_block(web_sys::ScrollLogicalPosition::Center);
            el.scroll_into_view_with_scroll_into_view_options(&options);
            found.set(target);
            seek.set(String::new());
            if let Ok(handle) = set_timeout_with_handle(move || { found.try_set(String::new()); }, Duration::from_millis(4000)) {
                // The outline fades even after the effect runs again for the cleared seek.
                let _ = handle;
            }
            return;
        }
        let tried = tries.get_value();
        if has_before && !busy && tried < 40 {
            tries.set_value(tried + 1);
            load_older();
        } else if !has_before {
            seek.set(String::new());
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
                }
            });
        }
    });

    let thread = Memo::new(move |_| page.with(|p| p.as_ref().map(|p| p.thread.clone())));
    let name = Memo::new(move |_| thread.with(|t| {
        let t = t.as_ref();
        let n = t.map(|t| t.name.clone()).unwrap_or_default();
        if !n.is_empty() { return n; }
        let p = phone_of(t.map(|t| t.jid.as_str()).unwrap_or_default());
        if p.is_empty() { "Chat".into() } else { p }
    }));
    let has_thread = Memo::new(move |_| thread.with(Option::is_some));
    let group = Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(|t| t.kind == "group")));
    let discord = Memo::new(move |_| account.with(|a| a.platform == "discord"));
    let channel = Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(|t| !t.server_id.is_empty())));
    let service = Memo::new(move |_| platform_name(&account.with(|a| a.platform.clone())));
    let shown = Memo::new(move |_| if channel.get() { format!("#{}", name.get()) } else { name.get() });
    let can_send = Memo::new(move |_| account.with(|a| matches!(a.state.as_str(), "connected" | "connecting" | "disconnected")));
    // Discord: fetch what was said while Dar was away; the change feed shows it.
    if account.with_untracked(|a| a.platform == "discord" && a.state == "connected") {
        spawn_local(async move { let _ = post("catchUp", json!({})).await; });
    }
    /* Discord: earlier messages come straight from Discord, then show above the ones loaded. */
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
                let result = api::send::<History>(&format!("{BASE}/history"), "POST", Some(json!({ "threadId": id })), &csrf).await?;
                if !result.more {
                    start.try_set(true);
                }
                if let Some(oldest) = oldest {
                    let before = format!("{id}/{}/{}", chat_time_key(oldest.at), oldest.message_id);
                    let next = api::get::<ChatMessagePage>(&format!("{BASE}/messages?{}", search_params(&[("threadId", &id), ("before", &before)]))).await?;
                    older.try_update(|o| {
                        let mut messages = next.messages;
                        messages.append(&mut o.messages);
                        *o = Older { messages, before: next.before };
                    });
                    keep_offset(height);
                } else {
                    load().await;
                }
                Ok(())
            }
            .await;
            if let Err(cause) = result {
                { on_notice.try_run(message_of(&cause)); };
            }
            loading_older.try_set(false);
        });
    };
    let focus_input = move || {
        if let Some(i) = input.get_untracked() {
            let _ = i.focus();
        }
    };
    let send = move || {
        let body = text.get_untracked().trim().to_owned();
        if body.is_empty() || sending.get_untracked() || !can_send.get_untracked() {
            return;
        }
        if let Some(target) = editing.get_untracked() {
            sending.set(true);
            spawn_local(async move {
                match post("edit", json!({ "messageId": target.message_id, "text": body })).await {
                    Ok(()) => {
                        editing.try_set(None);
                        text.try_set(String::new());
                        load().await;
                    }
                    Err(cause) => { on_notice.try_run(message_of(&cause)); },
                }
                sending.try_set(false);
                focus_input();
            });
            return;
        }
        // The composer clears at once; the text comes back if Dar could not store the message.
        let quoted = reply.get_untracked();
        sending.set(true);
        stick.set_value(true);
        text.set(String::new());
        reply.set(None);
        spawn_local(async move {
            let mut payload = json!({ "text": body });
            if let Some(q) = &quoted {
                payload["quotedId"] = Value::String(q.message_id.clone());
            }
            match post("send", payload).await {
                Ok(()) => load().await,
                Err(cause) => {
                    text.try_update(|t| if t.is_empty() { *t = body.clone() });
                    reply.try_update(|r| if r.is_none() { *r = quoted.clone() });
                    { on_notice.try_run(message_of(&cause)); };
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
                { on_notice.try_run(message); };
            }
        });
    };
    let ask_phone = move || {
        spawn_local(async move {
            match post("history", json!({})).await {
                Ok(()) => {
                    asked.try_set(true);
                }
                Err(cause) => { on_notice.try_run(message_of(&cause)); },
            }
        });
    };
    let quiet = Memo::new(move |_| thread.with(|t| t.as_ref().is_some_and(muted)));
    let chat_actions = move || {
        let Some(t) = thread.get_untracked() else { return Vec::new() };
        let mut actions = Vec::new();
        if quiet.get_untracked() {
            actions.push(MenuAction::new("Unmute", "bell").run(move || { on_setting.try_run(ChatSetting { muted_until: Some(0), ..Default::default() }); }));
        } else {
            actions.push(MenuAction::new("Mute", "bell").items(mute_items(on_setting)));
        }
        if !discord.get_untracked() {
            let pinned = t.pinned;
            let archived = t.archived;
            actions.push(MenuAction::new(if pinned { "Unpin" } else { "Pin" }, "pin").disabled(archived).run(move || { on_setting.try_run(ChatSetting { pinned: Some(!pinned), ..Default::default() }); }));
            actions.push(MenuAction::new(if archived { "Unarchive" } else { "Archive" }, "archive").run(move || { on_setting.try_run(ChatSetting { archived: Some(!archived), ..Default::default() }); }));
        }
        actions
    };

    let rows = Memo::new(move |_| {
        let list = messages.get();
        let d = discord.get();
        let g = group.get();
        let found = found.get();
        let can_change = can_send.get();
        let mut last_day = String::new();
        let days: Vec<String> = list.iter().map(|m| day_label(m.at)).collect();
        list.iter()
            .enumerate()
            .map(|(i, m)| {
                let day = &days[i];
                let next = list.get(i + 1);
                let tail = next.is_none_or(|n| n.from_me != m.from_me || n.sender != m.sender || days[i + 1] != *day);
                let prev = i.checked_sub(1).map(|p| &list[p]);
                // Discord rows name the sender at the start of each run, as Discord does (a new run after seven minutes).
                let show_name = if d {
                    prev.is_none_or(|p| p.sender != m.sender || p.from_me != m.from_me || days[i - 1] != *day || m.at - p.at > 7.0 * 60_000.0 || !m.quoted_id.is_empty())
                } else {
                    g && !m.from_me && prev.is_none_or(|p| p.sender != m.sender || p.from_me || days[i - 1] != *day)
                };
                let heading = (*day != last_day).then(|| {
                    last_day = day.clone();
                    day.clone()
                });
                Row { message: m.clone(), day: heading, tail, show_name, found: !found.is_empty() && m.message_id == found, can_change }
            })
            .collect::<Vec<_>>()
    });

    let keys = move |event: web_sys::KeyboardEvent| {
        if event.key() == "Enter" && !event.shift_key() && !event.is_composing() {
            event.prevent_default();
            send();
        }
        if event.key() == "Escape" && (reply.with_untracked(Option::is_some) || editing.with_untracked(Option::is_some)) {
            event.prevent_default();
            reply.set(None);
            if editing.with_untracked(Option::is_some) {
                editing.set(None);
                text.set(String::new());
            }
        }
    };
    let platform = Memo::new(move |_| account.with(|a| a.platform.clone()));
    let account_name = Memo::new(move |_| account.with(|a| a.name.clone()));
    let account_state = Memo::new(move |_| account.with(|a| a.state.clone()));

    view! {
        <section class=move || if discord.get() { "chat-conversation is-flat" } else { "chat-conversation" }>
            <header class="chat-conversation-head">
                <button type="button" class="icon-button chat-back" aria-label="Back to chats" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
                {move || if channel.get() {
                    let voice = thread.with(|t| t.as_ref().is_some_and(|t| t.kind == "voice"));
                    view! { <span class="chat-channel-mark" aria-hidden="true"><Icon name=if voice { "speaker" } else { "hash" }/></span> }.into_any()
                } else {
                    view! { <ChatAvatar name=name.get() group=group.get() size="is-small"/> }.into_any()
                }}
                <div class="chat-who">
                    <h2 node_ref=heading tabindex="-1">{move || shown.get()}</h2>
                    <small>{move || {
                        let base = if channel.get() {
                            thread.with(|t| t.as_ref().map(|t| t.topic.clone()).filter(|t| !t.is_empty())).unwrap_or_else(|| "Channel".into())
                        } else if discord.get() {
                            if group.get() { "Group".into() } else { "Direct message".into() }
                        } else if group.get() {
                            "Group".into()
                        } else {
                            phone_of(&thread.with(|t| t.as_ref().map(|t| t.jid.clone()).unwrap_or_default()))
                        };
                        format!("{base}{}", if quiet.get() { " \u{b7} Muted" } else { "" })
                    }}</small>
                </div>
                <button type="button" class="icon-button" aria-label=move || if channel.get() { "Channel info" } else { "Chat info" } aria-pressed=move || info.get().to_string() title=move || if channel.get() { "Channel info" } else { "Chat info" } on:click=move |_| { on_info.try_run(()); }><Icon name="info"/></button>
                {move || (can_send.get() && has_thread.get()).then(|| view! {
                    <ActionMenu label=format!("Actions for {}", shown.get()) actions=Callback::new(move |_| chat_actions())><Icon name="more"/></ActionMenu>
                })}
                <button type="button" class="icon-button chat-close" aria-label="Close chat" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
            </header>
            {move || (account_state.get() == "disconnected").then(|| view! {
                <div class="chat-banner slim" role="status"><Icon name="alert"/><span><b>"Reconnecting\u{2026}"</b>{format!("Messages you send wait until {} answers.", service.get())}</span></div>
            })}
            {move || {
                let state = account_state.get();
                (state == "loggedOut" || state == "failed").then(|| {
                    let heading = if state == "loggedOut" {
                        if discord.get() { "Discord signed Dar out".to_owned() } else { "WhatsApp unlinked Dar".to_owned() }
                    } else {
                        format!("{} stopped", service.get())
                    };
                    view! { <div class="chat-banner slim is-error" role="status"><Icon name="alert"/><span><b>{heading}</b>"Your history stays here, read-only. Link again to send and receive."</span></div> }
                })
            }}
            <div class="chat-messages" node_ref=scroller role="log" aria-label=move || format!("Messages in {}", shown.get()) aria-live="polite"
                on:scroll=move |_| {
                    if let Some(el) = scroller.get_untracked() {
                        stick.set_value(el.scroll_height() - el.scroll_top() - el.client_height() < 80);
                        if el.scroll_top() < 200 {
                            load_older();
                        }
                    }
                }>
                {move || older.with(|o| o.before.is_some()).then(|| view! {
                    <div class="chat-more"><button class="text-button" disabled=move || loading_older.get() on:click=move |_| load_older()>{move || if loading_older.get() { "Loading\u{2026}" } else { "Load earlier messages" }}</button></div>
                })}
                {move || (!discord.get() && page.with(Option::is_some) && older.with(|o| o.before.is_none()) && !messages.with(Vec::is_empty) && can_send.get()).then(|| view! {
                    <div class="chat-more">{move || if asked.get() {
                        view! { <span class="chat-hint">"Asked your phone; earlier messages appear here as they arrive."</span> }.into_any()
                    } else {
                        view! { <button class="text-button" on:click=move |_| ask_phone()>"Get earlier messages from your phone"</button> }.into_any()
                    }}</div>
                })}
                {move || (discord.get() && page.with(Option::is_some) && older.with(|o| o.before.is_none()) && !messages.with(Vec::is_empty)).then(|| view! {
                    <div class="chat-more">{move || if start.get() {
                        view! { <span class="chat-hint">{format!("This is the start of {}.", shown.get())}</span> }.into_any()
                    } else if can_send.get() && account_state.get() == "connected" {
                        view! { <button class="text-button" disabled=move || loading_older.get() on:click=move |_| earlier()>{move || if loading_older.get() { "Loading\u{2026}" } else { "Load earlier messages from Discord" }}</button> }.into_any()
                    } else {
                        ().into_any()
                    }}</div>
                })}
                {move || (!error.with(String::is_empty)).then(|| view! {
                    <div class="chat-empty" role="alert"><h2>"This chat could not be opened"</h2><p>{error.get()}</p><button class="secondary-button" on:click=move |_| spawn_local(load())>"Retry"</button></div>
                })}
                {move || (page.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p class="chat-empty" role="status">"Opening chat\u{2026}"</p> })}
                {move || (page.with(Option::is_some) && messages.with(Vec::is_empty)).then(|| view! {
                    <div class="chat-empty"><h2>"No messages yet"</h2><p>{if can_send.get() { format!("Say hello. Messages go out from {}.", account_name.get()) } else { "Messages from this chat appear here.".into() }}</p></div>
                })}
                <For each=move || rows.get() key=|row| format!("{row:?}") let:row>
                    {
                        let m = row.message.clone();
                        let m_save = StoredValue::new(m.clone());
                        view! {
                            {row.day.clone().map(|day| view! { <div class="chat-day"><span>{day}</span></div> })}
                            <Bubble thread_id=thread_id.get_value() message=m found=row.found tail=row.tail show_name=row.show_name can_change=row.can_change platform=platform.get_untracked()
                                on_save=Callback::new(move |_| { on_save.try_run(m_save.get_value()); })
                                on_reply=Callback::new(move |_| { editing.set(None); reply.set(Some(m_save.get_value())); focus_input(); })
                                on_copy=Callback::new(move |_| {
                                    let _ = window_clipboard_write(&m_save.with_value(|m| m.text.clone()));
                                    { on_notice.try_run("Copied".into()); };
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
                                    let run: Arc<dyn Fn() -> RunFuture + Send + Sync> = Arc::new(move || Box::pin(act("revoke", m_save.get_value(), json!({}))));
                                    confirm.set(Some(Confirm { title: "Delete for everyone?".into(), text: "This message is removed for everyone in the chat. People may already have seen it.".into(), action: "Delete for everyone".into(), run }));
                                })
                                on_delete_for_me=Callback::new(move |_| {
                                    let run: Arc<dyn Fn() -> RunFuture + Send + Sync> = Arc::new(move || Box::pin(act("deleteForMe", m_save.get_value(), json!({}))));
                                    let text = if discord.get_untracked() { "This removes the message from Dar only. Discord keeps it." } else { "This removes the message from Dar only. WhatsApp and your phone keep it." };
                                    confirm.set(Some(Confirm { title: "Delete from Dar?".into(), text: text.into(), action: "Delete".into(), run }));
                                })
                                on_retry=Callback::new(move |_| act_now("retry", m_save.get_value(), json!({})))
                                on_discard=Callback::new(move |_| act_now("discard", m_save.get_value(), json!({})))/>
                        }
                    }
                </For>
            </div>
            <form class="chat-composer" on:submit=move |event: web_sys::SubmitEvent| { event.prevent_default(); send(); }>
                {move || reply.get().map(|r| {
                    let who = if r.from_me { "yourself".to_owned() } else if !r.sender_name.is_empty() { r.sender_name.clone() } else { name.get_untracked() };
                    let body = if r.text.is_empty() { kind_label(&r.kind).to_owned() } else { r.text.clone() };
                    view! {
                        <div class="chat-reply-bar"><div><b>{format!("Replying to {who}")}</b><span>{body}</span></div>
                            <button type="button" class="icon-button" aria-label="Cancel reply" on:click=move |_| reply.set(None)><Icon name="close"/></button></div>
                    }
                })}
                {move || editing.get().map(|e| view! {
                    <div class="chat-reply-bar"><div><b>"Editing message"</b><span>{e.text.clone()}</span></div>
                        <button type="button" class="icon-button" aria-label="Cancel editing" on:click=move |_| { editing.set(None); text.set(String::new()); }><Icon name="close"/></button></div>
                })}
                <div class="chat-compose-box">
                    <textarea node_ref=input rows="1" prop:value=move || text.get() maxlength=move || if discord.get() { CHAT_LIMITS_DISCORD_TEXT } else { CHAT_LIMITS_TEXT } disabled=move || !can_send.get()
                        aria-label=move || format!("Message {}", shown.get())
                        placeholder=move || if !can_send.get() { "Link again to send messages".to_owned() } else if discord.get() { format!("Message {}", if channel.get() { shown.get() } else { format!("@{}", name.get()) }) } else { "Message".into() }
                        on:input=move |event| text.set(event_target_value(&event)) on:keydown=keys></textarea>
                    <button type="submit" class="chat-send" aria-label=move || if editing.with(Option::is_some) { "Save edit" } else { "Send" } disabled=move || !can_send.get() || text.with(|t| t.trim().is_empty()) || sending.get()>
                        {move || view! { <Icon name=if editing.with(Option::is_some) { "check" } else { "send" }/> }}
                    </button>
                </div>
            </form>
            {move || confirm.get().map(|c| view! { <ConfirmAction confirm=c on_close=Callback::new(move |_| confirm.set(None))/> })}
        </section>
    }
}

fn window_clipboard_write(text: &str) -> Option<()> {
    let clipboard = crate::ui::window().navigator().clipboard();
    let _ = clipboard.write_text(text);
    Some(())
}

#[component]
fn Bubble(
    thread_id: String,
    message: ChatMessage,
    found: bool,
    tail: bool,
    show_name: bool,
    can_change: bool,
    platform: String,
    on_save: Callback<()>,
    on_reply: Callback<()>,
    on_copy: Callback<()>,
    on_react: Callback<(String, bool)>,
    on_edit: Callback<()>,
    on_revoke: Callback<()>,
    on_delete_for_me: Callback<()>,
    on_retry: Callback<()>,
    on_discard: Callback<()>,
) -> impl IntoView {
    let m = message;
    let label = if m.kind != "text" { kind_label(&m.kind) } else { "" };
    let icon = kind_icon(&m.kind);
    let discord = platform == "discord";
    let file = if m.media.as_ref().is_some_and(|media| media.state != "expired") { media_url(&thread_id, &m.message_id) } else { String::new() };
    let mine = m.reactions.iter().find(|r| r.mine).map(|r| r.emoji.clone()).unwrap_or_default();
    let live = m.status != "pending" && m.status != "failed";
    // WhatsApp keeps one reaction per person; Discord allows several, each toggled on its own.
    let reactions = StoredValue::new(m.reactions.clone());
    let actions = {
        let (file, mine, deleted, text, from_me, kind, at) = (file.clone(), mine.clone(), m.deleted, m.text.clone(), m.from_me, m.kind.clone(), m.at);
        move || -> Vec<MenuAction> {
            if deleted {
                return vec![MenuAction::new("Delete from Dar\u{2026}", "trash").danger().run(move || { on_delete_for_me.try_run(()); })];
            }
            let mut actions = Vec::new();
            if can_change && live {
                let react_items: Vec<MenuAction> = if discord {
                    CHAT_QUICK_REACTIONS.iter().map(|emoji| {
                        let emoji = (*emoji).to_owned();
                        let on = reactions.with_value(|r| r.iter().any(|r| r.emoji == emoji && r.mine));
                        MenuAction::plain(emoji.clone()).current(on).run(move || { on_react.try_run((emoji.clone(), on)); })
                    }).collect()
                } else {
                    let mut items: Vec<MenuAction> = CHAT_QUICK_REACTIONS.iter().map(|emoji| {
                        let emoji = (*emoji).to_owned();
                        let is_mine = emoji == mine;
                        MenuAction::plain(emoji.clone()).current(is_mine).run(move || { on_react.try_run((if is_mine { String::new() } else { emoji.clone() }, false)); })
                    }).collect();
                    if !mine.is_empty() {
                        items.push(MenuAction::new("Remove reaction", "close").separator().run(move || { on_react.try_run((String::new(), false)); }));
                    }
                    items
                };
                actions.push(MenuAction::new("React", "star").items(react_items));
                actions.push(MenuAction::new("Reply", "reply").run(move || { on_reply.try_run(()); }));
            }
            if !text.is_empty() {
                actions.push(MenuAction::new("Copy text", "copy").run(move || { on_copy.try_run(()); }));
            }
            if can_change && live && from_me && kind == "text" && (discord || now() - at < EDIT_WINDOW) {
                actions.push(MenuAction::new("Edit", "pencil").run(move || { on_edit.try_run(()); }));
            }
            if !file.is_empty() {
                actions.push(MenuAction::new("Download", "download").href(format!("{file}&download=1")).download().separator());
                actions.push(MenuAction::new("Save to Files\u{2026}", "folder").run(move || { on_save.try_run(()); }));
            }
            let revoke = can_change && live && from_me;
            if revoke {
                actions.push(MenuAction::new("Delete for everyone\u{2026}", "trash").danger().separator().run(move || { on_revoke.try_run(()); }));
            }
            let mut delete = MenuAction::new("Delete from Dar\u{2026}", "trash").danger();
            if !revoke {
                delete = delete.separator();
            }
            actions.push(delete.run(move || { on_delete_for_me.try_run(()); }));
            actions
        }
    };
    let photo = !m.deleted && m.media.as_ref().is_some_and(|media| (m.kind == "image" || m.kind == "sticker") && media.state != "expired" && media.state != "retrying");
    let who = if m.from_me && discord {
        "You".to_owned()
    } else if !m.sender_name.is_empty() {
        m.sender_name.clone()
    } else {
        Some(phone_of(&m.sender)).filter(|p| !p.is_empty()).unwrap_or_else(|| "Unknown".into())
    };
    let class = format!(
        "chat-message {}{}{}{}{}",
        if m.from_me { "is-out" } else { "is-in" },
        if tail { " is-tail" } else { "" },
        if show_name { " is-first" } else { "" },
        if m.status == "failed" { " is-failed" } else { "" },
        if found { " is-found" } else { "" }
    );
    let time_text = local_time(m.at, &[("hour", "numeric"), ("minute", "2-digit")]);
    let time_title = local_date_time(m.at, &[("dateStyle", "medium"), ("timeStyle", "short")]);
    let at_iso = iso(m.at);
    let reactions_view = (!m.reactions.is_empty()).then(|| {
        let mine_any = mine.clone();
        let _ = mine_any;
        view! {
            <div class="chat-reactions">
                {m.reactions.iter().map(|r| {
                    let who = if discord {
                        if r.mine {
                            if r.count > 1 { format!("You and {} other{}", r.count - 1, if r.count > 2 { "s" } else { "" }) } else { "You".into() }
                        } else {
                            String::new()
                        }
                    } else {
                        let names = if r.mine { std::iter::once("You".to_owned()).chain(r.names.iter().cloned()).collect::<Vec<_>>().join(", ") } else { r.names.join(", ") };
                        let others = r.count as usize > r.names.len() + usize::from(r.mine);
                        format!("{names}{}", if others { " and others" } else { "" })
                    };
                    let shown = emoji_label(&r.emoji);
                    let count = r.count;
                    let class = if r.mine { "is-mine" } else { "" };
                    if can_change && live {
                        let (emoji, is_mine) = (r.emoji.clone(), r.mine);
                        let aria = format!("{shown} {count}{}. {}", if who.is_empty() { String::new() } else { format!(", {who}") }, if is_mine { "Remove your reaction".to_owned() } else { format!("React with {shown}") });
                        view! {
                            <button type="button" class=class aria-pressed=is_mine.to_string() title=who aria-label=aria on:click=move |_| {
                                if discord { { on_react.try_run((emoji.clone(), is_mine)); } } else { { on_react.try_run((if is_mine { String::new() } else { emoji.clone() }, false)); } }
                            }>{shown.clone()}{(count > 1).then(|| view! { <small>{count}</small> })}</button>
                        }.into_any()
                    } else {
                        view! { <span class=class title=who>{shown.clone()}{(count > 1).then(|| view! { <small>{count}</small> })}</span> }.into_any()
                    }
                }).collect_view()}
            </div>
        }
    });
    let body = if m.deleted {
        view! { <span class="chat-text is-deleted"><Icon name="close"/>"This message was deleted"</span> }.into_any()
    } else {
        let media = if m.media.is_some() {
            Some(view! { <MediaView thread_id=thread_id.clone() message=m.clone() platform=platform.clone()/> }.into_any())
        } else if !label.is_empty() {
            let text = if m.kind == "document" && !m.file_name.is_empty() { m.file_name.clone() } else { label.to_owned() };
            Some(view! { <span class="chat-media"><Icon name=icon.unwrap_or("file")/>{text}</span> }.into_any())
        } else {
            None
        };
        view! { {media}{(!m.text.is_empty()).then(|| view! { <span class="chat-text">{m.text.clone()}</span> })} }.into_any()
    };
    view! {
        <div data-message=m.message_id.clone() class=class>
            {(discord && show_name).then(|| view! { <span class="chat-message-avatar"><ChatAvatar name=who.clone() group=false size="is-small"/></span> })}
            <div class="chat-line">
                <div class=if photo { "chat-bubble has-photo" } else { "chat-bubble" }>
                    {show_name.then(|| view! {
                        <span class=format!("chat-sender {}", avatar_hue(&m.sender))>{who.clone()}{discord.then(|| view! { <time class="chat-sender-time" datetime=at_iso.clone()>{time_text.clone()}</time> })}</span>
                    })}
                    {(!m.quoted_id.is_empty()).then(|| view! {
                        <span class="chat-quote"><b>{if m.quoted_sender.is_empty() { "Message".to_owned() } else { m.quoted_sender.clone() }}</b><span>{if m.quoted_text.is_empty() { "Message".to_owned() } else { m.quoted_text.clone() }}</span></span>
                    })}
                    {body}
                    {(discord && m.edited && !m.deleted).then(|| view! { <small class="chat-edited">"(edited)"</small> })}
                    <span class="chat-meta">{(m.edited && !m.deleted && !discord).then_some("edited \u{b7} ")}<time datetime=at_iso.clone() title=time_title>{time_text.clone()}</time>{m.from_me.then(|| view! { <Ticks status=m.status.clone()/> })}</span>
                </div>
                <ActionMenu label="Message actions" class="chat-message-menu" actions=Callback::new(move |_| actions())><Icon name="more"/></ActionMenu>
            </div>
            {reactions_view}
            {(m.status == "failed").then(|| view! {
                <span class="chat-failed"><Icon name="alert"/>"Not sent \u{b7} "<button type="button" class="text-button" on:click=move |_| { on_retry.try_run(()); }>"Retry"</button>" \u{b7} "<button type="button" class="text-button" on:click=move |_| { on_discard.try_run(()); }>"Delete"</button></span>
            })}
        </div>
    }
}

/// An attachment in a bubble. Opening it downloads it from WhatsApp first if Dar does not have it yet.
#[component]
fn MediaView(thread_id: String, message: ChatMessage, platform: String) -> impl IntoView {
    let m = message;
    let media = m.media.clone().unwrap_or_default();
    let attempt = RwSignal::new(0u32);
    let failed = RwSignal::new(false);
    let play = RwSignal::new(false);
    let base_url = media_url(&thread_id, &m.message_id);
    let url = Memo::new(move |_| {
        let a = attempt.get();
        if a > 0 { format!("{base_url}&attempt={a}") } else { base_url.clone() }
    });
    let label = kind_label(&m.kind);
    let name = local_name(&m.kind, &media.mime, &m.file_name, m.at, &platform);
    if media.state == "expired" {
        return view! { <span class="chat-media is-missing"><Icon name="alert"/>{format!("{label} is no longer on {}", platform_name(&platform))}</span> }.into_any();
    }
    if media.state == "retrying" {
        return view! { <span class="chat-media is-missing"><span class="chat-spinner" aria-hidden="true"></span>{format!("{label}: waiting for your phone to send it again")}</span> }.into_any();
    }
    let kind = m.kind.clone();
    let text = m.text.clone();
    (move || {
        if failed.get() {
            return view! {
                <span class="chat-media is-missing"><Icon name="alert"/>{format!("{label} could not be loaded")}<button type="button" class="text-button" on:click=move |_| { failed.set(false); attempt.update(|n| *n += 1); }>"Retry"</button></span>
            }.into_any();
        }
        match kind.as_str() {
            "image" | "sticker" => {
                let sticker = kind == "sticker";
                let (w, h) = fit(media.width, media.height, if sticker { 160.0 } else { 280.0 }, if sticker { 160.0 } else { 320.0 });
                view! {
                    <a class=if sticker { "chat-photo is-sticker" } else { "chat-photo" } href=url.get() target="_blank" rel="noopener" aria-label=format!("Open {}", label.to_lowercase())>
                        <img src=url.get() alt=if text.is_empty() { label.to_owned() } else { text.clone() } width=w height=h loading="lazy" decoding="async" on:error=move |_| failed.set(true)/>
                    </a>
                }.into_any()
            }
            "video" => {
                if play.get() {
                    return view! { <video class="chat-video" src=url.get() controls autoplay playsinline preload="metadata" on:error=move |_| failed.set(true)></video> }.into_any();
                }
                let detail = [if media.seconds > 0 { clock(media.seconds) } else { String::new() }, if media.size > 0.0 { format_bytes(media.size) } else { String::new() }]
                    .into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
                view! {
                    <button type="button" class="chat-file" on:click=move |_| play.set(true)><span class="chat-file-icon"><Icon name="media"/></span>
                        <span class="chat-file-name"><b>"Play video"</b><small>{detail}</small></span></button>
                }.into_any()
            }
            "audio" | "voice" => view! {
                <span class="chat-audio"><audio controls preload="none" src=url.get() aria-label=label on:error=move |_| failed.set(true)></audio>{(media.seconds > 0).then(|| view! { <small>{clock(media.seconds)}</small> })}</span>
            }.into_any(),
            _ => view! {
                <a class="chat-file" href=format!("{}&download=1", url.get()) download=name.clone()><FileTile kind="file" name=name.clone() media_type=media.mime.clone()/>
                    <span class="chat-file-name"><b>{name.clone()}</b><small>{format_bytes(media.size)}</small></span><Icon name="download"/></a>
            }.into_any(),
        }
    })
    .into_any()
}

/// Chat info (mock A2): who it is, mute/pin/archive, attachments and, for groups, members.
#[component]
pub(super) fn ChatInfoSheet(
    thread_id: String,
    session: BrowserSession,
    epoch: RwSignal<u32>,
    on_close: Callback<()>,
    on_setting: Callback<ChatSetting>,
    on_add_contact: Callback<ChatThread>,
) -> impl IntoView {
    let _ = session;
    let info: RwSignal<Option<ChatInfo>> = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let thread_key = StoredValue::new(thread_id.clone());
    Effect::new(move |_| {
        epoch.track();
        let gone = Arc::new(AtomicBool::new(false));
        {
            let gone = gone.clone();
            on_cleanup(move || gone.store(true, Ordering::Relaxed));
        }
        let id = thread_key.get_value();
        spawn_local(async move {
            let result = api::get::<ChatInfo>(&format!("{BASE}/info?{}", search_params(&[("threadId", &id)]))).await;
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
                <button type="button" class="icon-button chat-back" aria-label="Back to chat" on:click=move |_| { on_close.try_run(()); }><Icon name="back"/></button>
                <h2 class="chat-info-title">{title}</h2>
                <button type="button" class="icon-button chat-close" aria-label="Close chat info" on:click=move |_| { on_close.try_run(()); }><Icon name="close"/></button>
            </header>
            <div class="chat-info-body">
                {move || (!error.with(String::is_empty)).then(|| view! { <p class="error" role="alert">{error.get()}</p> })}
                {move || (info.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p class="chat-empty" role="status">"Loading\u{2026}"</p> })}
                {move || info.get().map(|info| {
                    let thread = info.thread.clone();
                    let name = if !thread.name.is_empty() { thread.name.clone() } else { Some(phone_of(&thread.jid)).filter(|p| !p.is_empty()).unwrap_or_else(|| "Chat".into()) };
                    let quiet = muted(&thread);
                    let group = thread.kind == "group";
                    let pictures: Vec<_> = info.media.iter().filter(|m| (m.kind == "image" || m.kind == "sticker") && m.state == "stored").take(9).cloned().collect();
                    let files: Vec<_> = info.media.iter().filter(|m| m.kind == "document").take(6).cloned().collect();
                    let linked = matches!(info.account.state.as_str(), "connected" | "connecting" | "disconnected");
                    let discord = info.account.platform == "discord";
                    let channel = !thread.server_id.is_empty();
                    let platform = info.account.platform.clone();
                    let subtitle = if channel {
                        format!("{}{}", info.server.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "Server".into()), if thread.section.is_empty() { String::new() } else { format!(" \u{b7} {}", thread.section) })
                    } else if group {
                        format!("Group{}", info.group.as_ref().map(|g| format!(" \u{b7} {} members", g.members.len())).unwrap_or_default())
                    } else if discord {
                        "Direct message".into()
                    } else {
                        phone_of(&thread.jid)
                    };
                    let topic = info.group.as_ref().map(|g| g.topic.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| thread.topic.clone());
                    let pinned = thread.pinned;
                    let archived = thread.archived;
                    let tid = thread_key.get_value();
                    let thread_sv = StoredValue::new(thread.clone());
                    view! {
                        <div class="chat-info-hero">
                            {if channel {
                                view! { <span class="chat-channel-mark is-large" aria-hidden="true"><Icon name=if thread.kind == "voice" { "speaker" } else { "hash" }/></span> }.into_any()
                            } else {
                                view! { <ChatAvatar name=name.clone() group=group size="is-large"/> }.into_any()
                            }}
                            <h3>{if channel { format!("#{name}") } else { name.clone() }}</h3>
                            <p>{format!("{subtitle} \u{b7} {}", platform_name(&platform))}</p>
                        </div>
                        {linked.then(|| view! {
                            <div class="chat-info-actions">
                                {if quiet {
                                    view! { <button type="button" on:click=move |_| { on_setting.try_run(ChatSetting { muted_until: Some(0), ..Default::default() }); }><span><Icon name="bell"/></span>"Unmute"</button> }.into_any()
                                } else {
                                    view! { <ActionMenu label="Mute" class="chat-info-mute" actions=Callback::new(move |_| mute_items(on_setting))><span><Icon name="bell"/></span>"Mute"</ActionMenu> }.into_any()
                                }}
                                {(!discord).then(|| view! {
                                    <button type="button" disabled=archived on:click=move |_| { on_setting.try_run(ChatSetting { pinned: Some(!pinned), ..Default::default() }); }><span><Icon name="pin"/></span>{if pinned { "Unpin" } else { "Pin" }}</button>
                                    <button type="button" on:click=move |_| { on_setting.try_run(ChatSetting { archived: Some(!archived), ..Default::default() }); }><span><Icon name="archive"/></span>{if archived { "Unarchive" } else { "Archive" }}</button>
                                })}
                            </div>
                        })}
                        {(!topic.is_empty()).then(|| view! { <p class="chat-info-topic">{topic.clone()}</p> })}
                        {(!group && !discord).then(|| {
                            let phone = phone_of(&thread.jid);
                            view! {
                                <div class="chat-contact">{match thread.contact.clone() {
                                    Some(contact) => {
                                        let on_whatsapp = if !thread.whatsapp_name.is_empty() && thread.whatsapp_name != contact.name && thread.whatsapp_name != phone {
                                            format!(" \u{b7} on WhatsApp \u{201c}{}\u{201d}", thread.whatsapp_name)
                                        } else {
                                            String::new()
                                        };
                                        let href = format!("/contacts?{}", search_params(&[("book", &contact.book), ("contact", &contact.href)]));
                                        view! {
                                            <ChatAvatar name=contact.name.clone() group=false size="is-small"/>
                                            <div><b>{contact.name.clone()}</b><small>{format!("Dar contact{on_whatsapp}")}</small></div>
                                            <a class="text-button" href=href>"Open"</a>
                                        }.into_any()
                                    }
                                    None => view! {
                                        <span class="chat-empty-icon is-small"><Icon name="contacts"/></span>
                                        <div><b>"Not in your contacts"</b><small>"Save this number in Dar Contacts."</small></div>
                                        <button type="button" class="text-button" on:click=move |_| { on_add_contact.try_run(thread_sv.get_value()); }>"Add"</button>
                                    }.into_any(),
                                }}</div>
                            }
                        })}
                        <dl class="chat-info-card">
                            <div><dt><Icon name="chat"/>"Account"</dt><dd>{info.account.name.clone()}</dd></div>
                            <div><dt><Icon name="clock"/>"Messages in Dar"</dt><dd>{format!("{}{}", locale_number(info.messages), if info.complete { "" } else { "+" })}</dd></div>
                            {quiet.then(|| view! {
                                <div><dt><Icon name="bell"/>"Muted"</dt><dd>{if thread.muted_until == -1.0 { "Always".to_owned() } else { format!("Until {}", local_date_time(thread.muted_until, &[("dateStyle", "medium"), ("timeStyle", "short")])) }}</dd></div>
                            })}
                        </dl>
                        {(!pictures.is_empty()).then(|| {
                            let tid = tid.clone();
                            view! {
                                <h4 class="chat-info-heading">"Photos"</h4>
                                <div class="chat-info-grid">{pictures.into_iter().map(|m| {
                                    let url = media_url(&tid, &m.message_id);
                                    view! { <a href=url.clone() target="_blank" rel="noopener"><img src=url.clone() alt="" loading="lazy"/></a> }
                                }).collect_view()}</div>
                            }
                        })}
                        {(!files.is_empty()).then(|| {
                            let tid = tid.clone();
                            view! {
                                <h4 class="chat-info-heading">"Documents"</h4>
                                <div class="chat-info-files">{files.into_iter().map(|m| {
                                    let file = local_name(&m.kind, &m.mime, &m.file_name, m.at, &platform);
                                    view! {
                                        <a class="chat-file" href=format!("{}&download=1", media_url(&tid, &m.message_id)) download=file.clone()>
                                            <FileTile kind="file" name=file.clone() media_type=m.mime.clone()/>
                                            <span class="chat-file-name"><b>{file.clone()}</b><small>{format_bytes(m.size)}</small></span>
                                        </a>
                                    }
                                }).collect_view()}</div>
                            }
                        })}
                        {(group && !discord).then(|| view! {
                            <h4 class="chat-info-heading">{format!("Members{}", info.group.as_ref().map(|g| format!(" \u{b7} {}", g.members.len())).unwrap_or_default())}</h4>
                            {(!info.group_error.is_empty()).then(|| view! { <p class="chat-hint">{info.group_error.clone()}</p> })}
                            {info.group.clone().map(|g| view! {
                                <ul class="chat-info-members">{g.members.into_iter().map(|member| {
                                    let shown = if !member.name.is_empty() { member.name.clone() } else { Some(phone_of(&member.jid)).filter(|p| !p.is_empty()).unwrap_or_else(|| "Unknown".into()) };
                                    view! {
                                        <li><ChatAvatar name=if member.name.is_empty() { member.jid.clone() } else { member.name.clone() } group=false size="is-tiny"/><span>{shown}</span>{member.admin.then(|| view! { <small>"Admin"</small> })}</li>
                                    }
                                }).collect_view()}</ul>
                            })}
                        })}
                    }
                })}
            </div>
        </section>
    }
}

/// `number.toLocaleString()`.
fn locale_number(value: u64) -> String {
    js_sys::Number::from(value as f64).to_locale_string("default").as_string().unwrap_or_else(|| value.to_string())
}
