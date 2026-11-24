//! The Mail workspace: sidebar, message list, reader and the panes they open.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use super::compose::{ComposeSheet, ComposeSpec, InlineReply};
use super::panes::{AddAccountPane, MailSettings};
use super::*;
use crate::api::{self, BrowserSession};
use crate::dates::{days_ago, local_date, local_date_time, local_time};
use crate::icons::Icon;
use crate::shared::context;
use crate::shell::WorkspaceHeader;
use crate::ui::{ActionMenu, Dialog, MenuAction, document, listen, unique_id, window};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum View {
    Unified(&'static str),
    Mailbox(String),
}

impl View {
    fn query(&self) -> String {
        match self {
            View::Unified(v) => format!("view={v}"),
            View::Mailbox(id) => format!("view=mailbox&id={}", encode(id)),
        }
    }
}

const UNIFIED: [(&str, &str, &str); 6] = [
    ("inbox", "All inboxes", "mail"),
    ("unread", "Unread", "unread"),
    ("flagged", "Flagged", "flag"),
    ("drafts", "Drafts", "pencil"),
    ("sent", "Sent", "send"),
    ("all", "All mail", "archive"),
];

#[derive(Clone)]
pub(crate) enum Pane {
    AddAccount(Option<String>),
    Settings(&'static str),
}

#[derive(Clone)]
pub(crate) struct Toast {
    pub message: String,
    pub action: Option<(&'static str, Arc<dyn Fn() + Send + Sync>)>,
    pub error: bool,
}

#[derive(Clone)]
struct Confirm {
    title: String,
    message: String,
    action: &'static str,
    run: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone, Copy)]
pub(crate) struct MailState {
    pub overview: RwSignal<Option<Overview>>,
    load_error: RwSignal<String>,
    view: RwSignal<View>,
    items: RwSignal<Vec<Summary>>,
    cursor: RwSignal<Option<String>>,
    list_state: RwSignal<&'static str>,
    list_error: RwSignal<String>,
    selected: RwSignal<Option<String>>,
    detail: RwSignal<Option<Detail>>,
    detail_error: RwSignal<String>,
    checked: RwSignal<HashSet<String>>,
    pub pane: RwSignal<Option<Pane>>,
    pub compose: RwSignal<Option<ComposeSpec>>,
    pub toast: RwSignal<Option<Toast>>,
    confirm: RwSignal<Option<Confirm>>,
    syncing: RwSignal<bool>,
    collapsed: RwSignal<HashSet<String>>,
    navigation: RwSignal<bool>,
    generation: StoredValue<u32>,
    pub session: StoredValue<BrowserSession>,
    pub images: Memo<String>,
    mark_read: Memo<String>,
    sending: StoredValue<u32>,
}

pub(crate) fn day_label(time: f64) -> String {
    let diff = days_ago(time);
    if diff <= 0 {
        "Today".into()
    } else if diff == 1 {
        "Yesterday".into()
    } else if diff < 7 {
        local_date(time, &[("weekday", "long")])
    } else if js_sys::Date::new(&JsValue::from_f64(time)).get_full_year()
        == js_sys::Date::new_0().get_full_year()
    {
        local_date(time, &[("day", "numeric"), ("month", "long")])
    } else {
        local_date(
            time,
            &[("day", "numeric"), ("month", "long"), ("year", "numeric")],
        )
    }
}

fn time_label(time: f64) -> String {
    if days_ago(time) <= 0 {
        local_time(time, &[("hour", "numeric"), ("minute", "2-digit")])
    } else if days_ago(time) < 7 {
        local_date(time, &[("weekday", "short")])
    } else {
        local_date(time, &[("month", "short"), ("day", "numeric")])
    }
}

fn relative(time: f64) -> String {
    let mins = ((js_sys::Date::now() - time) / 60_000.0).round() as i64;
    if time <= 0.0 {
        "not yet".into()
    } else if mins < 1 {
        "just now".into()
    } else if mins < 60 {
        format!("{mins} min ago")
    } else if mins < 1440 {
        format!("{} h ago", mins / 60)
    } else {
        local_date(time, &[])
    }
}

fn read_url() -> (View, Option<String>, HashMap<String, String>) {
    let params =
        web_sys::UrlSearchParams::new_with_str(&window().location().search().unwrap_or_default())
            .ok();
    let get = |k: &str| {
        params
            .as_ref()
            .and_then(|p| p.get(k))
            .filter(|v| !v.is_empty())
    };
    let view = match get("view").as_deref() {
        Some("mailbox") => get("id")
            .map(View::Mailbox)
            .unwrap_or(View::Unified("inbox")),
        Some(v) => UNIFIED
            .iter()
            .find(|(id, ..)| *id == v)
            .map(|(id, ..)| View::Unified(id))
            .unwrap_or(View::Unified("inbox")),
        None => View::Unified("inbox"),
    };
    let mut extra = HashMap::new();
    for k in ["account", "oauthError"] {
        if let Some(v) = get(k) {
            extra.insert(k.to_owned(), v);
        }
    }
    (view, get("message"), extra)
}

fn write_url(view: &View, message: Option<&str>) {
    let mut query = view.query();
    if let Some(id) = message {
        query.push_str(&format!("&message={}", encode(id)));
    }
    let next = if query == "view=inbox" {
        "/mail".to_owned()
    } else {
        format!("/mail?{query}")
    };
    let loc = window().location();
    if format!(
        "{}{}",
        loc.pathname().unwrap_or_default(),
        loc.search().unwrap_or_default()
    ) != next
        && let Ok(history) = window().history()
    {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&next));
    }
}

impl MailState {
    pub fn csrf(self) -> String {
        self.session.with_value(|s| s.csrf_token.clone())
    }

    pub fn notify(self, message: impl Into<String>) {
        let _ = self.toast.try_set(Some(Toast {
            message: message.into(),
            action: None,
            error: false,
        }));
    }

    pub fn fail(self, message: impl Into<String>) {
        let _ = self.toast.try_set(Some(Toast {
            message: message.into(),
            action: None,
            error: true,
        }));
    }

    pub fn account(self, id: &str) -> Option<Account> {
        self.overview.with_untracked(|o| {
            o.as_ref()
                .and_then(|o| o.accounts.iter().find(|a| a.id == id).cloned())
        })
    }

    fn mailbox(self, id: &str) -> Option<(Account, Mailbox)> {
        self.overview.with_untracked(|o| {
            o.as_ref().and_then(|o| {
                o.accounts.iter().find_map(|a| {
                    a.mailboxes
                        .iter()
                        .find(|m| m.id == id)
                        .map(|m| (a.clone(), m.clone()))
                })
            })
        })
    }

    pub async fn fetch_overview(self) {
        match api::get::<Overview>(&format!("{BASE}/overview")).await {
            Ok(o) => {
                if let View::Mailbox(id) = self.view.get_untracked()
                    && !o
                        .accounts
                        .iter()
                        .any(|a| a.mailboxes.iter().any(|m| m.id == id))
                {
                    let _ = self.view.try_set(View::Unified("inbox"));
                }
                let _ = self.overview.try_set(Some(o));
                let _ = self.load_error.try_set(String::new());
            }
            Err(e) => {
                let _ = self.load_error.try_set(e.message);
            }
        }
    }

    pub fn reload(self) {
        spawn_local(async move {
            self.fetch_overview().await;
            self.fetch_items(true).await;
        });
    }

    async fn fetch_items(self, reset: bool) {
        let generation = self.generation.get_value() + 1;
        self.generation.set_value(generation);
        let Some(view) = self.view.try_get_untracked() else {
            return;
        };
        let cursor = if reset {
            None
        } else {
            self.cursor.get_untracked()
        };
        if reset && self.items.with_untracked(Vec::is_empty) {
            let _ = self.list_state.try_set("loading");
        }
        let mut url = format!("{BASE}/messages?{}", view.query());
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={}", encode(c)));
        }
        let result = api::get::<Page>(&url).await;
        if self.generation.try_get_value() != Some(generation) {
            return;
        }
        match result {
            Ok(page) => {
                let _ = self.items.try_update(|items| {
                    if reset {
                        *items = page.items;
                    } else {
                        items.extend(page.items);
                    }
                });
                let _ = self.cursor.try_set(page.cursor);
                let _ = self.list_state.try_set("ready");
            }
            Err(e) => {
                let _ = self.list_error.try_set(e.message);
                let _ = self.list_state.try_set("error");
            }
        }
    }

    pub fn load_items(self) {
        spawn_local(self.fetch_items(true));
    }

    pub fn choose(self, view: View) {
        self.view.set(view.clone());
        self.selected.set(None);
        self.detail.set(None);
        self.checked.set(HashSet::new());
        self.navigation.set(false);
        self.items.set(Vec::new());
        write_url(&view, None);
        self.load_items();
    }

    pub fn open(self, id: String) {
        self.selected.set(Some(id.clone()));
        self.detail_error.set(String::new());
        write_url(&self.view.get_untracked(), Some(&id));
        let images = self.images.get_untracked() == "load";
        spawn_local(async move {
            let result = api::get::<Detail>(&format!(
                "{BASE}/messages/{}{}",
                encode(&id),
                if images { "?images=1" } else { "" }
            ))
            .await;
            if self.selected.try_get_untracked().flatten().as_deref() != Some(id.as_str()) {
                return;
            }
            match result {
                Ok(d) => {
                    let unseen = !d.summary.seen;
                    let _ = self.detail.try_set(Some(d));
                    if unseen && self.mark_read.get_untracked() == "open" {
                        self.set_flags(vec![id], Some(true), None, false);
                    }
                }
                Err(e) => {
                    let _ = self.detail.try_set(None);
                    let _ = self.detail_error.try_set(e.message);
                }
            }
        });
    }

    pub fn close_reader(self) {
        self.selected.set(None);
        self.detail.set(None);
        write_url(&self.view.get_untracked(), None);
    }

    /// Moves the selection by `delta` in the list and opens that message.
    fn step(self, delta: i64) {
        let items = self.items.get_untracked();
        if items.is_empty() {
            return;
        }
        let index = self
            .selected
            .get_untracked()
            .and_then(|s| items.iter().position(|i| i.id == s))
            .map_or(if delta > 0 { -1 } else { items.len() as i64 }, |i| {
                i as i64
            });
        let next = (index + delta).clamp(0, items.len() as i64 - 1) as usize;
        let id = items[next].id.clone();
        self.open(id.clone());
        if let Some(el) = document()
            .query_selector(&format!("[data-message=\"{id}\"] button"))
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    }

    pub fn set_flags(
        self,
        ids: Vec<String>,
        seen: Option<bool>,
        flagged: Option<bool>,
        announce: bool,
    ) {
        let ids2 = ids.clone();
        let apply = move |seen: Option<bool>, flagged: Option<bool>| {
            let _ = self.items.try_update(|items| {
                for i in items.iter_mut().filter(|i| ids.contains(&i.id)) {
                    if let Some(v) = seen {
                        i.seen = v;
                    }
                    if let Some(v) = flagged {
                        i.flagged = v;
                    }
                }
            });
            let _ = self.detail.try_update(|d| {
                if let Some(d) = d.as_mut().filter(|d| ids.contains(&d.summary.id)) {
                    if let Some(v) = seen {
                        d.summary.seen = v;
                    }
                    if let Some(v) = flagged {
                        d.summary.flagged = v;
                    }
                }
            });
        };
        apply(seen, flagged);
        let csrf = self.csrf();
        spawn_local(async move {
            let mut body = json!({ "ids": ids2 });
            if let Some(v) = seen {
                body["seen"] = json!(v);
            }
            if let Some(v) = flagged {
                body["flagged"] = json!(v);
            }
            match api::send::<Value>(&format!("{BASE}/messages/flags"), "POST", Some(body), &csrf)
                .await
            {
                Ok(_) => {
                    if announce {
                        let what = match (seen, flagged) {
                            (Some(true), _) => "Marked as read",
                            (Some(false), _) => "Marked as unread",
                            (_, Some(true)) => "Flagged",
                            _ => "Flag removed",
                        };
                        self.notify(what);
                    }
                    self.fetch_overview().await;
                }
                Err(e) => {
                    apply(seen.map(|v| !v), flagged.map(|v| !v));
                    self.fail(e.message);
                }
            }
        });
    }

    /// Archive, delete, spam or move; offers Undo by moving the messages back.
    pub fn move_to(
        self,
        ids: Vec<String>,
        role: Option<&'static str>,
        mailbox_id: Option<String>,
        label: String,
    ) {
        let origins: Vec<(String, String)> = self.items.with_untracked(|items| {
            items
                .iter()
                .filter(|i| ids.contains(&i.id))
                .map(|i| (i.id.clone(), i.mailbox_id.clone()))
                .collect()
        });
        let detail_origin = self.detail.with_untracked(|d| {
            d.as_ref()
                .filter(|d| ids.contains(&d.summary.id))
                .map(|d| (d.summary.id.clone(), d.summary.mailbox_id.clone()))
        });
        let mut origins = origins;
        if let Some(o) = detail_origin.filter(|o| !origins.contains(o)) {
            origins.push(o);
        }
        let in_trash = role == Some("trash")
            && origins
                .iter()
                .all(|(_, m)| self.mailbox(m).is_some_and(|(_, b)| b.role == "trash"));
        let csrf = self.csrf();
        let removed: Vec<Summary> = self.items.with_untracked(|items| {
            items
                .iter()
                .filter(|i| ids.contains(&i.id))
                .cloned()
                .collect()
        });
        let _ = self
            .items
            .try_update(|items| items.retain(|i| !ids.contains(&i.id)));
        self.checked.set(HashSet::new());
        if self
            .selected
            .get_untracked()
            .is_some_and(|s| ids.contains(&s))
        {
            self.close_reader();
        }
        spawn_local(async move {
            let mut body = json!({ "ids": ids });
            if let Some(r) = role {
                body["role"] = json!(r);
            }
            if let Some(m) = &mailbox_id {
                body["mailboxId"] = json!(m);
            }
            match api::send::<Value>(&format!("{BASE}/messages/move"), "POST", Some(body), &csrf)
                .await
            {
                Ok(_) => {
                    let count = removed.len().max(1);
                    let noun = if count == 1 {
                        "Message".to_owned()
                    } else {
                        format!("{count} messages")
                    };
                    let message = if in_trash {
                        format!("{noun} deleted for good")
                    } else {
                        format!("{noun} {label}")
                    };
                    let undo: Option<(&'static str, Arc<dyn Fn() + Send + Sync>)> = (!in_trash)
                        .then(|| {
                            let origins = origins.clone();
                            let undo: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                                let origins = origins.clone();
                                let csrf = self.csrf();
                                spawn_local(async move {
                                    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
                                    for (id, mailbox) in origins {
                                        groups.entry(mailbox).or_default().push(id);
                                    }
                                    for (mailbox, ids) in groups {
                                        if let Err(e) = api::send::<Value>(
                                            &format!("{BASE}/messages/move"),
                                            "POST",
                                            Some(json!({ "ids": ids, "mailboxId": mailbox })),
                                            &csrf,
                                        )
                                        .await
                                        {
                                            self.fail(e.message);
                                        }
                                    }
                                    self.reload();
                                });
                            });
                            ("Undo", undo)
                        });
                    let _ = self.toast.try_set(Some(Toast {
                        message,
                        action: undo,
                        error: false,
                    }));
                    self.fetch_overview().await;
                }
                Err(e) => {
                    self.fail(e.message);
                    self.reload();
                }
            }
        });
    }

    pub fn sync(self, account: Option<String>) {
        self.syncing.set(true);
        let csrf = self.csrf();
        spawn_local(async move {
            let body = match &account {
                Some(id) => json!({ "id": id }),
                None => json!({}),
            };
            let result =
                api::send::<Value>(&format!("{BASE}/sync"), "POST", Some(body), &csrf).await;
            if let Err(e) = result {
                self.fail(e.message);
            } else if account.is_none() {
                // The pass runs in the background; look again shortly.
                crate::sleep(2500).await;
            }
            let _ = self.syncing.try_set(false);
            self.fetch_overview().await;
            self.fetch_items(true).await;
        });
    }

    /// Sends after ten seconds unless undone; the compose sheet reopens on Undo or failure.
    pub fn send(self, spec: ComposeSpec) {
        let token = self.sending.get_value() + 1;
        self.sending.set_value(token);
        self.compose.set(None);
        let reopen = spec.clone();
        let cancel: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            self.sending.set_value(self.sending.get_value() + 1);
            self.compose.set(Some(reopen.clone()));
        });
        let _ = self.toast.try_set(Some(Toast {
            message: format!(
                "Sending \u{201c}{}\u{201d}\u{2026}",
                if spec.subject.is_empty() {
                    "No subject"
                } else {
                    &spec.subject
                }
            ),
            action: Some(("Undo", cancel)),
            error: false,
        }));
        spawn_local(async move {
            crate::sleep(10_000).await;
            if self.sending.try_get_value() != Some(token) {
                return;
            }
            let csrf = self.csrf();
            match api::send::<Value>(&format!("{BASE}/send"), "POST", Some(spec.body()), &csrf)
                .await
            {
                Ok(_) => {
                    self.notify("Message sent");
                    if let Some(id) = spec.in_reply_to.clone() {
                        let _ = self.items.try_update(|items| {
                            if let Some(i) = items.iter_mut().find(|i| i.id == id) {
                                i.answered = true;
                            }
                        });
                    }
                    self.fetch_overview().await;
                    self.fetch_items(true).await;
                }
                Err(e) => {
                    let mut spec = spec;
                    spec.error = e.message.clone();
                    let _ = self.compose.try_set(Some(spec));
                    self.fail(format!("Not sent: {}", e.message));
                }
            }
        });
    }

    fn selection(self) -> Vec<String> {
        let checked: Vec<String> = self.checked.get_untracked().into_iter().collect();
        if !checked.is_empty() {
            return checked;
        }
        self.selected.get_untracked().into_iter().collect()
    }

    fn reply(self, mode: &'static str) {
        if let Some(d) = self.detail.get_untracked() {
            let account = self.account(&d.summary.account_id);
            self.compose
                .set(Some(ComposeSpec::reply(&d, mode, account.as_ref())));
        }
    }

    fn new_message(self) {
        let account = match self.view.get_untracked() {
            View::Mailbox(id) => self.mailbox(&id).map(|(a, _)| a),
            _ => None,
        }
        .or_else(|| {
            self.overview
                .with_untracked(|o| o.as_ref().and_then(|o| o.accounts.first().cloned()))
        });
        match account {
            Some(a) => self.compose.set(Some(ComposeSpec::new(&a))),
            None => self.pane.set(Some(Pane::AddAccount(None))),
        }
    }
}

fn message_actions(
    state: MailState,
    ids: Vec<String>,
    roles: Vec<String>,
    account: Option<Account>,
    single: Option<Detail>,
) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    let has = |role: &str| {
        account
            .as_ref()
            .is_some_and(|a| a.mailboxes.iter().any(|m| m.role == role))
    };
    if let Some(d) = &single {
        let id = d.summary.id.clone();
        actions.push(
            MenuAction::new("Reply", "reply")
                .shortcut("r")
                .section("Reply")
                .run(move || state.reply("reply")),
        );
        actions.push(
            MenuAction::new("Reply all", "reply")
                .shortcut("a")
                .run(move || state.reply("all")),
        );
        actions.push(
            MenuAction::new("Forward", "share")
                .shortcut("f")
                .run(move || state.reply("forward")),
        );
        let seen = d.summary.seen;
        let ids2 = vec![id.clone()];
        actions.push(
            MenuAction::new(
                if seen {
                    "Mark as unread"
                } else {
                    "Mark as read"
                },
                "unread",
            )
            .shortcut("u")
            .section("Organize")
            .run(move || state.set_flags(ids2.clone(), Some(!seen), None, true)),
        );
        let _ = &id;
    }
    if let Some(a) = account.clone() {
        let mut targets: Vec<MenuAction> = Vec::new();
        for m in a
            .mailboxes
            .iter()
            .filter(|m| m.role != "folder" && !roles.iter().all(|r| *r == m.id))
        {
            let (ids, target, name) = (ids.clone(), m.id.clone(), m.name.clone());
            targets.push(MenuAction::plain(m.name.clone()).run(move || {
                state.move_to(
                    ids.clone(),
                    None,
                    Some(target.clone()),
                    format!("moved to {name}"),
                )
            }));
        }
        if !targets.is_empty() {
            actions.push(MenuAction::new("Move to", "move").items(targets));
        }
    }
    if has("junk") {
        let ids = ids.clone();
        actions.push(
            MenuAction::new("Report spam", "alert").run(move || {
                state.move_to(ids.clone(), Some("junk"), None, "moved to Junk".into())
            }),
        );
    }
    if let Some(d) = single {
        let id = d.summary.id.clone();
        let file = d.summary.file_id.clone();
        actions.push(
            MenuAction::new("Show in Files", "folder")
                .section("File")
                .run(move || {
                    let (id, file) = (id.clone(), file.clone());
                    spawn_local(async move {
                        match api::get::<Value>(&format!(
                            "{BASE}/messages/{}/location",
                            encode(&id)
                        ))
                        .await
                        {
                            Ok(v) => crate::navigation::navigate(&format!(
                                "/?folder={}&file={}",
                                encode(v["parentId"].as_str().unwrap_or_default()),
                                encode(v["fileId"].as_str().unwrap_or(&file))
                            )),
                            Err(e) => state.fail(e.message),
                        }
                    });
                }),
        );
        actions.push(
            MenuAction::new("Download .eml", "download")
                .href(format!("{BASE}/messages/{}/raw", encode(&d.summary.id)))
                .download(),
        );
    }
    actions
}

#[component]
pub fn MailWorkspace(session: BrowserSession, on_signed_out: Callback<()>) -> impl IntoView {
    let (start_view, start_message, start_extra) = read_url();
    let prefs = context().preferences;
    let images = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailImages"))
                .and_then(Value::as_str)
                .unwrap_or("block")
                .to_owned()
        })
    });
    let mark_read = Memo::new(move |_| {
        prefs.value.with(|p| {
            p.as_ref()
                .and_then(|p| p.rest.get("mailMarkRead"))
                .and_then(Value::as_str)
                .unwrap_or("open")
                .to_owned()
        })
    });
    let state = MailState {
        overview: RwSignal::new(None),
        load_error: RwSignal::new(String::new()),
        view: RwSignal::new(start_view.clone()),
        items: RwSignal::new(Vec::new()),
        cursor: RwSignal::new(None),
        list_state: RwSignal::new("loading"),
        list_error: RwSignal::new(String::new()),
        selected: RwSignal::new(None),
        detail: RwSignal::new(None),
        detail_error: RwSignal::new(String::new()),
        checked: RwSignal::new(HashSet::new()),
        pane: RwSignal::new(None),
        compose: RwSignal::new(None),
        toast: RwSignal::new(None),
        confirm: RwSignal::new(None),
        syncing: RwSignal::new(false),
        collapsed: RwSignal::new(HashSet::new()),
        navigation: RwSignal::new(false),
        generation: StoredValue::new(0),
        session: StoredValue::new(session.clone()),
        images,
        mark_read,
        sending: StoredValue::new(0),
    };
    // After OAuth: say whether the account was added.
    if let Some(error) = start_extra.get("oauthError") {
        state.fail(format!("The account was not added: {error}"));
    } else if start_extra.contains_key("account") {
        state.notify("Account added; its mail is being mirrored");
    }
    if start_extra.is_empty() {
        write_url(&start_view, start_message.as_deref());
    } else {
        write_url(&start_view, None);
    }

    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = alive.clone();
        on_cleanup(move || alive.store(false, Ordering::Relaxed));
    }
    spawn_local(async move {
        state.fetch_overview().await;
        state.fetch_items(true).await;
        // New mail arrives through the background pass; look for it every half minute.
        while alive.load(Ordering::Relaxed) {
            crate::sleep(30_000).await;
            if !alive.load(Ordering::Relaxed) {
                break;
            }
            state.fetch_overview().await;
            if state.checked.try_with_untracked(HashSet::is_empty) == Some(true) {
                state.fetch_items(true).await;
            }
        }
    });
    if let Some(id) = start_message {
        state.open(id);
    }
    // Images: reload the open message when the preference changes.
    Effect::new(move |prev: Option<String>| {
        let current = state.images.get();
        if prev.is_some_and(|p| p != current)
            && let Some(id) = state.selected.get_untracked()
        {
            state.open(id);
        }
        current
    });
    // Toasts leave after eight seconds (ten while a send can be undone).
    let toast_seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        let Some(t) = state.toast.get() else { return };
        let id = toast_seq.get_value() + 1;
        toast_seq.set_value(id);
        let wait = if t.action.is_some() { 10_000 } else { 8000 };
        spawn_local(async move {
            crate::sleep(wait).await;
            if toast_seq.try_get_value() == Some(id) {
                let _ = state.toast.try_set(None);
            }
        });
    });
    // Keyboard: j/k, c, r/a/f, e, #, s, u, Escape.
    Effect::new(move |_| {
        listen(&document(), "keydown", false, move |event| {
            let event: web_sys::KeyboardEvent = event.unchecked_into();
            if event.default_prevented() || event.ctrl_key() || event.meta_key() || event.alt_key()
            {
                return;
            }
            let typing = document().active_element().is_some_and(|e| {
                matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                    || e.closest("[contenteditable],[role=dialog],.editing-pane,.mail-compose")
                        .ok()
                        .flatten()
                        .is_some()
            });
            if typing
                || state.pane.get_untracked().is_some()
                || state.compose.with_untracked(Option::is_some)
            {
                return;
            }
            let selected = state.selection();
            let handled = match event.key().as_str() {
                "j" | "ArrowDown" if event.key() == "j" => {
                    state.step(1);
                    true
                }
                "k" => {
                    state.step(-1);
                    true
                }
                "c" => {
                    state.new_message();
                    true
                }
                "r" => {
                    state.reply("reply");
                    true
                }
                "a" => {
                    state.reply("all");
                    true
                }
                "f" => {
                    state.reply("forward");
                    true
                }
                "e" if !selected.is_empty() => {
                    state.move_to(selected, Some("archive"), None, "archived".into());
                    true
                }
                "#" if !selected.is_empty() => {
                    state.move_to(selected, Some("trash"), None, "moved to Trash".into());
                    true
                }
                "s" if !selected.is_empty() => {
                    let flagged = state
                        .detail
                        .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.flagged));
                    state.set_flags(selected, None, Some(!flagged), true);
                    true
                }
                "u" if !selected.is_empty() => {
                    state.set_flags(selected, Some(false), None, true);
                    true
                }
                "Escape" if state.selected.get_untracked().is_some() => {
                    state.close_reader();
                    true
                }
                _ => false,
            };
            if handled {
                event.prevent_default();
            }
        });
    });

    let title = Memo::new(move |_| match state.view.get() {
        View::Unified(v) => UNIFIED
            .iter()
            .find(|(id, ..)| *id == v)
            .map(|(_, l, _)| (*l).to_owned())
            .unwrap_or_default(),
        View::Mailbox(id) => state
            .overview
            .with(|o| {
                o.as_ref().and_then(|o| {
                    o.accounts.iter().find_map(|a| {
                        a.mailboxes
                            .iter()
                            .find(|m| m.id == id)
                            .map(|m| format!("{} \u{00b7} {}", a.name, m.name))
                    })
                })
            })
            .unwrap_or_else(|| "Mailbox".into()),
    });
    let has_reader = Memo::new(move |_| state.selected.get().is_some());
    let navigation_id = unique_id();
    let list_ref = NodeRef::<html::Div>::new();

    let view_row = move |target: View,
                         label: String,
                         icon: &'static str,
                         count: Signal<u32>,
                         extra_class: &'static str| {
        let t2 = target.clone();
        let active = Memo::new(move |_| state.view.get() == t2);
        let t3 = target.clone();
        view! {
            <button type="button" class=move || format!("nav-item mail-view {extra_class}{}", if active.get() { " active" } else { "" })
                aria-current=move || active.get().then_some("page") on:click=move |_| state.choose(t3.clone())>
                <Icon name=icon/>
                <span class="mail-row-title">{label}</span>
                {move || (count.get() > 0).then(|| view! { <span class="mail-count">{count.get()}</span> })}
            </button>
        }
    };

    view! {
        <div class=move || format!("app-shell pim-shell mail-shell{}", if has_reader.get() { " has-pim-details" } else { "" })>
            <WorkspaceHeader app="mail" session=session.clone() on_signed_out=on_signed_out on_settings=Callback::new(move |_| state.pane.set(Some(Pane::Settings("Accounts"))))/>
            <div class="pim-workspace mail-workspace">
                {move || state.navigation.get().then(|| view! { <button class="nav-scrim" aria-label="Close navigation" on:click=move |_| state.navigation.set(false)></button> })}
                <aside id=navigation_id class=move || if state.navigation.get() { "sidebar mail-sidebar is-open" } else { "sidebar mail-sidebar" } aria-label="Mail navigation">
                    <button type="button" class="new-button ui-new" on:click=move |_| state.new_message()><Icon name="pencil"/>"Compose"</button>
                    <nav aria-label="Mail views">
                        {UNIFIED.into_iter().map(|(id, label, icon)| {
                            let count = Signal::derive(move || state.overview.with(|o| o.as_ref().map(|o| match id { "inbox" => o.counts.inbox, "unread" => o.counts.unread, "flagged" => o.counts.flagged, "drafts" => o.counts.drafts, _ => 0 }).unwrap_or(0)));
                            view_row(View::Unified(id), label.to_owned(), icon, count, "")
                        }).collect_view()}
                    </nav>
                    <div class="sidebar-heading mail-heading">
                        <h2>"Accounts"</h2>
                        <button type="button" class="icon-button" aria-label="Add account" title="Add account" on:click=move |_| state.pane.set(Some(Pane::AddAccount(None)))><Icon name="plus"/></button>
                    </div>
                    <nav aria-label="Accounts and mailboxes" class="mail-accounts">
                        {move || {
                            let accounts = state.overview.with(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
                            accounts.into_iter().map(|a| {
                                let account_id = a.id.clone();
                                let open = Memo::new({ let id = account_id.clone(); move |_| !state.collapsed.with(|c| c.contains(&id)) });
                                let protocol = if a.protocol == "jmap" { "JMAP".to_owned() } else if a.auth == "oauth" { a.provider_name.clone() } else { "IMAP".to_owned() };
                                let problem = a.status == "error" || a.status == "signIn" || !a.provider_available;
                                let unread: u32 = a.mailboxes.iter().filter(|m| m.role == "inbox").map(|m| m.unread).sum();
                                let mut boxes = a.mailboxes.clone();
                                // Children follow their parent.
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
                                let toggle_label = format!("{} {}", if open.get_untracked() { "Collapse" } else { "Expand" }, a.name);
                                let id_toggle = account_id.clone();
                                let id_menu = account_id.clone();
                                let id_menu2 = account_id.clone();
                                let id_menu3 = account_id.clone();
                                let name = a.name.clone();
                                view! {
                                    <div class="mail-row mail-account-row">
                                        <button type="button" class="icon-button mail-toggle" aria-label=toggle_label aria-expanded=move || open.get().to_string()
                                            on:click=move |_| state.collapsed.update(|c| { if !c.remove(&id_toggle) { c.insert(id_toggle.clone()); } })><Icon name="chevron"/></button>
                                        <span class=format!("mail-acct {}", tone(a.color)) aria-hidden="true">{initials(&a.name).chars().next().unwrap_or('?').to_string()}</span>
                                        <span class="mail-row-title mail-account-name" title=a.address.clone()>{a.name.clone()}</span>
                                        {problem.then(|| view! { <span class="mail-warning" title=a.last_error.clone()><Icon name="alert"/><span class="sr-only">"Needs attention"</span></span> })}
                                        {(unread > 0 && !open.get_untracked()).then(|| view! { <span class="mail-count">{unread}</span> })}
                                        <span class="mail-proto">{protocol}</span>
                                        <ActionMenu label=format!("Actions for {name}") class="mail-row-menu" actions=move |_| vec![
                                            MenuAction::new("Sync now", "refresh").run({ let id = id_menu.clone(); move || state.sync(Some(id.clone())) }),
                                            MenuAction::new("Account settings", "settings").run(move || state.pane.set(Some(Pane::Settings("Accounts")))),
                                            MenuAction::new("Sign in again", "key").run({ let id = id_menu2.clone(); move || state.pane.set(Some(Pane::AddAccount(Some(id.clone())))) }),
                                            MenuAction::new("Open in Files", "folder").href(format!("/?folder={}", encode(&state.account(&id_menu3).map(|a| a.folder_id).unwrap_or_default()))),
                                        ]><Icon name="more"/></ActionMenu>
                                    </div>
                                    {move || open.get().then(|| ordered.iter().map(|(m, depth)| {
                                        let target = View::Mailbox(m.id.clone());
                                        let icon = match m.role.as_str() { "inbox" => "mail", "sent" => "send", "drafts" => "pencil", "trash" => "trash", "junk" => "alert", "archive" => "archive", _ => "folder" };
                                        let count = m.unread;
                                        let class = if *depth > 0 { "is-nested is-deep" } else { "is-nested" };
                                        if m.role == "folder" {
                                            return view! { <div class=format!("nav-item mail-view mail-container {class}") data-depth=depth.min(&6).to_string()><Icon name="folder"/><span class="mail-row-title">{m.name.clone()}</span></div> }.into_any();
                                        }
                                        let count = Signal::derive(move || if count > 0 { count } else { 0 });
                                        view! { <div class="mail-mailbox" data-depth=depth.min(&6).to_string()>{view_row(target, m.name.clone(), icon, count, class)}</div> }.into_any()
                                    }).collect_view())}
                                }
                            }).collect_view()
                        }}
                    </nav>
                    <div class="sidebar-footer mail-footer">
                        <button type="button" class="text-button" disabled=move || state.syncing.get() on:click=move |_| state.sync(None)>
                            <Icon name="refresh"/>
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
                                        format!("Updated {}", relative(last))
                                    }
                                })
                            }}
                        </button>
                    </div>
                    <button type="button" class="nav-item ui-app-settings-entry" on:click=move |_| state.pane.set(Some(Pane::Settings("Accounts")))><Icon name="settings"/>"Mail settings"</button>
                </aside>
                <main class="pim-main mail-main" tabindex="-1" aria-label=move || title.get()>
                    {move || if state.checked.with(HashSet::is_empty) {
                        view! {
                            <div class="mail-list-heading">
                                <button type="button" class="icon-button mail-menu-button" aria-label="Mail navigation" on:click=move |_| state.navigation.set(true)><Icon name="menu"/></button>
                                <h1>{move || title.get()}</h1>
                                <div class="mail-tools">
                                    <button type="button" class="icon-button" aria-label="Check for mail" title="Check for mail" disabled=move || state.syncing.get() on:click=move |_| state.sync(None)><Icon name="refresh"/></button>
                                    <button type="button" class="icon-button" aria-label="Mark all shown as read" title="Mark all shown as read"
                                        disabled=move || state.items.with(|i| !i.iter().any(|i| !i.seen))
                                        on:click=move |_| { let ids: Vec<String> = state.items.with_untracked(|i| i.iter().filter(|i| !i.seen).map(|i| i.id.clone()).collect()); state.set_flags(ids, Some(true), None, true); }><Icon name="checks"/></button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        let count = Memo::new(move |_| state.checked.with(HashSet::len));
                        let ids = move || state.checked.get_untracked().into_iter().collect::<Vec<_>>();
                        view! {
                            <div class="mail-list-heading mail-selection" role="toolbar" aria-label="Selected messages">
                                <button type="button" class="icon-button" aria-label="Clear selection" on:click=move |_| state.checked.set(HashSet::new())><Icon name="close"/></button>
                                <b>{move || format!("{} selected", count.get())}</b>
                                <button type="button" class="icon-button" aria-label="Archive" title="Archive (e)" on:click=move |_| state.move_to(ids(), Some("archive"), None, "archived".into())><Icon name="archive"/></button>
                                <button type="button" class="icon-button" aria-label="Delete" title="Delete (#)" on:click=move |_| state.move_to(ids(), Some("trash"), None, "moved to Trash".into())><Icon name="trash"/></button>
                                <button type="button" class="icon-button" aria-label="Mark as read" title="Mark as read" on:click=move |_| state.set_flags(ids(), Some(true), None, true)><Icon name="checks"/></button>
                                <button type="button" class="icon-button" aria-label="Mark as unread" title="Mark as unread" on:click=move |_| state.set_flags(ids(), Some(false), None, true)><Icon name="unread"/></button>
                                <button type="button" class="icon-button" aria-label="Flag" title="Flag" on:click=move |_| state.set_flags(ids(), None, Some(true), true)><Icon name="flag"/></button>
                                <ActionMenu label="More actions for the selection" actions=move |_| {
                                    let ids = ids();
                                    let first = state.items.with_untracked(|i| i.iter().find(|i| ids.contains(&i.id)).map(|i| i.account_id.clone()));
                                    let same = state.items.with_untracked(|i| i.iter().filter(|i| ids.contains(&i.id)).all(|i| Some(&i.account_id) == first.as_ref()));
                                    let account = first.filter(|_| same).and_then(|id| state.account(&id));
                                    let roles = state.items.with_untracked(|i| i.iter().filter(|i| ids.contains(&i.id)).map(|i| i.mailbox_id.clone()).collect());
                                    message_actions(state, ids, roles, account, None)
                                }><Icon name="more"/></ActionMenu>
                            </div>
                        }.into_any()
                    }}
                    {move || (!state.load_error.with(String::is_empty)).then(|| view! {
                        <div class="mail-banner is-error" role="alert"><Icon name="alert"/><span><b>"Mail could not be loaded"</b>{state.load_error.get()}</span><button class="secondary-button" on:click=move |_| state.reload()>"Retry"</button></div>
                    })}
                    {move || state.overview.with(|o| o.as_ref().map(|o| o.accounts.iter().filter(|a| a.status == "signIn" || a.status == "error" || !a.provider_available).cloned().collect::<Vec<_>>()).unwrap_or_default()).into_iter().map(|a| {
                        let id = a.id.clone();
                        let sign_in = a.status == "signIn";
                        let heading = if !a.provider_available { format!("{}: the administrator turned off its provider", a.name) } else if sign_in { format!("{} needs you to sign in again", a.name) } else { format!("{} is not syncing", a.name) };
                        view! {
                            <div class=if sign_in { "mail-banner is-error" } else { "mail-banner" } role="status">
                                <Icon name="alert"/><span><b>{heading}</b>{a.last_error.clone()}" Mirrored messages stay readable."</span>
                                {if sign_in {
                                    view! { <button class="secondary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(Some(id.clone()))))>"Sign in again"</button> }.into_any()
                                } else {
                                    view! { <button class="secondary-button" on:click=move |_| state.sync(Some(id.clone()))>"Retry now"</button> }.into_any()
                                }}
                            </div>
                        }
                    }).collect_view()}
                    <div class="mail-list" node_ref=list_ref role="list" aria-label=move || format!("{} messages", title.get())>
                        {move || (state.list_state.get() == "loading" && state.items.with(Vec::is_empty)).then(|| view! { <p class="mail-empty" role="status">"Loading messages\u{2026}"</p> })}
                        {move || (state.list_state.get() == "error").then(|| view! {
                            <div class="mail-empty" role="alert"><h2>"Messages could not be loaded"</h2><p>{state.list_error.get()}</p><button class="secondary-button" on:click=move |_| state.load_items()>"Retry"</button></div>
                        })}
                        {move || (state.list_state.get() == "ready" && state.items.with(Vec::is_empty)).then(|| {
                            let none = state.overview.with(|o| o.as_ref().is_some_and(|o| o.accounts.is_empty()));
                            let administrator = state.overview.with(|o| o.as_ref().is_some_and(|o| o.administrator));
                            let providers = state.overview.with(|o| o.as_ref().map(|o| o.providers.len()).unwrap_or(0));
                            if none {
                                view! {
                                    <div class="mail-empty">
                                        <Icon name="mail"/>
                                        <h2>"Add your first mail account"</h2>
                                        {if providers == 0 && administrator {
                                            view! { <p>"Set up a mail provider first: its servers and, for Google or Microsoft, its OAuth client."</p><button class="primary-button" on:click=move |_| state.pane.set(Some(Pane::Settings("Providers")))>"Set up a provider"</button> }.into_any()
                                        } else if providers == 0 {
                                            view! { <p>"Your administrator has not set up a mail provider yet."</p> }.into_any()
                                        } else {
                                            view! { <p>"Its messages are mirrored as .eml files in Files, so they stay readable even when the server is not."</p><button class="primary-button" on:click=move |_| state.pane.set(Some(Pane::AddAccount(None)))><Icon name="plus"/>"Add account"</button> }.into_any()
                                        }}
                                    </div>
                                }.into_any()
                            } else {
                                view! { <div class="mail-empty"><Icon name="mail"/><h2>"No messages here"</h2><p>"Messages you move here will appear in this list."</p></div> }.into_any()
                            }
                        })}
                        {move || {
                            let items = state.items.get();
                            let unified = matches!(state.view.get(), View::Unified(_));
                            let colors: HashMap<String, u32> = state.overview.with(|o| o.as_ref().map(|o| o.accounts.iter().map(|a| (a.id.clone(), a.color)).collect()).unwrap_or_default());
                            let mut last_day = String::new();
                            items.into_iter().map(|item| {
                                let day = day_label(item.date);
                                let heading = (day != last_day).then(|| { last_day = day.clone(); view! { <div class="mail-day" role="presentation">{day}</div> } });
                                let id = item.id.clone();
                                let (id_open, id_check, id_sel, id_checked) = (id.clone(), id.clone(), id.clone(), id.clone());
                                let is_sel = Memo::new(move |_| state.selected.get().as_deref() == Some(id_sel.as_str()));
                                let is_checked = Memo::new(move |_| state.checked.with(|c| c.contains(&id_checked)));
                                let sender = if item.mailbox_role == "sent" || item.mailbox_role == "drafts" || item.from_address.is_empty() && !item.recipients.to.is_empty() {
                                    item.recipients.to.first().map(|t| format!("To {}", t.label())).unwrap_or_else(|| item.sender())
                                } else {
                                    item.sender()
                                };
                                let account_dot = unified.then(|| colors.get(&item.account_id).map(|c| view! { <span class=format!("mail-acct is-dot {}", tone(*c)) aria-hidden="true"></span> }));
                                let unread = !item.seen;
                                let check_label = format!("Select message from {}", item.sender());
                                view! {
                                    {heading}
                                    <div role="listitem" data-message=id.clone() class=move || format!("mail-message{}{}{}", if unread { "" } else { " is-read" }, if is_sel.get() { " is-selected" } else { "" }, if is_checked.get() { " is-checked" } else { "" })>
                                        <input type="checkbox" class="mail-check" aria-label=check_label prop:checked=move || is_checked.get()
                                            on:change=move |e| { let on = event_target_checked(&e); let id = id_check.clone(); state.checked.update(|c| { if on { c.insert(id); } else { c.remove(&id); } }); }/>
                                        <button type="button" aria-current=move || is_sel.get().then_some("true") on:click=move |_| state.open(id_open.clone())>
                                            <span class="mail-dot" aria-hidden="true"></span>
                                            <span class=avatar_tone(&item.from_address) aria-hidden="true">{initials(&item.sender())}</span>
                                            <span class="mail-from">{sender}{(item.thread_count > 1).then(|| view! { <span class="mail-thread-count">{item.thread_count}</span> })}{unread.then(|| view! { <span class="sr-only">", unread"</span> })}</span>
                                            <span class="mail-time">
                                                {item.attachments.then(|| view! { <span class="mail-clip" title="Has attachments"><Icon name="link"/><span class="sr-only">"Has attachments"</span></span> })}
                                                {item.flagged.then(|| view! { <span class="mail-flag" title="Flagged"><Icon name="flag"/><span class="sr-only">"Flagged"</span></span> })}
                                                {item.answered.then(|| view! { <span class="mail-answered" title="Answered"><Icon name="reply"/><span class="sr-only">"Answered"</span></span> })}
                                                <time>{time_label(item.date)}</time>
                                            </span>
                                            <span class="mail-subject">{item.subject_or()}</span>
                                            <span class="mail-snippet">{account_dot}{item.draft.then(|| view! { <span class="mail-label">"Draft"</span> })}{item.snippet.clone()}</span>
                                        </button>
                                    </div>
                                }
                            }).collect_view()
                        }}
                        {move || (state.cursor.with(Option::is_some) && state.list_state.get() == "ready").then(|| view! {
                            <div class="mail-more"><button class="text-button" on:click=move |_| { spawn_local(state.fetch_items(false)); }>"Load more"</button></div>
                        })}
                    </div>
                </main>
                <Show when=move || has_reader.get()>
                    <aside class="pim-details mail-details" tabindex="-1" aria-label="Message"><Reader state=state/></aside>
                </Show>
            </div>
            {move || state.pane.get().map(|p| match p {
                Pane::AddAccount(account) => view! { <AddAccountPane state=state account=account/> }.into_any(),
                Pane::Settings(section) => view! { <MailSettings state=state section=section/> }.into_any(),
            })}
            {move || state.compose.get().map(|spec| view! { <ComposeSheet state=state spec=spec/> })}
            {move || state.confirm.get().map(|c| {
                let run = StoredValue::new(c.run.clone());
                view! {
                    <Dialog title=c.title.clone() on_close=Callback::new(move |_| state.confirm.set(None))>
                        <p>{c.message.clone()}</p>
                        <div class="dialog-actions">
                            <button type="button" class="text-button" on:click=move |_| state.confirm.set(None)>"Cancel"</button>
                            <button type="button" class="primary-button danger" on:click=move |_| { state.confirm.set(None); run.with_value(|r| r()); }>{c.action}</button>
                        </div>
                    </Dialog>
                }
            })}
            <div class="mail-toast-region" role="status" aria-live="polite">
                {move || state.toast.get().map(|t| view! {
                    <div class=if t.error { "settings-toast mail-toast is-error" } else { "settings-toast mail-toast" }>
                        <span>{t.message.clone()}</span>
                        {t.action.map(|(label, run)| view! { <button type="button" class="text-button" on:click=move |_| { state.toast.set(None); run(); }>{label}</button> })}
                    </div>
                })}
            </div>
        </div>
    }
}

#[component]
fn Reader(state: MailState) -> impl IntoView {
    let expanded = RwSignal::new(HashSet::<String>::new());
    let bodies = RwSignal::new(HashMap::<String, Body>::new());
    // A new message starts with only itself expanded.
    Effect::new(move |_| {
        let id = state
            .detail
            .with(|d| d.as_ref().map(|d| d.summary.id.clone()));
        expanded.set(id.into_iter().collect());
        bodies.set(HashMap::new());
    });
    let load_body = move |id: String| {
        if bodies.with_untracked(|b| b.contains_key(&id))
            || state
                .detail
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.id == id))
        {
            return;
        }
        let images = state.images.get_untracked() == "load";
        spawn_local(async move {
            match api::get::<Body>(&format!(
                "{BASE}/messages/{}/body{}",
                encode(&id),
                if images { "?images=1" } else { "" }
            ))
            .await
            {
                Ok(b) => {
                    let _ = bodies.try_update(|m| {
                        m.insert(id, b);
                    });
                }
                Err(e) => state.fail(e.message),
            }
        });
    };
    let show_images = move |id: String| {
        spawn_local(async move {
            match api::get::<Body>(&format!("{BASE}/messages/{}/body?images=1", encode(&id))).await
            {
                Ok(b) => {
                    let main = state
                        .detail
                        .with_untracked(|d| d.as_ref().is_some_and(|d| d.summary.id == id));
                    if main {
                        let _ = state.detail.try_update(|d| {
                            if let Some(d) = d {
                                d.body = b;
                            }
                        });
                    } else {
                        let _ = bodies.try_update(|m| {
                            m.insert(id, b);
                        });
                    }
                }
                Err(e) => state.fail(e.message),
            }
        });
    };
    view! {
        <div class="mail-reader">
            {move || {
                if let Some(error) = Some(state.detail_error.get()).filter(|e| !e.is_empty()) {
                    return view! {
                        <div class="mail-reader-head"><div class="mail-reader-tools"><button type="button" class="icon-button" aria-label="Close message" on:click=move |_| state.close_reader()><Icon name="close"/></button></div></div>
                        <div class="mail-empty" role="alert"><h2>"This message could not be opened"</h2><p>{error}</p>
                            <button class="secondary-button" on:click=move |_| { if let Some(id) = state.selected.get_untracked() { state.open(id); } }>"Retry"</button></div>
                    }.into_any();
                }
                let Some(d) = state.detail.get() else { return view! { <p class="mail-empty" role="status">"Opening message\u{2026}"</p> }.into_any() };
                let account = state.account(&d.summary.account_id);
                let id = d.summary.id.clone();
                let flagged = d.summary.flagged;
                let seen = d.summary.seen;
                let in_trash = d.summary.mailbox_role == "trash" || d.thread.iter().find(|t| t.id == id).is_some_and(|t| t.mailbox_role == "trash");
                let (id1, id2, id3, id4) = (id.clone(), id.clone(), id.clone(), id.clone());
                let detail_menu = d.clone();
                let account_menu = account.clone();
                let crumb_tone = account.as_ref().map(|a| tone(a.color)).unwrap_or_default();
                let thread = if d.thread.is_empty() { vec![d.summary.clone()] } else { d.thread.clone() };
                let earlier = thread.len().saturating_sub(1);
                let reply_detail = d.clone();
                view! {
                    <div class="mail-reader-head">
                        <button type="button" class="icon-button mail-back" aria-label="Back to messages" on:click=move |_| state.close_reader()><Icon name="back"/></button>
                        <span class="mail-reader-crumb"><span class=format!("mail-acct is-dot {crumb_tone}") aria-hidden="true"></span>{format!("{} \u{00b7} {}", d.account_name, d.mailbox_name)}</span>
                        <div class="mail-reader-tools">
                            <button type="button" class="icon-button" aria-label="Archive" title="Archive (e)" on:click=move |_| state.move_to(vec![id1.clone()], Some("archive"), None, "archived".into())><Icon name="archive"/></button>
                            <button type="button" class="icon-button" aria-label=if in_trash { "Delete for good" } else { "Delete" } title="Delete (#)" on:click=move |_| state.move_to(vec![id2.clone()], Some("trash"), None, "moved to Trash".into())><Icon name="trash"/></button>
                            <button type="button" class="icon-button" aria-label=if seen { "Mark as unread" } else { "Mark as read" } title=if seen { "Mark as unread (u)" } else { "Mark as read" } on:click=move |_| state.set_flags(vec![id3.clone()], Some(!seen), None, true)><Icon name="unread"/></button>
                            <button type="button" class=if flagged { "icon-button is-flagged" } else { "icon-button" } aria-label=if flagged { "Remove flag" } else { "Flag" } aria-pressed=flagged.to_string() title="Flag (s)" on:click=move |_| state.set_flags(vec![id4.clone()], None, Some(!flagged), true)><Icon name="flag"/></button>
                            <ActionMenu label="More message actions" actions=move |_| message_actions(state, vec![detail_menu.summary.id.clone()], vec![detail_menu.summary.mailbox_id.clone()], account_menu.clone(), Some(detail_menu.clone()))><Icon name="more"/></ActionMenu>
                            <button type="button" class="icon-button mail-close" aria-label="Close message" on:click=move |_| state.close_reader()><Icon name="close"/></button>
                        </div>
                    </div>
                    <div class="mail-thread">
                        <h1 tabindex="-1">{d.summary.subject_or()}</h1>
                        <p class="mail-thread-meta"><Icon name="file"/>{format!("{} message{} \u{00b7} mirrored as {}", thread.len(), if thread.len() == 1 { "" } else { "s" }, if d.file_name.is_empty() { "an .eml file".to_owned() } else { d.file_name.clone() })}</p>
                        {(earlier > 3).then(|| view! { <p class="mail-earlier"><span>{format!("{earlier} earlier messages")}</span></p> })}
                        {thread.into_iter().map(|m| {
                            let mid = m.id.clone();
                            let is_main = mid == d.summary.id;
                            let main_body = is_main.then(|| d.body.clone());
                            let (mid_toggle, mid_open, mid_img) = (mid.clone(), mid.clone(), mid.clone());
                            let open = Memo::new(move |_| expanded.with(|e| e.contains(&mid_open)));
                            let to_line = {
                                let to: Vec<String> = m.recipients.to.iter().chain(&m.recipients.cc).map(Addr::label).collect();
                                if to.is_empty() { String::new() } else { format!("to {}", to.join(", ")) }
                            };
                            let m2 = m.clone();
                            view! {
                                <article class=move || if open.get() { "mail-msg is-open" } else { "mail-msg" }>
                                    <button type="button" class="mail-msg-head" aria-expanded=move || open.get().to_string()
                                        on:click=move |_| { let id = mid_toggle.clone(); let now = !expanded.with_untracked(|e| e.contains(&id)); expanded.update(|e| { if now { e.insert(id.clone()); } else { e.remove(&id); } }); if now { load_body(id); } }>
                                        <span class=avatar_tone(&m.from_address) aria-hidden="true">{initials(&m.sender())}</span>
                                        <span class="mail-msg-from"><b>{m.sender()}</b>{(!m.from_address.is_empty() && !m.from_name.is_empty()).then(|| view! { <small>{m.from_address.clone()}</small> })}</span>
                                        <time class="mail-msg-date" title=local_date_time(m.date, &[])>{time_label(m.date)}</time>
                                        <span class="mail-msg-to">{move || if open.get() { to_line.clone() } else { m2.snippet.clone() }}</span>
                                    </button>
                                    {move || open.get().then(|| {
                                        let body = main_body.clone().or_else(|| bodies.with(|b| b.get(&mid).cloned()));
                                        let mid_img = mid_img.clone();
                                        match body {
                                            None => view! { <p class="mail-msg-loading" role="status">"Loading\u{2026}"</p> }.into_any(),
                                            Some(b) => view! { <MessageBody id=mid.clone() body=b on_images=Callback::new(move |_| show_images(mid_img.clone()))/> }.into_any(),
                                        }
                                    })}
                                </article>
                            }
                        }).collect_view()}
                        <InlineReply state=state detail=reply_detail/>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

#[component]
fn MessageBody(id: String, body: Body, on_images: Callback<()>) -> impl IntoView {
    let html_ref = NodeRef::<html::Div>::new();
    let html = body.html.clone();
    Effect::new(move |_| {
        let Some(el) = html_ref.get() else { return };
        while let Some(first) = el.first_child() {
            let _ = el.remove_child(&first);
        }
        let _ = el.append_child(&render_html(&html));
    });
    let attachments: Vec<Part> = body
        .attachments
        .iter()
        .filter(|a| !a.inline)
        .cloned()
        .collect();
    view! {
        {(body.blocked_images > 0).then(|| view! {
            <div class="mail-images-blocked" role="note"><Icon name="image"/>
                <span>{format!("{} remote image{} blocked", body.blocked_images, if body.blocked_images == 1 { "" } else { "s" })}</span>
                <button type="button" class="secondary-button" on:click=move |_| { on_images.try_run(()); }>"Show images"</button>
            </div>
        })}
        {if body.html.is_empty() {
            view! { <div class="mail-body mail-text">{body.text.clone()}</div> }.into_any()
        } else {
            view! { <div node_ref=html_ref class="mail-body"></div> }.into_any()
        }}
        {(!attachments.is_empty()).then(|| view! {
            <ul class="mail-attachments" aria-label="Attachments">
                {attachments.into_iter().map(|a| {
                    let href = format!("{BASE}/messages/{}/attachments/{}", encode(&id), a.index);
                    let kind = if a.kind.starts_with("image/") { "image" } else if a.kind == "application/pdf" { "pdf" } else { "file" };
                    view! {
                        <li class="mail-attachment">
                            <span class=format!("mail-attachment-icon is-{kind}") aria-hidden="true"><Icon name=kind/></span>
                            <span class="mail-attachment-name"><b>{a.name.clone()}</b><small>{api::format_bytes(a.size)}</small></span>
                            <a class="icon-button" href=href download=a.name.clone() aria-label=format!("Download {}", a.name)><Icon name="download"/></a>
                        </li>
                    }
                }).collect_view()}
            </ul>
        })}
    }
}
