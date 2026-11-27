//! Chat (`ChatWorkspace`): account-scoped WhatsApp and Discord workspace,
//! global keyboard shortcuts, persistent voice dock, preferences, lightbox, and live updates.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table};
use wasm_bindgen::JsCast;

use super::conversation::{Conversation, MediaLightbox};
use super::inspector::ChatInfoSheet;
use super::model::{
    BASE, ChatAccount, ChatMessage, ChatOverview, ChatPrefs, ChatServer, ChatSetting, ChatThread,
    ChatThreadPage, ChatView, Confirm, ConfirmAction, LightboxItem, QuickFilter, RunFuture,
    message_of, search_params,
};
use super::panes::{
    AddContactPane, LinkPane, NewChatPane, NewMessagePane, RenamePane, SaveMediaPane,
};
use super::settings::ChatSettings;
use super::sidebar::ChatSidebar;
use super::thread_list::ThreadList;
use super::voice::{CallCodeDialog, VoiceDock, VoiceStage, use_voice};
use crate::api::{self, BrowserSession};
use crate::live::Live;
use crate::module_bindings::*;
use crate::shared::context;
use crate::shell::WorkspaceHeader;
use crate::ui::{MenuAction, active_element, document, listen, unique_id, window};

#[derive(Clone)]
enum Pane {
    Link {
        account_id: Option<String>,
        platform: Option<String>,
    },
    NewChat,
    Settings(&'static str),
    Rename(ChatAccount),
    Save {
        thread_id: String,
        message: ChatMessage,
    },
    AddContact(ChatThread),
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
    let next = if query.is_empty() {
        "/chat".to_owned()
    } else {
        format!("/chat?{query}")
    };
    let location = window().location();
    let current = format!(
        "{}{}",
        location.pathname().unwrap_or_default(),
        location.search().unwrap_or_default()
    );
    if next != current {
        let _ = window().history().expect("history").replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&next),
        );
    }
}

/// Mobile navigation focus management inside the app-frame drawer (`useMobileNavigation`).
fn mobile_navigation(open: RwSignal<bool>, panel: NodeRef<html::Aside>) {
    let trigger = StoredValue::new_local(None::<web_sys::HtmlElement>);
    let was_open = StoredValue::new(false);
    Effect::new(move |_| {
        let is_open = open.get();
        if is_open {
            trigger.set_value(active_element());
            if let Some(el) = panel
                .get()
                .and_then(|r| r.query_selector("button,a").ok().flatten())
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
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
            let on_body = document().body().is_some_and(|b| {
                active
                    .as_ref()
                    .is_some_and(|a| a.unchecked_ref::<web_sys::Node>() == b.as_ref())
            });
            let inside = panel.get_untracked().is_some_and(|r| {
                r.contains(active.as_ref().map(|a| a.unchecked_ref::<web_sys::Node>()))
            });
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

fn editing_target(target: &Option<web_sys::EventTarget>) -> bool {
    let Some(el) = target
        .as_ref()
        .and_then(|t| t.dyn_ref::<web_sys::Element>())
    else {
        return false;
    };
    el.closest("input, textarea, select, [contenteditable='true'], [contenteditable='']")
        .ok()
        .flatten()
        .is_some()
        || el
            .closest("[role='dialog'], [data-editor-pane]")
            .ok()
            .flatten()
            .is_some()
}

#[derive(Clone, Copy)]
struct Chat {
    session: StoredValue<BrowserSession>,
    overview: RwSignal<Option<ChatOverview>>,
    load_error: RwSignal<String>,
    account_id: RwSignal<String>,
    view: RwSignal<ChatView>,
    quick_filter: RwSignal<QuickFilter>,
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
            let url = format!(
                "{BASE}/threads?{}",
                search_params(&[("accountId", &account.id), ("view", view.as_str())])
            );
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
        let Some(account) = self.account.get_untracked() else {
            return;
        };
        let Some(cursor) = self
            .page
            .with_untracked(|p| p.as_ref().and_then(|p| p.cursor.clone()))
        else {
            return;
        };
        if self.more.get_untracked() {
            return;
        }
        self.more.set(true);
        let ticket = self.generation.get_value();
        let view = self.view.get_untracked();
        spawn_local(async move {
            let url = format!(
                "{BASE}/threads?{}",
                search_params(&[
                    ("accountId", &account.id),
                    ("view", view.as_str()),
                    ("cursor", &cursor)
                ])
            );
            match api::get::<ChatThreadPage>(&url).await {
                Ok(next) => {
                    if self.generation.try_get_value() == Some(ticket) {
                        self.page.try_update(|p| {
                            if let Some(p) = p {
                                let fresh: Vec<ChatThread> = next
                                    .threads
                                    .into_iter()
                                    .filter(|t| !p.threads.iter().any(|o| o.id == t.id))
                                    .collect();
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
        self.quick_filter.set(QuickFilter::All);
    }

    fn open_thread(self, thread_id: String, from: Option<web_sys::HtmlElement>) {
        self.origin
            .set_value(from.map(send_wrapper::SendWrapper::new));
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
            match api::send::<Value>(&format!("{BASE}/chatSetting"), "POST", Some(body), &csrf)
                .await
            {
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
            MenuAction::new("Rename", "pencil")
                .run(move || self.pane.set(Some(Pane::Rename(a.clone()))))
        }];
        if a.state == "loggedOut" || a.state == "failed" {
            let (id, platform) = (a.id.clone(), a.platform.clone());
            actions.push(MenuAction::new("Link again", "link").run(move || {
                self.pane.set(Some(Pane::Link {
                    account_id: Some(id.clone()),
                    platform: Some(platform.clone()),
                }))
            }));
        }
        if a.state != "loggedOut" {
            let a = a.clone();
            actions.push(
                MenuAction::new(
                    if discord { "Sign out\u{2026}" } else { "Unlink\u{2026}" },
                    "logout",
                )
                .separator()
                .run(move || {
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
                            let result = api::send::<Unlinked>(
                                &format!("{BASE}/accounts/{}/unlink", a.id),
                                "POST",
                                Some(json!({})),
                                &csrf,
                            )
                            .await
                            .map_err(|c| c.message)?;
                            let discord = a.platform == "discord";
                            self.notice.try_set(if result.confirmed {
                                format!(
                                    "{} {}",
                                    a.name,
                                    if discord { "signed out" } else { "unlinked" }
                                )
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
                        title: if a.platform == "discord" {
                            format!("Sign {} out?", a.name)
                        } else {
                            format!("Unlink {}?", a.name)
                        },
                        text: if a.platform == "discord" {
                            "Dar ends its Discord session and stops receiving messages. Messages stay in Dar until you delete the account.".into()
                        } else {
                            "Dar stops receiving messages from this account and removes itself from your phone\u{2019}s linked devices. Chat history stays in Dar until you delete it.".into()
                        },
                        action: if a.platform == "discord" {
                            "Sign out".into()
                        } else {
                            "Unlink".into()
                        },
                        run,
                    }));
                }),
            );
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
                    api::send::<Value>(&format!("{BASE}/accounts/{id}"), "DELETE", None, &csrf)
                        .await
                        .map_err(|c| c.message)?;
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

/// Account-scoped Chat workspace with WhatsApp chats, Discord servers & DMs, voice calls,
/// inline markdown & links, voice note player, media lightbox, inspector tabs, and Chat settings.
#[component]
pub fn ChatWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let start = read_view();
    let start_chat = StoredValue::new(start.chat.clone());
    let start_message = StoredValue::new(start.message.clone());
    let overview: RwSignal<Option<ChatOverview>> = RwSignal::new(None);
    let account_id = RwSignal::new(start.account);
    let accounts = Memo::new(move |_| {
        overview.with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default())
    });
    let account = Memo::new(move |_| {
        let id = account_id.get();
        accounts.with(|list| {
            list.iter()
                .find(|a| a.id == id)
                .or_else(|| list.first())
                .cloned()
        })
    });
    let quick_filter = RwSignal::new(QuickFilter::All);
    let chat = Chat {
        session: StoredValue::new(session.clone()),
        overview,
        load_error: RwSignal::new(String::new()),
        account_id,
        view: RwSignal::new(start.view),
        quick_filter,
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
    let all_servers: RwSignal<BTreeMap<String, Vec<ChatServer>>> = RwSignal::new(BTreeMap::new());
    let pending_voice_thread: RwSignal<Option<String>> = RwSignal::new(None);
    let drafts: RwSignal<BTreeMap<String, String>> = RwSignal::new(BTreeMap::new());
    let lightbox: RwSignal<Option<LightboxItem>> = RwSignal::new(None);
    let code_for = RwSignal::new(String::new());

    let shared_ctx = context();
    let prefs = Memo::new(move |_| {
        shared_ctx
            .preferences
            .value
            .with(|p| ChatPrefs::from_preferences(p.as_ref()))
    });

    let discord =
        Memo::new(move |_| account.with(|a| a.as_ref().is_some_and(|a| a.platform == "discord")));
    let account_key =
        Memo::new(move |_| account.with(|a| a.as_ref().map(|a| a.id.clone()).unwrap_or_default()));
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
        if let Ok(handle) = set_timeout_with_handle(
            move || {
                chat.notice.try_set(String::new());
            },
            Duration::from_millis(5000),
        ) {
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

    // Discord: servers and their channels for the sidebar, cached across account switches.
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
            let result = api::get::<Servers>(&format!(
                "{BASE}/servers?{}",
                search_params(&[("accountId", &id)])
            ))
            .await;
            if !gone.load(Ordering::Relaxed) {
                let list = result.map(|r| r.servers).unwrap_or_default();
                all_servers.try_update(|map| {
                    map.insert(id, list.clone());
                });
                servers.try_set(Some(list));
            }
        });
    });

    Effect::new(move |_| write_view(&account_key.get(), chat.view.get(), &chat.selected.get()));

    // While linking or importing, poll gently until sync catches up.
    let busy = Memo::new(move |_| {
        accounts.with(|list| list.iter().any(|a| a.importing || a.state == "connecting"))
    });
    Effect::new(move |_| {
        if !busy.get() {
            return;
        }
        if let Ok(handle) =
            set_interval_with_handle(move || chat.load_overview(), Duration::from_millis(5000))
        {
            on_cleanup(move || handle.clear());
        }
    });

    // SpacetimeDB live change feed: reload overview, active threads, and servers when rows change.
    let live = expect_context::<Live>();
    Effect::new(move |_| {
        let Some(c) = live.connection.get() else {
            return;
        };
        let disposed = Arc::new(AtomicBool::new(false));
        let revision = Arc::new(std::sync::atomic::AtomicI64::new(-1));
        let timer: StoredValue<Option<TimeoutHandle>> = StoredValue::new(None);
        let bump = {
            let disposed = disposed.clone();
            move || {
                if let Some(Some(handle)) = timer.try_get_value() {
                    handle.clear();
                }
                let disposed = disposed.clone();
                let handle = set_timeout_with_handle(
                    move || {
                        if !disposed.load(Ordering::Relaxed) {
                            chat.epoch.try_update(|n| *n += 1);
                        }
                    },
                    Duration::from_millis(250),
                )
                .ok();
                timer.try_set_value(handle);
            }
        };
        let changed = {
            let (disposed, revision, bump) = (disposed.clone(), revision.clone(), bump.clone());
            move |ctx: &EventContext| {
                if disposed.load(Ordering::Relaxed) {
                    return;
                }
                let next = ctx
                    .db
                    .my_chat_changes()
                    .iter()
                    .last()
                    .map_or(0, |r| r.revision as i64);
                let before = revision.swap(next, Ordering::Relaxed);
                if before >= 0 && next != before {
                    bump();
                }
            }
        };
        let inserted = c.db.my_chat_changes().on_insert({
            let changed = changed.clone();
            move |ctx, _| changed(ctx)
        });
        let deleted = c.db.me().on_delete({
            let disposed = disposed.clone();
            move |ctx: &EventContext, _| {
                if !disposed.load(Ordering::Relaxed)
                    && !matches!(ctx.event, spacetimedb_sdk::Event::UnsubscribeApplied)
                {
                    on_signed_out.try_run(());
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
                    let next = ctx
                        .db
                        .my_chat_changes()
                        .iter()
                        .last()
                        .map_or(0, |r| r.revision as i64);
                    revision.store(next, Ordering::Relaxed);
                }
            })
            .on_error(|_, _| {})
            .subscribe([
                "SELECT * FROM me",
                "SELECT * FROM my_chat_changes",
                "SELECT * FROM my_pim_changes",
            ]);
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

    let all = Memo::new(move |_| {
        chat.page.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.pinned
                        .iter()
                        .chain(p.threads.iter())
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let channel = Memo::new(move |_| {
        let id = chat.selected.get();
        servers.with(|s| {
            s.as_ref().and_then(|s| {
                s.iter()
                    .flat_map(|s| s.channels.iter())
                    .find(|t| t.id == id)
                    .cloned()
            })
        })
    });
    let current = Memo::new(move |_| {
        let id = chat.selected.get();
        channel
            .get()
            .or_else(|| all.with(|all| all.iter().find(|t| t.id == id).cloned()))
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
                .or_else(|| {
                    v.accounts
                        .get(&id)
                        .filter(|a| !id.is_empty() && a.call.is_some())
                        .map(|a| (id.clone(), a.clone()))
                })
        })
    });
    let call_account = Memo::new(move |_| {
        call_entry.with(|e| e.as_ref().map(|e| e.0.clone()).unwrap_or_default())
    });
    let the_call =
        Memo::new(move |_| call_entry.with(|e| e.as_ref().and_then(|e| e.1.call.clone())));

    // Populate cross-account servers if a background Discord account already has an active call.
    Effect::new(move |_| {
        let cid = call_account.get();
        if cid.is_empty() || all_servers.with(|m| m.contains_key(&cid)) {
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
            if let Ok(resp) = api::get::<Servers>(&format!(
                "{BASE}/servers?{}",
                search_params(&[("accountId", &cid)])
            ))
            .await
                && !gone.load(Ordering::Relaxed)
            {
                all_servers.try_update(|map| {
                    map.insert(cid, resp.servers);
                });
            }
        });
    });

    // Resolve server & channel for the voice dock across ANY linked Discord account.
    let call_server = Memo::new(move |_| {
        let cid = call_account.get();
        let server_id = the_call.with(|c| c.as_ref().map(|c| c.server_id.clone()))?;
        all_servers.with(|map| {
            map.get(&cid)
                .and_then(|list| list.iter().find(|s| s.server_id == server_id).cloned())
        })
    });
    let call_thread = Memo::new(move |_| {
        let channel_id = the_call.with(|c| c.as_ref().map(|c| c.channel_id.clone()))?;
        call_server.with(|s| {
            s.as_ref()
                .and_then(|s| s.channels.iter().find(|c| c.jid == channel_id).cloned())
        })
    });
    let call_channel = Memo::new(move |_| {
        call_thread
            .with(|t| t.as_ref().map(|t| t.name.clone()))
            .unwrap_or_else(|| "Voice".into())
    });
    let call_server_name = Memo::new(move |_| {
        call_server
            .with(|s| s.as_ref().map(|s| s.name.clone()))
            .unwrap_or_else(|| {
                let id = call_account.get();
                accounts
                    .with(|list| list.iter().find(|a| a.id == id).map(|a| a.name.clone()))
                    .unwrap_or_else(|| "Discord".into())
            })
    });

    // Complete pending cross-account navigation into a voice channel once servers load.
    Effect::new(move |_| {
        let Some(target_thread) = pending_voice_thread.get() else {
            return;
        };
        let target_acc = call_account.get();
        if account_key.get() == target_acc && servers.with(Option::is_some) {
            pending_voice_thread.set(None);
            chat.open_thread(target_thread, None);
        }
    });

    let voice_run = move |work: RunFuture| {
        spawn_local(async move {
            if let Err(message) = work.await {
                chat.notice.try_set(message);
            }
        });
    };
    let set_self = move |id: String| {
        Callback::new(move |(muted, deafened): (bool, bool)| {
            voice_run(Box::pin(voice.set_self(id.clone(), muted, deafened)));
        })
    };
    let open_call = move || {
        let target = call_account.get_untracked();
        let thread = call_thread.get_untracked();
        if target != account_key.get_untracked() {
            if let Some(t) = thread {
                pending_voice_thread.set(Some(t.id));
            }
            chat.choose_account(target);
            return;
        }
        if let Some(t) = thread {
            chat.open_thread(t.id, None);
        }
    };
    let render_dock = Callback::new(move |class: &'static str| {
        let Some(call) = the_call.get() else {
            return ().into_any();
        };
        let Some((id, entry)) = call_entry.get() else {
            return ().into_any();
        };
        let id_set = id.clone();
        let id_leave = id.clone();
        let id_here = id.clone();
        let id_code = id;
        view! {
            <VoiceDock
                call=call
                channel=call_channel.get()
                server=call_server_name.get()
                here=entry.here
                elsewhere=entry.elsewhere
                mic=voice_state.with(|v| v.mic)
                class=class
                on_set=set_self(id_set)
                on_leave=Callback::new(move |_| voice_run(Box::pin(voice.leave(id_leave.clone()))))
                on_use_here=Callback::new(move |_| voice_run(Box::pin(voice.use_here(id_here.clone()))))
                on_open=Callback::new(move |_| open_call())
                on_code=Callback::new(move |_| code_for.set(id_code.clone()))
            />
        }
        .into_any()
    });

    // Global keyboard shortcuts inside Chat (Alt+1..9 accounts, N new, I info, J/K next/prev chat).
    listen(&document(), "keydown", false, move |event| {
        let e: web_sys::KeyboardEvent = event.unchecked_into();
        if e.default_prevented() || e.is_composing() {
            return;
        }
        if chat.pane.with_untracked(Option::is_some)
            || chat.confirm.with_untracked(Option::is_some)
            || lightbox.with_untracked(Option::is_some)
            || !code_for.with_untracked(String::is_empty)
        {
            return;
        }
        if e.alt_key() && !e.ctrl_key() && !e.meta_key() {
            let key = e.key();
            if key.len() == 1
                && let Some(idx) = key
                    .chars()
                    .next()
                    .and_then(|c| c.to_digit(10))
                    .and_then(|d| d.checked_sub(1))
                && let Some(target) = accounts.with_untracked(|list| list.get(idx as usize).cloned())
            {
                e.prevent_default();
                chat.choose_account(target.id);
                return;
            }
        }
        if editing_target(&e.target()) || e.ctrl_key() || e.meta_key() || e.alt_key() {
            return;
        }
        match e.key().as_str() {
            "n" | "N" => {
                let writable = account.with_untracked(|a| {
                    a.as_ref()
                        .is_some_and(|a| a.state != "loggedOut" && a.state != "linking")
                });
                if writable {
                    e.prevent_default();
                    chat.pane.set(Some(Pane::NewChat));
                }
            }
            "i" | "I" => {
                if !chat.selected.with_untracked(String::is_empty) && has_account.get_untracked() {
                    e.prevent_default();
                    info.update(|v| *v = !*v);
                }
            }
            "j" | "J" | "k" | "K" => {
                let forward = matches!(e.key().as_str(), "j" | "J");
                let visible: Vec<String> = all.with_untracked(|list| {
                    let filter_on = !discord.get_untracked()
                        && has_account.get_untracked()
                        && chat.view.get_untracked() == ChatView::Chats;
                    let f = quick_filter.get_untracked();
                    list.iter()
                        .filter(|t| !filter_on || f == QuickFilter::All || f.matches(t))
                        .map(|t| t.id.clone())
                        .collect()
                });
                if visible.is_empty() {
                    return;
                }
                e.prevent_default();
                let cur = chat.selected.get_untracked();
                let next_id = match visible.iter().position(|id| *id == cur) {
                    Some(i) => {
                        let len = visible.len();
                        if forward {
                            visible[(i + 1) % len].clone()
                        } else {
                            visible[(i + len - 1) % len].clone()
                        }
                    }
                    None => visible[0].clone(),
                };
                chat.open_thread(next_id, None);
            }
            _ => {}
        }
    });

    let link_pane = move |account_id: Option<String>, platform: Option<String>| {
        chat.pane.set(Some(Pane::Link {
            account_id,
            platform,
        }))
    };

    let title = Memo::new(move |_| {
        if discord.get() {
            "Direct messages".to_owned()
        } else {
            chat.view.get().label().to_owned()
        }
    });

    let empty_text = Callback::new(move |_: ()| {
        let a = account.get()?;
        let view = chat.view.get();
        let d = a.platform == "discord";
        let heading = if a.importing {
            "Importing your chats\u{2026}"
        } else if a.state == "linking" {
            if d {
                "Waiting for Discord"
            } else {
                "Waiting for WhatsApp"
            }
        } else if d {
            "No direct messages yet"
        } else {
            match view {
                ChatView::Unread => "All caught up",
                ChatView::Archived => "No archived chats",
                ChatView::Groups => "No groups",
                ChatView::Chats => "No chats yet",
            }
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
    });

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
        if prefs.with(ChatPrefs::is_compact) {
            class.push_str(" is-compact");
        }
        class
    };

    let is_voice = Memo::new(move |_| {
        discord.get() && current.with(|c| c.as_ref().is_some_and(|c| c.kind == "voice"))
    });
    let channel_selected = Memo::new(move |_| channel.with(Option::is_some));
    let adding_account = Memo::new(move |_| matches!(chat.pane.get(), Some(Pane::Link { .. })));
    let session_sv = StoredValue::new(session.clone());

    view! {
        <div class=shell_class>
            <WorkspaceHeader
                app="chat"
                session=session.clone()
                on_signed_out=on_signed_out
                on_settings=Callback::new(move |_| chat.pane.set(Some(Pane::Settings("Accounts"))))
            />

            <div class="pim-workspace chat-workspace">
                <ChatSidebar
                    navigation_id=navigation_id
                    navigation_ref=navigation_ref
                    navigation=chat.navigation
                    accounts=accounts
                    account=account
                    account_key=account_key
                    adding=adding_account
                    discord=discord
                    has_account=has_account
                    view=chat.view
                    selected=chat.selected
                    channel_selected=channel_selected
                    servers=servers
                    voice=Signal::derive(move || my_voice.with(|v| v.as_ref().map(|v| v.servers.clone()).unwrap_or_default()))
                    call=Signal::derive(move || my_voice.with(|v| v.as_ref().and_then(|v| v.call.clone())))
                    speaking=Signal::derive(move || voice_state.with(|v| v.speaking.clone()))
                    me=Signal::derive(move || account.with(|a| a.as_ref().map(|a| a.jid.clone()).unwrap_or_default()))
                    render_dock=render_dock
                    on_new_chat=Callback::new(move |_| chat.pane.set(Some(Pane::NewChat)))
                    on_choose_account=Callback::new(move |id| chat.choose_account(id))
                    on_link=Callback::new(move |(acc, plat)| link_pane(acc, plat))
                    on_choose_view=Callback::new(move |v| {
                        chat.view.set(v);
                        chat.quick_filter.set(QuickFilter::All);
                        chat.navigation.set(false);
                    })
                    on_direct_messages=Callback::new(move |_| {
                        chat.view.set(ChatView::Chats);
                        if channel.with_untracked(Option::is_some) {
                            chat.selected.set(String::new());
                        }
                        chat.navigation.set(false);
                    })
                    on_open_channel=Callback::new(move |(thread, el): (ChatThread, web_sys::HtmlElement)| {
                        chat.open_thread(thread.id, Some(el));
                    })
                    on_settings=Callback::new(move |_| chat.pane.set(Some(Pane::Settings("Accounts"))))
                    account_actions=Callback::new(move |a: ChatAccount| chat.account_actions(a))
                />

                <ThreadList
                    title=title
                    account=account
                    accounts=accounts
                    account_key=account_key
                    adding=adding_account
                    overview=overview
                    load_error=chat.load_error
                    list_state=chat.list_state
                    list_error=chat.list_error
                    view=chat.view
                    quick_filter=quick_filter
                    discord=discord
                    has_account=has_account
                    selected=chat.selected
                    threads=all
                    has_more=Signal::derive(move || chat.page.with(|p| p.as_ref().is_some_and(|p| p.cursor.is_some())))
                    loading_more=chat.more
                    empty_text=empty_text
                    render_dock=render_dock
                    on_choose_account=Callback::new(move |id| chat.choose_account(id))
                    on_link=Callback::new(move |plat: Option<String>| link_pane(None, plat))
                    on_open=Callback::new(move |(id, el): (String, web_sys::HtmlElement)| {
                        chat.open_thread(id, Some(el));
                    })
                    on_load_more=Callback::new(move |_| chat.load_more())
                    on_retry_overview=Callback::new(move |_| {
                        chat.load_overview();
                        chat.load_threads(false);
                    })
                    on_retry_threads=Callback::new(move |_| chat.load_threads(false))
                    on_setting=Callback::new(move |(tid, change): (String, ChatSetting)| {
                        chat.set_chat(tid, change);
                    })
                    on_open_shortcuts=Callback::new(move |_| chat.pane.set(Some(Pane::Settings("Keyboard shortcuts"))))
                />

                {
                    let voice_selected = Memo::new(move |_| {
                        let selected = chat.selected.get();
                        (!selected.is_empty() && is_voice.get()).then_some(selected)
                    });
                    move || {
                        let _selected = voice_selected.get()?;
                        let a = account.get_untracked()?;
                        let thread = current.get_untracked()?;
                        let thread_id = thread.id.clone();
                        let server_of = move || {
                            servers.with(|s| {
                                s.as_ref().and_then(|s| {
                                    s.iter()
                                        .find(|s| s.channels.iter().any(|c| c.id == thread_id))
                                        .cloned()
                                })
                            })
                        };
                        let server_of = StoredValue::new(server_of());
                        let jid = thread.jid.clone();
                        let jid2 = jid.clone();
                        let a_id = StoredValue::new(a.id.clone());
                        let a_jid = a.jid.clone();
                        let join_thread = StoredValue::new(thread.id.clone());
                        Some(view! {
                            <aside class="pim-details chat-details" aria-label=thread.name.clone()>
                                <VoiceStage
                                    thread_name=thread.name.clone()
                                    thread_jid=thread.jid.clone()
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
                                        spawn_local(async move {
                                            done.try_run(voice.join(id, thread, muted).await);
                                        });
                                    })
                                    on_set=set_self(a.id.clone())
                                    on_leave=Callback::new(move |_| voice_run(Box::pin(voice.leave(a_id.get_value()))))
                                    on_code=Callback::new(move |_| code_for.set(a_id.get_value()))
                                    on_use_here=Callback::new(move |_| voice_run(Box::pin(voice.use_here(a_id.get_value()))))
                                    on_close=Callback::new(move |_| chat.close_thread())
                                />
                            </aside>
                        })
                    }
                }

                {
                    let conv_selected = Memo::new(move |_| {
                        let selected = chat.selected.get();
                        (!selected.is_empty() && !is_voice.get() && has_account.get()).then_some(selected)
                    });
                    move || {
                        let selected = conv_selected.get()?;
                        let a = account.get_untracked()?;
                        let focus = if selected == start_chat.get_value() {
                            start_message.get_value()
                        } else {
                            String::new()
                        };
                        let setting_id = selected.clone();
                        let save_id = selected.clone();
                        Some(view! {
                            <aside
                                class="pim-details chat-details"
                                aria-label=move || current.with(|c| c.as_ref().map(|c| c.name.clone())).unwrap_or_else(|| "Conversation".into())
                            >
                                <Conversation
                                    thread_id=selected
                                    focus_message=focus
                                    account=Signal::derive(move || account.get().unwrap_or_else(|| a.clone()))
                                    session=session_sv.get_value()
                                    epoch=chat.epoch
                                    info=info
                                    prefs=prefs
                                    drafts=drafts
                                    on_close=Callback::new(move |_| chat.close_thread())
                                    on_notice=Callback::new(move |text: String| chat.notice.set(text))
                                    on_save=Callback::new(move |message: ChatMessage| {
                                        chat.pane.set(Some(Pane::Save {
                                            thread_id: save_id.clone(),
                                            message,
                                        }))
                                    })
                                    on_info=Callback::new(move |_| info.update(|i| *i = !*i))
                                    on_setting=Callback::new(move |change: ChatSetting| chat.set_chat(setting_id.clone(), change))
                                    on_lightbox=Callback::new(move |item: LightboxItem| lightbox.set(Some(item)))
                                />
                            </aside>
                        })
                    }
                }

                {
                    let info_selected = Memo::new(move |_| {
                        let selected = chat.selected.get();
                        (!selected.is_empty() && has_account.get() && info.get()).then_some(selected)
                    });
                    move || info_selected.get().map(|selected| {
                        let setting_id = selected.clone();
                        view! {
                            <aside class="chat-info-sheet" aria-label="Chat info">
                                <ChatInfoSheet
                                    thread_id=selected
                                    session=session_sv.get_value()
                                    epoch=chat.epoch
                                    on_close=Callback::new(move |_| info.set(false))
                                    on_setting=Callback::new(move |change: ChatSetting| chat.set_chat(setting_id.clone(), change))
                                    on_add_contact=Callback::new(move |thread: ChatThread| chat.pane.set(Some(Pane::AddContact(thread))))
                                    on_lightbox=Callback::new(move |item: LightboxItem| lightbox.set(Some(item)))
                                />
                            </aside>
                        }
                    })
                }

                {
                    let show_placeholder =
                        Memo::new(move |_| chat.selected.with(String::is_empty) && has_account.get());
                    move || show_placeholder.get().then(|| view! {
                        <div class="chat-placeholder" aria-hidden="true">
                            <span class="chat-empty-icon"><crate::icons::Icon name="chat"/></span>
                            <h2>"Choose a conversation"</h2>
                            <p>"Pick a chat from the list or press J / K to move through conversations."</p>
                        </div>
                    })
                }
            </div>

            {move || chat.pane.get().map(|pane| {
                let session = session_sv.get_value();
                let close = Callback::new(move |_| chat.pane.set(None));
                match pane {
                    Pane::Link { account_id, platform } => view! {
                        <LinkPane
                            session=session
                            account_id=account_id
                            platform=platform
                            on_close=close
                            on_linked=Callback::new(move |id: String| {
                                chat.pane.set(None);
                                chat.choose_account(id);
                                chat.load_overview();
                                chat.notice.set("Linked. Your chats are on their way.".into());
                            })
                        />
                    }.into_any(),
                    Pane::NewChat => account.get().map(|a| {
                        if a.platform == "discord" {
                            view! {
                                <NewMessagePane
                                    session=session
                                    account=a
                                    on_close=close
                                    on_opened=Callback::new(move |thread: ChatThread| {
                                        chat.pane.set(None);
                                        chat.open_thread(thread.id, None);
                                        chat.load_threads(true);
                                    })
                                />
                            }.into_any()
                        } else {
                            view! {
                                <NewChatPane
                                    session=session
                                    account=a
                                    on_close=close
                                    on_opened=Callback::new(move |thread: ChatThread| {
                                        chat.pane.set(None);
                                        chat.open_thread(thread.id, None);
                                        chat.load_threads(true);
                                    })
                                />
                            }.into_any()
                        }
                    }).into_any(),
                    Pane::Save { thread_id, message } => view! {
                        <SaveMediaPane
                            session=session
                            thread_id=thread_id
                            message=message
                            platform=account.get().map(|a| a.platform).unwrap_or_else(|| "whatsapp".into())
                            on_close=close
                            on_saved=Callback::new(move |name: String| {
                                chat.pane.set(None);
                                chat.notice.set(format!("Saved to Files as {name}"));
                            })
                        />
                    }.into_any(),
                    Pane::AddContact(thread) => view! {
                        <AddContactPane
                            session=session
                            thread=thread
                            on_close=close
                            on_saved=Callback::new(move |name: String| {
                                chat.pane.set(None);
                                chat.epoch.update(|n| *n += 1);
                                chat.notice.set(format!("{name} added to Contacts"));
                            })
                        />
                    }.into_any(),
                    Pane::Rename(a) => view! {
                        <RenamePane
                            session=session
                            account=a
                            on_close=close
                            on_saved=Callback::new(move |_| {
                                chat.pane.set(None);
                                chat.load_overview();
                            })
                        />
                    }.into_any(),
                    Pane::Settings(initial_section) => view! {
                        <ChatSettings
                            session=session
                            initial_section=initial_section
                            accounts=accounts
                            on_close=close
                            on_link=Callback::new(move |_| link_pane(None, None))
                            actions=Callback::new(move |a: ChatAccount| chat.account_actions(a))
                        />
                    }.into_any(),
                }
            })}

            {move || lightbox.get().map(|item| {
                let save_tid = chat.selected.get_untracked();
                let can_save = !save_tid.is_empty();
                view! {
                    <MediaLightbox
                        item=item
                        can_save=can_save
                        on_save=Callback::new(move |message: ChatMessage| {
                            if !save_tid.is_empty() {
                                chat.pane.set(Some(Pane::Save {
                                    thread_id: save_tid.clone(),
                                    message,
                                }));
                            }
                        })
                        on_close=Callback::new(move |_| lightbox.set(None))
                    />
                }
            })}

            {move || chat.confirm.get().map(|confirm| view! {
                <ConfirmAction
                    confirm=confirm
                    on_close=Callback::new(move |_| chat.confirm.set(None))
                />
            })}

            {move || (!code_for.with(String::is_empty)).then(|| view! {
                <CallCodeDialog
                    hub=voice
                    account_id=code_for.get_untracked()
                    on_close=Callback::new(move |_| code_for.set(String::new()))
                />
            })}

            <div class="chat-toast-region" role="status" aria-live="polite">
                {move || (!chat.notice.with(String::is_empty)).then(|| view! {
                    <div class="settings-toast chat-toast">
                        <span>{chat.notice.get()}</span>
                        <button type="button" class="text-button" on:click=move |_| chat.notice.set(String::new())>
                            "Dismiss"
                        </button>
                    </div>
                })}
            </div>
        </div>
    }
}
