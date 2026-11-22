//! Chat (`chat/workspace.tsx`, mock A with account tabs): one account at a time, its chat list
//! and the open conversation; Discord accounts show their servers and voice.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table};
use wasm_bindgen::JsCast;

use super::conversation::{ChatInfoSheet, Conversation};
use super::panes::{AddContactPane, ChatSettings, LinkPane, NewChatPane, NewMessagePane, RenamePane, SaveMediaPane};
use super::voice::{CallCodeDialog, VoiceDock, VoiceMembers, VoiceStage, use_voice};
use super::{BASE, ChatAccount, ChatMessage, ChatServer, ChatSetting, ChatThread, ChatThreadPage, ChatOverview, ChatView, platform_name, search_params, kind_label};
use crate::api::{self, BrowserSession};
use crate::dates::{local_date, local_time};
use crate::icons::Icon;
use crate::live::Live;
use crate::module_bindings::*;
use crate::pim::avatar_hue;
use crate::shell::WorkspaceHeader;
use crate::ui::{ActionMenu, Dialog, MenuAction, active_element, document, listen, unique_id, window};

pub type RunFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;

/// A confirmation dialog's text and the work it runs (`Confirm`).
#[derive(Clone)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub action: String,
    pub run: Arc<dyn Fn() -> RunFuture + Send + Sync>,
}

#[derive(Clone)]
enum Pane {
    Link { account_id: Option<String>, platform: Option<String> },
    NewChat,
    Settings,
    Rename(ChatAccount),
    Save { thread_id: String, message: ChatMessage },
    AddContact(ChatThread),
}

pub(super) fn kind_icon(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "image" => "image",
        "video" | "audio" | "voice" => "media",
        "document" => "file",
        "sticker" => "image",
        "location" => "location",
        "contact" => "user",
        "poll" => "checklist",
        "unsupported" => "info",
        _ => return None,
    })
}

fn state_text(detail: &str) -> &'static str {
    match detail {
        "replaced" => "Another copy of Dar is using this link. Unlink it and link again here.",
        "banned" => "WhatsApp has temporarily blocked this number.",
        "outdated" => "WhatsApp no longer accepts this version of the bridge. Update Dar.",
        "unlinked" => "You unlinked this account. Its chats stay here, read-only.",
        "unlinkedHere" => "Dar stopped using this link. Remove Dar under Linked devices on your phone too.",
        "notLinked" => "This account is no longer linked.",
        "timeout" => "The code expired before it was used.",
        "passkey" => "This WhatsApp account asks for a passkey, which Dar does not support yet.",
        "authenticationFailed" => "Discord no longer accepts this sign-in, after a new password or a sign-out elsewhere.",
        "gateway" => "Discord refused the connection.",
        _ => "",
    }
}

pub(super) fn now() -> f64 {
    js_sys::Date::now()
}

/// A custom Discord emoji is kept as "name:id" and shown as :name:.
pub(super) fn emoji_label(emoji: &str) -> String {
    if let Some((name, id)) = emoji.split_once(':')
        && !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '~' || c == '-')
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_digit())
    {
        return format!(":{name}:");
    }
    emoji.to_owned()
}

pub(super) fn quiet_until(until: f64) -> bool {
    until == -1.0 || until > now()
}

pub(super) fn initials(name: &str) -> String {
    let trimmed = name.strip_prefix('+').unwrap_or(name);
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let Some(first) = words.first() else { return "?".into() };
    if first.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        let chars: Vec<char> = first.chars().collect();
        return chars[chars.len().saturating_sub(2)..].iter().collect();
    }
    let mut out: String = first.chars().next().map(String::from).unwrap_or_default();
    if words.len() > 1
        && let Some(c) = words[words.len() - 1].chars().next()
    {
        out.push(c);
    }
    out.to_uppercase()
}

pub(super) fn phone_of(jid: &str) -> String {
    match jid.strip_suffix("@s.whatsapp.net") {
        Some(_) => format!("+{}", jid.split('@').next().unwrap_or_default()),
        None => String::new(),
    }
}

fn start_of_day(time: f64) -> f64 {
    let d = js_sys::Date::new(&time.into());
    d.set_hours(0);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time()
}

pub(super) fn day_label(time: f64) -> String {
    let today = start_of_day(now());
    let start = start_of_day(time);
    let diff = ((today - start) / 86_400_000.0).round();
    if diff <= 0.0 {
        "Today".into()
    } else if diff == 1.0 {
        "Yesterday".into()
    } else if diff < 7.0 {
        local_date(time, &[("weekday", "long")])
    } else {
        let year = js_sys::Date::new(&time.into()).get_full_year();
        let this_year = js_sys::Date::new(&now().into()).get_full_year();
        if year != this_year {
            local_date(time, &[("day", "numeric"), ("month", "long"), ("year", "numeric")])
        } else {
            local_date(time, &[("day", "numeric"), ("month", "long")])
        }
    }
}

fn list_time(time: f64) -> String {
    if time == 0.0 {
        return String::new();
    }
    let label = day_label(time);
    if label == "Today" {
        local_time(time, &[("hour", "numeric"), ("minute", "2-digit")])
    } else if label == "Yesterday" {
        label
    } else if now() - time < 6.0 * 86_400_000.0 {
        local_date(time, &[("weekday", "short")])
    } else {
        local_date(time, &[("month", "short"), ("day", "numeric")])
    }
}

pub(super) fn iso(time: f64) -> String {
    js_sys::Date::new(&time.into()).to_iso_string().as_string().unwrap_or_default()
}

pub(super) fn muted(thread: &ChatThread) -> bool {
    thread.muted_until == -1.0 || thread.muted_until > now()
}

pub(super) fn narrow() -> bool {
    window().match_media("(max-width:800px)").ok().flatten().is_some_and(|m| m.matches())
}

fn preview(thread: &ChatThread) -> String {
    if thread.preview_kind == "deleted" {
        return "This message was deleted".into();
    }
    let label = if thread.preview_kind.is_empty() { "" } else { kind_label(&thread.preview_kind) };
    if thread.preview.is_empty() {
        label.into()
    } else if !label.is_empty() && thread.preview_kind != "text" && thread.preview_kind != "document" {
        format!("{label} \u{b7} {}", thread.preview)
    } else {
        thread.preview.clone()
    }
}

struct Start {
    account: String,
    view: ChatView,
    chat: String,
    message: String,
}

fn read_view() -> Start {
    let search = window().location().search().unwrap_or_default();
    let p = web_sys::UrlSearchParams::new_with_str(&search).expect("params");
    Start {
        account: p.get("account").unwrap_or_default(),
        view: ChatView::from_str(&p.get("view").unwrap_or_default()),
        chat: p.get("chat").unwrap_or_default(),
        message: p.get("message").unwrap_or_default(),
    }
}

fn write_view(account: &str, view: ChatView, chat: &str) {
    let mut pairs = Vec::new();
    if !account.is_empty() {
        pairs.push(("account", account));
    }
    if view != ChatView::Chats {
        pairs.push(("view", view.as_str()));
    }
    if !chat.is_empty() {
        pairs.push(("chat", chat));
    }
    let query = search_params(&pairs);
    let next = if query.is_empty() { "/chat".to_owned() } else { format!("/chat?{query}") };
    let location = window().location();
    let current = format!("{}{}", location.pathname().unwrap_or_default(), location.search().unwrap_or_default());
    if next != current {
        let _ = window().history().expect("history").replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&next));
    }
}

pub(super) fn message_of(cause: &api::RequestError) -> String {
    if cause.message.is_empty() { "The request could not be completed.".into() } else { cause.message.clone() }
}

#[component]
pub(super) fn ChatAvatar(#[prop(into)] name: String, group: bool, #[prop(optional, into)] size: String) -> impl IntoView {
    let class = format!("chat-avatar contact-initials {} {size}", avatar_hue(&name));
    view! {
        <span class=class aria-hidden="true">{if group { view! { <Icon name="contacts"/> }.into_any() } else { initials(&name).into_any() }}</span>
    }
}

#[component]
pub(super) fn Ticks(#[prop(into)] status: String) -> impl IntoView {
    match status.as_str() {
        "pending" => view! { <span class="chat-tick" title="Sending"><Icon name="clock"/><span class="sr-only">"Sending"</span></span> }.into_any(),
        "failed" => view! { <span class="chat-tick is-failed" title="Not sent"><Icon name="alert"/><span class="sr-only">"Not sent"</span></span> }.into_any(),
        "sent" => view! { <span class="chat-tick" title="Sent"><Icon name="check"/><span class="sr-only">"Sent"</span></span> }.into_any(),
        "delivered" | "read" => {
            let read = status == "read";
            let label = if read { "Read" } else { "Delivered" };
            view! { <span class=if read { "chat-tick is-read" } else { "chat-tick" } title=label><Icon name="checks"/><span class="sr-only">{label}</span></span> }.into_any()
        }
        _ => ().into_any(),
    }
}

/// Runs a confirmation's work, closing on success and showing the error otherwise.
#[component]
pub(super) fn ConfirmAction(confirm: Confirm, on_close: Callback<()>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let run = StoredValue::new(confirm.run);
    let text = confirm.text;
    let action = confirm.action;
    view! {
        <Dialog title=confirm.title on_close=on_close>
            <p>{text.clone()}</p>
            {move || (!error.with(String::is_empty)).then(|| view! { <p role="alert" class="error">{error.get()}</p> })}
            <div class="dialog-actions">
                <button class="text-button" on:click=move |_| { on_close.try_run(()); }>"Cancel"</button>
                <button class="danger-button" disabled=move || busy.get() on:click=move |_| {
                    busy.set(true);
                    error.set(String::new());
                    let work = run.with_value(|r| r());
                    spawn_local(async move {
                        match work.await {
                            Ok(()) => { on_close.try_run(()); },
                            Err(message) => {
                                error.try_set(message);
                                busy.try_set(false);
                            }
                        }
                    });
                }>{
                    let action = action.clone();
                    move || if busy.get() { "Working\u{2026}".to_owned() } else { action.clone() }
                }</button>
            </div>
        </Dialog>
    }
}

/// Mobile navigation: focus moves into the drawer, Escape closes it and focus returns (`useMobileNavigation`).
fn mobile_navigation(open: RwSignal<bool>, panel: NodeRef<html::Aside>) {
    let trigger = StoredValue::new_local(None::<web_sys::HtmlElement>);
    let was_open = StoredValue::new(false);
    Effect::new(move |_| {
        let is_open = open.get();
        if is_open {
            trigger.set_value(active_element());
            if let Some(el) = panel.get().and_then(|r| r.query_selector("button,a").ok().flatten()).and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                let _ = el.focus();
            }
            listen(&document(), "keydown", false, move |event| {
                let event: web_sys::KeyboardEvent = event.unchecked_into();
                if event.key() == "Escape" && !event.default_prevented() {
                    event.prevent_default();
                    open.set(false);
                }
            });
        } else if was_open.get_value() {
            let active = document().active_element();
            let on_body = document().body().is_some_and(|b| active.as_ref().is_some_and(|a| a.unchecked_ref::<web_sys::Node>() == b.as_ref()));
            let inside = panel.get_untracked().is_some_and(|r| r.contains(active.as_ref().map(|a| a.unchecked_ref::<web_sys::Node>())));
            if on_body || inside {
                trigger.with_value(|t| {
                    if let Some(el) = t {
                        let _ = el.focus();
                    }
                });
            }
        }
        was_open.set_value(is_open);
    });
}

#[derive(Clone, Copy)]
struct Chat {
    session: StoredValue<BrowserSession>,
    overview: RwSignal<Option<ChatOverview>>,
    load_error: RwSignal<String>,
    account_id: RwSignal<String>,
    view: RwSignal<ChatView>,
    page: RwSignal<Option<ChatThreadPage>>,
    list_state: RwSignal<&'static str>,
    list_error: RwSignal<String>,
    more: RwSignal<bool>,
    selected: RwSignal<String>,
    pane: RwSignal<Option<Pane>>,
    confirm: RwSignal<Option<Confirm>>,
    notice: RwSignal<String>,
    epoch: RwSignal<u32>,
    navigation: RwSignal<bool>,
    generation: StoredValue<u32>,
    origin: StoredValue<Option<send_wrapper::SendWrapper<web_sys::HtmlElement>>>,
    account: Memo<Option<ChatAccount>>,
}

impl Chat {
    fn csrf(self) -> String {
        self.session.with_value(|s| s.csrf_token.clone())
    }

    fn fail(self, cause: &api::RequestError) {
        self.notice.try_set(message_of(cause));
    }

    fn load_overview(self) {
        spawn_local(async move {
            match api::get::<ChatOverview>(&format!("{BASE}/overview")).await {
                Ok(value) => {
                    self.overview.try_set(Some(value));
                    self.load_error.try_set(String::new());
                }
                Err(cause) => {
                    self.load_error.try_set(cause.message);
                }
            }
        });
    }

    fn load_threads(self, quiet: bool) {
        let Some(account) = self.account.get_untracked() else {
            self.page.set(None);
            self.list_state.set("ready");
            return;
        };
        let view = self.view.get_untracked();
        let ticket = self.generation.get_value() + 1;
        self.generation.set_value(ticket);
        if !quiet {
            self.list_state.set("loading");
        }
        spawn_local(async move {
            let url = format!("{BASE}/threads?{}", search_params(&[("accountId", &account.id), ("view", view.as_str())]));
            let result = api::get::<ChatThreadPage>(&url).await;
            if self.generation.try_get_value() != Some(ticket) {
                return;
            }
            match result {
                Ok(page) => {
                    self.page.try_set(Some(page));
                    self.list_state.try_set("ready");
                }
                Err(cause) => {
                    self.list_state.try_set("error");
                    self.list_error.try_set(cause.message);
                }
            }
        });
    }

    fn load_more(self) {
        let Some(account) = self.account.get_untracked() else { return };
        let Some(cursor) = self.page.with_untracked(|p| p.as_ref().and_then(|p| p.cursor.clone())) else { return };
        if self.more.get_untracked() {
            return;
        }
        self.more.set(true);
        let ticket = self.generation.get_value();
        let view = self.view.get_untracked();
        spawn_local(async move {
            let url = format!("{BASE}/threads?{}", search_params(&[("accountId", &account.id), ("view", view.as_str()), ("cursor", &cursor)]));
            match api::get::<ChatThreadPage>(&url).await {
                Ok(next) => {
                    if self.generation.try_get_value() == Some(ticket) {
                        self.page.try_update(|p| {
                            if let Some(p) = p {
                                let fresh: Vec<ChatThread> = next.threads.into_iter().filter(|t| !p.threads.iter().any(|o| o.id == t.id)).collect();
                                p.threads.extend(fresh);
                                p.cursor = next.cursor;
                            }
                        });
                    }
                }
                Err(cause) => self.fail(&cause),
            }
            self.more.try_set(false);
        });
    }

    fn choose_account(self, id: String) {
        self.account_id.set(id);
        self.selected.set(String::new());
        self.view.set(ChatView::Chats);
    }

    fn open_thread(self, thread_id: String, from: Option<web_sys::HtmlElement>) {
        self.origin.set_value(from.map(send_wrapper::SendWrapper::new));
        self.selected.set(thread_id);
        self.navigation.set(false);
    }

    fn close_thread(self) {
        self.selected.set(String::new());
        request_animation_frame(move || {
            if let Some(Some(target)) = self.origin.try_get_value()
                && target.is_connected()
            {
                let _ = target.focus();
            }
        });
    }

    /// Mute, pin or archive on WhatsApp too; the list and info refresh from the change.
    fn set_chat(self, thread_id: String, change: ChatSetting) {
        let csrf = self.csrf();
        spawn_local(async move {
            let mut body = serde_json::to_value(&change).unwrap_or_else(|_| json!({}));
            body["threadId"] = Value::String(thread_id);
            match api::send::<Value>(&format!("{BASE}/chatSetting"), "POST", Some(body), &csrf).await {
                Ok(_) => {
                    let text = match (change.archived, change.pinned, change.muted_until) {
                        (Some(true), _, _) => "Chat archived",
                        (Some(false), _, _) => "Chat unarchived",
                        (_, Some(true), _) => "Chat pinned",
                        (_, Some(false), _) => "Chat unpinned",
                        (_, _, Some(m)) if m != 0 => "Chat muted",
                        _ => "Chat unmuted",
                    };
                    self.notice.try_set(text.into());
                }
                Err(cause) => self.fail(&cause),
            }
        });
    }

    fn account_actions(self, a: ChatAccount) -> Vec<MenuAction> {
        let discord = a.platform == "discord";
        let mut actions = vec![{
            let a = a.clone();
            MenuAction::new("Rename", "pencil").run(move || self.pane.set(Some(Pane::Rename(a.clone()))))
        }];
        if a.state == "loggedOut" || a.state == "failed" {
            let (id, platform) = (a.id.clone(), a.platform.clone());
            actions.push(MenuAction::new("Link again", "link").run(move || self.pane.set(Some(Pane::Link { account_id: Some(id.clone()), platform: Some(platform.clone()) }))));
        }
        if a.state != "loggedOut" {
            let a = a.clone();
            actions.push(MenuAction::new(if discord { "Sign out\u{2026}" } else { "Unlink\u{2026}" }, "logout").separator().run(move || {
                let a = a.clone();
                let target = a.clone();
                let run: Arc<dyn Fn() -> RunFuture + Send + Sync> = Arc::new(move || {
                    let a = target.clone();
                    let csrf = self.csrf();
                    Box::pin(async move {
                        #[derive(serde::Deserialize)]
                        struct Unlinked {
                            confirmed: bool,
                        }
                        let result = api::send::<Unlinked>(&format!("{BASE}/accounts/{}/unlink", a.id), "POST", Some(json!({})), &csrf).await.map_err(|c| c.message)?;
                        let discord = a.platform == "discord";
                        self.notice.try_set(if result.confirmed {
                            format!("{} {}", a.name, if discord { "signed out" } else { "unlinked" })
                        } else if discord {
                            "Stopped here. Remove the session under Devices in Discord\u{2019}s settings too.".into()
                        } else {
                            "Unlinked here. Remove Dar under Linked devices on your phone too.".into()
                        });
                        self.load_overview();
                        Ok(())
                    })
                });
                self.confirm.set(Some(Confirm {
                    title: if a.platform == "discord" { format!("Sign {} out?", a.name) } else { format!("Unlink {}?", a.name) },
                    text: if a.platform == "discord" {
                        "Dar ends its Discord session and stops receiving messages. Messages stay in Dar until you delete the account.".into()
                    } else {
                        "Dar stops receiving messages from this account and removes itself from your phone\u{2019}s linked devices. Chat history stays in Dar until you delete it.".into()
                    },
                    action: if a.platform == "discord" { "Sign out".into() } else { "Unlink".into() },
                    run,
                }));
            }));
        }
        let mut delete = MenuAction::new("Delete from Dar\u{2026}", "trash").danger();
        if a.state == "loggedOut" {
            delete = delete.separator();
        }
        actions.push(delete.run(move || {
            let a = a.clone();
            let id = a.id.clone();
            let run: Arc<dyn Fn() -> RunFuture + Send + Sync> = Arc::new(move || {
                let id = id.clone();
                let csrf = self.csrf();
                Box::pin(async move {
                    api::send::<Value>(&format!("{BASE}/accounts/{id}"), "DELETE", None, &csrf).await.map_err(|c| c.message)?;
                    self.selected.try_set(String::new());
                    self.account_id.try_set(String::new());
                    self.load_overview();
                    Ok(())
                })
            });
            self.confirm.set(Some(Confirm {
                title: format!("Delete {} from Dar?", a.name),
                text: if a.platform == "discord" {
                    "This signs Dar out and deletes the account\u{2019}s servers, chats and messages from Dar. Discord keeps them.".into()
                } else {
                    "This unlinks the account and deletes its chats and messages from Dar. Your phone keeps them.".into()
                },
                action: "Delete".into(),
                run,
            }));
        }));
        actions
    }
}

/// Chat (selected mock A with account tabs): one account at a time, its chat list and the open conversation.
#[component]
pub fn ChatWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let start = read_view();
    let start_chat = StoredValue::new(start.chat.clone());
    let start_message = StoredValue::new(start.message.clone());
    let overview: RwSignal<Option<ChatOverview>> = RwSignal::new(None);
    let account_id = RwSignal::new(start.account.clone());
    let accounts = Memo::new(move |_| overview.with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default()));
    let account = Memo::new(move |_| {
        let id = account_id.get();
        accounts.with(|list| list.iter().find(|a| a.id == id).or_else(|| list.first()).cloned())
    });
    let chat = Chat {
        session: StoredValue::new(session.clone()),
        overview,
        load_error: RwSignal::new(String::new()),
        account_id,
        view: RwSignal::new(start.view),
        page: RwSignal::new(None),
        list_state: RwSignal::new("loading"),
        list_error: RwSignal::new(String::new()),
        more: RwSignal::new(false),
        selected: RwSignal::new(start.chat),
        pane: RwSignal::new(None),
        confirm: RwSignal::new(None),
        notice: RwSignal::new(String::new()),
        epoch: RwSignal::new(0),
        navigation: RwSignal::new(false),
        generation: StoredValue::new(0),
        origin: StoredValue::new(None),
        account,
    };
    let info = RwSignal::new(false);
    let servers: RwSignal<Option<Vec<ChatServer>>> = RwSignal::new(None);
    let collapsed: RwSignal<BTreeSet<String>> = RwSignal::new(BTreeSet::new());
    let code_for = RwSignal::new(String::new());
    let discord = Memo::new(move |_| account.with(|a| a.as_ref().is_some_and(|a| a.platform == "discord")));
    let account_key = Memo::new(move |_| account.with(|a| a.as_ref().map(|a| a.id.clone()).unwrap_or_default()));
    let has_account = Memo::new(move |_| account.with(Option::is_some));
    let navigation_id = unique_id();
    let navigation_ref = NodeRef::<html::Aside>::new();
    mobile_navigation(chat.navigation, navigation_ref);

    // Discord voice: a live socket per account, and the call this tab carries.
    let voice = use_voice(session.clone(), accounts.into());
    let voice_state = voice.snapshot;
    Effect::new(move |_| {
        if let Some(ended) = voice_state.with(|v| v.ended.clone()) {
            chat.notice.set(ended.message);
        }
    });
    Effect::new(move |_| {
        if chat.notice.with(String::is_empty) {
            return;
        }
        if let Ok(handle) = set_timeout_with_handle(move || { chat.notice.try_set(String::new()); }, Duration::from_millis(5000)) {
            on_cleanup(move || handle.clear());
        }
    });

    chat.load_overview();
    Effect::new(move |_| {
        account_key.track();
        chat.view.track();
        chat.load_threads(false);
    });
    Effect::new(move |previous: Option<()>| {
        let epoch = chat.epoch.get();
        if previous.is_some() && epoch > 0 {
            chat.load_overview();
            chat.load_threads(true);
        }
    });
    // Discord: servers and their channels for the sidebar.
    Effect::new(move |_| {
        chat.epoch.track();
        let id = account_key.get();
        if !discord.get() || id.is_empty() {
            servers.set(None);
            return;
        }
        let gone = Arc::new(AtomicBool::new(false));
        {
            let gone = gone.clone();
            on_cleanup(move || gone.store(true, Ordering::Relaxed));
        }
        spawn_local(async move {
            #[derive(serde::Deserialize)]
            struct Servers {
                servers: Vec<ChatServer>,
            }
            let result = api::get::<Servers>(&format!("{BASE}/servers?{}", search_params(&[("accountId", &id)]))).await;
            if !gone.load(Ordering::Relaxed) {
                servers.try_set(Some(result.map(|r| r.servers).unwrap_or_default()));
            }
        });
    });
    Effect::new(move |_| write_view(&account_key.get(), chat.view.get(), &chat.selected.get()));
    // While linking or importing, nothing in the database changes for the browser to hear about the bridge; poll gently.
    let busy = Memo::new(move |_| accounts.with(|list| list.iter().any(|a| a.importing || a.state == "connecting")));
    Effect::new(move |_| {
        if !busy.get() {
            return;
        }
        if let Ok(handle) = set_interval_with_handle(move || chat.load_overview(), Duration::from_millis(5000)) {
            on_cleanup(move || handle.clear());
        }
    });
    // Changes to the account's chats (and to Dar contacts, which name chats) reload the list.
    let live = expect_context::<Live>();
    Effect::new(move |_| {
        let Some(c) = live.connection.get() else { return };
        let disposed = Arc::new(AtomicBool::new(false));
        let revision = Arc::new(std::sync::atomic::AtomicI64::new(-1));
        let timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
        // History arrives in many small batches; one reload covers a burst of changes.
        let bump = {
            let disposed = disposed.clone();
            move || {
                if let Some(Some(handle)) = timer.try_get_value() {
                    handle.clear();
                }
                let disposed = disposed.clone();
                let handle = set_timeout_with_handle(move || {
                    if !disposed.load(Ordering::Relaxed) {
                        chat.epoch.try_update(|n| *n += 1);
                    }
                }, Duration::from_millis(250)).ok();
                timer.try_set_value(handle);
            }
        };
        let changed = {
            let (disposed, revision, bump) = (disposed.clone(), revision.clone(), bump.clone());
            move |ctx: &EventContext| {
                if disposed.load(Ordering::Relaxed) {
                    return;
                }
                let next = ctx.db.my_chat_changes().iter().last().map_or(0, |r| r.revision as i64);
                let before = revision.swap(next, Ordering::Relaxed);
                if before >= 0 && next != before {
                    bump();
                }
            }
        };
        let inserted = c.db.my_chat_changes().on_insert({ let changed = changed.clone(); move |ctx, _| changed(ctx) });
        let deleted = c.db.me().on_delete({
            let disposed = disposed.clone();
            // Another app's unsubscribing also removes the row from the cache; only a real deletion signs out.
                move |ctx: &EventContext, _| {
                if !disposed.load(Ordering::Relaxed) && !matches!(ctx.event, spacetimedb_sdk::Event::UnsubscribeApplied) {
                    { on_signed_out.try_run(()); };
                }
            }
        });
        let contacts = c.db.my_pim_changes().on_insert({
            let (disposed, revision, bump) = (disposed.clone(), revision.clone(), bump.clone());
            move |_, _| {
                if !disposed.load(Ordering::Relaxed) && revision.load(Ordering::Relaxed) >= 0 {
                    bump();
                }
            }
        });
        let handle = c
            .subscription_builder()
            .on_applied({
                let (disposed, revision) = (disposed.clone(), revision.clone());
                move |ctx| {
                    if disposed.load(Ordering::Relaxed) {
                        return;
                    }
                    if revision.load(Ordering::Relaxed) >= 0 {
                        chat.epoch.try_update(|n| *n += 1);
                    }
                    let next = ctx.db.my_chat_changes().iter().last().map_or(0, |r| r.revision as i64);
                    revision.store(next, Ordering::Relaxed);
                }
            })
            .on_error(|_, _| {})
            .subscribe(["SELECT * FROM me", "SELECT * FROM my_chat_changes", "SELECT * FROM my_pim_changes"]);
        let kept = send_wrapper::SendWrapper::new((c.clone(), handle, inserted, deleted, contacts));
        on_cleanup(move || {
            disposed.store(true, Ordering::Relaxed);
            if let Some(Some(handle)) = timer.try_get_value() {
                handle.clear();
            }
            let (c, handle, inserted, deleted, contacts) = kept.take();
            let _ = handle.unsubscribe();
            c.db.my_chat_changes().remove_on_insert(inserted);
            c.db.my_pim_changes().remove_on_insert(contacts);
            c.db.me().remove_on_delete(deleted);
        });
    });

    let all = Memo::new(move |_| chat.page.with(|p| p.as_ref().map(|p| p.pinned.iter().chain(p.threads.iter()).cloned().collect::<Vec<_>>()).unwrap_or_default()));
    let channel = Memo::new(move |_| {
        let id = chat.selected.get();
        servers.with(|s| s.as_ref().and_then(|s| s.iter().flat_map(|s| s.channels.iter()).find(|t| t.id == id).cloned()))
    });
    let current = Memo::new(move |_| {
        let id = chat.selected.get();
        channel.get().or_else(|| all.with(|all| all.iter().find(|t| t.id == id).cloned()))
    });
    let my_voice = Memo::new(move |_| {
        let id = account_key.get();
        voice_state.with(|v| v.accounts.get(&id).cloned())
    });
    // The dock shows the call this tab carries, or else the current account's call.
    let call_entry = Memo::new(move |_| {
        let id = account_key.get();
        voice_state.with(|v| {
            v.accounts
                .iter()
                .find(|(_, a)| a.here && a.call.is_some())
                .map(|(k, a)| (k.clone(), a.clone()))
                .or_else(|| v.accounts.get(&id).filter(|a| !id.is_empty() && a.call.is_some()).map(|a| (id.clone(), a.clone())))
        })
    });
    let call_account = Memo::new(move |_| call_entry.with(|e| e.as_ref().map(|e| e.0.clone()).unwrap_or_default()));
    let the_call = Memo::new(move |_| call_entry.with(|e| e.as_ref().and_then(|e| e.1.call.clone())));
    let call_server = Memo::new(move |_| {
        if call_account.get() != account_key.get() {
            return None;
        }
        let server_id = the_call.with(|c| c.as_ref().map(|c| c.server_id.clone()))?;
        servers.with(|s| s.as_ref().and_then(|s| s.iter().find(|s| s.server_id == server_id).cloned()))
    });
    let call_thread = Memo::new(move |_| {
        let channel_id = the_call.with(|c| c.as_ref().map(|c| c.channel_id.clone()))?;
        call_server.with(|s| s.as_ref().and_then(|s| s.channels.iter().find(|c| c.jid == channel_id).cloned()))
    });
    let call_channel = Memo::new(move |_| call_thread.with(|t| t.as_ref().map(|t| t.name.clone())).unwrap_or_else(|| "Voice".into()));
    let call_server_name = Memo::new(move |_| {
        call_server.with(|s| s.as_ref().map(|s| s.name.clone())).unwrap_or_else(|| {
            let id = call_account.get();
            accounts.with(|list| list.iter().find(|a| a.id == id).map(|a| a.name.clone())).unwrap_or_else(|| "Discord".into())
        })
    });
    let voice_run = move |work: Pin<Box<dyn Future<Output = Result<(), String>>>>| {
        spawn_local(async move {
            if let Err(message) = work.await {
                chat.notice.try_set(message);
            }
        });
    };
    let set_self = move |id: String| Callback::new(move |(muted, deafened): (bool, bool)| voice_run(Box::pin(voice.set_self(id.clone(), muted, deafened))));
    let open_call = move || {
        let target = call_account.get_untracked();
        if target != account_key.get_untracked() {
            chat.choose_account(target);
            return;
        }
        if let Some(thread) = call_thread.get_untracked() {
            chat.open_thread(thread.id, None);
        }
    };
    let dock = move |class: &'static str| {
        move || {
            let call = the_call.get()?;
            let (id, entry) = call_entry.get()?;
            Some(view! {
                <VoiceDock call=call channel=call_channel.get() server=call_server_name.get() here=entry.here elsewhere=entry.elsewhere mic=voice_state.with(|v| v.mic) class=class
                    on_set=set_self(id.clone())
                    on_leave={ let id = id.clone(); Callback::new(move |_| voice_run(Box::pin(voice.leave(id.clone())))) }
                    on_use_here={ let id = id.clone(); Callback::new(move |_| voice_run(Box::pin(voice.use_here(id.clone())))) }
                    on_open=Callback::new(move |_| open_call())
                    on_code=Callback::new(move |_| code_for.set(id.clone()))/>
            })
        }
    };

    let toggle_server = move |id: String| collapsed.update(|set| {
        if !set.remove(&id) {
            set.insert(id);
        }
    });
    let title = Memo::new(move |_| if discord.get() { "Direct messages".to_owned() } else { chat.view.get().label().to_owned() });

    let link_pane = move |account_id: Option<String>, platform: Option<String>| chat.pane.set(Some(Pane::Link { account_id, platform }));
    let status = move || {
        account.get().map(|a| {
            let (id, platform) = (a.id.clone(), a.platform.clone());
            let actions = { let a = a.clone(); Callback::new(move |_| chat.account_actions(a.clone())) };
            view! { <AccountStatus account=a on_link=Callback::new(move |_| link_pane(Some(id.clone()), Some(platform.clone()))) actions=actions/> }
        })
    };

    let sidebar = view! {
        <button type="button" class="new-button ui-new" disabled=move || account.with(|a| a.as_ref().is_none_or(|a| a.state == "loggedOut" || a.state == "linking")) on:click=move |_| chat.pane.set(Some(Pane::NewChat))>
            <Icon name="plus"/>{move || if discord.get() { "New message" } else { "New chat" }}
        </button>
        <div class="chat-accounts" role="tablist" aria-label="Accounts">
            <For each=move || accounts.get() key=|a| (a.id.clone(), a.name.clone(), a.unread, a.platform.clone()) let:a>
                {
                    let id = a.id.clone();
                    let is_current = { let id = id.clone(); move || account_key.get() == id };
                    let is_current2 = is_current.clone();
                    view! {
                        <button type="button" role="tab" aria-selected=move || is_current().to_string() class=move || if is_current2() { "chat-account-tab is-current" } else { "chat-account-tab" } title=a.name.clone() on:click=move |_| chat.choose_account(id.clone())>
                            <span class=format!("chat-platform is-{}", a.platform) aria-hidden="true"><Icon name=if a.platform == "discord" { "hash" } else { "chat" }/></span>
                            <span class="chat-account-name">{a.name.clone()}</span>
                            <span class="sr-only">{format!(" ({})", platform_name(&a.platform))}</span>
                            {(a.unread > 0).then(|| view! { <span class="chat-account-count">{a.unread}<span class="sr-only">" unread"</span></span> })}
                        </button>
                    }
                }
            </For>
            <button type="button" class="icon-button chat-account-add" aria-label="Link an account" title="Link an account" on:click=move |_| link_pane(None, None)><Icon name="plus"/></button>
        </div>
        {move || (discord.get()).then(|| account.get()).flatten().map(|a| {
            let unread = a.counts.unread;
            view! {
                <nav aria-label="Direct messages">
                    <button type="button" class=move || if channel.with(Option::is_none) { "nav-item chat-view active" } else { "nav-item chat-view" } aria-current=move || channel.with(Option::is_none).then_some("page") on:click=move |_| {
                        chat.view.set(ChatView::Chats);
                        if channel.with_untracked(Option::is_some) {
                            chat.selected.set(String::new());
                        }
                        chat.navigation.set(false);
                    }>
                        <Icon name="contacts"/><span class="chat-row-title">"Direct messages"</span>{(unread > 0).then(|| view! { <span class="chat-count">{unread}</span> })}
                    </button>
                </nav>
            }
        })}
        {move || (discord.get() && has_account.get()).then(|| view! {
            <ServerTree servers=servers selected=chat.selected collapsed=collapsed on_toggle=Callback::new(toggle_server)
                on_open=Callback::new(move |(thread, el): (ChatThread, web_sys::HtmlElement)| chat.open_thread(thread.id, Some(el)))
                voice=Signal::derive(move || my_voice.with(|v| v.as_ref().map(|v| v.servers.clone()).unwrap_or_default()))
                call=Signal::derive(move || my_voice.with(|v| v.as_ref().and_then(|v| v.call.clone())))
                speaking=Signal::derive(move || voice_state.with(|v| v.speaking.clone()))
                me=Signal::derive(move || account.with(|a| a.as_ref().map(|a| a.jid.clone()).unwrap_or_default()))/>
        })}
        {dock("")}
        {move || (!discord.get()).then(|| account.get()).flatten().map(|a| view! {
            <nav aria-label="Chat views">
                {ChatView::ALL.into_iter().map(|v| {
                    let count = a.counts.for_view(v);
                    view! {
                        <button type="button" class=move || if chat.view.get() == v { "nav-item chat-view active" } else { "nav-item chat-view" } aria-current=move || (chat.view.get() == v).then_some("page") on:click=move |_| {
                            chat.view.set(v);
                            chat.navigation.set(false);
                        }>
                            <Icon name=v.icon()/><span class="chat-row-title">{v.label()}</span>{(count > 0).then(|| view! { <span class="chat-count">{count}</span> })}
                        </button>
                    }
                }).collect_view()}
            </nav>
        })}
        {status}
        <button type="button" class="nav-item ui-app-settings-entry" on:click=move |_| chat.pane.set(Some(Pane::Settings))><Icon name="settings"/>"Chat settings"</button>
    };

    let empty_text = move || {
        let a = account.get()?;
        let view = chat.view.get();
        let d = a.platform == "discord";
        let heading = if a.importing {
            "Importing your chats\u{2026}".to_owned()
        } else if a.state == "linking" {
            format!("Waiting for {}", platform_name(&a.platform))
        } else if d {
            "No direct messages yet".into()
        } else {
            match view {
                ChatView::Unread => "All caught up",
                ChatView::Archived => "No archived chats",
                ChatView::Groups => "No groups",
                ChatView::Chats => "No chats yet",
            }
            .into()
        };
        let text = if a.importing {
            "They appear here as your phone sends them."
        } else if a.state == "linking" {
            "Finish linking to see your chats."
        } else if d {
            "Your servers are in the sidebar."
        } else if view == ChatView::Unread {
            "There are no unread chats."
        } else {
            ""
        };
        Some((heading, text))
    };

    let shell_class = move || {
        let mut class = "app-shell pim-shell chat-shell".to_owned();
        let selected = !chat.selected.with(String::is_empty);
        if selected {
            class.push_str(" has-pim-details");
        }
        if channel.with(Option::is_some) {
            class.push_str(" is-channel");
        }
        if selected && info.get() {
            class.push_str(" has-chat-info");
        }
        class
    };
    let is_voice = Memo::new(move |_| discord.get() && current.with(|c| c.as_ref().is_some_and(|c| c.kind == "voice")));
    let session_sv = StoredValue::new(session.clone());

    view! {
        <div class=shell_class>
            <WorkspaceHeader app="chat" session=session.clone() on_signed_out=on_signed_out on_settings=Callback::new(move |_| chat.pane.set(Some(Pane::Settings)))/>
            <div class="pim-workspace chat-workspace">
                {move || chat.navigation.get().then(|| view! { <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| chat.navigation.set(false)></button> })}
                <aside id=navigation_id node_ref=navigation_ref class=move || if chat.navigation.get() { "sidebar chat-sidebar is-open" } else { "sidebar chat-sidebar" } aria-label="Chat navigation">{sidebar}</aside>
                <main class="pim-main chat-main" tabindex="-1" aria-label=move || title.get()>
                    <div class="chat-list-heading">
                        <h1>{move || title.get()}</h1>
                        {move || account.get().map(|a| view! { <span class="chat-heading-count">{a.counts.for_view(chat.view.get())}</span> })}
                    </div>
                    {move || (!chat.load_error.with(String::is_empty)).then(|| view! {
                        <div class="chat-banner is-error" role="alert"><Icon name="alert"/><span><b>"Chat could not be loaded"</b>{chat.load_error.get()}</span>
                            <button class="secondary-button" on:click=move |_| { chat.load_overview(); chat.load_threads(false); }>"Retry"</button></div>
                    })}
                    {move || overview.with(|o| o.as_ref().filter(|o| !o.bridge.available && !o.bridge.detail.is_empty()).map(|o| o.bridge.detail.clone())).map(|detail| view! {
                        <div class="chat-banner" role="status"><Icon name="alert"/><span><b>"Chat is unavailable"</b>{detail}</span></div>
                    })}
                    <div class="chat-list" role="list" aria-label=move || title.get()>
                        {move || (overview.with(Option::is_none) && chat.load_error.with(String::is_empty)).then(|| view! { <p class="chat-empty" role="status">"Loading chats\u{2026}"</p> })}
                        {move || overview.with(|o| o.as_ref().filter(|o| o.accounts.is_empty()).map(|o| o.bridge.available)).map(|available| view! {
                            <div class="chat-empty"><span class="chat-empty-icon"><Icon name="chat"/></span><h2>"No accounts linked"</h2><p>"Link WhatsApp or Discord to bring their chats here. Each account keeps its own tab."</p>
                                <button class="primary-button" disabled=!available on:click=move |_| link_pane(None, None)>"Link an account"</button></div>
                        })}
                        {move || (has_account.get() && chat.list_state.get() == "loading" && chat.page.with(Option::is_none)).then(|| view! { <p class="chat-empty" role="status">"Loading chats\u{2026}"</p> })}
                        {move || (has_account.get() && chat.list_state.get() == "error").then(|| view! {
                            <div class="chat-empty" role="alert"><h2>"Chats could not be loaded"</h2><p>{chat.list_error.get()}</p><button class="secondary-button" on:click=move |_| chat.load_threads(false)>"Retry"</button></div>
                        })}
                        {move || (chat.list_state.get() == "ready" && chat.page.with(Option::is_some) && all.with(Vec::is_empty)).then(empty_text).flatten().map(|(heading, text)| view! {
                            <div class="chat-empty"><h2>{heading}</h2><p>{text}</p></div>
                        })}
                        <For each=move || all.get() key=|t| format!("{}{:?}", t.id, t) let:thread>
                            {
                                let id = thread.id.clone();
                                let open_id = id.clone();
                                view! { <ThreadRow thread=thread selected=Signal::derive(move || chat.selected.with(|s| *s == id)) on_open=Callback::new(move |el| chat.open_thread(open_id.clone(), Some(el)))/> }
                            }
                        </For>
                        {move || chat.page.with(|p| p.as_ref().is_some_and(|p| p.cursor.is_some())).then(|| view! {
                            <div class="chat-more"><button class="text-button" disabled=move || chat.more.get() on:click=move |_| chat.load_more()>{move || if chat.more.get() { "Loading\u{2026}" } else { "Load more" }}</button></div>
                        })}
                    </div>
                    {dock(" is-phone")}
                </main>
                {move || {
                    let selected = chat.selected.get();
                    if selected.is_empty() || !is_voice.get() {
                        return None;
                    }
                    let a = account.get_untracked()?;
                    let thread = current.get_untracked()?;
                    let thread_id = thread.id.clone();
                    let server_of = move || servers.with(|s| s.as_ref().and_then(|s| s.iter().find(|s| s.channels.iter().any(|c| c.id == thread_id)).cloned()));
                    let server_of = StoredValue::new(server_of());
                    let jid = thread.jid.clone();
                    let jid2 = jid.clone();
                    let a_id = StoredValue::new(a.id.clone());
                    let a_jid = a.jid.clone();
                    let join_thread = StoredValue::new(thread.id.clone());
                    Some(view! {
                        <aside class="pim-details chat-details" aria-label=thread.name.clone()>
                            <VoiceStage thread_name=thread.name.clone() thread_jid=thread.jid.clone()
                                server=Signal::derive(move || server_of.with_value(|s| s.as_ref().map(|s| s.name.clone()).unwrap_or_default()))
                                members=Signal::derive(move || {
                                    let server_id = server_of.with_value(|s| s.as_ref().map(|s| s.server_id.clone()).unwrap_or_default());
                                    my_voice.with(|v| v.as_ref().and_then(|v| v.servers.get(&server_id).and_then(|s| s.get(&jid)).cloned()).unwrap_or_default())
                                })
                                call=Signal::derive(move || my_voice.with(|v| v.as_ref().and_then(|v| v.call.clone())))
                                in_other=Signal::derive(move || {
                                    let other = the_call.with(|c| c.as_ref().is_some_and(|c| !(call_account.get() == a_id.get_value() && c.channel_id == jid2)));
                                    if other { call_channel.get() } else { String::new() }
                                })
                                speaking=Signal::derive(move || voice_state.with(|v| v.speaking.clone()))
                                me=Signal::derive(move || a_jid.clone())
                                mic=Signal::derive(move || voice_state.with(|v| v.mic))
                                here=Signal::derive(move || my_voice.with(|v| v.as_ref().is_some_and(|v| v.here)))
                                on_join=Callback::new(move |(muted, done): (bool, Callback<Result<(), String>>)| {
                                    let (id, thread) = (a_id.get_value(), join_thread.get_value());
                                    spawn_local(async move { { done.try_run(voice.join(id, thread, muted).await); }; });
                                })
                                on_set=set_self(a.id.clone())
                                on_leave=Callback::new(move |_| voice_run(Box::pin(voice.leave(a_id.get_value()))))
                                on_code=Callback::new(move |_| code_for.set(a_id.get_value()))
                                on_use_here=Callback::new(move |_| voice_run(Box::pin(voice.use_here(a_id.get_value()))))
                                on_close=Callback::new(move |_| chat.close_thread())/>
                        </aside>
                    })
                }}
                {move || {
                    let selected = chat.selected.get();
                    if selected.is_empty() || is_voice.get() || !has_account.get() {
                        return None;
                    }
                    let a = account.get_untracked()?;
                    let focus = if selected == start_chat.get_value() { start_message.get_value() } else { String::new() };
                    let setting_id = selected.clone();
                    let save_id = selected.clone();
                    Some(view! {
                        <aside class="pim-details chat-details" aria-label=move || current.with(|c| c.as_ref().map(|c| c.name.clone())).unwrap_or_else(|| "Conversation".into())>
                            <Conversation thread_id=selected focus_message=focus account=Signal::derive(move || account.get().unwrap_or_else(|| a.clone())) session=session_sv.get_value() epoch=chat.epoch info=info
                                on_close=Callback::new(move |_| chat.close_thread())
                                on_notice=Callback::new(move |text: String| chat.notice.set(text))
                                on_save=Callback::new(move |message: ChatMessage| chat.pane.set(Some(Pane::Save { thread_id: save_id.clone(), message })))
                                on_info=Callback::new(move |_| info.update(|i| *i = !*i))
                                on_setting=Callback::new(move |change: ChatSetting| chat.set_chat(setting_id.clone(), change))/>
                        </aside>
                    })
                }}
                {move || {
                    let selected = chat.selected.get();
                    (!selected.is_empty() && has_account.get() && info.get()).then(|| {
                        let setting_id = selected.clone();
                        view! {
                            <aside class="chat-info-sheet" aria-label="Chat info">
                                <ChatInfoSheet thread_id=selected session=session_sv.get_value() epoch=chat.epoch on_close=Callback::new(move |_| info.set(false))
                                    on_setting=Callback::new(move |change: ChatSetting| chat.set_chat(setting_id.clone(), change))
                                    on_add_contact=Callback::new(move |thread: ChatThread| chat.pane.set(Some(Pane::AddContact(thread))))/>
                            </aside>
                        }
                    })
                }}
                {move || (chat.selected.with(String::is_empty) && has_account.get()).then(|| view! {
                    <div class="chat-placeholder" aria-hidden="true"><span class="chat-empty-icon"><Icon name="chat"/></span><p>"Choose a chat"</p></div>
                })}
            </div>
            {move || chat.pane.get().map(|pane| {
                let session = session_sv.get_value();
                let close = Callback::new(move |_| chat.pane.set(None));
                match pane {
                    Pane::Link { account_id, platform } => view! {
                        <LinkPane session=session account_id=account_id platform=platform
                            on_close=Callback::new(move |_| { chat.pane.set(None); chat.load_overview(); })
                            on_linked=Callback::new(move |id: String| {
                                chat.pane.set(None);
                                chat.choose_account(id);
                                chat.load_overview();
                                chat.notice.set("Linked. Your chats are on their way.".into());
                            })/>
                    }.into_any(),
                    Pane::NewChat => {
                        let Some(a) = account.get_untracked() else { return ().into_any() };
                        let opened = Callback::new(move |thread: ChatThread| {
                            chat.pane.set(None);
                            chat.view.set(ChatView::Chats);
                            chat.load_threads(true);
                            chat.selected.set(thread.id);
                        });
                        if a.platform == "discord" {
                            view! { <NewMessagePane session=session account=a on_close=close on_opened=opened/> }.into_any()
                        } else {
                            view! { <NewChatPane session=session account=a on_close=close on_opened=opened/> }.into_any()
                        }
                    }
                    Pane::Save { thread_id, message } => view! {
                        <SaveMediaPane session=session thread_id=thread_id message=message platform=account.get_untracked().map(|a| a.platform).unwrap_or_else(|| "whatsapp".into())
                            on_close=close on_saved=Callback::new(move |name: String| { chat.pane.set(None); chat.notice.set(format!("Saved to Files as {name}")); })/>
                    }.into_any(),
                    Pane::AddContact(thread) => view! {
                        <AddContactPane session=session thread=thread on_close=close
                            on_saved=Callback::new(move |name: String| { chat.pane.set(None); chat.notice.set(format!("{name} added to Contacts")); chat.epoch.update(|n| *n += 1); })/>
                    }.into_any(),
                    Pane::Rename(a) => view! {
                        <RenamePane session=session account=a on_close=close on_saved=Callback::new(move |_| { chat.pane.set(None); chat.load_overview(); })/>
                    }.into_any(),
                    Pane::Settings => view! {
                        <ChatSettings accounts=accounts.into() on_close=close on_link=Callback::new(move |_| link_pane(None, None)) actions=Callback::new(move |a: ChatAccount| chat.account_actions(a))/>
                    }.into_any(),
                }
            })}
            {move || chat.confirm.get().map(|confirm| view! { <ConfirmAction confirm=confirm on_close=Callback::new(move |_| chat.confirm.set(None))/> })}
            {move || (!code_for.with(String::is_empty)).then(|| view! { <CallCodeDialog hub=voice account_id=code_for.get_untracked() on_close=Callback::new(move |_| code_for.set(String::new()))/> })}
            <div class="chat-toast-region" role="status" aria-live="polite">
                {move || (!chat.notice.with(String::is_empty)).then(|| view! {
                    <div class="settings-toast chat-toast"><span>{chat.notice.get()}</span><button type="button" class="text-button" on:click=move |_| chat.notice.set(String::new())>"Dismiss"</button></div>
                })}
            </div>
        </div>
    }
}

#[component]
fn AccountStatus(account: ChatAccount, on_link: Callback<()>, actions: Callback<(), Vec<MenuAction>>) -> impl IntoView {
    let phone = phone_of(&account.jid);
    let detail = state_text(&account.detail);
    let link = move |label: &'static str| view! { <button type="button" class="text-button" on:click=move |_| { on_link.try_run(()); }>{label}</button> };
    let (tone, icon, heading, text): (&str, &str, String, AnyView) = if account.platform == "discord" {
        match account.state.as_str() {
            "connected" => ("", "hash", "Signed in to Discord".into(), "Your own account. Dar sees what you can see.".into_any()),
            "connecting" => ("", "refresh", "Connecting\u{2026}".into(), "Loading your servers and messages.".into_any()),
            "disconnected" => ("is-warning", "alert", "Reconnecting\u{2026}".into(), "Dar keeps trying. New messages arrive once Discord answers.".into_any()),
            "linking" => ("", "link", "Not signed in yet".into(), link("Link Discord").into_any()),
            "loggedOut" => ("is-warning", "alert", "Signed out".into(), view! { {if detail.is_empty() { "Discord signed Dar out. Messages stay here, read-only." } else { detail }}" "{link("Link again")} }.into_any()),
            _ => ("is-error", "alert", "Discord stopped".into(), view! { {if detail.is_empty() { "Something went wrong with this account." } else { detail }}" "{link("Link again")} }.into_any()),
        }
    } else {
        match account.state.as_str() {
            "connected" => (
                "",
                if account.importing { "refresh" } else { "phone" },
                if phone.is_empty() { "Linked".into() } else { format!("Linked to {phone}") },
                if account.importing {
                    format!("Importing history{}", if account.history_progress > 0 { format!(" \u{b7} {}%", account.history_progress) } else { "\u{2026}".into() }).into_any()
                } else {
                    "Messages also stay on your phone.".into_any()
                },
            ),
            "connecting" => ("", "refresh", "Connecting\u{2026}".into(), phone.clone().into_any()),
            "disconnected" => ("is-warning", "alert", "Reconnecting\u{2026}".into(), "Dar keeps trying. New messages arrive once WhatsApp answers.".into_any()),
            "linking" => ("", "link", "Not linked yet".into(), link("Link WhatsApp").into_any()),
            "loggedOut" => ("is-warning", "alert", "Unlinked".into(), view! { {if detail.is_empty() { "WhatsApp unlinked Dar. Chats stay here, read-only." } else { detail }}" "{link("Link again")} }.into_any()),
            _ => ("is-error", "alert", "WhatsApp stopped".into(), view! { {if detail.is_empty() { "Something went wrong with this link." } else { detail }}" "{link("Link again")} }.into_any()),
        }
    };
    view! {
        <div class=format!("chat-status {tone}")><Icon name=icon/><div class="chat-status-text"><b>{heading}</b><span>{text}</span></div>
            <ActionMenu label=format!("Actions for {}", account.name) actions=actions><Icon name="more"/></ActionMenu></div>
    }
}

#[component]
fn ThreadRow(thread: ChatThread, #[prop(into)] selected: Signal<bool>, on_open: Callback<web_sys::HtmlElement>) -> impl IntoView {
    let name = if !thread.name.is_empty() { thread.name.clone() } else { Some(phone_of(&thread.jid)).filter(|p| !p.is_empty()).unwrap_or_else(|| "Unknown".into()) };
    let text = preview(&thread);
    let quiet = muted(&thread);
    let unread = thread.unread;
    let class = move || format!("chat-row{}{}", if selected.get() { " is-selected" } else { "" }, if unread > 0 { " is-unread" } else { "" });
    let at = thread.at;
    let icon = if thread.preview_kind.is_empty() { None } else { kind_icon(&thread.preview_kind) };
    view! {
        <div role="listitem" class=class>
            <button type="button" aria-current=move || selected.get().then_some("true") on:click=move |event| {
                if let Some(el) = event.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                    { on_open.try_run(el); };
                }
            }>
                <ChatAvatar name=name.clone() group=thread.kind == "group"/>
                <span class="chat-row-name">{name.clone()}</span>
                <time class="chat-row-time" datetime=(at != 0.0).then(|| iso(at))>{list_time(at)}</time>
                <span class="chat-row-preview">
                    {thread.preview_from_me.then(|| view! { <Ticks status=thread.preview_status.clone()/> })}
                    {(!thread.preview_sender.is_empty()).then(|| view! { <b>{thread.preview_sender.clone()}": "</b> })}
                    {icon.map(|icon| view! { <Icon name=icon/> })}
                    <span class="chat-row-text">{text}</span>
                </span>
                <span class="chat-row-marks">
                    {quiet.then(|| view! { <span title="Muted"><Icon name="bell"/><span class="sr-only">"Muted"</span></span> })}
                    {thread.pinned.then(|| view! { <span title="Pinned"><Icon name="pin"/><span class="sr-only">"Pinned"</span></span> })}
                    {(unread > 0).then(|| view! { <span class=if quiet { "chat-unread is-muted" } else { "chat-unread" }>{unread}<span class="sr-only">" unread"</span></span> })}
                </span>
            </button>
        </div>
    }
}

/// Discord servers in the sidebar: each opens to its channels, grouped by category (mock B).
#[component]
fn ServerTree(
    servers: RwSignal<Option<Vec<ChatServer>>>,
    selected: RwSignal<String>,
    collapsed: RwSignal<BTreeSet<String>>,
    on_toggle: Callback<String>,
    on_open: Callback<(ChatThread, web_sys::HtmlElement)>,
    voice: Signal<super::ChatVoiceServers>,
    call: Signal<Option<super::ChatVoiceCall>>,
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
                    let open = { let sid = sid.clone(); Memo::new(move |_| !collapsed.with(|c| c.contains(&sid))) };
                    let quiet = quiet_until(s.muted_until);
                    let unread = s.unread;
                    let name = s.name.clone();
                    let server_id = s.server_id.clone();
                    let channels = StoredValue::new(s.channels.clone());
                    let mentions = s.mentions;
                    view! {
                        <div class="chat-server">
                            <button type="button" class=move || format!("nav-item chat-server-row{}{}", if unread && !open.get() { " is-unread" } else { "" }, if quiet { " is-muted" } else { "" })
                                aria-expanded=move || open.get().to_string() on:click=move |_| { on_toggle.try_run(sid.clone()); }>
                                <span class=format!("chat-server-mark contact-initials {}", avatar_hue(&server_id)) aria-hidden="true">{initials(&name)}</span>
                                <span class="chat-row-title">{name.clone()}</span>
                                {(mentions > 0).then(|| view! { <span class="chat-mentions">{mentions}<span class="sr-only">" mentions"</span></span> })}
                                {move || if open.get() { view! { <Icon name="chevron" class="chat-server-chevron is-open"/> } } else { view! { <Icon name="chevron" class="chat-server-chevron"/> } }}
                            </button>
                            {move || open.get().then(|| {
                                let server_id = server_id.clone();
                                let list = channels.get_value();
                                let empty = list.is_empty();
                                let mut section = None::<String>;
                                let rows = list.into_iter().map(|c| {
                                    let heading = if section.as_deref() != Some(c.section.as_str()) {
                                        section = Some(c.section.clone());
                                        (!c.section.is_empty()).then(|| view! { <li class="chat-section" aria-hidden="true">{c.section.clone()}</li> })
                                    } else {
                                        None
                                    };
                                    let cid = c.id.clone();
                                    let is_voice = c.kind == "voice";
                                    let muted = quiet_until(c.muted_until);
                                    let jid = c.jid.clone();
                                    let sid = server_id.clone();
                                    let members = { let (jid, sid) = (jid.clone(), sid.clone()); Memo::new(move |_| if is_voice { voice.with(|v| v.get(&sid).and_then(|s| s.get(&jid)).cloned().unwrap_or_default()) } else { Vec::new() }) };
                                    let live = { let jid = jid.clone(); Memo::new(move |_| is_voice && call.with(|c| c.as_ref().is_some_and(|c| c.channel_id == jid))) };
                                    let current = { let cid = cid.clone(); Memo::new(move |_| selected.with(|s| *s == cid)) };
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
                                                if live.get() { ", you are in this call".to_owned() } else if count > 0 { format!(", {count} in voice") } else { String::new() },
                                                if mentions > 0 { format!(", {mentions} mentions") } else if unread { ", unread".into() } else { String::new() },
                                                if muted { ", muted" } else { "" }
                                            )
                                        }
                                    };
                                    let thread = StoredValue::new(c);
                                    view! {
                                        {heading}
                                        <li>
                                            <button type="button" class=move || format!("nav-item chat-channel{}{}{}{}", if current.get() { " active" } else { "" }, if unread { " is-unread" } else { "" }, if muted { " is-muted" } else { "" }, if live.get() { " is-live" } else { "" })
                                                aria-current=move || current.get().then_some("page") aria-label=label
                                                on:click=move |event| {
                                                    if let Some(el) = event.current_target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                                                        { on_open.try_run((thread.get_value(), el)); };
                                                    }
                                                }>
                                                <Icon name=if is_voice { "speaker" } else { "hash" }/><span class="chat-row-title">{name}</span>
                                                {(mentions > 0).then(|| view! { <span class="chat-mentions">{mentions}</span> })}
                                                {muted.then(|| view! { <Icon name="bell" class="chat-channel-muted"/> })}
                                            </button>
                                            {move || {
                                                let list = members.get();
                                                (!list.is_empty()).then(|| view! { <VoiceMembers members=list speaking=speaking.get() me=me.get()/> })
                                            }}
                                        </li>
                                    }
                                }).collect_view();
                                view! {
                                    <ul class="chat-channels" aria-label=format!("Channels in {}", name)>
                                        {rows}
                                        {empty.then(|| view! { <li class="chat-section">"No channels you can see"</li> })}
                                    </ul>
                                }
                            })}
                        </div>
                    }
                }).collect_view()}
            </nav>
        }
        .into_any()
    }
}
