//! Chat's editing panes and settings sheet (`LinkPane`, `NewChatPane`, `NewMessagePane`,
//! `SaveMediaPane`, `AddContactPane`, `RenamePane`, `ChatSettings` in `chat/workspace.tsx`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::{Value, json};

use super::workspace::{ChatAvatar, narrow, now, phone_of};
use super::{
    BASE, CHAT_LIMITS_ACCOUNTS, CHAT_LIMITS_NAME, ChatAccount, ChatFriend, ChatLinkState,
    ChatMessage, ChatThread, local_name, platform_name, qr_matrix, qr_path, search_params,
};
use crate::api::{self, BrowserSession, Req, format_bytes};
use crate::files::picker::{FolderPicker, Selection};
use crate::files::types::FileTile;
use crate::icons::Icon;
use crate::settings::{Segmented, SettingRow, SettingsSheet};
use crate::ui::{ActionMenu, EditorPane, MenuAction, Select};

fn write_clipboard(text: &str) {
    let _ = crate::ui::window().navigator().clipboard().write_text(text);
}

#[component]
pub(super) fn SaveMediaPane(
    session: BrowserSession,
    thread_id: String,
    message: ChatMessage,
    platform: String,
    on_close: Callback<()>,
    on_saved: Callback<String>,
) -> impl IntoView {
    let folder: RwSignal<Option<Selection>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let media = message.media.clone().unwrap_or_default();
    let name = local_name(
        &message.kind,
        &media.mime,
        &message.file_name,
        message.at,
        &platform,
    );
    let csrf = StoredValue::new(session.csrf_token.clone());
    let request = StoredValue::new((thread_id, message.message_id.clone()));
    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let Some(target) = folder.get_untracked() else {
            return;
        };
        busy.set(true);
        error.set(String::new());
        let (thread_id, message_id) = request.get_value();
        let csrf = csrf.get_value();
        spawn_local(async move {
            #[derive(Deserialize)]
            struct Saved {
                name: String,
            }
            match api::send::<Saved>(&format!("{BASE}/save"), "POST", Some(json!({ "threadId": thread_id, "messageId": message_id, "folderId": target.id })), &csrf).await {
                Ok(saved) => { on_saved.try_run(saved.name); },
                Err(cause) => {
                    error.try_set(cause.message);
                    busy.try_set(false);
                }
            }
        });
    };
    let name = StoredValue::new(name);
    let mime = StoredValue::new(media.mime.clone());
    let size = media.size;
    view! {
        <EditorPane title="Save to Files" on_close=on_close>
            <form class="chat-link" on:submit=save>
                <div class="chat-file is-static"><FileTile kind="file" name=name.get_value() media_type=mime.get_value()/><span class="chat-file-name"><b>{name.get_value()}</b><small>{format_bytes(size)}</small></span></div>
                <FolderPicker initial_folder_id="" exclude_ids=Vec::new() disabled=Signal::derive(move || busy.get()) on_change=Callback::new(move |s| folder.set(s))/>
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || folder.with(Option::is_none)>{move || if busy.get() { "Saving\u{2026}".to_owned() } else { folder.with(|f| f.as_ref().map(|f| format!("Save to {}", f.name))).unwrap_or_else(|| "Save".into()) }}</button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub(super) fn LinkPane(
    session: BrowserSession,
    account_id: Option<String>,
    platform: Option<String>,
    on_close: Callback<()>,
    on_linked: Callback<String>,
) -> impl IntoView {
    let given = platform.is_some();
    let again = account_id.is_some();
    let account_id = StoredValue::new(account_id);
    let platform = RwSignal::new(platform.unwrap_or_else(|| "whatsapp".into()));
    let discord = Memo::new(move |_| platform.get() == "discord");
    let method = RwSignal::new(if narrow() {
        "phone".to_owned()
    } else {
        "qr".to_owned()
    });
    let name = RwSignal::new("Personal".to_owned());
    let phone = RwSignal::new(String::new());
    let link: RwSignal<Option<ChatLinkState>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let clock = RwSignal::new(now());
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled = StoredValue::new(cancelled);
    let csrf = StoredValue::new(session.csrf_token.clone());
    let id = Memo::new(move |_| link.with(|l| l.as_ref().map(|l| l.account_id.clone())));
    let state = Memo::new(move |_| link.with(|l| l.as_ref().map(|l| l.state.clone())));
    Effect::new(move |_| {
        let Some(id) = id.get() else { return };
        let state = state.get().unwrap_or_default();
        if state == "failed" || state == "paired" {
            return;
        }
        let tick = move || {
            let id = id.clone();
            spawn_local(async move {
                if let Ok(next) =
                    api::get::<ChatLinkState>(&format!("{BASE}/accounts/{id}/link")).await
                {
                    if cancelled.with_value(|c| c.load(Ordering::Relaxed)) {
                        return;
                    }
                    let paired = next.state == "paired";
                    link.try_set(Some(next));
                    if paired {
                        let _ = set_timeout_with_handle(
                            move || {
                                on_linked.try_run(id.clone());
                            },
                            Duration::from_millis(1200),
                        );
                    }
                }
            });
            clock.try_set(now());
        };
        if let Ok(handle) = set_interval_with_handle(tick, Duration::from_millis(1500)) {
            on_cleanup(move || handle.clear());
        }
    });
    let begin = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let mut body = match account_id.get_value() {
            Some(id) => json!({ "accountId": id }),
            None => {
                let p = platform.get_untracked();
                let n = name.get_untracked().trim().to_owned();
                json!({ "name": if n.is_empty() { platform_name(&p).to_owned() } else { n }, "platform": p })
            }
        };
        if !discord.get_untracked() && method.get_untracked() == "phone" {
            body["phone"] = Value::String(phone.get_untracked());
        }
        let csrf = csrf.get_value();
        spawn_local(async move {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Started {
                account_id: String,
            }
            match api::send::<Started>(&format!("{BASE}/accounts/link"), "POST", Some(body), &csrf)
                .await
            {
                Ok(result) => {
                    link.try_set(Some(ChatLinkState {
                        account_id: result.account_id,
                        state: "starting".into(),
                        ..Default::default()
                    }));
                }
                Err(cause) => {
                    error.try_set(cause.message);
                }
            }
            busy.try_set(false);
        });
    };
    let close = move || {
        cancelled.with_value(|c| c.store(true, Ordering::Relaxed));
        let target = id
            .get_untracked()
            .filter(|_| state.get_untracked().as_deref() != Some("paired"));
        let csrf = csrf.get_value();
        spawn_local(async move {
            if let Some(id) = target {
                let _ = api::request::<Value>(
                    &format!("{BASE}/accounts/{id}/link"),
                    Req {
                        method: "DELETE",
                        csrf: Some(&csrf),
                        ..Default::default()
                    },
                )
                .await;
            }
            {
                on_close.try_run(());
            };
        });
    };
    let qr = Memo::new(move |_| {
        link.with(|l| l.as_ref().and_then(|l| l.qr.clone()))
            .and_then(|text| qr_matrix(&text).ok())
            .map(|m| qr_path(&m))
    });
    let left = move || {
        link.with(|l| l.as_ref().and_then(|l| l.expires_at))
            .map_or(0, |at| {
                ((at - clock.get()) / 1000.0).round().max(0.0) as i64
            })
    };
    let service = Memo::new(move |_| platform_name(&platform.get()));
    let title = Signal::derive(move || {
        if again {
            format!("Link {} again", service.get())
        } else if link.with(Option::is_some) || given {
            format!("Link {}", service.get())
        } else {
            "Link an account".into()
        }
    });
    view! {
        <EditorPane title=title on_close=Callback::new(move |_| close())>
            {move || link.with(Option::is_none).then(|| view! {
                <form class="chat-link" on:submit=begin>
                    {(!again).then(|| view! {
                        <Segmented label="Service" value=Signal::derive(move || platform.get()) options=vec![("whatsapp", "WhatsApp"), ("discord", "Discord")]
                            on_change=Callback::new(move |value: String| { name.set(if value == "discord" { "Discord".into() } else { "Personal".into() }); platform.set(value); })/>
                    })}
                    {move || discord.get().then(|| view! {
                        <div class="chat-banner is-warning" role="note"><Icon name="alert"/><span><b>"Discord does not allow other apps on your account"</b>"Using Dar with your own Discord account breaks Discord\u{2019}s terms; Discord can limit or close the account. Dar never sees your password."</span></div>
                    })}
                    {move || (!discord.get()).then(|| view! {
                        <Segmented label="How to link" value=Signal::derive(move || method.get()) options=vec![("qr", "QR code"), ("phone", "Phone number")] on_change=Callback::new(move |v: String| method.set(v))/>
                    })}
                    {move || (!discord.get() && method.get() == "phone").then(|| view! {
                        <p class="chat-hint">"On a phone you can\u{2019}t scan your own screen, so WhatsApp gives you a code to type instead."</p>
                        <label>"Phone number"<input type="tel" inputmode="tel" autocomplete="tel" required prop:value=move || phone.get() placeholder="+966 55 123 4567" on:input=move |e| phone.set(event_target_value(&e))/></label>
                    })}
                    {(!again).then(|| view! {
                        <label>"Account name"<input prop:value=move || name.get() maxlength=CHAT_LIMITS_NAME on:input=move |e| name.set(event_target_value(&e))/><small class="chat-hint">"Names the account\u{2019}s tab when you link more than one."</small></label>
                    })}
                    <p class="chat-hint">{move || if discord.get() {
                        "Sign in by scanning a code with the Discord app on your phone. Dar shows your servers, channels and direct messages, and keeps the messages it receives."
                    } else {
                        "Dar becomes a linked device, like WhatsApp Web. Your phone keeps working, and you can unlink Dar from either side at any time. Dar imports as much history as your phone sends."
                    }}</p>
                    {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                    <div class="dialog-actions">
                        <button type="button" class="text-button" on:click=move |_| close()>"Cancel"</button>
                        <button class="primary-button" disabled=move || busy.get() || (!discord.get() && method.get() == "phone" && phone.with(|p| p.trim().is_empty()))>
                            {move || if busy.get() { "Starting\u{2026}" } else if discord.get() || method.get() == "qr" { "Show QR code" } else { "Get code" }}
                        </button>
                    </div>
                </form>
            })}
            {move || link.get().map(|l| {
                let s = l.state.clone();
                let d = discord.get_untracked();
                let service = service.get_untracked();
                view! {
                    <div class="chat-link" aria-live="polite">
                        {(s == "starting").then(|| view! { <p class="chat-progress" role="status"><span class="chat-spinner" aria-hidden="true"></span>{format!("Contacting {service}\u{2026}")}</p> })}
                        {(s == "qr").then(|| qr.get()).flatten().map(|(d_path, size)| view! {
                            {d.then(|| view! { <div class="chat-banner is-warning" role="note"><Icon name="alert"/><span><b>"Discord does not allow other apps on your account"</b>"Discord can limit or close an account that uses Dar. Dar never sees your password."</span></div> })}
                            <div class="chat-qr"><svg viewBox=format!("0 0 {size} {size}") role="img" aria-label=format!("QR code to scan with {service}")><path d=d_path></path></svg></div>
                            <p class="chat-progress" role="status"><span class="chat-spinner" aria-hidden="true"></span>{move || {
                                let left = left();
                                format!("{}{}", if d { "Waiting for the Discord app" } else { "Waiting for scan" }, if left > 0 { format!(" \u{b7} {} in {left} s", if d { "code expires" } else { "new code" }) } else { String::new() })
                            }}</p>
                            {if d {
                                view! { <ol class="chat-steps"><li>"Open Discord on your phone."</li><li>"Go to "<b>"You"</b>" and tap "<b>"Scan QR code"</b>"."</li><li>"Point it at this code and confirm "<b>"Log in"</b>"."</li></ol> }.into_any()
                            } else {
                                view! { <ol class="chat-steps"><li>"Open WhatsApp on your phone."</li><li>"Go to "<b>"Settings \u{2192} Linked devices"</b>" and tap "<b>"Link a device"</b>"."</li><li>"Point your phone at this code."</li></ol> }.into_any()
                            }}
                        })}
                        {(s == "scanned").then(|| view! {
                            <p class="chat-progress" role="status"><span class="chat-spinner" aria-hidden="true"></span>{format!("Scanned{}. Confirm ", l.name.clone().map(|n| format!(" by {n}")).unwrap_or_default())}<b>"\u{a0}Log in\u{a0}"</b>" in the Discord app."</p>
                        })}
                        {(s == "code").then(|| l.code.clone()).flatten().map(|code| {
                            let chars: Vec<char> = code.replacen('-', "", 1).chars().collect();
                            let copy = code.clone();
                            view! {
                                <p class="chat-pairing" aria-label=format!("Pairing code {code}")>{chars.into_iter().enumerate().map(|(i, c)| view! {
                                    {(i == 4).then(|| view! { <i aria-hidden="true">"\u{2013}"</i> })}<span>{c.to_string()}</span>
                                }).collect_view()}</p>
                                <ol class="chat-steps"><li>"In WhatsApp, open "<b>"Linked devices \u{2192} Link a device"</b>"."</li><li>"Tap "<b>"Link with phone number instead"</b>"."</li><li>"Enter this code."</li></ol>
                                <p class="chat-progress" role="status"><span class="chat-spinner" aria-hidden="true"></span>"Waiting for WhatsApp\u{2026}"</p>
                                <button type="button" class="secondary-button" on:click=move |_| write_clipboard(&copy)><Icon name="copy"/>"Copy code"</button>
                            }
                        })}
                        {(s == "paired").then(|| view! { <p class="chat-progress is-done" role="status"><Icon name="check"/>{if d { "Signed in. Loading your servers\u{2026}" } else { "Linked. Importing your chats\u{2026}" }}</p> })}
                        {(s == "failed").then(|| view! {
                            <div class="chat-banner is-error" role="alert"><Icon name="alert"/><span><b>"Linking stopped"</b>{l.reason.clone().unwrap_or_else(|| format!("{service} could not link this device."))}</span></div>
                        })}
                        <div class="dialog-actions">
                            {(s == "failed").then(|| view! { <button type="button" class="primary-button" on:click=move |_| link.set(None)>"Try again"</button> })}
                            <button type="button" class="text-button" on:click=move |_| close()>{if s == "paired" { "Close" } else { "Cancel" }}</button>
                        </div>
                    </div>
                }
            })}
        </EditorPane>
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ContactChoice {
    name: String,
    phone: String,
    label: String,
    book: String,
    href: String,
}

impl Default for ContactChoice {
    fn default() -> Self {
        Self {
            name: String::new(),
            phone: String::new(),
            label: String::new(),
            book: String::new(),
            href: String::new(),
        }
    }
}

fn open_chat(
    csrf: String,
    body: Value,
    busy: RwSignal<bool>,
    error: RwSignal<String>,
    on_opened: Callback<ChatThread>,
    phone: bool,
) {
    busy.set(true);
    error.set(String::new());
    spawn_local(async move {
        match api::send::<ChatThread>(&format!("{BASE}/chats"), "POST", Some(body), &csrf).await {
            Ok(thread) => {
                on_opened.try_run(thread);
            }
            Err(cause) => {
                error.try_set(if phone && cause.message == "invalidInput" {
                    "Enter the full number with its country code.".into()
                } else {
                    cause.message
                });
                busy.try_set(false);
            }
        }
    });
}

/// New chat: pick a Dar contact with a phone number, or type a number.
#[component]
pub(super) fn NewChatPane(
    session: BrowserSession,
    account: ChatAccount,
    on_close: Callback<()>,
    on_opened: Callback<ChatThread>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let choices: RwSignal<Option<Vec<ContactChoice>>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let account_id = StoredValue::new(account.id.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    Effect::new(move |_| {
        let q = query.get();
        let id = account_id.get_value();
        let fetch = move || {
            let (q, id) = (q.clone(), id.clone());
            spawn_local(async move {
                #[derive(Deserialize)]
                struct Items {
                    items: Vec<ContactChoice>,
                }
                let result = api::get::<Items>(&format!(
                    "{BASE}/contacts?{}",
                    search_params(&[("accountId", &id), ("q", &q)])
                ))
                .await;
                choices.try_set(Some(result.map(|r| r.items).unwrap_or_default()));
            });
        };
        let delay = if query.get_untracked().is_empty() {
            0
        } else {
            200
        };
        if let Ok(handle) = set_timeout_with_handle(fetch, Duration::from_millis(delay)) {
            on_cleanup(move || handle.clear());
        }
    });
    let number = Memo::new(move |_| {
        let typed: String = query
            .get()
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect();
        let digits = typed.strip_prefix('+').unwrap_or(&typed);
        if (7..=15).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
            typed
        } else {
            String::new()
        }
    });
    let open = move |phone: String| {
        open_chat(
            csrf.get_value(),
            json!({ "accountId": account_id.get_value(), "phone": phone }),
            busy,
            error,
            on_opened,
            true,
        )
    };
    let name = account.name.clone();
    view! {
        <EditorPane title="New chat" on_close=on_close>
            <form class="chat-link" on:submit=move |event: web_sys::SubmitEvent| {
                event.prevent_default();
                let n = number.get_untracked();
                if !n.is_empty() {
                    open(n);
                } else if let Some(only) = choices.with_untracked(|c| c.as_ref().filter(|c| c.len() == 1).map(|c| c[0].phone.clone())) {
                    open(only);
                }
            }>
                <label>"Contact or phone number"<input type="search" autofocus prop:value=move || query.get() placeholder="Name or +966 55 123 4567" on:input=move |e| query.set(event_target_value(&e))/>
                    <small class="chat-hint">{format!("Your Dar contacts with a phone number. The chat starts from {name}.")}</small></label>
                {move || {
                    let n = number.get();
                    (!n.is_empty()).then(|| {
                        let shown = if n.starts_with('+') { n.clone() } else { format!("+{n}") };
                        view! {
                            <button type="button" class="chat-pick" disabled=move || busy.get() on:click=move |_| open(n.clone())>
                                <span class="chat-avatar contact-initials avatar-hue-0 is-small" aria-hidden="true"><Icon name="phone"/></span>
                                <span><b>{format!("Chat with {shown}")}</b><small>"Number as typed"</small></span>
                            </button>
                        }
                    })
                }}
                {move || choices.get().map(|list| {
                    let empty = list.is_empty() && number.with(String::is_empty);
                    view! {
                        <div class="chat-picks" role="list" aria-label="Dar contacts">
                            {list.into_iter().map(|c| {
                                let phone = c.phone.clone();
                                view! {
                                    <div role="listitem"><button type="button" class="chat-pick" disabled=move || busy.get() on:click=move |_| open(phone.clone())>
                                        <ChatAvatar name=c.name.clone() group=false size="is-small"/><span><b>{c.name.clone()}</b><small>{format!("{}{}", c.phone, if c.label.is_empty() { String::new() } else { format!(" \u{b7} {}", c.label) })}</small></span>
                                    </button></div>
                                }
                            }).collect_view()}
                            {empty.then(|| view! { <p class="chat-hint">{if query.get_untracked().is_empty() { "None of your Dar contacts has a phone number yet." } else { "No Dar contact with a phone number matches." }}</p> })}
                        </div>
                    }
                })}
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions"><button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button></div>
            </form>
        </EditorPane>
    }
}

/// Discord: start a direct message with a friend.
#[component]
pub(super) fn NewMessagePane(
    session: BrowserSession,
    account: ChatAccount,
    on_close: Callback<()>,
    on_opened: Callback<ChatThread>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let friends: RwSignal<Option<Vec<ChatFriend>>> = RwSignal::new(None);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let account_id = StoredValue::new(account.id.clone());
    let csrf = StoredValue::new(session.csrf_token.clone());
    spawn_local(async move {
        #[derive(Deserialize)]
        struct Friends {
            friends: Vec<ChatFriend>,
        }
        match api::get::<Friends>(&format!(
            "{BASE}/friends?{}",
            search_params(&[("accountId", &account_id.get_value())])
        ))
        .await
        {
            Ok(r) => {
                friends.try_set(Some(r.friends));
            }
            Err(cause) => {
                friends.try_set(Some(Vec::new()));
                error.try_set(cause.message);
            }
        }
    });
    let shown = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        friends.with(|f| {
            f.iter()
                .flatten()
                .filter(|f| {
                    q.is_empty()
                        || f.name.to_lowercase().contains(&q)
                        || f.username.to_lowercase().contains(&q)
                })
                .take(50)
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let open = move |user: String| {
        open_chat(
            csrf.get_value(),
            json!({ "accountId": account_id.get_value(), "user": user }),
            busy,
            error,
            on_opened,
            false,
        )
    };
    let name = account.name.clone();
    view! {
        <EditorPane title="New message" on_close=on_close>
            <form class="chat-link" on:submit=move |event: web_sys::SubmitEvent| {
                event.prevent_default();
                if let Some(only) = shown.with_untracked(|s| (s.len() == 1).then(|| s[0].id.clone())) {
                    open(only);
                }
            }>
                <label>"Friend"<input type="search" autofocus prop:value=move || query.get() placeholder="Name or username" on:input=move |e| query.set(event_target_value(&e))/>
                    <small class="chat-hint">{format!("Your Discord friends. The message goes out from {name}.")}</small></label>
                {move || (friends.with(Option::is_none) && error.with(String::is_empty)).then(|| view! { <p class="chat-hint" role="status">"Loading friends\u{2026}"</p> })}
                {move || friends.with(Option::is_some).then(|| {
                    let list = shown.get();
                    let empty = list.is_empty();
                    view! {
                        <div class="chat-picks" role="list" aria-label="Friends">
                            {list.into_iter().map(|f| {
                                let id = f.id.clone();
                                view! {
                                    <div role="listitem"><button type="button" class="chat-pick" disabled=move || busy.get() on:click=move |_| open(id.clone())>
                                        <ChatAvatar name=f.name.clone() group=false size="is-small"/><span><b>{f.name.clone()}</b><small>{f.username.clone()}</small></span>
                                    </button></div>
                                }
                            }).collect_view()}
                            {empty.then(|| view! { <p class="chat-hint">{if query.with_untracked(|q| q.trim().is_empty()) { "You have no Discord friends yet." } else { "No friend matches." }}</p> })}
                        </div>
                    }
                })}
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions"><button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button></div>
            </form>
        </EditorPane>
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Book {
    id: String,
    name: String,
    role: String,
}

/// Saves a WhatsApp number as a Dar contact; nothing is written to WhatsApp.
#[component]
pub(super) fn AddContactPane(
    session: BrowserSession,
    thread: ChatThread,
    on_close: Callback<()>,
    on_saved: Callback<String>,
) -> impl IntoView {
    let phone = phone_of(&thread.jid);
    let name = RwSignal::new(
        if !thread.whatsapp_name.is_empty() && thread.whatsapp_name != phone {
            thread.whatsapp_name.clone()
        } else {
            String::new()
        },
    );
    let books: RwSignal<Option<Vec<Book>>> = RwSignal::new(None);
    let book = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let phone_sv = StoredValue::new(phone.clone());
    spawn_local(async move {
        #[derive(Deserialize)]
        struct Items {
            items: Vec<Book>,
        }
        match api::get::<Items>("/api/v1/contacts/books").await {
            Ok(r) => {
                let writable: Vec<Book> =
                    r.items.into_iter().filter(|b| b.role != "read").collect();
                book.try_set(writable.first().map(|b| b.id.clone()).unwrap_or_default());
                books.try_set(Some(writable));
            }
            Err(cause) => {
                error.try_set(cause.message);
            }
        }
    });
    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        let (target, display) = (book.get_untracked(), name.get_untracked().trim().to_owned());
        if target.is_empty() || display.is_empty() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        let csrf = csrf.get_value();
        let phone = phone_sv.get_value();
        spawn_local(async move {
            let body = json!({ "data": { "displayName": display, "phones": [{ "value": phone, "label": "cell" }] } }).to_string();
            let result = api::request::<Value>(
                &format!("/api/v1/contacts/books/{target}/items"),
                Req {
                    method: "POST",
                    body: Some(body),
                    csrf: Some(&csrf),
                    headers: &[("if-none-match", "*")],
                    ..Default::default()
                },
            )
            .await;
            match result {
                Ok(_) => {
                    on_saved.try_run(display);
                }
                Err(cause) => {
                    error.try_set(cause.message);
                    busy.try_set(false);
                }
            }
        });
    };
    view! {
        <EditorPane title="Add to Contacts" on_close=on_close>
            <form class="chat-link" on:submit=save>
                <label>"Name"<input autofocus required maxlength="1024" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                <label>"Phone"<input readonly prop:value=phone.clone()/></label>
                {move || books.get().filter(|b| b.len() > 1).map(|choices| {
                    let options: Vec<(String, String)> = choices.into_iter().map(|b| (b.id, b.name)).collect();
                    view! { <label>"Address book"<Select options=options value=book/></label> }
                })}
                {move || books.with(|b| b.as_ref().is_some_and(Vec::is_empty)).then(|| view! { <p class="chat-hint">"You have no address book you can add to. Create one in Contacts first."</p> })}
                <p class="chat-hint">"The contact is saved in Dar only; WhatsApp and your phone are not changed."</p>
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || book.with(String::is_empty) || name.with(|n| n.trim().is_empty())>{move || if busy.get() { "Saving\u{2026}" } else { "Add contact" }}</button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub(super) fn RenamePane(
    session: BrowserSession,
    account: ChatAccount,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let name = RwSignal::new(account.name.clone());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let csrf = StoredValue::new(session.csrf_token.clone());
    let id = StoredValue::new(account.id.clone());
    let save = move |event: web_sys::SubmitEvent| {
        event.prevent_default();
        busy.set(true);
        error.set(String::new());
        let (csrf, id, value) = (
            csrf.get_value(),
            id.get_value(),
            name.get_untracked().trim().to_owned(),
        );
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/accounts/{id}"),
                "PATCH",
                Some(json!({ "name": value })),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    on_saved.try_run(());
                }
                Err(cause) => {
                    error.try_set(cause.message);
                    busy.try_set(false);
                }
            }
        });
    };
    view! {
        <EditorPane title="Rename account" on_close=on_close>
            <form class="chat-link" on:submit=save>
                <label>"Name"<input autofocus required maxlength=CHAT_LIMITS_NAME prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/></label>
                {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
                <div class="dialog-actions">
                    <button type="button" class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                    <button class="primary-button" disabled=move || busy.get() || name.with(|n| n.trim().is_empty())>{move || if busy.get() { "Saving\u{2026}" } else { "Save" }}</button>
                </div>
            </form>
        </EditorPane>
    }
}

#[component]
pub(super) fn ChatSettings(
    accounts: Signal<Vec<ChatAccount>>,
    on_close: Callback<()>,
    on_link: Callback<()>,
    actions: Callback<ChatAccount, Vec<MenuAction>>,
) -> impl IntoView {
    let section = RwSignal::new("Accounts");
    let notice = RwSignal::new(None);
    view! {
        <SettingsSheet title="Chat settings" icon="chat" sections=vec!["Accounts", "Keyboard shortcuts"] section=section on_close=on_close notice=notice footer="Each account keeps its own chats.">
            {move || match section.get() {
                "Accounts" => view! {
                    <section class="settings-group" aria-label="Accounts">
                        <h3>"Linked accounts"</h3>
                        {move || accounts.get().into_iter().map(|a| {
                            let phone = phone_of(&a.jid);
                            let state = match a.state.as_str() {
                                "connected" => "Connected",
                                "loggedOut" => if a.platform == "discord" { "Signed out" } else { "Unlinked" },
                                "linking" => "Not linked yet",
                                "failed" => "Stopped",
                                _ => "Reconnecting",
                            };
                            let hint = format!("{}{} \u{b7} {state}", platform_name(&a.platform), if a.platform == "whatsapp" && !phone.is_empty() { format!(" \u{b7} {phone}") } else { String::new() });
                            let label = format!("Actions for {}", a.name);
                            let account = a.clone();
                            view! {
                                <SettingRow label=a.name.clone() hint=hint state=Signal::stored(None)>
                                    <ActionMenu label=label actions=Callback::new(move |_| actions.run(account.clone()))><Icon name="more"/></ActionMenu>
                                </SettingRow>
                            }
                        }).collect_view()}
                        <SettingRow label="Link an account" hint=format!("WhatsApp or Discord, up to {CHAT_LIMITS_ACCOUNTS}.") state=Signal::stored(None)>
                            <button type="button" class="secondary-button" on:click=move |_| { on_link.try_run(()); }><Icon name="plus"/>"Link an account"</button>
                        </SettingRow>
                    </section>
                }.into_any(),
                _ => view! {
                    <dl class="ui-shortcuts"><dt>"Enter"</dt><dd>"Send the message"</dd><dt>"Shift Enter"</dt><dd>"New line"</dd><dt>"Escape"</dt><dd>"Cancel a reply"</dd><dt>"Ctrl / Cmd K"</dt><dd>"Search all apps"</dd></dl>
                }.into_any(),
            }}
        </SettingsSheet>
    }
}
