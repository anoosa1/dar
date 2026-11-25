//! Mail data model, state management, conversation grouping, HTML sanitization,
//! quoted-text folding, and List-Unsubscribe parsing.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};

use super::compose::ComposeSpec;
use crate::api::{self, BrowserSession};
use crate::dates::{days_ago, local_date, local_time};
use crate::shared::context;
use crate::ui::{document, window};

pub(crate) const BASE: &str = "/api/v1/mail";

pub(crate) const UNIFIED: [(&str, &str, &str); 6] = [
    ("inbox", "All inboxes", "inbox"),
    ("unread", "Unread", "mailUnread"),
    ("flagged", "Flagged", "flag"),
    ("drafts", "Drafts", "pencil"),
    ("sent", "Sent", "paperPlane"),
    ("all", "All mail", "allMail"),
];

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub auth: String,
    pub domains: Vec<String>,
    pub source: String,
    pub enabled: bool,
    pub document: serde_json::Value,
    pub has_secret: bool,
    pub accounts: u32,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Mailbox {
    pub id: String,
    pub name: String,
    pub role: String,
    pub parent_id: String,
    pub total: u32,
    pub unread: u32,
    pub folder_id: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Account {
    pub id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub protocol: String,
    pub auth: String,
    pub provider_available: bool,
    pub name: String,
    pub address: String,
    pub display_name: String,
    pub username: String,
    pub color: u32,
    pub folder_id: String,
    pub status: String,
    pub last_error: String,
    pub synced_at: f64,
    pub signature: String,
    pub mailboxes: Vec<Mailbox>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Counts {
    pub inbox: u32,
    pub unread: u32,
    pub flagged: u32,
    pub drafts: u32,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Overview {
    pub providers: Vec<Provider>,
    pub accounts: Vec<Account>,
    pub counts: Counts,
    pub mail_folder: String,
    pub administrator: bool,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Addr {
    pub name: String,
    pub address: String,
}

impl Addr {
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            self.name.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Recipients {
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Summary {
    pub id: String,
    pub account_id: String,
    pub mailbox_id: String,
    pub subject: String,
    pub from_name: String,
    pub from_address: String,
    pub recipients: Recipients,
    pub date: f64,
    pub snippet: String,
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
    pub attachments: bool,
    pub size: f64,
    pub message_id: String,
    pub file_id: String,
    pub thread_key: String,
    pub thread_count: u32,
    pub mailbox_role: String,
}

impl Summary {
    pub fn sender(&self) -> String {
        if !self.from_name.is_empty() {
            self.from_name.clone()
        } else if !self.from_address.is_empty() {
            self.from_address.clone()
        } else {
            "Unknown sender".into()
        }
    }

    pub fn short_sender(&self) -> String {
        if !self.from_name.is_empty() {
            self.from_name
                .split_whitespace()
                .next()
                .unwrap_or(&self.from_name)
                .to_owned()
        } else if !self.from_address.is_empty() {
            self.from_address
                .split('@')
                .next()
                .unwrap_or(&self.from_address)
                .to_owned()
        } else {
            "Unknown".into()
        }
    }

    pub fn subject_or(&self) -> String {
        if self.subject.trim().is_empty() {
            "No subject".into()
        } else {
            self.subject.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Part {
    pub index: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: f64,
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Body {
    pub html: String,
    pub text: String,
    pub attachments: Vec<Part>,
    pub blocked_images: u32,
    pub reply_to: Vec<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    /// The plain part as written (a Markdown message's source).
    pub text_part: String,
    /// "markdown" for messages written in Markdown in Dar, else "plain".
    pub format: String,
    /// Raw RFC 2369 List-Unsubscribe header if present.
    pub list_unsubscribe: String,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Detail {
    #[serde(flatten)]
    pub summary: Summary,
    pub file_name: String,
    pub mailbox_name: String,
    pub account_name: String,
    pub thread: Vec<Summary>,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Page {
    pub items: Vec<Summary>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum View {
    Unified(&'static str),
    Mailbox(String),
}

impl View {
    pub fn query(&self) -> String {
        match self {
            View::Unified(v) => format!("view={v}"),
            View::Mailbox(id) => format!("view=mailbox&id={}", encode(id)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailFilter {
    All,
    Unread,
    Flagged,
    Attachments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailSort {
    Newest,
    Oldest,
    Sender,
    Size,
}

impl MailSort {
    pub fn label(self) -> &'static str {
        match self {
            MailSort::Newest => "Newest",
            MailSort::Oldest => "Oldest",
            MailSort::Sender => "Sender",
            MailSort::Size => "Largest",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailLayout {
    Split,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailDensity {
    Comfortable,
    Compact,
}

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
pub(crate) struct Confirm {
    pub title: String,
    pub message: String,
    pub action: &'static str,
    pub run: Arc<dyn Fn() + Send + Sync>,
}

/// A display row in the message list — either one message or a grouped conversation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConversationRow {
    pub head: Summary,
    pub ids: Vec<String>,
    pub participants: String,
    pub unread: bool,
    pub flagged: bool,
    pub answered: bool,
    pub attachments: bool,
    pub count: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct MailState {
    pub overview: RwSignal<Option<Overview>>,
    pub load_error: RwSignal<String>,
    pub view: RwSignal<View>,
    pub items: RwSignal<Vec<Summary>>,
    pub cursor: RwSignal<Option<String>>,
    pub list_state: RwSignal<&'static str>,
    pub list_error: RwSignal<String>,
    pub selected: RwSignal<Option<String>>,
    pub detail: RwSignal<Option<Detail>>,
    pub detail_error: RwSignal<String>,
    pub checked: RwSignal<HashSet<String>>,
    pub last_checked_id: RwSignal<Option<String>>,
    pub filter: RwSignal<MailFilter>,
    pub account_filter: RwSignal<Option<String>>,
    pub sort: RwSignal<MailSort>,
    pub layout: RwSignal<MailLayout>,
    pub density: RwSignal<MailDensity>,
    pub shortcuts_open: RwSignal<bool>,
    pub expand_all_signal: RwSignal<u32>,
    pub pane: RwSignal<Option<Pane>>,
    pub compose: RwSignal<Option<ComposeSpec>>,
    pub compose_minimized: RwSignal<bool>,
    pub toast: RwSignal<Option<Toast>>,
    pub confirm: RwSignal<Option<Confirm>>,
    pub syncing: RwSignal<bool>,
    pub collapsed: RwSignal<HashSet<String>>,
    pub navigation: RwSignal<bool>,
    pub generation: StoredValue<u32>,
    pub session: StoredValue<BrowserSession>,
    pub images: Memo<String>,
    pub mark_read: Memo<String>,
    pub group_threads: Memo<bool>,
    pub allowed_senders: Memo<Vec<String>>,
    pub sending: StoredValue<u32>,
}

/// Groups, filters and sorts loaded messages into display conversation rows.
pub(crate) fn build_rows(
    items: &[Summary],
    view: &View,
    group_threads: bool,
    filter: MailFilter,
    account_filter: Option<&str>,
    sort: MailSort,
    accounts: &[Account],
) -> Vec<ConversationRow> {
    let own_addrs: HashSet<String> = accounts.iter().map(|a| a.address.to_lowercase()).collect();
    let is_drafts_or_sent = matches!(view, View::Unified("drafts" | "sent"));

    // Filter by account first (in unified views)
    let scoped: Vec<&Summary> = items
        .iter()
        .filter(|m| {
            account_filter
                .map(|aid| m.account_id == aid)
                .unwrap_or(true)
        })
        .collect();

    let mut rows: Vec<ConversationRow> = if group_threads && !is_drafts_or_sent {
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<&Summary>> = HashMap::new();
        for m in scoped {
            let key = if m.thread_key.is_empty() {
                format!("msg:{}", m.id)
            } else {
                format!("thr:{}:{}", m.account_id, m.thread_key)
            };
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(m);
        }
        order
            .into_iter()
            .filter_map(|k| {
                let mut list = groups.remove(&k)?;
                list.sort_by(|a, b| b.date.total_cmp(&a.date));
                let head = list[0].clone();
                let ids: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                let unread = list.iter().any(|m| !m.seen);
                let flagged = list.iter().any(|m| m.flagged);
                let answered = list.iter().any(|m| m.answered);
                let attachments = list.iter().any(|m| m.attachments);
                let count = head.thread_count.max(list.len() as u32);
                let participants = participant_label(&list, &head, &own_addrs);
                Some(ConversationRow {
                    head,
                    ids,
                    participants,
                    unread,
                    flagged,
                    answered,
                    attachments,
                    count,
                })
            })
            .collect()
    } else {
        scoped
            .into_iter()
            .map(|m| {
                let participants = single_label(m);
                ConversationRow {
                    head: m.clone(),
                    ids: vec![m.id.clone()],
                    participants,
                    unread: !m.seen,
                    flagged: m.flagged,
                    answered: m.answered,
                    attachments: m.attachments,
                    count: m.thread_count.max(1),
                }
            })
            .collect()
    };

    // Quick filter chips
    rows.retain(|r| match filter {
        MailFilter::All => true,
        MailFilter::Unread => r.unread,
        MailFilter::Flagged => r.flagged,
        MailFilter::Attachments => r.attachments,
    });

    // Sort
    match sort {
        MailSort::Newest => rows.sort_by(|a, b| b.head.date.total_cmp(&a.head.date)),
        MailSort::Oldest => rows.sort_by(|a, b| a.head.date.total_cmp(&b.head.date)),
        MailSort::Sender => rows.sort_by(|a, b| {
            a.participants
                .to_lowercase()
                .cmp(&b.participants.to_lowercase())
                .then_with(|| b.head.date.total_cmp(&a.head.date))
        }),
        MailSort::Size => rows.sort_by(|a, b| {
            b.head
                .size
                .total_cmp(&a.head.size)
                .then_with(|| b.head.date.total_cmp(&a.head.date))
        }),
    }
    rows
}

fn single_label(item: &Summary) -> String {
    if item.mailbox_role == "sent"
        || item.mailbox_role == "drafts"
        || item.draft
        || (item.from_address.is_empty() && !item.recipients.to.is_empty())
    {
        item.recipients
            .to
            .first()
            .map(|t| format!("To {}", t.label()))
            .unwrap_or_else(|| {
                if item.draft {
                    "(No recipient)".into()
                } else {
                    item.sender()
                }
            })
    } else {
        item.sender()
    }
}

fn participant_label(list: &[&Summary], head: &Summary, own: &HashSet<String>) -> String {
    if list.len() <= 1 {
        return single_label(head);
    }
    let mut names: Vec<String> = Vec::new();
    let mut seen_addr: HashSet<String> = HashSet::new();
    // Oldest-to-newest distinct senders so conversations read naturally ("Lina, Rami, me")
    for m in list.iter().rev() {
        let addr = m.from_address.to_lowercase();
        let label = if !addr.is_empty() && own.contains(&addr) {
            "me".to_owned()
        } else {
            m.short_sender()
        };
        let key = if addr.is_empty() {
            label.to_lowercase()
        } else {
            addr
        };
        if seen_addr.insert(key) {
            names.push(label);
        }
    }
    if names.len() <= 1 {
        return single_label(head);
    }
    if names.len() > 3 {
        format!("{}, {}…", names[0], names[1])
    } else {
        names.join(", ")
    }
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

pub(crate) fn time_label(time: f64) -> String {
    if days_ago(time) <= 0 {
        local_time(time, &[("hour", "numeric"), ("minute", "2-digit")])
    } else if days_ago(time) < 7 {
        local_date(time, &[("weekday", "short")])
    } else {
        local_date(time, &[("month", "short"), ("day", "numeric")])
    }
}

pub(crate) fn relative(time: f64) -> String {
    let mins = ((js_sys::Date::now() - time) / 60_000.0).round() as i64;
    if time <= 0.0 {
        "not yet".into()
    } else if mins < 1 {
        "just now".into()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if mins < 1440 {
        format!("{}h ago", mins / 60)
    } else {
        local_date(time, &[])
    }
}

pub(crate) fn read_url() -> (View, Option<String>, HashMap<String, String>) {
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

pub(crate) fn write_url(view: &View, message: Option<&str>) {
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

    pub fn mailbox(self, id: &str) -> Option<(Account, Mailbox)> {
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

    pub fn is_sender_allowed(self, address: &str) -> bool {
        let lower = address.trim().to_lowercase();
        !lower.is_empty()
            && self
                .allowed_senders
                .with_untracked(|list| list.iter().any(|s| s == &lower))
    }

    pub fn allow_sender(self, address: String) {
        let lower = address.trim().to_lowercase();
        if lower.is_empty() {
            return;
        }
        let mut list = self.allowed_senders.get_untracked();
        if !list.contains(&lower) {
            list.push(lower.clone());
        }
        let csrf = self.csrf();
        let prefs = context().preferences;
        let rev = prefs
            .value
            .with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
        spawn_local(async move {
            let body = json!({ "revision": rev, "mailAllowedSenders": list });
            if api::send::<Value>("/api/v1/preferences", "PATCH", Some(body), &csrf)
                .await
                .is_ok()
            {
                prefs.refresh();
                self.notify(format!("Remote images allowed for {lower}"));
            }
        });
    }

    pub fn save_pref(self, key: &'static str, value: Value) {
        let csrf = self.csrf();
        let prefs = context().preferences;
        let rev = prefs
            .value
            .with_untracked(|p| p.as_ref().map(|p| p.revision).unwrap_or(0));
        spawn_local(async move {
            let mut body = json!({ "revision": rev });
            body[key] = value;
            if api::send::<Value>("/api/v1/preferences", "PATCH", Some(body), &csrf)
                .await
                .is_ok()
            {
                prefs.refresh();
            }
        });
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

    pub async fn fetch_items(self, reset: bool) {
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
        self.last_checked_id.set(None);
        self.filter.set(MailFilter::All);
        self.account_filter.set(None);
        self.navigation.set(false);
        self.items.set(Vec::new());
        write_url(&view, None);
        self.load_items();
    }

    pub fn open(self, id: String) {
        self.selected.set(Some(id.clone()));
        self.detail_error.set(String::new());
        write_url(&self.view.get_untracked(), Some(&id));
        let sender_allowed = self.items.with_untracked(|items| {
            items
                .iter()
                .find(|i| i.id == id)
                .is_some_and(|i| self.is_sender_allowed(&i.from_address))
        });
        let images = self.images.get_untracked() == "load" || sender_allowed;
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
                Ok(mut d) => {
                    // If sender is allowed but wasn't known before fetch, reload body with images
                    if !images
                        && d.body.blocked_images > 0
                        && self.is_sender_allowed(&d.summary.from_address)
                        && let Ok(b) = api::get::<Body>(&format!(
                            "{BASE}/messages/{}/body?images=1",
                            encode(&id)
                        ))
                        .await
                    {
                        d.body = b;
                    }
                    let unseen_ids: Vec<String> = if self.mark_read.get_untracked() == "open" {
                        let mut u: Vec<String> = d
                            .thread
                            .iter()
                            .filter(|m| !m.seen)
                            .map(|m| m.id.clone())
                            .collect();
                        if !d.summary.seen && !u.contains(&d.summary.id) {
                            u.push(d.summary.id.clone());
                        }
                        u
                    } else {
                        Vec::new()
                    };
                    let _ = self.detail.try_set(Some(d));
                    if !unseen_ids.is_empty() {
                        self.set_flags(unseen_ids, Some(true), None, false);
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

    pub fn visible_rows(self) -> Vec<ConversationRow> {
        let items = self.items.get_untracked();
        let view = self.view.get_untracked();
        let group = self.group_threads.get_untracked();
        let filter = self.filter.get_untracked();
        let af = self.account_filter.get_untracked();
        let sort = self.sort.get_untracked();
        let accounts = self
            .overview
            .with_untracked(|o| o.as_ref().map(|o| o.accounts.clone()).unwrap_or_default());
        build_rows(&items, &view, group, filter, af.as_deref(), sort, &accounts)
    }

    /// Moves selection by `delta` in the visible conversation list and opens that message.
    pub fn step(self, delta: i64) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let index = self
            .selected
            .get_untracked()
            .and_then(|s| {
                rows.iter()
                    .position(|r| r.head.id == s || r.ids.contains(&s))
            })
            .map_or(if delta > 0 { -1 } else { rows.len() as i64 }, |i| i as i64);
        let next = (index + delta).clamp(0, rows.len() as i64 - 1) as usize;
        let id = rows[next].head.id.clone();
        self.open(id.clone());
        if let Some(el) = document()
            .query_selector(&format!("[data-message=\"{id}\"] .mail-row-open"))
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    }

    pub fn toggle_check(self, id: String, shift: bool) {
        if shift && let Some(anchor) = self.last_checked_id.get_untracked() {
            let rows = self.visible_rows();
            if let (Some(a), Some(b)) = (
                rows.iter().position(|r| r.head.id == anchor),
                rows.iter().position(|r| r.head.id == id),
            ) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                self.checked.update(|set| {
                    for r in &rows[lo..=hi] {
                        for mid in &r.ids {
                            set.insert(mid.clone());
                        }
                    }
                });
                self.last_checked_id.set(Some(id));
                return;
            }
        }
        let row_ids = self
            .visible_rows()
            .into_iter()
            .find(|r| r.head.id == id)
            .map(|r| r.ids)
            .unwrap_or_else(|| vec![id.clone()]);
        self.checked.update(|set| {
            let all_on = row_ids.iter().all(|x| set.contains(x));
            for x in row_ids {
                if all_on {
                    set.remove(&x);
                } else {
                    set.insert(x);
                }
            }
        });
        self.last_checked_id.set(Some(id));
    }

    pub fn set_flags(
        self,
        ids: Vec<String>,
        seen: Option<bool>,
        flagged: Option<bool>,
        announce: bool,
    ) {
        if ids.is_empty() {
            return;
        }
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
                if let Some(d) = d.as_mut() {
                    if ids.contains(&d.summary.id) {
                        if let Some(v) = seen {
                            d.summary.seen = v;
                        }
                        if let Some(v) = flagged {
                            d.summary.flagged = v;
                        }
                    }
                    for m in d.thread.iter_mut().filter(|m| ids.contains(&m.id)) {
                        if let Some(v) = seen {
                            m.seen = v;
                        }
                        if let Some(v) = flagged {
                            m.flagged = v;
                        }
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

    /// Archive, delete, spam or move; confirms permanent deletion from Trash and offers Undo otherwise.
    pub fn move_to(
        self,
        ids: Vec<String>,
        role: Option<&'static str>,
        mailbox_id: Option<String>,
        label: String,
    ) {
        if ids.is_empty() {
            return;
        }
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
            && !origins.is_empty()
            && origins
                .iter()
                .all(|(_, m)| self.mailbox(m).is_some_and(|(_, b)| b.role == "trash"));
        if in_trash {
            let count = ids.len();
            let ids_c = ids.clone();
            let label_c = label.clone();
            self.confirm.set(Some(Confirm {
                title: if count == 1 {
                    "Delete message permanently?".into()
                } else {
                    format!("Delete {count} messages permanently?")
                },
                message: "This removes the messages from the mail server and moves their mirrored .eml copies to Deleted files.".into(),
                action: "Delete permanently",
                run: Arc::new(move || {
                    self.execute_move(ids_c.clone(), Some("trash"), None, label_c.clone(), origins.clone(), true);
                }),
            }));
            return;
        }
        self.execute_move(ids, role, mailbox_id, label, origins, false);
    }

    fn execute_move(
        self,
        ids: Vec<String>,
        role: Option<&'static str>,
        mailbox_id: Option<String>,
        label: String,
        origins: Vec<(String, String)>,
        in_trash: bool,
    ) {
        let csrf = self.csrf();
        let removed_count = self
            .items
            .with_untracked(|items| items.iter().filter(|i| ids.contains(&i.id)).count());
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
                    let count = removed_count.max(1);
                    let noun = if count == 1 {
                        "Message".to_owned()
                    } else {
                        format!("{count} messages")
                    };
                    let message = if in_trash {
                        format!("{noun} deleted permanently")
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

    pub fn create_mailbox(self, account_id: String, name: String) {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let csrf = self.csrf();
        spawn_local(async move {
            match api::send::<Value>(
                &format!("{BASE}/mailboxes"),
                "POST",
                Some(json!({ "accountId": account_id, "name": name })),
                &csrf,
            )
            .await
            {
                Ok(_) => {
                    self.notify(format!("Folder “{name}” created; syncing to server"));
                    crate::sleep(1200).await;
                    self.fetch_overview().await;
                }
                Err(e) => self.fail(e.message),
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
        self.compose_minimized.set(false);
        let reopen = spec.clone();
        let cancel: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            self.sending.set_value(self.sending.get_value() + 1);
            self.compose_minimized.set(false);
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
                    let _ = self.compose_minimized.try_set(false);
                    let _ = self.compose.try_set(Some(spec));
                    self.fail(format!("Not sent: {}", e.message));
                }
            }
        });
    }

    pub fn selection(self) -> Vec<String> {
        let checked: Vec<String> = self.checked.get_untracked().into_iter().collect();
        if !checked.is_empty() {
            return checked;
        }
        self.selected.get_untracked().into_iter().collect()
    }

    pub fn open_compose(self, spec: ComposeSpec) {
        self.compose_minimized.set(false);
        self.compose.set(Some(spec));
    }

    pub fn reply(self, mode: &'static str) {
        if let Some(d) = self.detail.get_untracked() {
            let account = self.account(&d.summary.account_id);
            self.open_compose(ComposeSpec::reply(&d, mode, account.as_ref()));
        }
    }

    pub fn reply_to_message(self, summary: &Summary, body: &Body, mode: &'static str) {
        let d = Detail {
            summary: summary.clone(),
            body: body.clone(),
            ..Default::default()
        };
        let account = self.account(&summary.account_id);
        self.open_compose(ComposeSpec::reply(&d, mode, account.as_ref()));
    }

    pub fn new_message(self) {
        let account = match self.view.get_untracked() {
            View::Mailbox(id) => self.mailbox(&id).map(|(a, _)| a),
            _ => self
                .account_filter
                .get_untracked()
                .and_then(|id| self.account(&id)),
        }
        .or_else(|| {
            self.overview
                .with_untracked(|o| o.as_ref().and_then(|o| o.accounts.first().cloned()))
        });
        match account {
            Some(a) => self.open_compose(ComposeSpec::new(&a)),
            None => self.pane.set(Some(Pane::AddAccount(None))),
        }
    }

    /// Collect recent correspondents from currently loaded messages for autocomplete.
    pub fn recent_correspondents(self) -> Vec<(String, String)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        self.items.with_untracked(|items| {
            for m in items {
                if !m.from_address.is_empty() && seen.insert(m.from_address.to_lowercase()) {
                    out.push((m.from_name.clone(), m.from_address.clone()));
                }
                for r in m.recipients.to.iter().chain(&m.recipients.cc) {
                    if !r.address.is_empty() && seen.insert(r.address.to_lowercase()) {
                        out.push((r.name.clone(), r.address.clone()));
                    }
                }
            }
        });
        out
    }
}

/// An account's tone (0–5) as a class, matching the sidebar marks.
pub(crate) fn tone(color: u32) -> String {
    format!("mail-tone-{}", color % 6)
}

/// Computes clean 1–2 letter initials. For bare emails ("lina@example.test"), uses only the local part.
pub(crate) fn initials(name: &str) -> String {
    let source = if name.contains('@') && !name.contains(' ') {
        name.split('@').next().unwrap_or(name)
    } else {
        name
    };
    let words: Vec<&str> = source
        .split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = words
        .iter()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters
    }
}

/// A stable avatar tone from a sender's address.
pub(crate) fn avatar_tone(seed: &str) -> String {
    let sum: u32 = seed.bytes().map(u32::from).sum();
    format!("mail-avatar tone-{}", sum % 5)
}

pub(crate) fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}

/// Base64 for attachment uploads.
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Parses an RFC 2369 `List-Unsubscribe` header, preferring an `https://` URL over `mailto:`.
pub(crate) fn parse_unsubscribe(header: &str) -> Option<(String, bool)> {
    let mut http_url = None;
    let mut mailto = None;
    for part in header.split(',') {
        let trimmed = part
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim();
        if trimmed.starts_with("https://") {
            http_url = Some(trimmed.to_owned());
        } else if trimmed.starts_with("mailto:") {
            mailto = Some(trimmed.to_owned());
        }
    }
    if let Some(u) = http_url {
        Some((u, true))
    } else {
        mailto.map(|m| (m, false))
    }
}

/// Splits a plain-text message into its fresh body and any trailing quoted reply block
/// so the reader can collapse repeated history behind a `···` toggle.
pub(crate) fn split_quoted_text(text: &str) -> (String, Option<String>) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 3 {
        return (text.to_owned(), None);
    }
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let is_attribution = (t.starts_with("On ") && t.ends_with("wrote:"))
            || t.starts_with("---------- Forwarded message")
            || t == "-----Original Message-----";
        if is_attribution && i > 0 {
            let main = lines[..i].join("\n").trim_end().to_owned();
            let quoted = lines[i..].join("\n").trim().to_owned();
            if !main.is_empty() && !quoted.is_empty() {
                return (main, Some(quoted));
            }
        }
    }
    // Check for a trailing block of `>` quoted lines
    let mut last_non_quote = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('>') {
            last_non_quote = i;
        } else {
            break;
        }
    }
    if last_non_quote > 0 && last_non_quote < lines.len() {
        let tail = lines[last_non_quote..].join("\n").trim().to_owned();
        if tail.lines().any(|l| l.trim_start().starts_with('>')) {
            let main = lines[..last_non_quote].join("\n").trim_end().to_owned();
            if !main.is_empty() {
                return (main, Some(tail));
            }
        }
    }
    (text.to_owned(), None)
}

const ALLOWED: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "dd",
    "del",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "q",
    "s",
    "small",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "time",
    "tr",
    "u",
    "ul",
];
const DROPPED: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "svg", "math", "form", "template", "noscript",
    "link", "meta", "base", "video", "audio", "canvas",
];

fn walk(
    doc: &web_sys::Document,
    from: &web_sys::Node,
    to: &web_sys::Node,
    depth: usize,
    origin: &str,
) {
    let children = from.child_nodes();
    for i in 0..children.length() {
        let Some(child) = children.item(i) else {
            continue;
        };
        match child.node_type() {
            web_sys::Node::TEXT_NODE => {
                let _ = to
                    .append_child(&doc.create_text_node(&child.text_content().unwrap_or_default()));
                continue;
            }
            web_sys::Node::ELEMENT_NODE => {}
            _ => continue,
        }
        let element: web_sys::Element = child.unchecked_into();
        let tag = element.tag_name().to_lowercase();
        if DROPPED.contains(&tag.as_str()) {
            continue;
        }
        if !ALLOWED.contains(&tag.as_str()) || depth > 60 {
            walk(doc, &element, to, depth, origin);
            continue;
        }
        if tag == "img" {
            // The server rewrote every image it allows to Mail's own API; anything else is blocked.
            let Some(src) = element
                .get_attribute("src")
                .filter(|s| s.starts_with(&format!("{origin}/api/v1/mail/")))
            else {
                continue;
            };
            let Ok(el) = doc.create_element("img") else {
                continue;
            };
            let _ = el.set_attribute("src", &src);
            let _ = el.set_attribute("alt", &element.get_attribute("alt").unwrap_or_default());
            let _ = el.set_attribute("loading", "lazy");
            let _ = el.set_attribute("referrerpolicy", "no-referrer");
            let _ = to.append_child(&el);
            continue;
        }
        let Ok(copy) = doc.create_element(&tag) else {
            continue;
        };
        if tag == "a"
            && let Some(href) = element
                .get_attribute("href")
                .and_then(|h| web_sys::Url::new(&h).ok())
                .filter(|u| matches!(u.protocol().as_str(), "http:" | "https:" | "mailto:"))
        {
            let _ = copy.set_attribute("href", &href.href());
            let _ = copy.set_attribute("target", "_blank");
            let _ = copy.set_attribute("rel", "noopener noreferrer");
        }
        for name in ["colspan", "rowspan", "start", "title", "dir"] {
            if let Some(value) = element.get_attribute(name).filter(|v| {
                !v.is_empty() && v.len() < 300 && (name != "dir" || v == "rtl" || v == "ltr")
            }) {
                let _ = copy.set_attribute(name, &value);
            }
        }
        walk(doc, &element, &copy, depth + 1, origin);
        let _ = to.append_child(&copy);
    }
}

/// Server-sanitized HTML rebuilt through a second allowlist in an inert document.
pub(crate) fn render_html(html: &str) -> web_sys::DocumentFragment {
    let doc = document();
    let out = doc.create_document_fragment();
    let origin = crate::ui::window().location().origin().unwrap_or_default();
    let Ok(parser) = web_sys::DomParser::new() else {
        return out;
    };
    let Ok(source) = parser.parse_from_string(
        &format!("<!doctype html><body>{html}</body>"),
        web_sys::SupportedType::TextHtml,
    ) else {
        return out;
    };
    if let Some(body) = source.body() {
        walk(&doc, &body, &out, 0, &origin);
    }
    out
}
